// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Local-primary ↔ shared store replication (local disk is source of truth on
//! the hot path). Uploader pushes this node's pending blocks; puller brings in
//! shared ids and propagates shared-side compaction; seeder bootstraps fresh shared.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;

use crate::catalog::PartitionKey;

use super::pending::PendingStore;
use super::store::BlockStore;

// uploader

pub async fn run_uploader(local: BlockStore, shared: BlockStore, pending: Arc<PendingStore>) {
    loop {
        tokio::time::sleep(crate::runtime_config::block_sync_interval()).await;
        let (l, s, p) = (local.clone(), shared.clone(), pending.clone());
        let _ = tokio::task::spawn_blocking(move || upload_once(&l, &s, &p)).await;
    }
}

fn upload_once(local: &BlockStore, shared: &BlockStore, pending: &PendingStore) {
    let partitions = match local.list_partitions() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("block sync: list local partitions: {e}");
            return;
        }
    };
    for key in partitions {
        if let Err(e) = upload_partition(local, shared, &key, pending) {
            eprintln!("block sync: upload {key:?}: {e}");
        }
    }
}

fn upload_partition(
    local: &BlockStore,
    shared: &BlockStore,
    key: &PartitionKey,
    pending: &PendingStore,
) -> Result<()> {
    let pending_ids = pending.load(key);
    if pending_ids.is_empty() {
        return Ok(()); // nothing this node owes shared — replicas land here
    }
    let shared_ids: HashSet<String> = shared.load_manifest(key)?.blocks.into_iter().collect();

    let mut uploaded = Vec::new();
    let mut done = Vec::new(); // ids to clear from pending (landed, or unrecoverable)
    for id in &pending_ids {
        if shared_ids.contains(id) {
            done.push(id.clone()); // already in shared — no longer owed
            continue;
        }
        match local.read_block(key, id) {
            Ok(bytes) => match shared.put_block(key, id, &bytes) {
                Ok(()) => {
                    uploaded.push(id.clone());
                    done.push(id.clone());
                }
                // Upload failed (e.g. shared-store outage) — keep it pending and
                // retry next pass.
                Err(e) => eprintln!("block sync: upload block {id}: {e:#}"),
            },
            // An owned block with no local bytes and not in shared can't be
            // uploaded — drop it from pending so we don't spin on it.
            Err(_) => done.push(id.clone()),
        }
    }
    if !uploaded.is_empty() {
        shared.swap_blocks(key, &[], &uploaded)?;
    }
    pending.remove(key, &done);
    Ok(())
}

// puller

pub async fn run_puller(local: BlockStore, shared: BlockStore, pending: Arc<PendingStore>) {
    // Pull once eagerly before sleeping, so a freshly-started replica mirrors the
    // shared manifest immediately rather than returning empty until the first interval.
    loop {
        let (l, s, p) = (local.clone(), shared.clone(), pending.clone());
        let _ = tokio::task::spawn_blocking(move || pull_once(&l, &s, &p)).await;
        tokio::time::sleep(crate::runtime_config::block_sync_interval()).await;
    }
}

fn pull_once(local: &BlockStore, shared: &BlockStore, pending: &PendingStore) {
    let partitions = match shared.list_partitions() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("block sync: list shared partitions: {e}");
            return;
        }
    };
    for key in partitions {
        if let Err(e) = pull_partition(local, shared, &key, pending) {
            eprintln!("block sync: pull {key:?}: {e}");
        }
    }
}

fn pull_partition(
    local: &BlockStore,
    shared: &BlockStore,
    key: &PartitionKey,
    pending: &PendingStore,
) -> Result<()> {
    let shared_ids = shared.load_manifest(key)?.blocks;
    let shared_set: HashSet<&String> = shared_ids.iter().collect();
    let local_ids = local.load_manifest(key)?.blocks;
    let local_set: HashSet<&String> = local_ids.iter().collect();

    // Additions: ids in shared not yet in the local manifest (block bytes fetch
    // lazily on read via the local-first store).
    let to_add: Vec<String> = shared_ids
        .iter()
        .filter(|id| !local_set.contains(*id))
        .cloned()
        .collect();
    if !to_add.is_empty() {
        local.swap_blocks(key, &[], &to_add)?;
    }

    // Removals: a local id gone from shared and NOT pending was compacted away → drop +
    // delete it; pending (own, un-uploaded) ids stay. Persisted ownership keeps this restart-correct.
    let owned: HashSet<String> = pending.load(key).into_iter().collect();
    let to_remove: Vec<String> = local_ids
        .iter()
        .filter(|id| !shared_set.contains(*id) && !owned.contains(*id))
        .cloned()
        .collect();
    if !to_remove.is_empty() {
        local.swap_blocks(key, &to_remove, &[])?;
        for id in &to_remove {
            local.delete_block(key, id);
        }
    }
    Ok(())
}

