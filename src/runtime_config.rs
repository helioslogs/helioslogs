// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Live-tunable server knobs, resolved as **env var > control setting > built-in
//! default**. Consumers read cheap relaxed atomics every time they need a value,
//! so edits on Admin → General take effect without a restart; a background task
//! ([`run_refresher`]) re-reads the control plane periodically and updates them.
//! An env override is captured at startup and always wins (the UI shows it locked).
//!
//! One knob is restart-only ([`Knob::QueryThreads`] — rayon's global pool is
//! immutable once built); its atomic is set once at [`init`] and never refreshed,
//! so its reported "effective" value is what the process actually booted with.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use crate::control::Control;

/// How often the refresher re-reads control-backed tunables.
const REFRESH_SECS: u64 = 15;

/// Identity of one live tunable. Discriminants index [`KNOBS`] and the atomic
/// cell vector, so the two MUST stay in the same order (asserted in tests).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Knob {
    BlockCompactSecs = 0,
    BlockTargetMb,
    BlockMinCompactMb,
    BlockMaxSmallBlocks,
    BlockFlushRows,
    BlockFlushSecs,
    BlockSyncSecs,
    RetentionSweepSecs,
    AggMaxPartitions,
    QueryCacheMb,
    QueryThreads,
    AuthTokenTtlHours,
}

/// Static metadata for one tunable — drives both resolution and the admin API.
pub struct KnobDef {
    pub id: Knob,
    /// Stable API id, e.g. `"block_compact_secs"`.
    pub slug: &'static str,
    /// Env var that overrides everything when set.
    pub env: &'static str,
    /// Control-plane settings key holding the configured value.
    pub key: &'static str,
    pub category: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub default: u64,
    /// Smallest accepted value; anything below is treated as unset (falls through).
    pub min_value: u64,
    /// `false` = a change needs a restart to take effect (shown in the UI).
    pub live: bool,
    pub description: &'static str,
}

