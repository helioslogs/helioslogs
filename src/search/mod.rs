// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! The **read** path. Owns the public `search` / `histogram` / `aggregate` /
//! `stats` functions; delegates the query language, partition planning, and
//! pipe execution to the submodules.

pub mod discover;
pub mod pipeline;
pub mod query;
pub mod scatter;

use anyhow::{bail, Result};
use chrono::{DateTime, TimeZone, Utc};
use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Instant;

use crate::catalog::{Catalog, PartitionKey};
use crate::engine::{select_read_engine, FieldBuckets, TimeRange};
use crate::schema::Fields;
use query::extract_highlight_terms;
use scatter::{plan, plan_with_explicit_keys};

// -------- search (scatter-gather) --------

/// A search result row. Schema-on-read: only universal-core fields are typed;
/// everything else lives in `raw` as the verbatim ingested JSON.
#[derive(Serialize, Clone)]
pub struct Hit {
    pub timestamp: Option<String>,
    pub message: Option<String>,
    pub score: f32,
    pub partition: String, // "<index>/<day>"
    /// Per-event source tag set during ingestion (distinct from the
    /// `index` partition key). `None` when the doc didn't set one.
    pub source: Option<String>,
    /// Original ingested JSON for this event — the verbatim source record.
    /// All non-universal-core fields are read from here by the frontend.
    pub raw: Option<String>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub total: u64,
    pub took_us: u128,
    pub hits: Vec<Hit>,
    pub highlight_terms: Vec<String>,
    pub partitions_scanned: usize,
    /// Echoed back so the UI doesn't need to recompute. `offset` is the
    /// 0-based index of the first returned hit; `limit` is page size.
    pub offset: usize,
    pub limit: usize,
    /// Populated for pipe queries; the frontend renders a table, not hits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<crate::search::pipeline::Table>,
}

pub fn search(
    catalog: &Catalog,
    fields: &Fields,
    query_str: &str,
    env: Option<&str>,
    index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    offset: usize,
    limit: usize,
    extra_index_filter: &[crate::control::settings::EnvIndexAllow],
    explicit_partitions: Option<Vec<PartitionKey>>,
) -> Result<SearchResponse> {
    // Pipe queries need every matching doc, so they return a table and reject
    // the per-partition streaming mode (explicit_partitions).
    if crate::search::pipeline::has_pipe(query_str) {
        if explicit_partitions.is_some() {
            bail!(
                "pipe queries (`| stats`, `| top`, etc.) do not support the partition-bounded \
                 search mode — they need all matching docs to compute the final table"
            );
        }
        let pipeline = crate::search::pipeline::parse_pipeline(query_str)?;
        let table = crate::search::pipeline::execute(
            catalog,
            fields,
            &pipeline,
            env,
            index,
            start,
            end,
            extra_index_filter,
        )?;
        return Ok(SearchResponse {
            total: table.scanned_docs as u64,
            took_us: table.took_us,
            hits: Vec::new(),
            highlight_terms: extract_highlight_terms(&pipeline.search_str),
            partitions_scanned: table.partitions_scanned,
            offset: 0,
            limit: 0,
            table: Some(table),
        });
    }

    let p = plan_with_explicit_keys(
        catalog,
        query_str,
        env,
        index,
        start,
        end,
        extra_index_filter,
        explicit_partitions,
    )?;
    let keys = p.keys;
    let stripped = p.node;
    let limit = limit.max(1);
    // Any partition could supply the whole window, so fetch offset+limit each
    // and slice after the cross-partition sort.
    let per_partition_limit = offset.saturating_add(limit).max(1);

    let t0 = Instant::now();

    // Fan out per-partition through the engine seam; each rayon worker returns
    // a partial (count, hits) that we sum + merge after.
    let engine = select_read_engine(catalog, fields);
    let time = TimeRange { start, end };
    let partials: Vec<(u64, Vec<(i64, Hit)>)> = keys
        .par_iter()
        .map(|k| engine.search(k, stripped.as_ref(), time, per_partition_limit))
        .collect::<Result<Vec<_>>>()?;

    let scanned = partials.len();
    let total: u64 = partials.iter().map(|(c, _)| *c).sum();
    let mut all_hits: Vec<(i64, Hit)> = partials
        .into_iter()
        .flat_map(|(_, h)| h.into_iter())
        .collect();

    // Cross-partition sort, then take the requested page slice.
    all_hits.sort_by(|a, b| b.0.cmp(&a.0));
    let page: Vec<Hit> = all_hits
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, h)| h)
        .collect();

    Ok(SearchResponse {
        total,
        took_us: t0.elapsed().as_micros(),
        hits: page,
        highlight_terms: extract_highlight_terms(query_str),
        partitions_scanned: scanned,
        offset,
        limit,
        table: None,
    })
}

