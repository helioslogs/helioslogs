// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Block reader: parse the tail footer, then decode only the sections a query
//! touches. Section bytes come from either an in-memory buffer (`Source::Owned`,
//! for compaction/tests) or on-demand `get_range` reads against the object store
//! (`Source::Ranged`, the query path) — same accessors over both.

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use roaring::RoaringBitmap;

use super::codec::{
    decode_dict, decode_f64, decode_i64_delta, decode_i64_plain, decode_opt_strings,
    decode_opt_strings_subset, read_uvarint, Bloom,
};
use super::objstore::ObjectStore;
use super::{
    FieldValue, Footer, LogType, Row, SectionRef, FORMAT_VERSION, MAGIC, MIN_READABLE_VERSION,
};

const HEADER_LEN: usize = 6;

/// Decoded values of one dynamic `(path, type)` column: `present` row-ids + one `values`
/// entry per present row, ascending (for `Bool`, the truth set is a sub-bitmap of `present`).
pub enum ColumnValues {
    I64(Vec<i64>),
    F64(Vec<f64>),
    Bool(RoaringBitmap),
}

pub struct ValueColumn {
    pub present: RoaringBitmap,
    pub values: ColumnValues,
    pub min: f64,
    pub max: f64,
}

/// Fully-decoded term index for one field: `postings[i]` is the row set for `terms[i]`,
/// `positions[i][r]` the token offsets (tokenized fields only; `None` = exact, no phrase).
pub struct FieldIndex {
    pub terms: Vec<String>,
    pub postings: Vec<RoaringBitmap>,
    pub positions: Option<Vec<Vec<Vec<u32>>>>,
}

impl FieldIndex {
    /// Token positions of `term` in row `row`, or empty if absent / not
    /// position-bearing. Used by phrase verification.
    pub fn positions_for(&self, term_idx: usize, row: u32) -> &[u32] {
        let Some(pos) = &self.positions else {
            return &[];
        };
        let bm = &self.postings[term_idx];
        if !bm.contains(row) {
            return &[];
        }
        let rank = (bm.rank(row) - 1) as usize;
        &pos[term_idx][rank]
    }
}

/// The shred kind of a dynamic column, for the field catalog — the physical
/// `(path, type)` of a value column or a string column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Int,
    Float,
    Bool,
    Str,
}

/// One dynamic `(path, kind)` column's coverage within a block.
#[derive(Clone, Debug)]
pub struct BlockFieldStat {
    pub path: String,
    pub kind: FieldKind,
    /// Rows in this block carrying this typed value.
    pub rows: u32,
    /// Distinct values in this block (string columns only; 0 = unknown).
    pub cardinality: u32,
}

/// Footer-derived field coverage for one block (the field catalog's input). Cheap on
/// v2 blocks; falls back to present-bitmap reads on legacy v1 blocks.
#[derive(Clone, Debug)]
pub struct BlockFieldStats {
    pub row_count: u32,
    pub min_ts: i64,
    pub max_ts: i64,
    pub fields: Vec<BlockFieldStat>,
}

/// Where a block reads its section bytes from: an in-memory buffer (the whole
/// block, for compaction and tests) or ranged reads against the object store (the
/// query path — only the touched sections are fetched).
enum Source {
    Owned(Vec<u8>),
    Ranged {
        store: Arc<dyn ObjectStore>,
        key: String,
    },
}

pub struct Block {
    source: Source,
    footer: Footer,
}

impl Block {
    /// Open a block whose footer was already read (cheaply, via a ranged tail read),
    /// fetching each queried section on demand with `get_range`. The query path uses
    /// this so a selective filter never pulls the whole block off disk.
    pub fn open_ranged(store: Arc<dyn ObjectStore>, key: String, footer: Footer) -> Block {
        Block {
            source: Source::Ranged { store, key },
            footer,
        }
    }