pub const KNOBS: &[KnobDef] = &[
    KnobDef {
        id: Knob::BlockCompactSecs,
        slug: "block_compact_secs",
        env: "HELIOS_BLOCK_COMPACT_SECS",
        key: "tunable.block_compact_secs",
        category: "Storage engine",
        label: "Compaction interval",
        unit: "seconds",
        default: 30,
        min_value: 1,
        live: true,
        description: "How often the background compactor walks partitions and merges small blocks.",
    },
    KnobDef {
        id: Knob::BlockTargetMb,
        slug: "block_target_mb",
        env: "HELIOS_BLOCK_TARGET_MB",
        key: "tunable.block_target_mb",
        category: "Storage engine",
        label: "Compaction target size",
        unit: "MB",
        default: 64,
        min_value: 1,
        live: true,
        description: "Target size compaction packs small blocks toward.",
    },
    KnobDef {
        id: Knob::BlockMinCompactMb,
        slug: "block_min_compact_mb",
        env: "HELIOS_BLOCK_MIN_COMPACT_MB",
        key: "tunable.block_min_compact_mb",
        category: "Storage engine",
        label: "Compaction floor",
        unit: "MB",
        default: 5,
        min_value: 1,
        live: true,
        description: "Floor a merge group must reach before it's worth rewriting — avoids churning tiny blocks.",
    },
    KnobDef {
        id: Knob::BlockMaxSmallBlocks,
        slug: "block_max_small_blocks",
        env: "HELIOS_BLOCK_MAX_SMALL_BLOCKS",
        key: "tunable.block_max_small_blocks",
        category: "Storage engine",
        label: "Small-block waiver count",
        unit: "blocks",
        default: 100,
        min_value: 1,
        live: true,
        description: "Small-block count that waives the compaction floor — too many tiny files hurt query/listing cost even under the floor.",
    },
    KnobDef {
        id: Knob::BlockFlushRows,
        slug: "block_flush_rows",
        env: "HELIOS_BLOCK_FLUSH_ROWS",
        key: "tunable.block_flush_rows",
        category: "Storage engine",
        label: "Ingest flush rows",
        unit: "rows",
        default: 50_000,
        min_value: 1,
        live: true,
        description: "Buffered-ingest row-count threshold that triggers a block flush.",
    },
    KnobDef {
        id: Knob::BlockFlushSecs,
        slug: "block_flush_secs",
        env: "HELIOS_BLOCK_FLUSH_SECS",
        key: "tunable.block_flush_secs",
        category: "Storage engine",
        label: "Ingest flush interval",
        unit: "seconds",
        default: 5,
        min_value: 1,
        live: true,
        description: "Time-based flush interval for buffered ingest.",
    },
    KnobDef {
        id: Knob::BlockSyncSecs,
        slug: "block_sync_secs",
        env: "HELIOS_BLOCK_SYNC_SECS",
        key: "tunable.block_sync_secs",
        category: "Storage engine",
        label: "Shared-store sync interval",
        unit: "seconds",
        default: 10,
        min_value: 1,
        live: true,
        description: "Interval for the uploader/puller/seeder shared-store sync tasks. Only active with --shared-store.",
    },
    KnobDef {
        id: Knob::RetentionSweepSecs,
        slug: "retention_sweep_secs",
        env: "HELIOS_RETENTION_SWEEP_SECS",
        key: "tunable.retention_sweep_secs",
        category: "Retention",
        label: "Retention sweep interval",
        unit: "seconds",
        default: 3600,
        min_value: 1,
        live: true,
        description: "How often the retention sweeper checks for day-partitions past their retention.",
    },
    KnobDef {
        id: Knob::AggMaxPartitions,
        slug: "agg_max_partitions",
        env: "HELIOS_AGG_MAX_PARTITIONS",
        key: "tunable.agg_max_partitions",
        category: "Query",
        label: "Aggregation partition budget",
        unit: "partitions",
        default: 96,
        min_value: 1,
        live: true,
        description: "Partitions an approximate aggregation scans exactly before stride-sampling kicks in.",
    },
    KnobDef {
        id: Knob::QueryCacheMb,
        slug: "query_cache_mb",
        env: "HELIOS_QUERY_CACHE_MB",
        key: "tunable.query_cache_mb",
        category: "Query",
        label: "Query result cache",
        unit: "MB",
        default: 1024,
        min_value: 0, // 0 disables the cache
        live: true,
        description: "Per-block content-match cache size. Caches row-match bitmaps by (immutable block id + filter) so repeated/paginated/time-zoomed queries skip re-scanning. Bounded LRU; 0 disables.",
    },
    KnobDef {
        id: Knob::QueryThreads,
        slug: "query_threads",
        env: "HELIOS_QUERY_THREADS",
        key: "tunable.query_threads",
        category: "Query",
        label: "Query threads",
        unit: "threads",
        default: 4,
        min_value: 1,
        live: false, // rayon's global pool is built once at startup
        description: "Threads for query fan-out (cross-partition scatter + within-partition block scan) — the total query-CPU ceiling shared across requests. Applied at startup; a change takes effect on the next restart.",
    },
    KnobDef {
        id: Knob::AuthTokenTtlHours,
        slug: "auth_token_ttl_hours",
        env: "HELIOS_AUTH_TOKEN_TTL_HOURS",
        key: "tunable.auth_token_ttl_hours",
        category: "Authentication",
        label: "Session token lifetime",
        unit: "hours",
        default: 168, // 7 days
        min_value: 1,
        live: true,
        description: "How long a login (JWT) stays valid before the user must sign in again. Active users get a sliding refresh at half this window, so this is the idle/absolute cap. Default 168 = 7 days. Lowering it only shortens tokens minted from then on.",
    },
];

/// Atomic cells, one per knob, seeded with env-or-default on first access so
/// getters return a sane value even before [`init`] (tests, CLI search).
fn cells() -> &'static Vec<AtomicU64> {
    static CELLS: OnceLock<Vec<AtomicU64>> = OnceLock::new();
    CELLS.get_or_init(|| {
        KNOBS
            .iter()
            .map(|k| AtomicU64::new(resolve(k, None)))
            .collect()
    })
}

/// Parse a raw string for `k`, accepting only values `>= min_value`.
fn parse(k: &KnobDef, raw: Option<&str>) -> Option<u64> {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n >= k.min_value)
}

/// The env override for `k`, if the var is set to a valid value.
fn env_value(k: &KnobDef) -> Option<u64> {
    parse(k, std::env::var(k.env).ok().as_deref())
}

/// Effective value with full precedence: env var > control setting > default.
fn resolve(k: &KnobDef, control_raw: Option<&str>) -> u64 {
    env_value(k)
        .or_else(|| parse(k, control_raw))
        .unwrap_or(k.default)
}

fn raw_value(k: Knob) -> u64 {
    cells()[k as usize].load(Ordering::Relaxed)
}

// ---- typed getters used by the consumers ----

