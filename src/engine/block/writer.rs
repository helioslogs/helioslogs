// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Block writer: accumulate rows, build/compress columns + term indexes, and
//! serialize to one self-describing object whose tail footer maps each section
//! (absolute offsets from buffer start) so the reader range-reads what it needs.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use anyhow::Result;
use roaring::RoaringBitmap;

use crate::indexer::tokenizer::tokenize_into;

use super::codec::{
    compress, encode_dict, encode_f64, encode_i64_delta, encode_i64_plain, encode_opt_strings,
    write_uvarint, Bloom,
};
use super::{
    Codec, FieldValue, Footer, LogType, Row, SectionRef, StrColumnDir, TermIndexDir,
    ValueColumnDir, FORMAT_VERSION, MAGIC,
};

const HEADER_LEN: u64 = 6; // MAGIC(4) + version(2)

pub struct BlockWriter {
    rows: Vec<Row>,
    codec: Codec,
}

impl BlockWriter {
    pub fn new(codec: Codec) -> Self {
        Self {
            rows: Vec::new(),
            codec,
        }
    }

    pub fn push(&mut self, row: Row) {
        self.rows.push(row);
    }

    /// Reserve capacity for `additional` rows up front (exact-step growth, no
    /// doubling-realloc churn). Compaction reserves each source block's row count.
    pub fn reserve(&mut self, additional: usize) {
        self.rows.reserve(additional);
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Serialize all accumulated rows into one block, time-sorted ascending (the reader
    /// walks them in reverse for time-DESC). Empty input yields a valid zero-row block.
    pub fn finish(mut self) -> Result<Vec<u8>> {
        self.rows.sort_by_key(|r| r.ts_millis);
        let n = self.rows.len();
        let codec = self.codec;

        let mut body = BodyBuilder::new(codec);

        // --- timestamp column (delta+zigzag+varint, ascending ⇒ tiny) ---
        let ts: Vec<i64> = self.rows.iter().map(|r| r.ts_millis).collect();
        let timestamp = body.push(&encode_i64_delta(&ts))?;
        let min_ts = ts.first().copied().unwrap_or(0);
        let max_ts = ts.last().copied().unwrap_or(0);

        // --- stored display columns ---
        let message = self.opt_string_column(&mut body, |r| r.message.as_deref())?;
        let source = self.opt_string_column(&mut body, |r| r.source.as_deref())?;
        let raw = self.opt_string_column(&mut body, |r| r.raw.as_deref())?;

        // --- term indexes: message/raw (tokenized), source (exact), dynamic str ---
        // Accumulators key on a borrowed `&str` in an FxHash map (no key clones in the
        // inner loop); dictionary order is restored by one sort at build time.
        let mut term_fields: FxHashMap<&str, FieldAccum> = FxHashMap::default();
        // dynamic numeric/bool accumulators: path -> (present rows, values).
        let mut num_i64: FxHashMap<&str, (Vec<u32>, Vec<i64>)> = FxHashMap::default();
        let mut num_f64: FxHashMap<&str, (Vec<u32>, Vec<f64>)> = FxHashMap::default();
        let mut num_bool: FxHashMap<&str, (Vec<u32>, RoaringBitmap)> = FxHashMap::default();
        // dynamic str value columns: path -> (present rows, original-case values),
        // ascending-by-row parallel arrays borrowing the row strings.
        let mut str_cols: FxHashMap<&str, (Vec<u32>, Vec<&str>)> = FxHashMap::default();

        // Reusable char-index buffer so the tokenizer allocates nothing per row.
        let mut chars: Vec<(usize, char)> = Vec::new();

        for (rid, row) in self.rows.iter().enumerate() {
            let rid = rid as u32;
            if let Some(m) = row.message.as_deref() {
                index_tokenized(&mut term_fields, "message", m, rid, &mut chars);
            }
            if let Some(r) = row.raw.as_deref() {
                index_tokenized(&mut term_fields, "raw", r, rid, &mut chars);
            }
            if let Some(s) = row.source.as_deref() {
                // `source` is indexed whole as one exact lowercased term.
                index_exact(&mut term_fields, "source", lower_cow(s), rid);
            }
            for (path, val) in &row.fields {
                let path = path.as_str();
                match val {
                    FieldValue::Str(s) => {
                        // Tokenized term index (filter) + untokenized value column (agg).
                        index_tokenized(&mut term_fields, path, s, rid, &mut chars);
                        let e = str_cols.entry(path).or_default();
                        if e.0.last() != Some(&rid) {
                            e.0.push(rid);
                            e.1.push(s);
                        }
                    }
                    // Numeric/bool: the value column keeps the row's FIRST value (range/agg/min-max);
                    // extra multi-valued elements are term-indexed so `items.qty:1` matches ANY element.
                    FieldValue::I64(v) => {
                        let e = num_i64.entry(path).or_default();
                        if e.0.last() != Some(&rid) {
                            e.0.push(rid);
                            e.1.push(*v);
                        } else {
                            index_exact(&mut term_fields, path, Cow::Owned(v.to_string()), rid);
                        }
                    }
                    FieldValue::F64(v) => {
                        let e = num_f64.entry(path).or_default();
                        if e.0.last() != Some(&rid) {
                            e.0.push(rid);
                            e.1.push(*v);
                        } else {
                            index_exact(&mut term_fields, path, Cow::Owned(v.to_string()), rid);
                        }
                    }
                    FieldValue::Bool(b) => {
                        let e = num_bool.entry(path).or_default();
                        if e.0.last() != Some(&rid) {
                            e.0.push(rid);
                            if *b {
                                e.1.insert(rid);
                            }
                        } else {
                            index_exact(&mut term_fields, path, Cow::Owned(b.to_string()), rid);
                        }
                    }
                }
            }
        }

        // Restore dictionary (sorted-field) order once, then build each index.
        let mut fields: Vec<(&str, FieldAccum)> = term_fields.into_iter().collect();
        fields.sort_unstable_by_key(|(f, _)| *f);
        let mut term_indexes = Vec::with_capacity(fields.len());
        for (field, accum) in &fields {
            term_indexes.push(build_term_index(&mut body, field, accum)?);
        }

        // --- value columns (numerics/bools) ---
        // Present-row arrays are ascending, so the bitmap builds from a sorted iterator, not N inserts.
        let mut value_columns = Vec::new();
        for (path, (present, vals)) in num_i64 {
            let (min, max) = i64_min_max(&vals);
            let rows = present.len() as u32;
            let present_ref = body.push(&serialize_sorted_rows(&present)?)?;
            let values_ref = body.push(&encode_i64_plain(&vals))?;
            value_columns.push(ValueColumnDir {
                path: path.to_string(),
                ty: LogType::I64,
                present: present_ref,
                values: values_ref,
                min,
                max,
                rows,
            });
        }
        for (path, (present, vals)) in num_f64 {
            let (min, max) = f64_min_max(&vals);
            let rows = present.len() as u32;
            let present_ref = body.push(&serialize_sorted_rows(&present)?)?;
            let values_ref = body.push(&encode_f64(&vals))?;
            value_columns.push(ValueColumnDir {
                path: path.to_string(),
                ty: LogType::F64,
                present: present_ref,
                values: values_ref,
                min,
                max,
                rows,
            });
        }
        for (path, (present, truth)) in num_bool {
            let rows = present.len() as u32;
            let present_ref = body.push(&serialize_sorted_rows(&present)?)?;
            let values_ref = body.push(&serialize_bitmap(&truth)?)?;
            value_columns.push(ValueColumnDir {
                path: path.to_string(),
                ty: LogType::Bool,
                present: present_ref,
                values: values_ref,
                min: 0.0,
                max: 1.0,
                rows,
            });
        }

        // --- str value columns (dynamic string paths, untokenized) ---
        let mut str_columns = Vec::new();
        for (path, (present, row_vals)) in str_cols {
            // Dictionary of distinct values; one id per present row, ascending.
            let mut dict: Vec<&str> = row_vals.clone();
            dict.sort_unstable();
            dict.dedup();
            let mut ids = Vec::new();
            for v in &row_vals {
                let id = dict.binary_search(v).unwrap();
                write_uvarint(&mut ids, id as u64);
            }
            let rows = present.len() as u32;
            let cardinality = dict.len() as u32;
            let present_ref = body.push(&serialize_sorted_rows(&present)?)?;
            let dict_ref = body.push(&encode_dict(&dict))?;
            let ids_ref = body.push(&ids)?;
            str_columns.push(StrColumnDir {
                path: path.to_string(),
                present: present_ref,
                dict: dict_ref,
                ids: ids_ref,
                rows,
                cardinality,
            });
        }

        // Sort dynamic columns by path for a deterministic footer layout — readers look up
        // by (path, type), but stable ordering keeps blocks reproducible.
        value_columns.sort_unstable_by(|a, b| (&a.path, a.ty).cmp(&(&b.path, b.ty)));
        str_columns.sort_unstable_by(|a, b| a.path.cmp(&b.path));

        let footer = Footer {
            format_version: FORMAT_VERSION,
            row_count: n as u32,
            min_ts,
            max_ts,
            timestamp,
            message,
            source,
            raw,
            term_indexes,
            value_columns,
            str_columns,
        };

        // Assemble: header + body + footer json + footer_len + trailing magic.
        let mut out = Vec::with_capacity(body.buf.len() + 256);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&body.buf);
        let footer_json = serde_json::to_vec(&footer)?;
        out.extend_from_slice(&footer_json);
        out.extend_from_slice(&(footer_json.len() as u32).to_le_bytes());
        out.extend_from_slice(MAGIC);
        Ok(out)
    }

    fn opt_string_column<F>(&self, body: &mut BodyBuilder, f: F) -> Result<Option<SectionRef>>
    where
        F: Fn(&Row) -> Option<&str>,
    {
        if !self.rows.iter().any(|r| f(r).is_some()) {
            return Ok(None);
        }
        let vals: Vec<Option<&str>> = self.rows.iter().map(&f).collect();
        Ok(Some(body.push(&encode_opt_strings(&vals))?))
    }
}

/// Accumulates compressed section bytes and hands back absolute offsets.
struct BodyBuilder {
    buf: Vec<u8>,
    codec: Codec,
}

impl BodyBuilder {
    fn new(codec: Codec) -> Self {
        Self {
            buf: Vec::new(),
            codec,
        }
    }