// seeder

/// Bootstrap seeder: for any local partition with no shared manifest, upload all its blocks
/// and create the manifest — so it never re-adds compacted-away blocks; replicas seed nothing.
pub async fn run_seeder(local: BlockStore, shared: BlockStore) {
    loop {
        let (l, s) = (local.clone(), shared.clone());
        let _ = tokio::task::spawn_blocking(move || seed_once(&l, &s)).await;
        tokio::time::sleep(crate::runtime_config::block_sync_interval()).await;
    }
}

fn seed_once(local: &BlockStore, shared: &BlockStore) {
    let partitions = match local.list_partitions() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("block sync: seed: list local partitions: {e}");
            return;
        }
    };
    for key in partitions {
        if let Err(e) = seed_partition(local, shared, &key) {
            eprintln!("block sync: seed {key:?}: {e}");
        }
    }
}

fn seed_partition(local: &BlockStore, shared: &BlockStore, key: &PartitionKey) -> Result<()> {
    if shared.has_manifest(key)? {
        return Ok(()); // already in shared — the incremental uploader owns it
    }
    let local_ids = local.load_manifest(key)?.blocks;
    if local_ids.is_empty() {
        return Ok(());
    }
    let mut uploaded = Vec::new();
    for id in &local_ids {
        let bytes = match local.read_block(key, id) {
            Ok(b) => b,
            Err(_) => continue, // no local bytes (e.g. a pulled-but-undownloaded ref)
        };
        match shared.put_block(key, id, &bytes) {
            Ok(()) => uploaded.push(id.clone()),
            Err(e) => eprintln!("block sync: seed block {id}: {e:#}"),
        }
    }
    if !uploaded.is_empty() {
        shared.swap_blocks(key, &[], &uploaded)?;
        tracing::info!(
            index = %key.index,
            day = %key.day_string(),
            blocks = uploaded.len(),
            "block sync: seeded local partition into shared store"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::block::{BlockStore, BlockWriter, Codec, PendingStore, Row};
    use chrono::NaiveDate;
    use tempfile::TempDir;

    fn key() -> PartitionKey {
        PartitionKey::new(
            "default",
            "orders",
            NaiveDate::from_ymd_opt(2026, 5, 30).unwrap(),
        )
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

    fn pending(dir: &TempDir) -> PendingStore {
        PendingStore::new(dir.path())
    }

    #[test]
    fn uploader_replicates_pending_blocks_and_clears_them() {
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let pend = pending(&ld);
        let k = key();
        let a = local.append_block(&k, &blk(1, "a")).unwrap();
        let b = local.append_block(&k, &blk(2, "b")).unwrap();
        pend.add(&k, &a); // simulate ingest recording ownership
        pend.add(&k, &b);

        upload_partition(&local, &shared, &k, &pend).unwrap();
        assert_eq!(shared.load_manifest(&k).unwrap().blocks.len(), 2);
        assert_eq!(
            shared
                .open_blocks(&k)
                .unwrap()
                .iter()
                .map(|b| b.row_count())
                .sum::<u32>(),
            2
        );
        // Uploaded blocks are cleared from pending, so a re-run does nothing.
        assert!(pend.load(&k).is_empty());
        upload_partition(&local, &shared, &k, &pend).unwrap();
        assert_eq!(shared.load_manifest(&k).unwrap().blocks.len(), 2);
    }

    #[test]
    fn uploader_ignores_blocks_this_node_does_not_own() {
        // A replica caches A,B by reading them: in its local manifest but never ingested,
        // so NOT pending. The uploader must not touch them — nothing reaches shared.
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let pend = pending(&ld);
        let k = key();
        local.append_block(&k, &blk(1, "a")).unwrap();
        local.append_block(&k, &blk(2, "b")).unwrap();
        // pend is empty: this node owns nothing.

        upload_partition(&local, &shared, &k, &pend).unwrap();
        assert!(
            shared.load_manifest(&k).unwrap().blocks.is_empty(),
            "a node uploads only blocks it owns"
        );
    }

    #[test]
    fn uploader_does_not_resurrect_compacted_away_blocks() {
        // A,B were ingested + uploaded (pending now clear), then shared compaction merged
        // them into M. The uploader must not re-upload A,B though still cached locally.
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let pend = pending(&ld);
        let k = key();
        let a = local.append_block(&k, &blk(1, "a")).unwrap();
        let b = local.append_block(&k, &blk(2, "b")).unwrap();
        pend.add(&k, &a);
        pend.add(&k, &b);
        upload_partition(&local, &shared, &k, &pend).unwrap(); // uploads + clears pending
        assert!(pend.load(&k).is_empty());

        // Shared-side compaction merges A,B → M and removes the originals.
        let m = shared.write_block(&k, &blk(3, "m")).unwrap();
        shared
            .swap_blocks_if_present(&k, &[a.clone(), b.clone()], std::slice::from_ref(&m))
            .unwrap();

        upload_partition(&local, &shared, &k, &pend).unwrap();
        assert_eq!(
            shared.load_manifest(&k).unwrap().blocks,
            vec![m],
            "compacted-away blocks must not be re-uploaded"
        );
    }

    #[test]
    fn puller_brings_shared_ids_into_local_manifest() {
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let pend = pending(&ld);
        let k = key();
        shared.append_block(&k, &blk(1, "a")).unwrap(); // peer wrote to shared
        shared.append_block(&k, &blk(2, "b")).unwrap();

        pull_partition(&local, &shared, &k, &pend).unwrap();
        // Local manifest now references both ids (bytes fetch lazily on read).
        assert_eq!(local.load_manifest(&k).unwrap().blocks.len(), 2);
    }

    #[test]
    fn puller_keeps_pending_local_blocks() {
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let pend = pending(&ld);
        let k = key();
        let a = local.append_block(&k, &blk(1, "a")).unwrap(); // owned, not yet uploaded
        pend.add(&k, &a);

        pull_partition(&local, &shared, &k, &pend).unwrap();
        // Not in shared but pending (ours) ⇒ NOT removed.
        assert_eq!(local.load_manifest(&k).unwrap().blocks, vec![a]);
    }

    #[test]
    fn puller_propagates_shared_compaction_without_prior_state() {
        // Restart-robustness: a fresh puller (no in-memory history) must still drop a local
        // block gone from shared and not pending. A,B are pulled (not owned, so not pending).
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let pend = pending(&ld);
        let k = key();
        let a = shared.append_block(&k, &blk(1, "a")).unwrap();
        let b = shared.append_block(&k, &blk(2, "b")).unwrap();
        pull_partition(&local, &shared, &k, &pend).unwrap(); // local mirrors shared: [a, b]
        assert_eq!(local.load_manifest(&k).unwrap().blocks.len(), 2);

        // Shared-side compaction merges a,b → m.
        let m = shared.write_block(&k, &blk(3, "m")).unwrap();
        shared
            .swap_blocks_if_present(&k, &[a.clone(), b.clone()], std::slice::from_ref(&m))
            .unwrap();

        pull_partition(&local, &shared, &k, &pend).unwrap();
        // Local reflects the compaction: only the merged block, a,b dropped.
        assert_eq!(local.load_manifest(&k).unwrap().blocks, vec![m]);
    }

    #[test]
    fn seeder_uploads_partition_absent_from_shared() {
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let k = key();
        let a = local.append_block(&k, &blk(1, "a")).unwrap();
        let b = local.append_block(&k, &blk(2, "b")).unwrap();
        assert!(!shared.has_manifest(&k).unwrap());

        seed_partition(&local, &shared, &k).unwrap();

        let sm = shared.load_manifest(&k).unwrap().blocks;
        assert_eq!(sm.len(), 2);
        assert!(sm.contains(&a) && sm.contains(&b));
        // Bytes really landed in shared, not just the manifest.
        assert_eq!(
            shared
                .open_blocks(&k)
                .unwrap()
                .iter()
                .map(|b| b.row_count())
                .sum::<u32>(),
            2
        );
    }

    #[test]
    fn seeder_skips_partition_already_in_shared() {
        // A partition already in shared (even with a different/compacted set) must be left
        // to the incremental uploader, so the seeder never resurrects compacted-away blocks.
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let k = key();
        local.append_block(&k, &blk(1, "a")).unwrap();
        local.append_block(&k, &blk(2, "b")).unwrap();
        let s = shared.append_block(&k, &blk(9, "merged")).unwrap(); // shared has its own state
        let before = shared.load_manifest(&k).unwrap().blocks;

        seed_partition(&local, &shared, &k).unwrap();

        assert_eq!(
            shared.load_manifest(&k).unwrap().blocks,
            before,
            "seeder must not touch a partition already in shared"
        );
        assert_eq!(before, vec![s]);
    }

    #[test]
    fn seeder_ignores_empty_local_partition() {
        let (ld, sd) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (local, shared) = (BlockStore::new(ld.path()), BlockStore::new(sd.path()));
        let k = key();
        // A local manifest with no blocks (e.g. everything dropped) — nothing to
        // seed, and no empty shared manifest should be created.
        local.swap_blocks(&k, &[], &[]).unwrap();

        seed_partition(&local, &shared, &k).unwrap();
        assert!(!shared.has_manifest(&k).unwrap());
    }
}
