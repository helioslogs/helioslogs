// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Query execution over blocks: compiles the [`Node`] AST to per-block roaring
//! bitmaps, then drains time-DESC (k-way merge) or runs a columnar agg pass.
//! Bare terms are exact-token (bloom-gated); `*`/`?` glob, quotes phrase-match.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;
use roaring::RoaringBitmap;

use crate::catalog::PartitionKey;
use crate::indexer::tokenizer::tokenize;
use crate::schema::{is_string_field, is_text_field, is_universal_core_field};
use crate::search::query::Node;
use crate::search::Hit;

use super::reader::ColumnValues;
use super::store::BlockStore;
use super::{Block, LogType};

/// Open one block by id for ranged section reads. `Ok(None)` for a manifest-named
/// block the store can't read (a torn append — skipped); only the footer is read
/// up front, so the query fetches just the sections its filter/display touch.
fn open_one(store: &BlockStore, key: &PartitionKey, id: &str) -> Result<Option<Block>> {
    match store.open_block_ranged(key, id) {
        Ok(b) => Ok(Some(b)),
        Err(_) => Ok(None),
    }
}

/// Matching row-ids for one block: content filter AND the time window.
pub fn eval(
    block: &Block,
    filter: Option<&Node>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
) -> Result<RoaringBitmap> {
    let content = match filter {
        None => all_rows(block),
        Some(node) => eval_node(block, node)?,
    };
    if start_ms.is_none() && end_ms.is_none() {
        return Ok(content);
    }
    Ok(content & time_rows(block, start_ms, end_ms)?)
}

/// Canonical filter cache key, computed **once per query** (identical across a
/// partition's blocks). `None` when there's no filter or the cache is off ("don't cache").
fn filter_cache_key(filter: Option<&Node>) -> Option<String> {
    match (filter, super::cache::query_cache()) {
        (Some(node), Some(_)) => Some(format!("{node:?}")),
        _ => None,
    }
}

/// Like [`eval`], but serves the content match from the per-block cache when available.
/// The time window is applied fresh on top, so range/zoom changes still hit the cache.
fn eval_cached(
    block_id: &str,
    block: &Block,
    filter: Option<&Node>,
    filter_key: Option<&str>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
) -> Result<RoaringBitmap> {
    let with_time = |content: &RoaringBitmap| -> Result<RoaringBitmap> {
        if start_ms.is_none() && end_ms.is_none() {
            Ok(content.clone())
        } else {
            Ok(content & &time_rows(block, start_ms, end_ms)?)
        }
    };
    match (filter, filter_key) {
        // Cacheable: a real content filter, with caching enabled.
        (Some(node), Some(fk)) => {
            let cache = super::cache::query_cache().expect("filter_key implies an installed cache");
            let key = (block_id.to_string(), fk.to_string());
            if let Some(hit) = cache.get(&key) {
                return with_time(&hit);
            }
            let content = Arc::new(eval_node(block, node)?);
            cache.insert(key, content.clone());
            with_time(&content)
        }
        // Filter present but caching off — compute directly.
        (Some(node), None) => with_time(&eval_node(block, node)?),
        // Match-all is trivial (a contiguous range); never worth caching.
        (None, _) => with_time(&all_rows(block)),
    }
}

/// Time-DESC top-`limit` across a partition's blocks + the total match count.
/// Thin wrapper over [`search_histogram`] with bucketing disabled.
pub fn search(
    store: &BlockStore,
    key: &PartitionKey,
    filter: Option<&Node>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    limit: usize,
) -> Result<(u64, Vec<(i64, Hit)>)> {
    let (total, hits, _) = search_histogram(store, key, filter, start_ms, end_ms, limit, 0)?;
    Ok((total, hits))
}