    fn push(&mut self, raw: &[u8]) -> Result<SectionRef> {
        let stored = compress(self.codec, raw)?;
        let offset = HEADER_LEN + self.buf.len() as u64;
        let section = SectionRef {
            offset,
            stored_len: stored.len() as u32,
            raw_len: raw.len() as u32,
            codec: self.codec,
        };
        self.buf.extend_from_slice(&stored);
        Ok(section)
    }
}

/// Postings for one term: the rows it occurs in (sorted by construction) plus flat
/// `positions`/`pos_counts` buffers for tokenized fields — two buffers, not N tiny `Vec`s.
#[derive(Default)]
struct TermPostings {
    rows: Vec<u32>,
    pos_counts: Vec<u32>,
    positions: Vec<u32>,
}

/// Per-field index accumulator. `tokenized` fields carry token positions.
#[derive(Default)]
struct FieldAccum<'a> {
    tokenized: bool,
    terms: FxHashMap<Cow<'a, str>, TermPostings>,
}

fn build_term_index(
    body: &mut BodyBuilder,
    field: &str,
    accum: &FieldAccum,
) -> Result<TermIndexDir> {
    // Sort the (term, postings) pairs into dict order once; everything below
    // walks this single ordering, so no term is looked up in the map twice.
    let mut items: Vec<(&str, &TermPostings)> =
        accum.terms.iter().map(|(k, v)| (k.as_ref(), v)).collect();
    items.sort_unstable_by_key(|(t, _)| *t);

    let mut bloom = Bloom::with_capacity(items.len());
    for (t, _) in &items {
        bloom.insert(t);
    }

    // postings section: uvarint(n) then per term uvarint(len)+bytes, dict order. Each
    // bitmap serializes straight into `postings_raw` (size known up front), no scratch `Vec`.
    let mut postings_raw = Vec::new();
    write_uvarint(&mut postings_raw, items.len() as u64);
    for (_, tp) in &items {
        let bm = RoaringBitmap::from_sorted_iter(tp.rows.iter().copied())
            .map_err(|e| anyhow::anyhow!("roaring from sorted rows: {e}"))?;
        write_uvarint(&mut postings_raw, bm.serialized_size() as u64);
        bm.serialize_into(&mut postings_raw)?;
    }

    let bloom_ref = body.push(&bloom.serialize())?;
    let dict: Vec<&str> = items.iter().map(|(t, _)| *t).collect();
    let dict_ref = body.push(&encode_dict(&dict))?;
    let postings_ref = body.push(&postings_raw)?;

    // positions section (tokenized fields only): for each term in dict order,
    // for each row ascending, uvarint(k) + k × uvarint(pos). Parallel to postings.
    let positions = if accum.tokenized {
        let mut pos_raw = Vec::new();
        for (_, tp) in &items {
            let mut off = 0usize;
            for &cnt in &tp.pos_counts {
                write_uvarint(&mut pos_raw, cnt as u64);
                for &p in &tp.positions[off..off + cnt as usize] {
                    write_uvarint(&mut pos_raw, p as u64);
                }
                off += cnt as usize;
            }
        }
        Some(body.push(&pos_raw)?)
    } else {
        None
    };

    Ok(TermIndexDir {
        field: field.to_string(),
        bloom: bloom_ref,
        dict: dict_ref,
        postings: postings_ref,
        positions,
    })
}