pub fn block_compact_interval() -> Duration {
    Duration::from_secs(raw_value(Knob::BlockCompactSecs))
}
pub fn compactor_lease_ttl() -> Duration {
    block_compact_interval() * 3
}
pub fn block_target_bytes() -> u64 {
    raw_value(Knob::BlockTargetMb) * 1024 * 1024
}
pub fn block_min_compact_bytes() -> u64 {
    raw_value(Knob::BlockMinCompactMb) * 1024 * 1024
}
pub fn block_max_small_blocks() -> usize {
    raw_value(Knob::BlockMaxSmallBlocks) as usize
}
pub fn block_flush_rows() -> usize {
    raw_value(Knob::BlockFlushRows) as usize
}
pub fn block_flush_interval() -> Duration {
    Duration::from_secs(raw_value(Knob::BlockFlushSecs))
}
pub fn block_sync_interval() -> Duration {
    Duration::from_secs(raw_value(Knob::BlockSyncSecs))
}
pub fn retention_sweep_interval() -> Duration {
    Duration::from_secs(raw_value(Knob::RetentionSweepSecs))
}
pub fn retention_sweeper_lease_ttl() -> Duration {
    retention_sweep_interval() * 3
}
pub fn agg_max_partitions() -> usize {
    raw_value(Knob::AggMaxPartitions) as usize
}
pub fn query_cache_mb() -> usize {
    raw_value(Knob::QueryCacheMb) as usize
}
pub fn query_threads() -> usize {
    raw_value(Knob::QueryThreads) as usize
}
pub fn auth_token_ttl_seconds() -> i64 {
    (raw_value(Knob::AuthTokenTtlHours) * 3600) as i64
}

/// Seed every knob from the control plane once, applying env precedence. Call in
/// `serve()` after the control facade is built and BEFORE the query pool / cache
/// are configured and the background loops are spawned.
pub async fn init(control: &Control) {
    apply(control, true).await;
}

/// Re-read control-backed knobs and update the atomics. With `include_restart_only
/// = false` the restart-only knobs are left untouched, so their reported value
/// keeps reflecting what the process actually booted with.
async fn apply(control: &Control, include_restart_only: bool) {
    for k in KNOBS {
        if !include_restart_only && !k.live {
            continue;
        }
        let raw = control.get_setting(k.key).await.ok().flatten();
        let v = resolve(k, raw.as_deref());
        cells()[k.id as usize].store(v, Ordering::Relaxed);
    }
    // Push the (possibly changed) cache size into the live cache.
    crate::engine::block::resize_query_cache(query_cache_mb());
}

/// Re-read control-backed (live) knobs right now — called after an admin edit so
/// the change applies immediately instead of waiting for the refresher tick.
pub async fn refresh(control: &Control) {
    apply(control, false).await;
}

/// Background task: every [`REFRESH_SECS`], pull control-backed tunables so admin
/// edits propagate to the running loops. Never touches restart-only knobs.
pub async fn run_refresher(control: Control) {
    let interval = Duration::from_secs(REFRESH_SECS);
    loop {
        tokio::time::sleep(interval).await;
        apply(&control, false).await;
    }
}

/// Look up a knob by its API slug (for the admin write path).
pub fn knob_by_slug(slug: &str) -> Option<&'static KnobDef> {
    KNOBS.iter().find(|k| k.slug == slug)
}

/// A snapshot of one knob for the admin read path: effective value plus the
/// configured (control) value and any env override.
pub struct KnobView {
    pub def: &'static KnobDef,
    pub effective: u64,
    pub configured: Option<u64>,
    pub env_override: Option<u64>,
}

/// Snapshot all knobs for `GET /api/admin/tunables`. Reads the configured value
/// fresh from control so the UI reflects pending (restart-only) changes too.
pub async fn snapshot(control: &Control) -> Vec<KnobView> {
    let mut out = Vec::with_capacity(KNOBS.len());
    for k in KNOBS {
        let configured = parse(
            k,
            control.get_setting(k.key).await.ok().flatten().as_deref(),
        );
        out.push(KnobView {
            def: k,
            effective: raw_value(k.id),
            configured,
            env_override: env_value(k),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(k: Knob) -> &'static KnobDef {
        &KNOBS[k as usize]
    }

    #[test]
    fn knob_discriminants_match_table_order() {
        for (i, k) in KNOBS.iter().enumerate() {
            assert_eq!(
                k.id as usize, i,
                "KNOBS order must match Knob discriminants"
            );
        }
        assert_eq!(def(Knob::QueryThreads).slug, "query_threads");
    }

    #[test]
    fn resolution_precedence_and_floor() {
        let k = def(Knob::BlockCompactSecs);
        // configured wins over default; env (unset here) doesn't interfere.
        assert_eq!(resolve(k, Some("45")), 45);
        // sub-floor / garbage configured value falls back to the default.
        assert_eq!(resolve(k, Some("0")), k.default);
        assert_eq!(resolve(k, Some("xyz")), k.default);
        assert_eq!(resolve(k, None), k.default);
    }

    #[test]
    fn cache_allows_zero_to_disable() {
        let k = def(Knob::QueryCacheMb);
        assert_eq!(resolve(k, Some("0")), 0);
        assert_eq!(resolve(k, Some("512")), 512);
    }
}