// -------- histogram (per-partition results merged by bucket key) --------

#[derive(Serialize)]
pub struct HistogramBucket {
    pub t: String,
    pub count: u64,
}

#[derive(Serialize)]
pub struct HistogramResponse {
    pub interval_ms: u64,
    pub took_us: u128,
    pub buckets: Vec<HistogramBucket>,
}

pub fn histogram(
    catalog: &Catalog,
    _fields: &Fields,
    query_str: &str,
    env: Option<&str>,
    index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    interval: &str,
    extra_index_filter: &[crate::control::settings::EnvIndexAllow],
    explicit_partitions: Option<Vec<PartitionKey>>,
) -> Result<HistogramResponse> {
    let p = plan_with_explicit_keys(
        catalog,
        query_str,
        env,
        index,
        start,
        end,
        extra_index_filter,
        explicit_partitions,
    )?;
    let keys = p.keys;
    let stripped = p.node;
    let interval_ms = parse_interval_ms(interval).unwrap_or(0);
    let t0 = Instant::now();

    // Fan out per-partition histogram aggs through the engine seam; sum bucket
    // counts after.
    let engine = select_read_engine(catalog, _fields);
    let time = TimeRange { start, end };
    let partials: Vec<BTreeMap<i64, u64>> = keys
        .par_iter()
        .map(|k| engine.histogram(k, stripped.as_ref(), time, interval))
        .collect::<Result<Vec<_>>>()?;

    let mut merged: BTreeMap<i64, u64> = BTreeMap::new();
    for partial in partials {
        for (k, v) in partial {
            *merged.entry(k).or_default() += v;
        }
    }

    let out: Vec<HistogramBucket> = merged
        .into_iter()
        .map(|(k, count)| HistogramBucket {
            t: Utc
                .timestamp_millis_opt(k)
                .single()
                .map(|t| t.to_rfc3339())
                .unwrap_or_default(),
            count,
        })
        .collect();

    Ok(HistogramResponse {
        interval_ms,
        took_us: t0.elapsed().as_micros(),
        buckets: out,
    })
}

// -------- combined search + histogram (one block pass per partition) --------

#[derive(Serialize)]
pub struct SearchHistogramResponse {
    pub total: u64,
    pub took_us: u128,
    pub hits: Vec<Hit>,
    pub highlight_terms: Vec<String>,
    pub partitions_scanned: usize,
    pub offset: usize,
    pub limit: usize,
    pub interval_ms: u64,
    pub buckets: Vec<HistogramBucket>,
}