/// One pass yielding both time-DESC top-`limit` hits and histogram buckets from the
/// same matched set (~half the work of separate calls). `interval_ms <= 0` skips bucketing.
pub fn search_histogram(
    store: &BlockStore,
    key: &PartitionKey,
    filter: Option<&Node>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    limit: usize,
    interval_ms: i64,
) -> Result<(u64, Vec<(i64, Hit)>, BTreeMap<i64, u64>)> {
    let ids = store.load_manifest(key)?.blocks;
    let fkey = filter_cache_key(filter);
    let fk = fkey.as_deref();

    // Phase 1 (parallel): per block, yield match count, histogram buckets, and the
    // newest-`limit` candidates as cheap `(ts, block_idx, row_id)` ids — no display decode yet.
    type BlockPartial = (u64, Vec<(i64, usize, u32)>, BTreeMap<i64, u64>);
    let partials: Vec<BlockPartial> = ids
        .par_iter()
        .enumerate()
        .map(|(bi, id)| -> Result<BlockPartial> {
            let empty = (0u64, Vec::new(), BTreeMap::new());
            let Some(b) = open_one(store, key, id)? else {
                return Ok(empty);
            };
            if !b.overlaps(start_ms, end_ms) {
                return Ok(empty);
            }
            let matched = eval_cached(id, &b, filter, fk, start_ms, end_ms)?;
            let m = matched.len();
            if m == 0 || (interval_ms <= 0 && limit == 0) {
                return Ok((m, Vec::new(), BTreeMap::new()));
            }
            let ts = b.timestamps()?;

            // Histogram: every matched row contributes to its time bucket.
            let mut buckets = BTreeMap::new();
            if interval_ms > 0 {
                for rid in &matched {
                    *buckets
                        .entry(floor_to(ts[rid as usize], interval_ms))
                        .or_insert(0) += 1;
                }
            }

            // Rows are timestamp-ascending, so newest matches are the highest
            // row-ids — iterate the match set in reverse, O(limit) from the tail.
            let mut cands = Vec::new();
            if limit > 0 {
                cands.reserve(limit.min(m as usize));
                for rid in matched.iter().rev().take(limit) {
                    cands.push((ts[rid as usize], bi, rid));
                }
            }
            Ok((m, cands, buckets))
        })
        .collect::<Result<Vec<_>>>()?;

    // Merge per-block partials.
    let mut total = 0u64;
    let mut candidates: Vec<(i64, usize, u32)> = Vec::new();
    let mut buckets: BTreeMap<i64, u64> = BTreeMap::new();
    for (m, cands, bks) in partials {
        total += m;
        candidates.extend(cands);
        for (k, v) in bks {
            *buckets.entry(k).or_insert(0) += v;
        }
    }
    // Newest-first; ties broken by block/row for a deterministic order.
    candidates.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    candidates.truncate(limit);

    // Phase 2 (parallel): decode display columns for ONLY the surviving candidate
    // rows, grouped by block. Decoding per-row (not whole columns) keeps memory
    // O(limit) even when the newest hits scatter across many uncompacted blocks.
    let mut by_block: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
    for &(_, bi, rid) in &candidates {
        by_block.entry(bi).or_default().push(rid);
    }
    type RowCols = (Option<String>, Option<String>, Option<String>);
    let decoded: Vec<(usize, BTreeMap<u32, RowCols>)> = by_block
        .par_iter()
        .filter_map(|(&bi, rids)| match open_one(store, key, &ids[bi]) {
            Ok(Some(b)) => {
                // display_rows needs ascending, unique row ids.
                let mut sorted = rids.clone();
                sorted.sort_unstable();
                sorted.dedup();
                match b.display_rows(&sorted) {
                    Ok(cols) => Some(Ok((bi, sorted.into_iter().zip(cols).collect()))),
                    Err(e) => Some(Err(e)),
                }
            }
            Ok(None) => None, // block vanished between passes — drop its rows
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<Vec<_>>>()?;
    let cols: BTreeMap<usize, BTreeMap<u32, RowCols>> = decoded.into_iter().collect();

    let mut hits = Vec::with_capacity(candidates.len());
    for (ts, bi, rid) in candidates {
        let Some((message, source, raw)) = cols.get(&bi).and_then(|m| m.get(&rid)) else {
            continue;
        };
        hits.push((
            ts,
            Hit {
                timestamp: millis_to_rfc3339(ts),
                message: message.clone(),
                score: 0.0,
                partition: String::new(),
                source: source.clone(),
                raw: raw.clone(),
            },
        ));
    }
    Ok((total, hits, buckets))
}

/// Unordered scan of up to `limit` matching rows + total match count, for the pipe
/// executor (`| stats`), which needs the matching set capped, not time-sorted.
pub fn scan(
    store: &BlockStore,
    key: &PartitionKey,
    filter: Option<&Node>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    limit: usize,
) -> Result<(u64, Vec<Hit>)> {
    let ids = store.load_manifest(key)?.blocks;
    let fkey = filter_cache_key(filter);
    let fk = fkey.as_deref();
    // Scan blocks in parallel (order unspecified): each materializes up to `limit`
    // hits and counts all matches; the merge sums counts and concatenates, capping at `limit`.
    let partials: Vec<(u64, Vec<Hit>)> = ids
        .par_iter()
        .map(|id| -> Result<(u64, Vec<Hit>)> {
            let Some(b) = open_one(store, key, id)? else {
                return Ok((0, Vec::new()));
            };
            if !b.overlaps(start_ms, end_ms) {
                return Ok((0, Vec::new()));
            }
            let matched = eval_cached(id, &b, filter, fk, start_ms, end_ms)?;
            let m = matched.len();
            if m == 0 || limit == 0 {
                return Ok((m, Vec::new()));
            }
            let ts = b.timestamps()?;
            // Decode display columns for just the rows we keep (ascending from
            // the match set), not the whole block.
            let rows: Vec<u32> = matched.iter().take(limit).collect();
            let cols = b.display_rows(&rows)?;
            let mut local = Vec::new();
            for (rid, (message, source, raw)) in rows.iter().zip(cols) {
                local.push(Hit {
                    timestamp: millis_to_rfc3339(ts[*rid as usize]),
                    message,
                    score: 0.0,
                    partition: String::new(),
                    source,
                    raw,
                });
            }
            Ok((m, local))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut total = 0u64;
    let mut hits = Vec::new();
    for (m, local) in partials {
        total += m;
        if hits.len() < limit {
            hits.extend(local);
            hits.truncate(limit);
        }
    }
    Ok((total, hits))
}

/// Parse a fixed-interval string (`"30s"`, `"5m"`, `"1h"`, `"2d"`,
/// `"500ms"`) into milliseconds. Returns `None` if unparseable.
pub fn parse_interval_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num, unit_ms): (&str, i64) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000)
    } else if let Some(n) = s.strip_suffix('d') {
        (n, 86_400_000)
    } else {
        (s, 1)
    };
    let q: i64 = num.trim().parse().ok()?;
    Some(q * unit_ms)
}

