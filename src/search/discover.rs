// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Field discovery — two surfaces: [`field_catalog`] (footer-derived, exact, query-
//! independent, block-id-cached) backs the sidebar; [`discover_fields`] (doc-sampling,
//! query-aware with sample values) backs the agent/MCP tools.

use crate::catalog::Catalog;
use crate::engine::{select_read_engine, TimeRange};
use crate::schema::Fields;
use crate::search::scatter::plan;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub const MAX_SAMPLE: usize = 10_000;
pub const DEFAULT_TOP: usize = 30;
pub const MAX_TOP: usize = 100;
/// Past this, stop collecting values and flag the field high-cardinality.
const CARDINALITY_CAP: usize = 1000;
/// Distinct values returned per field for instant preview.
const SAMPLE_VALUES_KEEP: usize = 5;

// "groupable" thresholds: keep facets dense enough (density = hits/cardinality)
// and present enough to be useful; drop UID-like and constant fields.
const MIN_COVERAGE: f64 = 0.05;
const MIN_DENSITY: f64 = 3.0;
const MIN_CARDINALITY: usize = 2;

// "interesting" thresholds (auto-expand panel): glanceable facets only —
// high coverage AND low cardinality (severity/status/method shape).
const INTERESTING_MIN_COVERAGE: f64 = 0.5;
const INTERESTING_MAX_CARDINALITY: usize = 20;

/// Universal-core fields, rendered structurally so discovery skips them.
const UNIVERSAL_CORE: &[&str] = &["timestamp", "message", "raw", "source", "index"];

#[derive(Serialize)]
pub struct DiscoverResponse {
    pub took_us: u128,
    pub sample_size: usize,
    pub partitions_scanned: usize,
    pub fields: Vec<DiscoveredField>,
}

#[derive(Serialize)]
pub struct DiscoveredField {
    pub name: String,
    /// Fraction of sampled docs that contained this key (0.0–1.0).
    pub coverage: f64,
    /// Number of distinct values seen, capped at `CARDINALITY_CAP`.
    pub cardinality_seen: usize,
    /// True when distinct-value tracking hit the cap and more values exist.
    pub cardinality_capped: bool,
    pub value_kind: ValueKind,
    /// First few distinct values, for the sidebar's instant preview.
    pub sample_values: Vec<Value>,
    /// True when a terms-agg yields useful buckets; sidebar filters by this.
    pub groupable: bool,
    /// True when glanceable enough to default-expand. Tighter than `groupable`.
    pub interesting: bool,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    String,
    Int,
    Float,
    Bool,
    Mixed,
}

/// Per-field accumulator. Distinct values keyed by stringified form (Value
/// isn't Hash); the original Values live in `sample_values`.
#[derive(Default)]
struct Stat {
    coverage_hits: usize,
    distinct_keys: HashSet<String>,
    distinct_capped: bool,
    sample_values: Vec<Value>,
    kind: Option<ValueKind>,
}

impl Stat {
    /// One observation of a non-null, non-object, non-array value at this path.
    fn observe(&mut self, value: &Value) {
        self.coverage_hits += 1;

        let new_kind = classify(value);
        self.kind = Some(match (self.kind, new_kind) {
            (None, k) => k,
            (Some(prev), k) if prev == k => prev,
            // Any mismatch collapses to Mixed — the UI only distinguishes
            // string vs numeric vs mixed anyway.
            _ => ValueKind::Mixed,
        });

        if self.distinct_capped {
            return;
        }
        let k = stringify_for_dedup(value);
        if self.distinct_keys.insert(k) {
            if self.sample_values.len() < SAMPLE_VALUES_KEEP {
                self.sample_values.push(value.clone());
            }
            if self.distinct_keys.len() >= CARDINALITY_CAP {
                self.distinct_capped = true;
            }
        }
    }

    /// Fold another partition's Stat for the same field path into `self`.
    /// Symmetric: equivalent to observe()-ing the union of inputs.
    fn merge(&mut self, other: Stat) {
        self.coverage_hits += other.coverage_hits;

        self.kind = match (self.kind, other.kind) {
            (None, k) | (k, None) => k,
            (Some(a), Some(b)) if a == b => Some(a),
            _ => Some(ValueKind::Mixed),
        };

        // Once either side hit the cap, the merged set is conceptually
        // capped — past `CARDINALITY_CAP` we never grow the value list.
        if other.distinct_capped {
            self.distinct_capped = true;
        }
        if !self.distinct_capped {
            for k in other.distinct_keys {
                if self.distinct_keys.insert(k) && self.distinct_keys.len() >= CARDINALITY_CAP {
                    self.distinct_capped = true;
                    break;
                }
            }
        }

        for v in other.sample_values {
            if self.sample_values.len() >= SAMPLE_VALUES_KEEP {
                break;
            }
            if !self.sample_values.iter().any(|x| x == &v) {
                self.sample_values.push(v);
            }
        }
    }
}