/// Lowercase `tok` for indexing, borrowing when already lowercase (the common case).
/// Matches `tok.to_lowercase()` exactly so index and query agree.
fn lower_cow(tok: &str) -> Cow<'_, str> {
    // Fast path: pure-ASCII with no uppercase ⇒ borrow as-is; anything else takes
    // the Unicode `to_lowercase`, which matches the query side.
    if tok.bytes().all(|b| b.is_ascii() && !b.is_ascii_uppercase()) {
        Cow::Borrowed(tok)
    } else {
        Cow::Owned(tok.to_lowercase())
    }
}

fn index_tokenized<'a>(
    fields: &mut FxHashMap<&'a str, FieldAccum<'a>>,
    field: &'a str,
    text: &'a str,
    rid: u32,
    chars: &mut Vec<(usize, char)>,
) {
    let accum = fields.entry(field).or_default();
    accum.tokenized = true;
    let terms = &mut accum.terms;
    tokenize_into(text, chars, |tok, pos| {
        let tp = terms.entry(lower_cow(tok)).or_default();
        if tp.rows.last() != Some(&rid) {
            tp.rows.push(rid);
            tp.pos_counts.push(0);
        }
        // Safe: a row was just ensured present, so `pos_counts` is non-empty.
        *tp.pos_counts.last_mut().unwrap() += 1;
        tp.positions.push(pos);
    });
}