/// Date histogram: `interval_ms`-wide buckets at epoch-aligned floors. With both bounds,
/// empty buckets across the window are filled with 0 for a contiguous series.
pub fn histogram(
    store: &BlockStore,
    key: &PartitionKey,
    filter: Option<&Node>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    interval_ms: i64,
) -> Result<BTreeMap<i64, u64>> {
    let mut out: BTreeMap<i64, u64> = BTreeMap::new();
    if interval_ms <= 0 {
        return Ok(out);
    }
    if let (Some(s), Some(e)) = (start_ms, end_ms) {
        let mut k = floor_to(s, interval_ms);
        while k <= e {
            out.insert(k, 0);
            k += interval_ms;
        }
    }
    let ids = store.load_manifest(key)?.blocks;
    let fkey = filter_cache_key(filter);
    let fk = fkey.as_deref();
    // Bucket each block in parallel, then merge into the pre-filled range.
    let partials: Vec<BTreeMap<i64, u64>> = ids
        .par_iter()
        .map(|id| -> Result<BTreeMap<i64, u64>> {
            let mut local = BTreeMap::new();
            let Some(b) = open_one(store, key, id)? else {
                return Ok(local);
            };
            if !b.overlaps(start_ms, end_ms) {
                return Ok(local);
            }
            let matched = eval_cached(id, &b, filter, fk, start_ms, end_ms)?;
            if matched.is_empty() {
                return Ok(local);
            }
            let ts = b.timestamps()?;
            for rid in &matched {
                *local
                    .entry(floor_to(ts[rid as usize], interval_ms))
                    .or_insert(0) += 1;
            }
            Ok(local)
        })
        .collect::<Result<Vec<_>>>()?;
    for partial in partials {
        for (k, v) in partial {
            *out.entry(k).or_insert(0) += v;
        }
    }
    Ok(out)
}