/// Serve hits + histogram in one fan-out: each partition evaluates the filter
/// once and returns both. Pipe queries are rejected (they belong on `/search`).
#[allow(clippy::too_many_arguments)]
pub fn search_histogram(
    catalog: &Catalog,
    fields: &Fields,
    query_str: &str,
    env: Option<&str>,
    index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    offset: usize,
    limit: usize,
    interval: &str,
    extra_index_filter: &[crate::control::settings::EnvIndexAllow],
    explicit_partitions: Option<Vec<PartitionKey>>,
) -> Result<SearchHistogramResponse> {
    if crate::search::pipeline::has_pipe(query_str) {
        bail!("pipe queries are not supported on /api/search_histogram — use /api/search");
    }

    let p = plan_with_explicit_keys(
        catalog,
        query_str,
        env,
        index,
        start,
        end,
        extra_index_filter,
        explicit_partitions,
    )?;
    let keys = p.keys;
    let stripped = p.node;
    let limit = limit.max(1);
    let per_partition_limit = offset.saturating_add(limit).max(1);
    let interval_ms = parse_interval_ms(interval).unwrap_or(0);
    let t0 = Instant::now();

    let engine = select_read_engine(catalog, fields);
    let time = TimeRange { start, end };
    let partials: Vec<(u64, Vec<(i64, Hit)>, BTreeMap<i64, u64>)> = keys
        .par_iter()
        .map(|k| engine.search_histogram(k, stripped.as_ref(), time, per_partition_limit, interval))
        .collect::<Result<Vec<_>>>()?;

    let scanned = partials.len();

    // Pre-fill the empty bucket range once, then sum partition counts in —
    // same result as `/histogram` without paying the fill per partition.
    let mut merged: BTreeMap<i64, u64> = BTreeMap::new();
    if interval_ms > 0 {
        if let (Some(s), Some(e)) = (start, end) {
            let i = interval_ms as i64;
            let (s_ms, e_ms) = (s.timestamp_millis(), e.timestamp_millis());
            let mut k = s_ms - s_ms.rem_euclid(i);
            while k <= e_ms {
                merged.insert(k, 0);
                k += i;
            }
        }
    }

    let mut total = 0u64;
    let mut all_hits: Vec<(i64, Hit)> = Vec::new();
    for (count, hits, buckets) in partials {
        total += count;
        all_hits.extend(hits);
        for (k, v) in buckets {
            *merged.entry(k).or_default() += v;
        }
    }

    all_hits.sort_by(|a, b| b.0.cmp(&a.0));
    let page: Vec<Hit> = all_hits
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, h)| h)
        .collect();

    let out_buckets: Vec<HistogramBucket> = merged
        .into_iter()
        .map(|(k, count)| HistogramBucket {
            t: Utc
                .timestamp_millis_opt(k)
                .single()
                .map(|t| t.to_rfc3339())
                .unwrap_or_default(),
            count,
        })
        .collect();

    Ok(SearchHistogramResponse {
        total,
        took_us: t0.elapsed().as_micros(),
        hits: page,
        highlight_terms: extract_highlight_terms(query_str),
        partitions_scanned: scanned,
        offset,
        limit,
        interval_ms,
        buckets: out_buckets,
    })
}

pub(crate) fn parse_interval_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| c.is_alphabetic())?);
    let n: u64 = num.parse().ok()?;
    let mult: u64 = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return None,
    };
    Some(n * mult)
}

// -------- terms aggregation (per-partition results summed by key) --------

#[derive(Serialize, Clone)]
pub struct TopBucket {
    pub key: Value,
    pub count: u64,
}

#[derive(Serialize)]
pub struct AggregateResponse {
    pub took_us: u128,
    pub aggs: BTreeMap<String, Vec<TopBucket>>,
    /// True when partition sampling was applied — bucket counts are scaled
    /// estimates, not exact totals. UI surfaces this with an "≈" badge.
    pub sampled: bool,
    /// Partitions actually scanned. Equals `total_partitions` when not
    /// sampled. Always present so callers don't have to special-case.
    pub sampled_partitions: usize,
    /// Partitions matching the query window before sampling.
    pub total_partitions: usize,
}

/// `index` is a partition key computed at the catalog layer, not an engine agg.
pub const SYNTHETIC_AGG_FIELD: &str = "index";

