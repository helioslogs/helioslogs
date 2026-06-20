// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Retention sweeper: drops day-partitions older than the effective retention
//! (per-env override, else the global `retention.default_days` setting; unset
//! = keep forever). Deletion leaves an **empty-manifest tombstone** in the
//! shared store — never a missing manifest — so the seeder won't re-upload a
//! dropped partition and every replica's puller converges to zero blocks.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use chrono::NaiveDate;

use crate::catalog::{Catalog, PartitionKey};
use crate::control::settings::KEY_RETENTION_DEFAULT_DAYS;
use crate::control::Control;
use crate::engine::block::{BlockStore, CompactionGate, PendingStore};

pub struct RetentionCtx {
    pub catalog: Catalog,
    pub control: Control,
    /// Plain local store (NOT the local-first read-through store).
    pub local: BlockStore,
    /// Shared replication target when `--shared-store` is on.
    pub shared: Option<BlockStore>,
    pub pending: Arc<PendingStore>,
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct SweepResult {
    pub partitions_dropped: usize,
    pub blocks_deleted: usize,
}

/// Env var pinning the global default retention; wins over the control setting.
pub const RETENTION_DEFAULT_DAYS_ENV: &str = "HELIOS_RETENTION_DEFAULT_DAYS";

/// Effective global default retention in days: env var > control setting. `None`
/// (env "0"/blank, an unset setting, or a non-positive value) means keep forever.
pub fn effective_default_days(setting: Option<String>) -> Option<i64> {
    // Env wins outright when present — even "0"/blank, read as "keep forever".
    if let Ok(raw) = std::env::var(RETENTION_DEFAULT_DAYS_ENV) {
        let t = raw.trim();
        if !t.is_empty() {
            return t.parse::<i64>().ok().filter(|&d| d > 0);
        }
    }
    setting
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&d| d > 0)
}

