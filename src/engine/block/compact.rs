// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Compaction (ENGINE.md §6): size-driven packing of small blocks into
//! target-sized ones, committed by a single CAS `swap_blocks` so a racing merge
//! aborts safely (the [`CompactionGate`] only avoids wasted work, not races).

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::catalog::{Catalog, PartitionKey};

use super::store::BlockStore;
use super::{Block, BlockWriter, Codec};

/// Decides whether *this* node should run a compaction pass now. Lets the caller plug
/// in cross-node single-writer election without the engine depending on the control plane.
#[async_trait]
pub trait CompactionGate: Send + Sync {
    async fn should_compact(&self) -> bool;
}

/// A gate that always runs — the single-node / local-only case (no peers to
/// coordinate with, so no election needed).
pub struct AlwaysCompact;

#[async_trait]
impl CompactionGate for AlwaysCompact {
    async fn should_compact(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompactionStats {
    /// Number of small blocks consumed by merges.
    pub merged_blocks: usize,
    /// Number of compacted blocks produced.
    pub new_blocks: usize,
    /// Rows rewritten.
    pub rows: usize,
}

const MIN_SMALL_BLOCKS: usize = 4;

/// Greedily pack `(id, size)` into groups, sealing each once cumulative size reaches
/// `target` (trailing remainder is its own group). Time order and sizes are preserved.
fn pack(blocks: Vec<(String, u64)>, target: u64) -> Vec<Vec<(String, u64)>> {
    let mut groups = Vec::new();
    let mut cur: Vec<(String, u64)> = Vec::new();
    let mut cur_bytes = 0u64;
    for (id, size) in blocks {
        cur_bytes += size;
        cur.push((id, size));
        if cur_bytes >= target {
            groups.push(std::mem::take(&mut cur));
            cur_bytes = 0;
        }
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    groups
}

/// Compact one partition: merge small (`< target_bytes`) blocks into target-sized ones.
/// `max_small_blocks` is a count override that waives the `min_compact_bytes` floor.
pub fn compact_partition(
    store: &BlockStore,
    codec: Codec,
    key: &PartitionKey,
    target_bytes: u64,
    min_small_blocks: usize,
    min_compact_bytes: u64,
    max_small_blocks: usize,
) -> Result<Option<CompactionStats>> {
    let manifest = store.load_manifest(key)?;
    // Small blocks, in manifest (≈time) order, with their on-disk sizes.
    let small: Vec<(String, u64)> = manifest
        .blocks
        .iter()
        .filter_map(|id| {
            store
                .block_size(key, id)
                .filter(|&sz| sz < target_bytes)
                .map(|sz| (id.clone(), sz))
        })
        .collect();
    if small.len() < min_small_blocks {
        return Ok(None);
    }
    // Too many tiny files: waive the byte floor and pack them down regardless.
    let waive_floor = small.len() >= max_small_blocks;

    let mut stats = CompactionStats {
        merged_blocks: 0,
        new_blocks: 0,
        rows: 0,
    };
    for group in pack(small, target_bytes) {
        if group.len() < 2 {
            continue; // a lone near-target block — nothing to merge
        }
        let group_bytes: u64 = group.iter().map(|(_, sz)| sz).sum();
        if !waive_floor && group_bytes < min_compact_bytes {
            continue; // too little data to be worth a rewrite — let it accumulate
        }
        let group: Vec<String> = group.into_iter().map(|(id, _)| id).collect();
        // Stream each source block's rows straight into the writer (no intermediate
        // Vec<Row>) and drop the block before opening the next, so peak memory is the
        // writer's accumulation plus one in-flight row, not a second full-block copy.
        let mut w = BlockWriter::new(codec);
        for id in &group {
            let block = Block::open(store.read_block(key, id)?)?;
            let n = block.row_count() as usize;
            w.reserve(n);
            block.for_each_row(|r| {
                w.push(r);
                Ok(())
            })?;
            stats.rows += n;
        }

        // Write the merged block, then guarded-swap it for the group. If another
        // compactor already merged these, the swap aborts and we drop the orphan.
        let new_id = store.write_block(key, &w.finish()?)?;
        if store.swap_blocks_if_present(key, &group, std::slice::from_ref(&new_id))? {
            for id in &group {
                store.delete_block(key, id);
            }
            stats.merged_blocks += group.len();
            stats.new_blocks += 1;
        } else {
            store.delete_block(key, &new_id); // lost the race — drop the orphan
        }
    }

    if stats.merged_blocks == 0 {
        Ok(None)
    } else {
        Ok(Some(stats))
    }
}

/// Background compactor: every interval, walk `store`'s partitions and compact those
/// with enough small blocks. Sequential `spawn_blocking`, so two never race a partition.
pub async fn run_compactor(store: BlockStore, codec: Codec, gate: Arc<dyn CompactionGate>) {
    loop {
        // Re-read each pass so Admin → General edits apply without a restart.
        tokio::time::sleep(crate::runtime_config::block_compact_interval()).await;
        // Single-writer gate: acquire/renew the lease (or always-true for a lone
        // node). If we don't hold it, another node is compacting — skip this pass.
        if !gate.should_compact().await {
            continue;
        }
        let target = crate::runtime_config::block_target_bytes();
        let min_compact = crate::runtime_config::block_min_compact_bytes();
        let max_small = crate::runtime_config::block_max_small_blocks();
        // List off the worker thread — for a shared (S3) store this is a network
        // call and must not block the reactor.
        let listing = {
            let store = store.clone();
            tokio::task::spawn_blocking(move || store.list_partitions()).await
        };
        let partitions = match listing {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                tracing::warn!("block compaction: list partitions failed: {e:#}");
                continue;
            }
            Err(_) => continue,
        };
        for key in partitions {
            let store = store.clone();
            let label = format!("{}/{}", key.index, key.day_string());
            let res = tokio::task::spawn_blocking(move || {
                compact_partition(
                    &store,
                    codec,
                    &key,
                    target,
                    MIN_SMALL_BLOCKS,
                    min_compact,
                    max_small,
                )
            })
            .await;
            match res {
                Ok(Ok(Some(s))) => tracing::info!(
                    partition = %label,
                    merged_blocks = s.merged_blocks,
                    new_blocks = s.new_blocks,
                    rows = s.rows,
                    "block compaction: merged small blocks"
                ),
                Ok(Ok(None)) => {}
                Ok(Err(e)) => tracing::warn!(partition = %label, "block compaction failed: {e}"),
                Err(e) => tracing::warn!(partition = %label, "block compaction join error: {e}"),
            }
        }
    }
}