pub fn aggregate(
    catalog: &Catalog,
    _fields: &Fields,
    query_str: &str,
    env: Option<&str>,
    index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    field_names: &[String],
    size: u32,
    approximate: bool,
    extra_index_filter: &[crate::control::settings::EnvIndexAllow],
) -> Result<AggregateResponse> {
    let p = plan(
        catalog,
        query_str,
        env,
        index,
        start,
        end,
        extra_index_filter,
    )?;
    let all_keys = p.keys;
    let stripped = p.node;
    let t0 = Instant::now();

    // Stride sampling: take every Nth catalog-sorted key and scale counts by N.
    // Catalog order `(index asc, day desc)` makes this a uniform-time sample.
    let total_partitions = all_keys.len();
    let max_partitions = crate::runtime_config::agg_max_partitions();
    let (keys, sampling_ratio, sampled) = if approximate && total_partitions > max_partitions {
        let stride = total_partitions.div_ceil(max_partitions);
        let sampled_keys: Vec<PartitionKey> = all_keys.into_iter().step_by(stride).collect();
        let ratio = total_partitions as f64 / sampled_keys.len().max(1) as f64;
        (sampled_keys, ratio, true)
    } else {
        (all_keys, 1.0, false)
    };
    let sampled_partitions = keys.len();

    // Split synthetic "index" (catalog-layer counts) from real doc fields,
    // which route through TermsAggregation on `dynamic.<name>`.
    let mut want_index_names: Vec<String> = Vec::new();
    let mut wanted_doc_fields: Vec<String> = Vec::new();
    for n in field_names {
        if n == SYNTHETIC_AGG_FIELD {
            want_index_names.push(n.clone());
        } else {
            wanted_doc_fields.push(n.clone());
        }
    }

    // Reject universal-core fields with no columnar backing to group on. Only
    // `source` (via `source_raw`) and the already-split `index` are aggregatable.
    for n in &wanted_doc_fields {
        if matches!(n.as_str(), "message" | "raw") {
            bail!(
                "field `{n}` is not aggregatable (text field, no fast column). \
                 Aggregatable universal-core fields: `source`, `index`. \
                 Any other JSON key from the events is aggregatable via the dynamic column."
            );
        }
        if n == "timestamp" {
            bail!(
                "field `timestamp` is not aggregatable as terms — use `/api/histogram` for time bucketing."
            );
        }
    }
    if want_index_names.is_empty() && wanted_doc_fields.is_empty() {
        return Ok(AggregateResponse {
            took_us: 0,
            aggs: BTreeMap::new(),
            sampled,
            sampled_partitions,
            total_partitions,
        });
    }

    // Scale counts when sampling (no-op at ratio 1.0). Round-to-nearest so the
    // common value doesn't lose a count per partition.
    let scale = |c: u64| -> u64 {
        if sampled {
            (c as f64 * sampling_ratio).round() as u64
        } else {
            c
        }
    };

    // Per-partition partial: (index_name, index_count, per_field_buckets).
    // Apply `scale` per partition before the cross-partition merge below.
    let engine = select_read_engine(catalog, _fields);
    let time = TimeRange { start, end };
    let want_count = !want_index_names.is_empty();
    // Pull 2x per-partition so the merged top-N stays accurate.
    let per_partition_size = size.saturating_mul(2).max(size);
    let partials: Vec<(String, u64, FieldBuckets)> = keys
        .par_iter()
        .map(|k| -> Result<(String, u64, FieldBuckets)> {
            let (raw_count, mut buckets) = engine.terms(
                k,
                stripped.as_ref(),
                time,
                &wanted_doc_fields,
                per_partition_size,
                want_count,
            )?;
            for bucket_map in buckets.values_mut() {
                for entry in bucket_map.values_mut() {
                    entry.0 = scale(entry.0);
                }
            }
            Ok((k.index.clone(), scale(raw_count), buckets))
        })
        .collect::<Result<Vec<_>>>()?;

    // field_name → key → (count, original-Value), for the real doc fields.
    let mut merged: BTreeMap<String, BTreeMap<String, (u64, Value)>> = BTreeMap::new();
    for n in &wanted_doc_fields {
        merged.insert(n.clone(), BTreeMap::new());
    }
    let mut index_counts: BTreeMap<String, u64> = BTreeMap::new();

    for (index_name, count, local_field_buckets) in partials {
        if !want_index_names.is_empty() {
            *index_counts.entry(index_name).or_default() += count;
        }
        for (name, bucket_map) in local_field_buckets {
            let target = merged.entry(name).or_default();
            for (key_str, (c, key_val)) in bucket_map {
                let entry = target.entry(key_str).or_insert_with(|| (0, key_val));
                entry.0 += c;
            }
        }
    }

    let mut out: BTreeMap<String, Vec<TopBucket>> = BTreeMap::new();
    for (name, bucket_map) in merged {
        let mut items: Vec<(u64, Value)> = bucket_map.into_values().map(|(c, k)| (c, k)).collect();
        items.sort_by(|a, b| b.0.cmp(&a.0));
        items.truncate(size as usize);
        let buckets: Vec<TopBucket> = items
            .into_iter()
            .map(|(count, key)| TopBucket { key, count })
            .collect();
        out.insert(name, buckets);
    }

    if !want_index_names.is_empty() {
        // Skip zero-count indexes (queries that pruned them away entirely).
        let mut items: Vec<(u64, Value)> = index_counts
            .into_iter()
            .filter(|(_, c)| *c > 0)
            .map(|(name, c)| (c, Value::String(name)))
            .collect();
        items.sort_by(|a, b| b.0.cmp(&a.0));
        items.truncate(size as usize);
        let buckets: Vec<TopBucket> = items
            .into_iter()
            .map(|(count, key)| TopBucket { key, count })
            .collect();
        for name in &want_index_names {
            out.insert(name.clone(), buckets.clone());
        }
    }

    Ok(AggregateResponse {
        took_us: t0.elapsed().as_micros(),
        aggs: out,
        sampled,
        sampled_partitions,
        total_partitions,
    })
}