fn index_exact<'a>(
    fields: &mut FxHashMap<&'a str, FieldAccum<'a>>,
    field: &'a str,
    term: Cow<'a, str>,
    rid: u32,
) {
    let tp = fields
        .entry(field)
        .or_default()
        .terms
        .entry(term)
        .or_default();
    if tp.rows.last() != Some(&rid) {
        tp.rows.push(rid);
        // Keep `pos_counts` aligned 1:1 with `rows` (zero-position group) so a path mixing
        // exact and tokenized terms still serializes a consistent positions section.
        tp.pos_counts.push(0);
    }
}

fn serialize_bitmap(bm: &RoaringBitmap) -> Result<Vec<u8>> {
    let mut v = Vec::with_capacity(bm.serialized_size());
    bm.serialize_into(&mut v)?;
    Ok(v)
}

/// Serialize an already-sorted row list as a roaring bitmap, building from the sorted
/// run instead of inserting one id at a time.
fn serialize_sorted_rows(rows: &[u32]) -> Result<Vec<u8>> {
    let bm = RoaringBitmap::from_sorted_iter(rows.iter().copied())
        .map_err(|e| anyhow::anyhow!("roaring from sorted rows: {e}"))?;
    serialize_bitmap(&bm)
}

// fast hashing

/// FxHash (rustc's string hash). SipHash, the std default, dominates block-write time under
/// millions of short-key lookups; FxHash is a few non-cryptographic instructions per 8 bytes.
type FxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

const FX_SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default)]
struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(FX_SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        while bytes.len() >= 8 {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes[..8]);
            self.add(u64::from_le_bytes(buf));
            bytes = &bytes[8..];
        }
        if bytes.len() >= 4 {
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&bytes[..4]);
            self.add(u32::from_le_bytes(buf) as u64);
            bytes = &bytes[4..];
        }
        let mut tail = 0u64;
        for &b in bytes {
            tail = (tail << 8) | b as u64;
        }
        self.add(tail);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

fn i64_min_max(vals: &[i64]) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &v in vals {
        min = min.min(v as f64);
        max = max.max(v as f64);
    }
    if vals.is_empty() {
        (0.0, 0.0)
    } else {
        (min, max)
    }
}

fn f64_min_max(vals: &[f64]) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &v in vals {
        min = min.min(v);
        max = max.max(v);
    }
    if vals.is_empty() {
        (0.0, 0.0)
    } else {
        (min, max)
    }
}