pub fn discover_fields(
    catalog: &Catalog,
    _fields: &Fields,
    query_str: &str,
    env: Option<&str>,
    index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    sample: usize,
    top: usize,
    extra_index_filter: &[crate::control::settings::EnvIndexAllow],
) -> Result<DiscoverResponse> {
    let sample = sample.clamp(1, MAX_SAMPLE);
    let top = top.clamp(1, MAX_TOP);

    let p = plan(
        catalog,
        query_str,
        env,
        index,
        start,
        end,
        extra_index_filter,
    )?;
    let keys = p.keys;
    let stripped = p.node;
    let t0 = Instant::now();

    // Split the budget evenly across partitions for parallelism and uniform-time
    // coverage, trading away the old recency-biased sample ordering.
    let per_partition = sample.div_ceil(keys.len().max(1)).max(1);

    // Sample the most-recent `per_partition` matching docs via the engine seam
    // (same time-DESC top-N as plain search), then walk each event's raw JSON.
    let engine = select_read_engine(catalog, _fields);
    let time = TimeRange { start, end };
    let partials: Vec<(usize, HashMap<String, Stat>)> = keys
        .par_iter()
        .map(|k| -> Result<(usize, HashMap<String, Stat>)> {
            let mut local: HashMap<String, Stat> = HashMap::new();
            let (_count, hits) = engine.search(k, stripped.as_ref(), time, per_partition)?;
            let mut sampled = 0usize;
            for (_ts, hit) in hits {
                let Some(s) = hit.raw.as_deref() else {
                    continue;
                };
                let Ok(Value::Object(map)) = serde_json::from_str::<Value>(s) else {
                    continue;
                };
                sampled += 1;
                for (key, val) in map {
                    if UNIVERSAL_CORE.contains(&key.as_str()) {
                        continue;
                    }
                    walk_value(&key, &val, &mut local);
                }
            }
            Ok((sampled, local))
        })
        .collect::<Result<Vec<_>>>()?;

    // Sum sampled-doc counts and union per-field Stats across partitions.
    let mut sample_size = 0usize;
    let mut stats: HashMap<String, Stat> = HashMap::new();
    for (n, partial) in partials {
        sample_size += n;
        for (key, stat) in partial {
            match stats.entry(key) {
                Entry::Occupied(mut e) => e.get_mut().merge(stat),
                Entry::Vacant(v) => {
                    v.insert(stat);
                }
            }
        }
    }

    // Rank: coverage × log(cardinality + 1). Favors common AND informative
    // fields over constant ones present everywhere.
    let mut ranked: Vec<(String, Stat)> = stats.into_iter().collect();
    ranked.sort_by(|a, b| {
        let sa = score(sample_size, &a.1);
        let sb = score(sample_size, &b.1);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked.truncate(top);

    let fields: Vec<DiscoveredField> = ranked
        .into_iter()
        .map(|(name, st)| {
            let coverage = if sample_size > 0 {
                st.coverage_hits as f64 / sample_size as f64
            } else {
                0.0
            };
            let cardinality = st.distinct_keys.len();
            let groupable =
                is_groupable(coverage, st.coverage_hits, cardinality, st.distinct_capped);
            let interesting = groupable && is_interesting(coverage, cardinality);
            DiscoveredField {
                coverage,
                cardinality_seen: cardinality,
                cardinality_capped: st.distinct_capped,
                value_kind: st.kind.unwrap_or(ValueKind::String),
                sample_values: st.sample_values,
                groupable,
                interesting,
                name,
            }
        })
        .collect();

    Ok(DiscoverResponse {
        took_us: t0.elapsed().as_micros(),
        sample_size,
        partitions_scanned: keys.len(),
        fields,
    })
}

// Field catalog — footer-derived (the true `(path, type)` columns with exact
// coverage), query-independent and cached by block id, so it's stable + cheap.

#[derive(Serialize)]
pub struct FieldCatalogResponse {
    pub took_us: u128,
    /// Total rows across the in-window blocks scanned (coverage denominator).
    pub total_rows: u64,
    pub partitions_scanned: usize,
    pub fields: Vec<CatalogField>,
}

#[derive(Serialize)]
pub struct CatalogField {
    pub name: String,
    /// Fraction of in-window rows carrying this field (0.0–1.0).
    pub coverage: f64,
    /// Distinct values — a lower bound (max over any single block; exact only
    /// for fields confined to one block). `0` for numeric columns (untracked).
    pub cardinality: u64,
    pub value_kind: ValueKind,
    /// Worth offering a value breakdown for (not a constant, present enough).
    pub groupable: bool,
    /// Low-cardinality categorical worth auto-pinning / default-expanding.
    pub interesting: bool,
}

// Kind bitset: Int=1, Float=2, Bool=4, Str=8.
const K_INT: u8 = 1;
const K_FLOAT: u8 = 2;
const K_BOOL: u8 = 4;
const K_STR: u8 = 8;

#[derive(Default)]
struct FieldAccumC {
    rows: u64,
    cardinality: u64,
    kinds: u8,
}

impl FieldAccumC {
    fn observe(&mut self, rows: u64, cardinality: u64, kind: crate::engine::block::FieldKind) {
        use crate::engine::block::FieldKind;
        self.rows += rows;
        self.cardinality = self.cardinality.max(cardinality);
        self.kinds |= match kind {
            FieldKind::Int => K_INT,
            FieldKind::Float => K_FLOAT,
            FieldKind::Bool => K_BOOL,
            FieldKind::Str => K_STR,
        };
    }

    fn merge(&mut self, other: FieldAccumC) {
        self.rows += other.rows;
        self.cardinality = self.cardinality.max(other.cardinality);
        self.kinds |= other.kinds;
    }

    fn value_kind(&self) -> ValueKind {
        match self.kinds {
            K_INT => ValueKind::Int,
            K_FLOAT => ValueKind::Float,
            K_BOOL => ValueKind::Bool,
            K_STR => ValueKind::String,
            _ => ValueKind::Mixed,
        }
    }
}

pub fn field_catalog(
    catalog: &Catalog,
    query_str: &str,
    env: Option<&str>,
    index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    top: usize,
    extra_index_filter: &[crate::control::settings::EnvIndexAllow],
) -> Result<FieldCatalogResponse> {
    let top = top.clamp(1, MAX_TOP);
    let p = plan(
        catalog,
        query_str,
        env,
        index,
        start,
        end,
        extra_index_filter,
    )?;
    let keys = p.keys;
    let t0 = Instant::now();
    let store = crate::engine::block::configured_store(catalog.root());
    let start_ms = start.map(|t| t.timestamp_millis());
    let end_ms = end.map(|t| t.timestamp_millis());

    // Per-partition: accumulate footer-derived stats from in-window blocks.
    let partials: Vec<(u64, HashMap<String, FieldAccumC>)> = keys
        .par_iter()
        .map(|k| -> Result<(u64, HashMap<String, FieldAccumC>)> {
            let ids = store.load_manifest(k)?.blocks;
            let mut total_rows = 0u64;
            let mut fields: HashMap<String, FieldAccumC> = HashMap::new();
            for id in &ids {
                // A named-but-unreadable block was compacted away under us (its
                // replacement lives in a newer, not-yet-mirrored manifest) — skip it.
                let Ok(stats) = store.block_field_stats(k, id) else {
                    continue;
                };
                if !block_overlaps(stats.min_ts, stats.max_ts, start_ms, end_ms) {
                    continue;
                }
                total_rows += stats.row_count as u64;
                for f in &stats.fields {
                    if UNIVERSAL_CORE.contains(&f.path.as_str()) {
                        continue;
                    }
                    fields.entry(f.path.clone()).or_default().observe(
                        f.rows as u64,
                        f.cardinality as u64,
                        f.kind,
                    );
                }
            }
            Ok((total_rows, fields))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut total_rows = 0u64;
    let mut fields: HashMap<String, FieldAccumC> = HashMap::new();
    for (rows, partial) in partials {
        total_rows += rows;
        for (path, fa) in partial {
            match fields.entry(path) {
                Entry::Occupied(mut e) => e.get_mut().merge(fa),
                Entry::Vacant(v) => {
                    v.insert(fa);
                }
            }
        }
    }

    let mut ranked: Vec<(String, FieldAccumC)> = fields.into_iter().collect();
    // Stable: coverage desc, then name asc — same data ⇒ same order every search.
    ranked.sort_by(|a, b| b.1.rows.cmp(&a.1.rows).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(top);

    let fields: Vec<CatalogField> = ranked
        .into_iter()
        .map(|(name, fa)| {
            let coverage = if total_rows > 0 {
                fa.rows as f64 / total_rows as f64
            } else {
                0.0
            };
            let kind = fa.value_kind();
            let groupable = catalog_groupable(coverage, fa.cardinality);
            let interesting = catalog_interesting(coverage, fa.cardinality, kind);
            CatalogField {
                name,
                coverage,
                cardinality: fa.cardinality,
                value_kind: kind,
                groupable,
                interesting,
            }
        })
        .collect();

    Ok(FieldCatalogResponse {
        took_us: t0.elapsed().as_micros(),
        total_rows,
        partitions_scanned: keys.len(),
        fields,
    })
}

/// Footer time-bounds prune (mirrors `Block::overlaps`).
fn block_overlaps(min_ts: i64, max_ts: i64, start: Option<i64>, end: Option<i64>) -> bool {
    if let Some(s) = start {
        if max_ts < s {
            return false;
        }
    }
    if let Some(e) = end {
        if min_ts > e {
            return false;
        }
    }
    true
}

/// Worth a value breakdown: present often enough and not a constant. Numeric
/// columns (cardinality 0 = unknown) stay groupable.
fn catalog_groupable(coverage: f64, cardinality: u64) -> bool {
    if coverage < MIN_COVERAGE {
        return false;
    }
    cardinality == 0 || cardinality >= MIN_CARDINALITY as u64
}

/// Auto-pin / default-expand candidate: a low-cardinality categorical
/// (string/bool) present in most rows — the severity/status/method shape.
fn catalog_interesting(coverage: f64, cardinality: u64, kind: ValueKind) -> bool {
    let categorical = matches!(kind, ValueKind::String | ValueKind::Bool);
    categorical
        && coverage >= INTERESTING_MIN_COVERAGE
        && (2..=INTERESTING_MAX_CARDINALITY as u64).contains(&cardinality)
}

/// Record every aggregatable leaf, descending one level into objects
/// (flattened with `.`). Arrays are skipped (need a multi-value facet UX).
fn walk_value(path: &str, value: &Value, stats: &mut std::collections::HashMap<String, Stat>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                walk_value(&child, v, stats);
            }
        }
        Value::Array(_) | Value::Null => {
            // Arrays: too much variance in shape to surface as a single facet.
            // Null: not informative on its own.
        }
        _ => {
            stats.entry(path.to_string()).or_default().observe(value);
        }
    }
}

fn classify(v: &Value) -> ValueKind {
    match v {
        Value::String(_) => ValueKind::String,
        Value::Bool(_) => ValueKind::Bool,
        Value::Number(n) if n.is_i64() || n.is_u64() => ValueKind::Int,
        Value::Number(_) => ValueKind::Float,
        _ => ValueKind::Mixed,
    }
}

fn stringify_for_dedup(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => v.to_string(),
    }
}

fn score(sample_size: usize, st: &Stat) -> f64 {
    if sample_size == 0 {
        return 0.0;
    }
    let coverage = st.coverage_hits as f64 / sample_size as f64;
    let card = st.distinct_keys.len() as f64;
    coverage * (1.0 + card).ln()
}

/// True when a terms-agg is worth showing: present often enough, not UID-like
/// (density > 1), more than one distinct value. A capped cardinality is out.
fn is_groupable(
    coverage: f64,
    coverage_hits: usize,
    cardinality: usize,
    cardinality_capped: bool,
) -> bool {
    if cardinality_capped {
        return false;
    }
    if cardinality < MIN_CARDINALITY {
        return false;
    }
    if coverage < MIN_COVERAGE {
        return false;
    }
    let density = coverage_hits as f64 / cardinality as f64;
    density >= MIN_DENSITY
}

/// Should the panel default-expand? Tighter than `is_groupable` (caller gates
/// on that first); judges glanceability: high coverage, low bucket count.
fn is_interesting(coverage: f64, cardinality: usize) -> bool {
    coverage >= INTERESTING_MIN_COVERAGE && cardinality <= INTERESTING_MAX_CARDINALITY
}