// -------- stats (aggregate across partitions) --------

#[derive(Serialize)]
pub struct Stats {
    pub num_docs: u64,
    pub num_segments: usize,
    pub num_partitions: usize,
}

pub fn stats(catalog: &Catalog) -> Result<Stats> {
    let keys = crate::engine::discover_partitions(catalog);
    // `partition_stats` doesn't use schema fields; build a throwaway `Fields`
    // (the schema is shared + deterministic) just to construct the engine.
    let fields = crate::schema::build_schema();
    let engine = select_read_engine(catalog, &fields);
    let mut docs = 0u64;
    let mut segs = 0usize;
    for k in &keys {
        if let Some((d, s)) = engine.partition_stats(k)? {
            docs += d;
            segs += s;
        }
    }
    Ok(Stats {
        num_docs: docs,
        num_segments: segs,
        num_partitions: keys.len(),
    })
}

// -------- CLI flavor --------

pub fn cli_print_search(
    catalog: &Catalog,
    fields: &Fields,
    query_str: &str,
    index: Option<&str>,
    limit: usize,
) -> Result<()> {
    // CLI has no env picker — scan every user env (scope `None`); system envs
    // are reachable only via the HTTP/UI by selecting `_system`.
    let resp = search(
        catalog,
        fields,
        query_str,
        None,
        index,
        None,
        None,
        0,
        limit,
        &[],
        None,
    )?;
    println!(
        "{} hits in {}µs across {} partitions (showing {})",
        resp.total,
        resp.took_us,
        resp.partitions_scanned,
        resp.hits.len()
    );
    for h in resp.hits {
        println!(
            "  [{}] {} {:18} {}",
            h.partition,
            h.timestamp.unwrap_or_default(),
            h.source.unwrap_or_default(),
            h.message.unwrap_or_default(),
        );
    }
    Ok(())
}