/// Per-field terms aggregation over matched rows, raw (unscaled) counts. `source`
/// and string paths group on the original-case value; numeric paths on the value column.
pub fn terms(
    store: &BlockStore,
    key: &PartitionKey,
    filter: Option<&Node>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    fields: &[String],
    size_each: usize,
    want_count: bool,
) -> Result<(u64, crate::engine::FieldBuckets)> {
    use serde_json::Value;
    type Buckets = BTreeMap<String, BTreeMap<String, (u64, Value)>>;
    let ids = store.load_manifest(key)?.blocks;
    let fkey = filter_cache_key(filter);
    let fk = fkey.as_deref();

    // Aggregate each block in parallel into a partial `(count, per-field buckets)`,
    // then merge — the per-block grouping is the cost and parallelizes cleanly.
    let partials: Vec<(u64, Buckets)> = ids
        .par_iter()
        .map(|id| -> Result<(u64, Buckets)> {
            let mut local: Buckets = BTreeMap::new();
            let Some(b) = open_one(store, key, id)? else {
                return Ok((0, local));
            };
            if !b.overlaps(start_ms, end_ms) {
                return Ok((0, local));
            }
            let matched = eval_cached(id, &b, filter, fk, start_ms, end_ms)?;
            let count = if want_count { matched.len() } else { 0 };
            if matched.is_empty() {
                return Ok((count, local));
            }
            for f in fields {
                let bucket = local.entry(f.clone()).or_default();
                if f == "source" {
                    let srcs = b.sources()?;
                    for rid in &matched {
                        if let Some(s) = &srcs[rid as usize] {
                            bucket
                                .entry(s.clone())
                                .or_insert_with(|| (0, Value::String(s.clone())))
                                .0 += 1;
                        }
                    }
                    continue;
                }
                let path = f.to_lowercase();
                let mut numeric = false;
                for (ty, is_int) in [(LogType::I64, true), (LogType::F64, false)] {
                    if let Some(col) = b.value_column(&path, ty)? {
                        numeric = true;
                        let vals: Vec<f64> = match &col.values {
                            ColumnValues::I64(v) => v.iter().map(|&x| x as f64).collect(),
                            ColumnValues::F64(v) => v.clone(),
                            ColumnValues::Bool(_) => Vec::new(),
                        };
                        for (rid, v) in col.present.iter().zip(vals.iter()) {
                            if !matched.contains(rid) {
                                continue;
                            }
                            let (k, jv) = if is_int {
                                let i = *v as i64;
                                (i.to_string(), Value::from(i))
                            } else {
                                (fmt_f64(*v), Value::from(*v))
                            };
                            bucket.entry(k).or_insert_with(|| (0, jv)).0 += 1;
                        }
                    }
                }
                if !numeric {
                    // String path: group on the untokenized, original-case value.
                    if let Some(vals) = b.str_column(&path)? {
                        for rid in &matched {
                            if let Some(v) = &vals[rid as usize] {
                                bucket
                                    .entry(v.clone())
                                    .or_insert_with(|| (0, Value::String(v.clone())))
                                    .0 += 1;
                            }
                        }
                    }
                }
            }
            Ok((count, local))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut acc: Buckets = BTreeMap::new();
    for f in fields {
        acc.insert(f.clone(), BTreeMap::new());
    }
    let mut total = 0u64;
    for (count, local) in partials {
        total += count;
        for (field, bucket) in local {
            let target = acc.entry(field).or_default();
            for (k, (c, v)) in bucket {
                target.entry(k).or_insert((0, v)).0 += c;
            }
        }
    }

    // Truncate to the per-field top-N by count (cross-partition layer over-fetches).
    for bucket in acc.values_mut() {
        if bucket.len() > size_each {
            let mut by_count: Vec<(String, (u64, Value))> =
                std::mem::take(bucket).into_iter().collect();
            by_count.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
            by_count.truncate(size_each);
            *bucket = by_count.into_iter().collect();
        }
    }
    Ok((total, acc))
}

fn floor_to(v: i64, interval: i64) -> i64 {
    v - v.rem_euclid(interval)
}

fn fmt_f64(v: f64) -> String {
    if v == v.trunc() && v.is_finite() {
        format!("{}", v as i64)
    } else {
        v.to_string()
    }
}

// AST → bitmap

fn eval_node(block: &Block, node: &Node) -> Result<RoaringBitmap> {
    match node {
        Node::All => Ok(all_rows(block)),
        Node::And(children) => {
            let mut acc: Option<RoaringBitmap> = None;
            for c in children {
                let b = eval_node(block, c)?;
                acc = Some(match acc {
                    None => b,
                    Some(a) => a & b,
                });
            }
            Ok(acc.unwrap_or_else(|| all_rows(block)))
        }
        Node::Or(children) => {
            let mut acc = RoaringBitmap::new();
            for c in children {
                acc |= eval_node(block, c)?;
            }
            Ok(acc)
        }
        Node::Not(inner) => Ok(all_rows(block) - eval_node(block, inner)?),
        Node::Range { field, lo, hi } => eval_range(block, field, lo, hi),
        Node::Term {
            field,
            value,
            quoted,
        } => eval_term(block, field.as_deref(), value, *quoted),
    }
}

fn eval_term(
    block: &Block,
    field: Option<&str>,
    value: &str,
    quoted: bool,
) -> Result<RoaringBitmap> {
    let Some(fname) = field else {
        // Unfielded: message OR raw OR source, all wildcard-aware — so bare `*gz`
        // finds gz-suffixed sources just like `source:*gz`.
        let mut acc = text_match(block, "message", value, quoted)?;
        acc |= text_match(block, "raw", value, quoted)?;
        acc |= source_match(block, &value.to_lowercase())?;
        return Ok(acc);
    };

    if is_text_field(fname) {
        return text_match(block, fname, value, quoted);
    }
    if is_string_field(fname) {
        // `source` / `source_raw` both index under the "source" term field.
        return source_match(block, &value.to_lowercase());
    }
    if is_universal_core_field(fname) {
        // timestamp / dynamic as a bare field name — no term index to hit.
        return Ok(RoaringBitmap::new());
    }
    eval_dynamic_term(block, &fname.to_lowercase(), value)
}

/// Text field (`message`/`raw`): quoted ⇒ phrase, wildcard ⇒ glob, bare ⇒ exact-token
/// (pipelined query — `error` matches the token, not the substring of `errors`; use `error*`).
fn text_match(block: &Block, field: &str, value: &str, quoted: bool) -> Result<RoaringBitmap> {
    if quoted {
        return phrase_match(block, field, value);
    }
    let needle = value.to_lowercase();
    if has_wildcard(&needle) {
        return glob_over_dict(block, field, &needle);
    }
    token_conjunction(block, field, &needle)
}

/// Match the `source` term field (wildcard-aware glob, else exact term lookup). `v`
/// must already be lowercased. Shared by the bare and `source:`-scoped paths.
fn source_match(block: &Block, v: &str) -> Result<RoaringBitmap> {
    if has_wildcard(v) {
        glob_over_dict(block, "source", v)
    } else {
        exact_term(block, "source", v)
    }
}

/// Rows carrying **every** token of `value` (bloom-gated exact lookups AND-ed). A
/// value that tokenizes to nothing falls back to an exact match on the whole value.
fn token_conjunction(block: &Block, field: &str, value: &str) -> Result<RoaringBitmap> {
    let tokens = tokenize_lower(value);
    let Some((first, rest)) = tokens.split_first() else {
        return exact_term(block, field, value);
    };
    let mut acc = exact_term(block, field, first)?;
    for t in rest {
        if acc.is_empty() {
            return Ok(acc); // AND can't recover once empty
        }
        acc &= exact_term(block, field, t)?;
    }
    Ok(acc)
}

/// Dynamic path term: union over whichever shredded type carries the value (string =
/// tokenized conjunction; numeric/bool = exact equality on the value column).
fn eval_dynamic_term(block: &Block, path: &str, value: &str) -> Result<RoaringBitmap> {
    let mut acc = match_one_path(block, path, value)?;
    // Bare leaf (no dot) also matches any nested column with the same final segment
    // — `qty:1` reaches `items.qty` / `refunds.qty`. Dotted paths stay exact.
    if !path.contains('.') {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (full, _ty) in block.dynamic_paths() {
            if full.as_str() == path || leaf_of(&full) != path {
                continue;
            }
            // dynamic_paths() may list a path under multiple types; resolve once.
            if seen.insert(full.clone()) {
                acc |= match_one_path(block, &full, value)?;
            }
        }
    }
    Ok(acc)
}

/// Last `.`-segment of a path (`a.b.c` → `c`; no dot → the whole string).
fn leaf_of(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// Match `path:value` against the shredded columns at exactly `path`: tokenized
/// string term index unioned with the numeric/bool value columns.
fn match_one_path(block: &Block, path: &str, value: &str) -> Result<RoaringBitmap> {
    let mut acc = phrase_match(block, path, value)?;
    if let Ok(i) = value.parse::<i64>() {
        acc |= numeric_equals(block, path, LogType::I64, i as f64)?;
    }
    if let Ok(f) = value.parse::<f64>() {
        acc |= numeric_equals(block, path, LogType::F64, f)?;
    }
    match value {
        "true" => acc |= bool_equals(block, path, true)?,
        "false" => acc |= bool_equals(block, path, false)?,
        _ => {}
    }
    Ok(acc)
}

/// Positional phrase match: query tokens must appear at consecutive positions in a
/// row. Single-token values reduce to an exact term match.
fn phrase_match(block: &Block, field: &str, value: &str) -> Result<RoaringBitmap> {
    let tokens = tokenize_lower(value);
    // 0 or 1 tokens need no adjacency check — a bloom-gated exact lookup answers
    // it directly, skipping the full (dict+postings+positions) index decode.
    match tokens.as_slice() {
        [] => return exact_term(block, field, &value.to_lowercase()),
        [only] => return exact_term(block, field, only),
        _ => {}
    }
    let Some(fi) = block.load_field_index(field)? else {
        return Ok(RoaringBitmap::new());
    };
    // Resolve each query token to a dict index; any missing ⇒ no match.
    let mut term_idx = Vec::with_capacity(tokens.len());
    for t in &tokens {
        match fi.terms.binary_search(t) {
            Ok(i) => term_idx.push(i),
            Err(_) => return Ok(RoaringBitmap::new()),
        }
    }
    // Candidate rows: all query terms co-present.
    let mut cand: Option<RoaringBitmap> = None;
    for &i in &term_idx {
        cand = Some(match cand {
            None => fi.postings[i].clone(),
            Some(a) => a & &fi.postings[i],
        });
    }
    let cand = cand.unwrap_or_default();
    // Single token ⇒ co-presence is the answer; no adjacency to verify.
    if term_idx.len() == 1 || fi.positions.is_none() {
        return Ok(cand);
    }
    let mut out = RoaringBitmap::new();
    for row in &cand {
        if phrase_aligned(&fi, &term_idx, row) {
            out.insert(row);
        }
    }
    Ok(out)
}

/// True if some offset `o` has query term `i` at position `o + i` for all `i`.
fn phrase_aligned(fi: &super::reader::FieldIndex, term_idx: &[usize], row: u32) -> bool {
    let first = fi.positions_for(term_idx[0], row);
    'offset: for &p0 in first {
        for (i, &ti) in term_idx.iter().enumerate().skip(1) {
            let want = p0 + i as u32;
            if fi.positions_for(ti, row).binary_search(&want).is_err() {
                continue 'offset;
            }
        }
        return true;
    }
    false
}

fn exact_term(block: &Block, field: &str, term: &str) -> Result<RoaringBitmap> {
    Ok(block.term_postings(field, term)?.unwrap_or_default())
}

fn glob_over_dict(block: &Block, field: &str, pattern: &str) -> Result<RoaringBitmap> {
    block.field_postings_or(field, |term| glob_match(pattern, term))
}

// numeric / range

fn eval_range(
    block: &Block,
    field: &str,
    lo: &Bound<String>,
    hi: &Bound<String>,
) -> Result<RoaringBitmap> {
    let path = field.to_lowercase();
    let mut acc = RoaringBitmap::new();
    // i64 column, parsing bounds as i64.
    if let (Some(l), Some(h)) = (parse_bound_f64(lo), parse_bound_f64(hi)) {
        acc |= range_scan(block, &path, LogType::I64, l, h)?;
        acc |= range_scan(block, &path, LogType::F64, l, h)?;
    }
    Ok(acc)
}

fn range_scan(
    block: &Block,
    path: &str,
    ty: LogType,
    lo: Bound<f64>,
    hi: Bound<f64>,
) -> Result<RoaringBitmap> {
    let Some(col) = block.value_column(path, ty)? else {
        return Ok(RoaringBitmap::new());
    };
    // Footer min/max prune: skip the column if the window can't intersect it.
    if let Bound::Included(l) | Bound::Excluded(l) = lo {
        if col.max < l {
            return Ok(RoaringBitmap::new());
        }
    }
    if let Bound::Included(h) | Bound::Excluded(h) = hi {
        if col.min > h {
            return Ok(RoaringBitmap::new());
        }
    }
    let vals: Vec<f64> = match &col.values {
        ColumnValues::I64(v) => v.iter().map(|&x| x as f64).collect(),
        ColumnValues::F64(v) => v.clone(),
        ColumnValues::Bool(_) => return Ok(RoaringBitmap::new()),
    };
    let mut out = RoaringBitmap::new();
    for (rid, v) in col.present.iter().zip(vals.iter()) {
        if in_bounds(*v, &lo, &hi) {
            out.insert(rid);
        }
    }
    Ok(out)
}

fn numeric_equals(block: &Block, path: &str, ty: LogType, target: f64) -> Result<RoaringBitmap> {
    let Some(col) = block.value_column(path, ty)? else {
        return Ok(RoaringBitmap::new());
    };
    let vals: Vec<f64> = match &col.values {
        ColumnValues::I64(v) => v.iter().map(|&x| x as f64).collect(),
        ColumnValues::F64(v) => v.clone(),
        ColumnValues::Bool(_) => return Ok(RoaringBitmap::new()),
    };
    let mut out = RoaringBitmap::new();
    for (rid, v) in col.present.iter().zip(vals.iter()) {
        if *v == target {
            out.insert(rid);
        }
    }
    Ok(out)
}

fn bool_equals(block: &Block, path: &str, target: bool) -> Result<RoaringBitmap> {
    let Some(col) = block.value_column(path, LogType::Bool)? else {
        return Ok(RoaringBitmap::new());
    };
    match col.values {
        ColumnValues::Bool(truth) => Ok(if target { truth } else { col.present - truth }),
        _ => Ok(RoaringBitmap::new()),
    }
}

// Time window

/// Rows whose timestamp is in `[start, end]` (inclusive). Timestamps are
/// ascending, so this is a contiguous range found by binary search.
fn time_rows(block: &Block, start_ms: Option<i64>, end_ms: Option<i64>) -> Result<RoaringBitmap> {
    let ts = block.timestamps()?;
    let lo = match start_ms {
        Some(s) => ts.partition_point(|&t| t < s),
        None => 0,
    };
    let hi = match end_ms {
        Some(e) => ts.partition_point(|&t| t <= e),
        None => ts.len(),
    };
    let mut bm = RoaringBitmap::new();
    if lo < hi {
        bm.insert_range(lo as u32..hi as u32);
    }
    Ok(bm)
}

// Helpers

fn all_rows(block: &Block) -> RoaringBitmap {
    let mut bm = RoaringBitmap::new();
    if block.row_count() > 0 {
        bm.insert_range(0..block.row_count());
    }
    bm
}

fn in_bounds(v: f64, lo: &Bound<f64>, hi: &Bound<f64>) -> bool {
    let lo_ok = match lo {
        Bound::Included(l) => v >= *l,
        Bound::Excluded(l) => v > *l,
        Bound::Unbounded => true,
    };
    let hi_ok = match hi {
        Bound::Included(h) => v <= *h,
        Bound::Excluded(h) => v < *h,
        Bound::Unbounded => true,
    };
    lo_ok && hi_ok
}

fn parse_bound_f64(b: &Bound<String>) -> Option<Bound<f64>> {
    match b {
        Bound::Included(s) => s.parse().ok().map(Bound::Included),
        Bound::Excluded(s) => s.parse().ok().map(Bound::Excluded),
        Bound::Unbounded => Some(Bound::Unbounded),
    }
}

fn has_wildcard(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

/// Minimal glob: `*` = any run, `?` = one char. Matches the surface
/// [`crate::search::query::build`] exposes (only globs, never raw regex).
fn glob_match(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark): (Option<usize>, usize) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn tokenize_lower(text: &str) -> Vec<String> {
    tokenize(text)
        .into_iter()
        .map(|t| t.text.to_lowercase())
        .collect()
}

fn millis_to_rfc3339(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.to_rfc3339())
}
