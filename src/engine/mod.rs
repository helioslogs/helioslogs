// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Engine seam: [`PartitionEngine`] hides the storage backend behind one
//! interface (only impl is [`block::BlockEngine`]); cross-partition orchestration
//! stays in [`crate::search`]. In/out are engine-neutral ([`Node`] AST, `Hit`).

pub mod block;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::catalog::{Catalog, PartitionKey};
use crate::schema::Fields;
use crate::search::query::Node;
use crate::search::Hit;

/// Inclusive time-range filter applied to a per-partition operation. Both
/// bounds are optional (unbounded when `None`).
#[derive(Clone, Copy, Default)]
pub struct TimeRange {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

/// Per-field terms-agg result: `field → key_str → (doc_count, original Value)`.
/// Counts are **raw** (unscaled); the cross-partition layer applies sampling scale.
pub type FieldBuckets = BTreeMap<String, BTreeMap<String, (u64, Value)>>;

/// Per-partition read/write operations a storage backend must provide. Engine-
/// neutral in and out; `Send + Sync` so the layer can fan out across rayon workers.
pub trait PartitionEngine: Send + Sync {
    /// Filter + time-DESC top-`limit`. Returns `(matching_count, hits)`; each hit
    /// carries its sort timestamp (epoch millis) for the merge. Empty ⇒ `(0, [])`.
    fn search(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        limit: usize,
    ) -> Result<(u64, Vec<(i64, Hit)>)>;

    /// Date-histogram bucket counts for one partition: `epoch_millis → count`.
    /// `interval` is a backend fixed-interval string (e.g. "5m", "1h").
    fn histogram(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        interval: &str,
    ) -> Result<BTreeMap<i64, u64>>;

    /// Combined [`search`](Self::search) + [`histogram`](Self::histogram) in one
    /// block pass (filter evaluated once). Buckets hold only non-empty buckets.
    #[allow(clippy::type_complexity)]
    fn search_histogram(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        limit: usize,
        interval: &str,
    ) -> Result<(u64, Vec<(i64, Hit)>, BTreeMap<i64, u64>)>;

    /// Terms aggregation over `fields` (+ optional total `want_count` for the
    /// `index` field). Counts **raw**; callers over-fetch `size_each` for merge accuracy.
    fn terms(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        fields: &[String],
        size_each: u32,
        want_count: bool,
    ) -> Result<(u64, FieldBuckets)>;

    /// Per-partition introspection: `(num_docs, num_segments)`, or `None` if
    /// the partition doesn't exist on disk.
    fn partition_stats(&self, key: &PartitionKey) -> Result<Option<(u64, usize)>>;

    /// Unordered scan of up to `limit` matching docs + total `count`, for the pipe
    /// executor's `| stats`. Returns `(count, hits)`; order is unspecified.
    fn scan(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        limit: usize,
    ) -> Result<(u64, Vec<Hit>)>;
}

// Block engine configuration

/// Block compression codec from `HELIOS_BLOCK_COMPRESSION` (default on). Set to
/// `off`/`none`/`0` to write uncompressed blocks for the compression benchmark.
pub fn block_codec() -> block::Codec {
    match std::env::var("HELIOS_BLOCK_COMPRESSION") {
        Ok(v) if matches!(v.to_lowercase().as_str(), "off" | "none" | "0" | "false") => {
            block::Codec::None
        }
        _ => block::Codec::Zstd,
    }
}

/// Query fan-out thread count (env > control setting > default). Bounds both
/// scatter and block scan (shared rayon pool), so it's the total query-CPU
/// ceiling across requests. Applied once at startup — rayon's global pool is
/// immutable, so a change only takes effect on the next restart.
pub fn query_threads() -> usize {
    crate::runtime_config::query_threads()
}

/// Size rayon's global pool for query work. Call once at serve startup, before
/// any query runs (rayon's global pool initializes exactly once).
pub fn configure_query_pool() {
    let n = query_threads();
    if let Err(e) = rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build_global()
    {
        tracing::warn!("query pool already initialized; HELIOS_QUERY_THREADS={n} not applied: {e}");
    }
}

/// Per-block content-match cache size in MB (env > control setting > default;
/// `0` = off). Caches row-match bitmaps by `(block_id, filter)` so repeats skip
/// re-scanning. Live-resizable via the runtime-config refresher.
pub fn query_cache_mb() -> usize {
    crate::runtime_config::query_cache_mb()
}

/// One-line description of the active engine selection for startup logging.
pub fn engine_startup_summary() -> String {
    let codec = match block_codec() {
        block::Codec::Zstd => "on (zstd)",
        block::Codec::None => "OFF",
    };
    let cache = match query_cache_mb() {
        0 => "off".to_string(),
        mb => format!("{mb} MB"),
    };
    format!(
        "block engine (custom) — compression {codec}, query threads {}, query cache {cache}",
        query_threads()
    )
}

/// Discover all partitions by listing the (possibly shared/S3) block store. The
/// cross-partition planner filters this with [`crate::catalog::filter_partitions`].
pub fn discover_partitions(catalog: &Catalog) -> Vec<PartitionKey> {
    block::configured_store(catalog.root())
        .list_partitions()
        .unwrap_or_default()
}

/// Build the read engine for this request — the block engine over this
/// instance's configured store.
pub fn select_read_engine<'a>(
    catalog: &'a Catalog,
    _fields: &'a Fields,
) -> Box<dyn PartitionEngine + 'a> {
    Box::new(block::BlockEngine::with_store(
        block::configured_store(catalog.root()),
        block_codec(),
    ))
}