/// Whether the env var pins the global retention default (the setting can't win).
pub fn is_default_days_env_overridden() -> bool {
    std::env::var(RETENTION_DEFAULT_DAYS_ENV)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

/// Hourly sweep. Non-leaders still run the pass with `leader=false` so each
/// node clears its own pending entries for expired partitions (the one
/// per-node step — otherwise its uploader would re-push dropped blocks).
pub async fn run_sweeper(ctx: Arc<RetentionCtx>, gate: Arc<dyn CompactionGate>) {
    loop {
        // Re-read each pass so an Admin → General change applies without a restart.
        tokio::time::sleep(crate::runtime_config::retention_sweep_interval()).await;
        let leader = gate.should_compact().await;
        match sweep_once(&ctx, leader).await {
            Ok(r) if r.partitions_dropped > 0 => tracing::info!(
                partitions = r.partitions_dropped,
                blocks = r.blocks_deleted,
                "retention: dropped expired partitions"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("retention sweep failed: {e:#}"),
        }
    }
}

pub async fn sweep_once(ctx: &RetentionCtx, leader: bool) -> Result<SweepResult> {
    let global_days: Option<i64> =
        effective_default_days(ctx.control.get_setting(KEY_RETENTION_DEFAULT_DAYS).await?);
    let env_days: HashMap<String, Option<i64>> = ctx
        .control
        .list_envs(true)
        .await?
        .into_iter()
        .map(|e| (e.name, e.retention_days))
        .collect();
    if global_days.is_none() && env_days.values().all(|d| d.is_none()) {
        return Ok(SweepResult::default()); // retention not configured anywhere
    }

    let today = chrono::Utc::now().date_naive();
    let ctx2 = RetentionSnapshot {
        local: ctx.local.clone(),
        shared: ctx.shared.clone(),
        pending: ctx.pending.clone(),
        root: ctx.catalog.root().to_path_buf(),
    };
    let catalog = ctx.catalog.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut keys = catalog.list_partitions();
        // Shared-only partitions (not yet pulled locally) are the leader's to clean.
        if let Some(shared) = &ctx2.shared {
            if let Ok(remote) = shared.list_partitions() {
                for k in remote {
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
            }
        }
        let mut out = SweepResult::default();
        for k in keys {
            let eff = env_days
                .get(&k.env)
                .copied()
                .flatten()
                .or(global_days)
                // Unknown env (orphan on-disk dir): fall back to the global default.
                .map(|d| d.max(1)); // floor: a misconfig can never delete today
            let Some(days) = eff else { continue };
            if !is_expired(k.day, today, days) {
                continue;
            }
            // Per-node step, leader or not: never re-upload an expired partition.
            let owed = ctx2.pending.load(&k);
            if !owed.is_empty() {
                ctx2.pending.remove(&k, &owed);
            }
            if !leader {
                continue;
            }
            match drop_partition(&ctx2, &k) {
                Ok(blocks) => {
                    out.partitions_dropped += 1;
                    out.blocks_deleted += blocks;
                }
                Err(e) => tracing::warn!(env = %k.env, index = %k.index, day = %k.day_string(),
                    "retention: drop failed: {e:#}"),
            }
        }
        out
    })
    .await?;
    Ok(result)
}

struct RetentionSnapshot {
    local: BlockStore,
    shared: Option<BlockStore>,
    pending: Arc<PendingStore>,
    root: std::path::PathBuf,
}

/// Expired iff the partition day is strictly older than `today - days`.
fn is_expired(day: NaiveDate, today: NaiveDate, days: i64) -> bool {
    day < today - chrono::Duration::days(days)
}

/// Empty out a store's manifest for `key` (one CAS commit), then delete the
/// block objects best-effort. Returns how many block ids were dropped.
fn empty_manifest(store: &BlockStore, key: &PartitionKey) -> Result<usize> {
    let ids = store.load_manifest(key)?.blocks;
    if ids.is_empty() {
        return Ok(0);
    }
    store.swap_blocks(key, &ids, &[])?;
    for id in &ids {
        store.delete_block(key, id);
    }
    Ok(ids.len())
}

fn drop_partition(ctx: &RetentionSnapshot, key: &PartitionKey) -> Result<usize> {
    let mut blocks = 0;
    if let Some(shared) = &ctx.shared {
        // Tombstone, not removal: an absent shared manifest would let the
        // seeder re-upload this partition from any replica still caching it.
        blocks += empty_manifest(shared, key)?;
    }
    blocks += empty_manifest(&ctx.local, key)?;
    if ctx.shared.is_none() {
        // Local-only: no resurrection vector, remove the whole partition dir.
        // A racing late-backfill write may recreate it; the next sweep re-drops.
        let path = ctx
            .root
            .join(&key.env)
            .join(&key.index)
            .join(key.day_string());
        if path.exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::block::{BlockWriter, Codec, Row};
    use tempfile::TempDir;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn expiry_day_boundaries() {
        let today = d("2026-06-11");
        // 7-day retention: the 7 most recent days (and today) survive.
        assert!(!is_expired(d("2026-06-11"), today, 7));
        assert!(!is_expired(d("2026-06-04"), today, 7));
        assert!(is_expired(d("2026-06-03"), today, 7));
        assert!(is_expired(d("2020-01-01"), today, 7));
        // Floor of 1 (applied by the sweeper) keeps today and yesterday.
        assert!(!is_expired(d("2026-06-10"), today, 1));
        assert!(is_expired(d("2026-06-09"), today, 1));
    }

    fn blk(ts: i64, msg: &str) -> Vec<u8> {
        let mut w = BlockWriter::new(Codec::None);
        w.push(Row {
            ts_millis: ts,
            message: Some(msg.into()),
            source: None,
            raw: Some("{}".into()),
            fields: vec![],
        });
        w.finish().unwrap()
    }

    fn key(day: &str) -> PartitionKey {
        PartitionKey::new("default", "web", d(day))
    }

    #[test]
    fn drop_leaves_shared_tombstone_and_seeder_skips() {
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let k = key("2026-01-01");
        let a = local.append_block(&k, &blk(1, "a")).unwrap();
        shared.put_block(&k, &a, &blk(1, "a")).unwrap();
        shared
            .swap_blocks(&k, &[], std::slice::from_ref(&a))
            .unwrap();

        let ctx = RetentionSnapshot {
            local: local.clone(),
            shared: Some(shared.clone()),
            pending: Arc::new(PendingStore::new(ld.path())),
            root: ld.path().to_path_buf(),
        };
        let blocks = drop_partition(&ctx, &k).unwrap();
        assert_eq!(blocks, 2); // one shared + one local id

        // Tombstone: manifest still exists, but holds zero blocks.
        assert!(shared.has_manifest(&k).unwrap());
        assert!(shared.load_manifest(&k).unwrap().blocks.is_empty());
        assert!(local.load_manifest(&k).unwrap().blocks.is_empty());

        // Idempotent re-run.
        assert_eq!(drop_partition(&ctx, &k).unwrap(), 0);
    }

    #[test]
    fn drop_local_only_removes_partition_dir() {
        let ld = TempDir::new().unwrap();
        let local = BlockStore::new(ld.path());
        let k = key("2026-01-01");
        local.append_block(&k, &blk(1, "a")).unwrap();
        let pdir = ld.path().join("default").join("web").join("2026-01-01");
        assert!(pdir.exists());

        let ctx = RetentionSnapshot {
            local: local.clone(),
            shared: None,
            pending: Arc::new(PendingStore::new(ld.path())),
            root: ld.path().to_path_buf(),
        };
        drop_partition(&ctx, &k).unwrap();
        assert!(!pdir.exists());
    }

    #[tokio::test]
    async fn sweep_respects_env_override_and_global_default() {
        use crate::control::store::build_control_store;
        let ld = TempDir::new().unwrap();
        let cd = TempDir::new().unwrap();
        let store = build_control_store(None, cd.path()).await.unwrap();
        let crypto = Arc::new(crate::control::crypto::Crypto::new(false).unwrap());
        let control = Control::new(store, crypto);
        control.upsert_env("default", false).await.unwrap();
        control.upsert_env("keepers", false).await.unwrap();
        control
            .set_env_retention("keepers", Some(36500))
            .await
            .unwrap();
        control
            .set_setting(KEY_RETENTION_DEFAULT_DAYS, "7")
            .await
            .unwrap();

        let local = BlockStore::new(ld.path());
        let old_default = PartitionKey::new("default", "web", d("2020-01-01"));
        let old_keeper = PartitionKey::new("keepers", "web", d("2020-01-01"));
        let fresh = PartitionKey::new("default", "web", chrono::Utc::now().date_naive());
        for k in [&old_default, &old_keeper, &fresh] {
            local.append_block(k, &blk(1, "x")).unwrap();
        }

        let ctx = RetentionCtx {
            catalog: Catalog::open(ld.path().to_path_buf()).unwrap(),
            control,
            local: local.clone(),
            shared: None,
            pending: Arc::new(PendingStore::new(ld.path())),
        };
        let r = sweep_once(&ctx, true).await.unwrap();
        assert_eq!(r.partitions_dropped, 1, "only the expired default-env day");
        assert!(local.load_manifest(&old_default).unwrap().blocks.is_empty());
        assert_eq!(local.load_manifest(&old_keeper).unwrap().blocks.len(), 1);
        assert_eq!(local.load_manifest(&fresh).unwrap().blocks.len(), 1);

        // Non-leader pass deletes nothing.
        let r2 = sweep_once(&ctx, false).await.unwrap();
        assert_eq!(r2.partitions_dropped, 0);
    }

    #[tokio::test]
    async fn sweep_noop_when_retention_unconfigured() {
        use crate::control::store::build_control_store;
        let ld = TempDir::new().unwrap();
        let cd = TempDir::new().unwrap();
        let store = build_control_store(None, cd.path()).await.unwrap();
        let crypto = Arc::new(crate::control::crypto::Crypto::new(false).unwrap());
        let control = Control::new(store, crypto);
        control.upsert_env("default", false).await.unwrap();

        let local = BlockStore::new(ld.path());
        let ancient = PartitionKey::new("default", "web", d("2000-01-01"));
        local.append_block(&ancient, &blk(1, "x")).unwrap();

        let ctx = RetentionCtx {
            catalog: Catalog::open(ld.path().to_path_buf()).unwrap(),
            control,
            local: local.clone(),
            shared: None,
            pending: Arc::new(PendingStore::new(ld.path())),
        };
        let r = sweep_once(&ctx, true).await.unwrap();
        assert_eq!(r.partitions_dropped, 0);
        assert_eq!(local.load_manifest(&ancient).unwrap().blocks.len(), 1);
    }
}