    /// Validate magic/version and parse the footer. Cheap — touches only the
    /// header and tail, not the body.
    pub fn open(bytes: Vec<u8>) -> Result<Block> {
        if bytes.len() < HEADER_LEN + 8 {
            bail!("block: too small ({} bytes)", bytes.len());
        }
        if &bytes[..4] != MAGIC {
            bail!("block: bad leading magic");
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if !(MIN_READABLE_VERSION..=FORMAT_VERSION).contains(&version) {
            bail!("block: unsupported format version {version}");
        }
        let len = bytes.len();
        if &bytes[len - 4..] != MAGIC {
            bail!("block: bad trailing magic");
        }
        let footer_len = u32::from_le_bytes([
            bytes[len - 8],
            bytes[len - 7],
            bytes[len - 6],
            bytes[len - 5],
        ]) as usize;
        let footer_end = len - 8;
        if footer_len > footer_end {
            bail!("block: footer length out of range");
        }
        let footer_start = footer_end - footer_len;
        let footer: Footer = serde_json::from_slice(&bytes[footer_start..footer_end])
            .map_err(|e| anyhow!("block: footer parse: {e}"))?;
        Ok(Block {
            source: Source::Owned(bytes),
            footer,
        })
    }

    pub fn footer(&self) -> &Footer {
        &self.footer
    }

    pub fn row_count(&self) -> u32 {
        self.footer.row_count
    }

    pub fn min_ts(&self) -> i64 {
        self.footer.min_ts
    }

    pub fn max_ts(&self) -> i64 {
        self.footer.max_ts
    }

    /// `true` if any block row could overlap `[start, end]` (inclusive,
    /// unbounded when `None`) — the footer-bounds prune.
    pub fn overlaps(&self, start: Option<i64>, end: Option<i64>) -> bool {
        if let Some(s) = start {
            if self.footer.max_ts < s {
                return false;
            }
        }
        if let Some(e) = end {
            if self.footer.min_ts > e {
                return false;
            }
        }
        true
    }

    fn read_section(&self, s: &SectionRef) -> Result<Vec<u8>> {
        match &self.source {
            Source::Owned(bytes) => {
                let start = s.offset as usize;
                let end = start + s.stored_len as usize;
                if end > bytes.len() {
                    bail!("block: section range out of bounds");
                }
                super::codec::decompress(s.codec, &bytes[start..end], s.raw_len)
            }
            Source::Ranged { store, key } => {
                let raw = store.get_range(key, s.offset, s.stored_len as u64)?;
                if raw.len() != s.stored_len as usize {
                    bail!("block: short ranged read for section");
                }
                super::codec::decompress(s.codec, &raw, s.raw_len)
            }
        }
    }

    pub fn timestamps(&self) -> Result<Vec<i64>> {
        let raw = self.read_section(&self.footer.timestamp)?;
        decode_i64_delta(&raw, self.footer.row_count as usize)
    }

    pub fn messages(&self) -> Result<Vec<Option<String>>> {
        self.opt_strings(self.footer.message.as_ref())
    }

    pub fn sources(&self) -> Result<Vec<Option<String>>> {
        self.opt_strings(self.footer.source.as_ref())
    }

    pub fn raws(&self) -> Result<Vec<Option<String>>> {
        self.opt_strings(self.footer.raw.as_ref())
    }

    fn opt_strings(&self, section: Option<&SectionRef>) -> Result<Vec<Option<String>>> {
        let n = self.footer.row_count as usize;
        match section {
            None => Ok(vec![None; n]),
            Some(s) => decode_opt_strings(&self.read_section(s)?, n),
        }
    }

    fn opt_strings_subset(
        &self,
        section: Option<&SectionRef>,
        rows: &[u32],
    ) -> Result<Vec<Option<String>>> {
        match section {
            None => Ok(vec![None; rows.len()]),
            Some(s) => decode_opt_strings_subset(
                &self.read_section(s)?,
                self.footer.row_count as usize,
                rows,
            ),
        }
    }

    /// Display columns (`message`, `source`, `raw`) for just `rows` (ascending,
    /// unique), in `rows` order — the hit-rendering path, which needs only the
    /// surviving candidates, not every row in the block.
    #[allow(clippy::type_complexity)]
    pub fn display_rows(
        &self,
        rows: &[u32],
    ) -> Result<Vec<(Option<String>, Option<String>, Option<String>)>> {
        let msgs = self.opt_strings_subset(self.footer.message.as_ref(), rows)?;
        let srcs = self.opt_strings_subset(self.footer.source.as_ref(), rows)?;
        let raws = self.opt_strings_subset(self.footer.raw.as_ref(), rows)?;
        Ok(msgs
            .into_iter()
            .zip(srcs)
            .zip(raws)
            .map(|((m, s), r)| (m, s, r))
            .collect())
    }

    fn term_dir(&self, field: &str) -> Option<&super::TermIndexDir> {
        self.footer.term_indexes.iter().find(|t| t.field == field)
    }

    /// Bloom-only existence probe — may false-positive, never false-negative.
    /// `false` ⇒ the term is definitely not in this field's block (skip it).
    pub fn bloom_might_contain(&self, field: &str, term: &str) -> Result<bool> {
        let Some(dir) = self.term_dir(field) else {
            return Ok(false);
        };
        let bloom = Bloom::deserialize(&self.read_section(&dir.bloom)?)?;
        Ok(bloom.contains(term))
    }

    /// Row-id bitmap for one exact term, or `None`. Bloom-gated so an absent term skips
    /// the dict/postings entirely; on a hit the dict is scanned in place (sorted, early-exit).
    pub fn term_postings(&self, field: &str, term: &str) -> Result<Option<RoaringBitmap>> {
        let Some(dir) = self.term_dir(field) else {
            return Ok(None);
        };
        let bloom = Bloom::deserialize(&self.read_section(&dir.bloom)?)?;
        if !bloom.contains(term) {
            return Ok(None);
        }
        // Locate the term's dict index with a sorted in-place scan: the dict is
        // ascending, so we stop as soon as we meet or pass `term`.
        let dict_raw = self.read_section(&dir.dict)?;
        let mut d = dict_raw.as_slice();
        let n = read_uvarint(&mut d)? as usize;
        let needle = term.as_bytes();
        let mut idx = None;
        for i in 0..n {
            let tlen = read_uvarint(&mut d)? as usize;
            if d.len() < tlen {
                bail!("dict: short buffer");
            }
            match d[..tlen].cmp(needle) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    idx = Some(i);
                    break;
                }
                std::cmp::Ordering::Greater => break, // passed it — sorted ⇒ absent
            }
            d = &d[tlen..];
        }
        let Some(idx) = idx else {
            return Ok(None);
        };
        let postings = self.read_section(&dir.postings)?;
        let bm = nth_posting(&postings, idx)?;
        Ok(Some(bm))
    }

    /// OR the postings of every term matching `pred`, deserializing only those bitmaps
    /// (the substring/glob scan path) — vs. deserializing all up front on a high-card field.
    pub fn field_postings_or<F: Fn(&str) -> bool>(
        &self,
        field: &str,
        pred: F,
    ) -> Result<RoaringBitmap> {
        let Some(dir) = self.term_dir(field) else {
            return Ok(RoaringBitmap::new());
        };
        // Pass 1: decode the dict (far cheaper than postings) and collect matching term
        // indices — a block matching nothing then skips the postings section entirely.
        let dict_raw = self.read_section(&dir.dict)?;
        let mut d = dict_raw.as_slice();
        let n = read_uvarint(&mut d)? as usize;
        let mut matches: Vec<usize> = Vec::new();
        for i in 0..n {
            let tlen = read_uvarint(&mut d)? as usize;
            if d.len() < tlen {
                bail!("dict: short buffer");
            }
            let term =
                std::str::from_utf8(&d[..tlen]).map_err(|e| anyhow!("dict: invalid utf8: {e}"))?;
            if pred(term) {
                matches.push(i);
            }
            d = &d[tlen..];
        }
        if matches.is_empty() {
            return Ok(RoaringBitmap::new());
        }

        // Pass 2: walk the postings once, deserializing only the matched terms'
        // bitmaps (skipping the rest by their length prefix).
        let post_raw = self.read_section(&dir.postings)?;
        let mut p = post_raw.as_slice();
        let pn = read_uvarint(&mut p)? as usize;
        let mut acc = RoaringBitmap::new();
        let mut mi = 0;
        for i in 0..pn {
            let plen = read_uvarint(&mut p)? as usize;
            if p.len() < plen {
                bail!("postings: short buffer");
            }
            if mi < matches.len() && matches[mi] == i {
                acc |= RoaringBitmap::deserialize_from(&p[..plen])?;
                mi += 1;
            }
            p = &p[plen..];
            if mi >= matches.len() {
                break; // all matched postings consumed
            }
        }
        Ok(acc)
    }

    /// Every `(term, postings)` pair for a field, dict order — the input to a
    /// terms aggregation. Empty when the field has no term index.
    pub fn field_postings(&self, field: &str) -> Result<Vec<(String, RoaringBitmap)>> {
        let Some(dir) = self.term_dir(field) else {
            return Ok(Vec::new());
        };
        let dict = decode_dict(&self.read_section(&dir.dict)?)?;
        let postings = self.read_section(&dir.postings)?;
        let mut buf = postings.as_slice();
        let n = read_uvarint(&mut buf)? as usize;
        let mut out = Vec::with_capacity(n);
        for term in dict.into_iter().take(n) {
            let len = read_uvarint(&mut buf)? as usize;
            if buf.len() < len {
                bail!("postings: short buffer");
            }
            let bm = RoaringBitmap::deserialize_from(&buf[..len])?;
            buf = &buf[len..];
            out.push((term, bm));
        }
        Ok(out)
    }

    pub fn value_column_dir(&self, path: &str, ty: LogType) -> Option<&super::ValueColumnDir> {
        self.footer
            .value_columns
            .iter()
            .find(|c| c.path == path && c.ty == ty)
    }

    /// Decode one dynamic numeric/bool column. `None` if no such `(path, type)`.
    pub fn value_column(&self, path: &str, ty: LogType) -> Result<Option<ValueColumn>> {
        let Some(dir) = self.value_column_dir(path, ty) else {
            return Ok(None);
        };
        let present = RoaringBitmap::deserialize_from(&self.read_section(&dir.present)?[..])?;
        let count = present.len() as usize;
        let values = match ty {
            LogType::I64 => {
                ColumnValues::I64(decode_i64_plain(&self.read_section(&dir.values)?, count)?)
            }
            LogType::F64 => ColumnValues::F64(decode_f64(&self.read_section(&dir.values)?, count)?),
            LogType::Bool => ColumnValues::Bool(RoaringBitmap::deserialize_from(
                &self.read_section(&dir.values)?[..],
            )?),
            LogType::Str => bail!("value_column: str paths use the term index, not a value column"),
        };
        Ok(Some(ValueColumn {
            present,
            values,
            min: dir.min,
            max: dir.max,
        }))
    }

    /// Fully decode a field's term index (dict + postings + positions). Used by
    /// phrase queries, which need positions across all candidate terms at once.
    pub fn load_field_index(&self, field: &str) -> Result<Option<FieldIndex>> {
        let Some(dir) = self.term_dir(field) else {
            return Ok(None);
        };
        let terms = decode_dict(&self.read_section(&dir.dict)?)?;
        let postings = decode_postings(&self.read_section(&dir.postings)?)?;
        let positions = match &dir.positions {
            None => None,
            Some(sec) => {
                let raw = self.read_section(sec)?;
                let mut buf = raw.as_slice();
                let mut per_term = Vec::with_capacity(postings.len());
                for bm in &postings {
                    let mut per_row = Vec::with_capacity(bm.len() as usize);
                    for _ in 0..bm.len() {
                        let k = read_uvarint(&mut buf)? as usize;
                        let mut ps = Vec::with_capacity(k);
                        for _ in 0..k {
                            ps.push(read_uvarint(&mut buf)? as u32);
                        }
                        per_row.push(ps);
                    }
                    per_term.push(per_row);
                }
                Some(per_term)
            }
        };
        Ok(Some(FieldIndex {
            terms,
            postings,
            positions,
        }))
    }

    /// Untokenized, original-case values of a dynamic string path, one slot per
    /// row (`None` where the path is absent). The terms-aggregation input.
    pub fn str_column(&self, path: &str) -> Result<Option<Vec<Option<String>>>> {
        let Some(dir) = self.footer.str_columns.iter().find(|c| c.path == path) else {
            return Ok(None);
        };
        let present = RoaringBitmap::deserialize_from(&self.read_section(&dir.present)?[..])?;
        let dict = decode_dict(&self.read_section(&dir.dict)?)?;
        let ids_raw = self.read_section(&dir.ids)?;
        let mut buf = ids_raw.as_slice();
        let mut out = vec![None; self.footer.row_count as usize];
        for row in &present {
            let id = read_uvarint(&mut buf)? as usize;
            out[row as usize] = dict.get(id).cloned();
        }
        Ok(Some(out))
    }

    /// Reconstruct every row as a [`Row`] for compaction, from the typed columns (not by
    /// re-parsing `raw`) — so default-tag `source` survives and dynamic values keep their types.
    pub fn rows(&self) -> Result<Vec<Row>> {
        let mut out = Vec::with_capacity(self.footer.row_count as usize);
        self.for_each_row(|r| {
            out.push(r);
            Ok(())
        })?;
        Ok(out)
    }

    /// Stream reconstructed rows to `f`, one at a time, without ever materializing a
    /// `Vec<Row>` of the whole block — the compaction read path, so the merge holds
    /// only the writer's accumulation plus a single in-flight row, not a second copy.
    pub fn for_each_row(&self, mut f: impl FnMut(Row) -> Result<()>) -> Result<()> {
        let n = self.footer.row_count as usize;
        let ts = self.timestamps()?;
        let msgs = self.messages()?;
        let srcs = self.sources()?;
        let raws = self.raws()?;

        // Scatter each dynamic column's values into per-row field lists. Columns must
        // be fully decoded (values for row i are spread across all columns), but the
        // assembled rows are emitted and dropped one at a time below.
        let mut fields: Vec<Vec<(String, FieldValue)>> = vec![Vec::new(); n];

        let numeric: Vec<(String, LogType)> = self
            .footer
            .value_columns
            .iter()
            .map(|c| (c.path.clone(), c.ty))
            .collect();
        for (path, ty) in numeric {
            let Some(col) = self.value_column(&path, ty)? else {
                continue;
            };
            match col.values {
                ColumnValues::I64(vals) => {
                    for (rid, v) in col.present.iter().zip(vals) {
                        fields[rid as usize].push((path.clone(), FieldValue::I64(v)));
                    }
                }
                ColumnValues::F64(vals) => {
                    for (rid, v) in col.present.iter().zip(vals) {
                        fields[rid as usize].push((path.clone(), FieldValue::F64(v)));
                    }
                }
                ColumnValues::Bool(truth) => {
                    for rid in col.present.iter() {
                        fields[rid as usize]
                            .push((path.clone(), FieldValue::Bool(truth.contains(rid))));
                    }
                }
            }
        }

        let str_paths: Vec<String> = self
            .footer
            .str_columns
            .iter()
            .map(|c| c.path.clone())
            .collect();
        for path in str_paths {
            if let Some(vals) = self.str_column(&path)? {
                for (i, v) in vals.into_iter().enumerate() {
                    if let Some(s) = v {
                        fields[i].push((path.clone(), FieldValue::Str(s)));
                    }
                }
            }
        }

        // Emit row-by-row, moving each row's strings + fields out as we go.
        for ((((ts_millis, message), source), raw), flds) in
            ts.into_iter().zip(msgs).zip(srcs).zip(raws).zip(fields)
        {
            f(Row {
                ts_millis,
                message,
                source,
                raw,
                fields: flds,
            })?;
        }
        Ok(())
    }

    /// Per-column coverage for the field catalog. Uses v2 footer fields when present;
    /// v1 blocks read each present bitmap (and string dict) to recover the missing counts.
    pub fn field_stats(&self) -> Result<BlockFieldStats> {
        if let Some(s) = footer_field_stats(&self.footer) {
            return Ok(s);
        }
        // Legacy v1: recover counts from sections.
        let mut fields = Vec::new();
        for c in &self.footer.value_columns {
            let kind = match c.ty {
                LogType::I64 => FieldKind::Int,
                LogType::F64 => FieldKind::Float,
                LogType::Bool => FieldKind::Bool,
                LogType::Str => continue, // never numeric-shredded
            };
            fields.push(BlockFieldStat {
                path: c.path.clone(),
                kind,
                rows: self.present_rows(&c.present)?,
                cardinality: 0,
            });
        }
        for c in &self.footer.str_columns {
            let cardinality = decode_dict(&self.read_section(&c.dict)?)?.len() as u32;
            fields.push(BlockFieldStat {
                path: c.path.clone(),
                kind: FieldKind::Str,
                rows: self.present_rows(&c.present)?,
                cardinality,
            });
        }
        Ok(BlockFieldStats {
            row_count: self.footer.row_count,
            min_ts: self.footer.min_ts,
            max_ts: self.footer.max_ts,
            fields,
        })
    }

    /// Cardinality of a present bitmap section without decoding its values.
    fn present_rows(&self, s: &SectionRef) -> Result<u32> {
        Ok(RoaringBitmap::deserialize_from(&self.read_section(s)?[..])?.len() as u32)
    }

    /// Logical names of every dynamic path in this block, each tagged with the
    /// types it shredded into (a mixed-type path lists more than one).
    pub fn dynamic_paths(&self) -> Vec<(String, LogType)> {
        let mut out = Vec::new();
        for c in &self.footer.value_columns {
            out.push((c.path.clone(), c.ty));
        }
        for c in &self.footer.str_columns {
            out.push((c.path.clone(), LogType::Str));
        }
        out
    }
}

/// Field-coverage stats from a footer alone (no body reads). `Some` only for v2+ footers;
/// `None` on v1 signals the caller to fall back to [`Block::field_stats`].
pub fn footer_field_stats(footer: &Footer) -> Option<BlockFieldStats> {
    if footer.format_version < 2 {
        return None;
    }
    let mut fields = Vec::with_capacity(footer.value_columns.len() + footer.str_columns.len());
    for c in &footer.value_columns {
        let kind = match c.ty {
            LogType::I64 => FieldKind::Int,
            LogType::F64 => FieldKind::Float,
            LogType::Bool => FieldKind::Bool,
            LogType::Str => continue,
        };
        fields.push(BlockFieldStat {
            path: c.path.clone(),
            kind,
            rows: c.rows,
            cardinality: 0,
        });
    }
    for c in &footer.str_columns {
        fields.push(BlockFieldStat {
            path: c.path.clone(),
            kind: FieldKind::Str,
            rows: c.rows,
            cardinality: c.cardinality,
        });
    }
    Some(BlockFieldStats {
        row_count: footer.row_count,
        min_ts: footer.min_ts,
        max_ts: footer.max_ts,
        fields,
    })
}

/// Decode a postings section (`uvarint(n)` then `uvarint(len)+bytes` repeated)
/// into one bitmap per term, dict order.
fn decode_postings(postings: &[u8]) -> Result<Vec<RoaringBitmap>> {
    let mut buf = postings;
    let n = read_uvarint(&mut buf)? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let len = read_uvarint(&mut buf)? as usize;
        if buf.len() < len {
            bail!("postings: short buffer");
        }
        out.push(RoaringBitmap::deserialize_from(&buf[..len])?);
        buf = &buf[len..];
    }
    Ok(out)
}

/// Read the `idx`-th roaring bitmap from a postings section
/// (`uvarint(n)` then `uvarint(len)+bytes` repeated).
fn nth_posting(postings: &[u8], idx: usize) -> Result<RoaringBitmap> {
    let mut buf = postings;
    let n = read_uvarint(&mut buf)? as usize;
    if idx >= n {
        bail!("postings: index {idx} out of range ({n})");
    }
    for _ in 0..idx {
        let len = read_uvarint(&mut buf)? as usize;
        if buf.len() < len {
            bail!("postings: short buffer");
        }
        buf = &buf[len..];
    }
    let len = read_uvarint(&mut buf)? as usize;
    if buf.len() < len {
        bail!("postings: short buffer");
    }
    Ok(RoaringBitmap::deserialize_from(&buf[..len])?)
}
