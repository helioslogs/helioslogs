// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Block store over an [`ObjectStore`]: a partition is a generationed manifest
//! plus write-once `.hb` blocks. Manifest commits are lock-free multi-writer via
//! CAS — write gen `N+1` with `put_if_absent`, reload and retry if a peer won.

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::catalog::PartitionKey;

use super::cache::block_stats_cache;
use super::objstore::{build_object_store, FsObjectStore, LocalFirstObjectStore, ObjectStore};
use super::{footer_field_stats, Block, BlockFieldStats, Footer, MAGIC};

/// Tail bytes probed for a footer in one ranged read; a larger footer triggers a follow-up.
const FOOTER_PROBE: u64 = 64 * 1024;

/// The only mutable per-partition metadata: the set of live block ids.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub blocks: Vec<String>,
}

/// Max CAS retries — a generous backstop; contention is rare and each race has one winner.
const MAX_CAS_RETRIES: u32 = 100;

/// Old manifest generations to keep — superseded ones are unused, but a slow reader may be mid-list.
const KEEP_GENERATIONS: usize = 3;

#[derive(Clone)]
pub struct BlockStore {
    store: Arc<dyn ObjectStore>,
}

impl BlockStore {
    /// Filesystem-backed store rooted at `root` (local dir or NFS mount). Kept
    /// as the simple constructor for tests and the default local layout.
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            store: Arc::new(FsObjectStore::new(root)),
        }
    }

    /// Store over an arbitrary object backend (e.g. S3).
    pub fn with_object_store(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    pub fn describe(&self) -> String {
        self.store.describe()
    }

    // key layout

    fn partition_prefix(key: &PartitionKey) -> String {
        format!("{}/{}/{}", key.env, key.index, key.day_string())
    }

    fn manifest_prefix(key: &PartitionKey) -> String {
        format!("{}/manifest/", Self::partition_prefix(key))
    }

    fn manifest_gen_key(key: &PartitionKey, gen: u64) -> String {
        // Zero-padded so lexicographic order == numeric order.
        format!("{}{gen:020}.json", Self::manifest_prefix(key))
    }

    fn legacy_manifest_key(key: &PartitionKey) -> String {
        format!("{}/manifest.json", Self::partition_prefix(key))
    }

    fn block_key(key: &PartitionKey, id: &str) -> String {
        format!("{}/blocks/{id}.hb", Self::partition_prefix(key))
    }

    fn basename(key: &str) -> &str {
        key.rsplit('/').next().unwrap_or(key)
    }

    // manifest generations

    /// Highest manifest generation, or `None` if the partition has none yet.
    fn latest_gen(&self, key: &PartitionKey) -> Result<Option<u64>> {
        let mut max: Option<u64> = None;
        for k in self.store.list(&Self::manifest_prefix(key))? {
            if let Some(stem) = Self::basename(&k).strip_suffix(".json") {
                if let Ok(g) = stem.parse::<u64>() {
                    max = Some(max.map_or(g, |m| m.max(g)));
                }
            }
        }
        Ok(max)
    }

    /// The current `(generation, manifest)`. Gen 0 = no `manifest/` generation yet
    /// (empty, or a legacy `manifest.json` read as the base; next CAS promotes to gen 1).
    fn current(&self, key: &PartitionKey) -> Result<(u64, Manifest)> {
        if let Some(gen) = self.latest_gen(key)? {
            let bytes = self.store.get(&Self::manifest_gen_key(key, gen))?;
            let manifest = serde_json::from_slice(&bytes).context("parsing manifest")?;
            return Ok((gen, manifest));
        }
        let legacy = Self::legacy_manifest_key(key);
        if self.store.size(&legacy)?.is_some() {
            let manifest = serde_json::from_slice(&self.store.get(&legacy)?)
                .context("parsing legacy manifest")?;
            return Ok((0, manifest));
        }
        Ok((0, Manifest::default()))
    }

    /// Read-modify-write the manifest under CAS. `mutate` runs on a fresh copy each
    /// attempt, returning `true` to commit or `false` to abort; retries on contention.
    fn try_update(
        &self,
        key: &PartitionKey,
        mutate: impl Fn(&mut Manifest) -> bool,
    ) -> Result<bool> {
        for _ in 0..MAX_CAS_RETRIES {
            let (gen, mut manifest) = self.current(key)?;
            if !mutate(&mut manifest) {
                return Ok(false);
            }
            let bytes = serde_json::to_vec_pretty(&manifest)?;
            if self
                .store
                .put_if_absent(&Self::manifest_gen_key(key, gen + 1), &bytes)?
            {
                self.gc_old_generations(key);
                return Ok(true);
            }
            // Lost the race for gen+1 — reload and retry against the winner.
        }
        bail!(
            "manifest CAS: exceeded {MAX_CAS_RETRIES} retries on {}",
            Self::partition_prefix(key)
        )
    }

    fn update(&self, key: &PartitionKey, mutate: impl Fn(&mut Manifest)) -> Result<()> {
        self.try_update(key, |m| {
            mutate(m);
            true
        })
        .map(|_| ())
    }

    // blocks

    /// Write `bytes` as a new immutable block object, returning its id, without touching
    /// the manifest. Used by compaction, which then swaps it in atomically.
    pub fn write_block(&self, key: &PartitionKey, bytes: &[u8]) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.store.put(&Self::block_key(key, &id), bytes)?;
        Ok(id)
    }

    /// Write a new block then CAS-append its id (object first, so a crash leaves an orphan, not
    /// a dangling entry). The owned-write primitive: marks the id pending-upload before the manifest.
    pub fn append_block(&self, key: &PartitionKey, bytes: &[u8]) -> Result<String> {
        let id = self.write_block(key, bytes)?;
        if let Some(pending) = super::configured_pending() {
            pending.add(key, &id);
        }
        self.update(key, |m| m.blocks.push(id.clone()))?;
        Ok(id)
    }

    /// Write a block object under a **specific** id (no manifest change). Used by the
    /// uploader to replicate under the same id, so the two stores reconcile by set comparison.
    pub fn put_block(&self, key: &PartitionKey, id: &str, bytes: &[u8]) -> Result<()> {
        self.store.put(&Self::block_key(key, id), bytes)
    }

    /// Object size of a block in bytes, or `None` if missing. Drives
    /// compaction's small-block selection (cheap — no read).
    pub fn block_size(&self, key: &PartitionKey, id: &str) -> Option<u64> {
        self.store.size(&Self::block_key(key, id)).ok().flatten()
    }

    /// Delete a block object. Best-effort.
    pub fn delete_block(&self, key: &PartitionKey, id: &str) {
        let _ = self.store.delete(&Self::block_key(key, id));
    }

    pub fn read_block(&self, key: &PartitionKey, id: &str) -> Result<Vec<u8>> {
        self.store.get(&Self::block_key(key, id))
    }

    /// Open a block for ranged section reads: only the footer is fetched up front
    /// (cheap tail read); each queried section is read on demand via `get_range`.
    /// The read path uses this so a selective query never pulls the whole block.
    pub fn open_block_ranged(&self, key: &PartitionKey, id: &str) -> Result<Block> {
        let footer = self.read_footer(key, id)?;
        Ok(Block::open_ranged(
            self.store.clone(),
            Self::block_key(key, id),
            footer,
        ))
    }

    /// Per-column field-coverage stats (the field catalog's input), cached by the block's
    /// immutable id. On a miss reads only the footer for v2 blocks; v1 falls back to a full read.
    pub fn block_field_stats(
        &self,
        key: &PartitionKey,
        id: &str,
    ) -> Result<std::sync::Arc<BlockFieldStats>> {
        if let Some(s) = block_stats_cache().get(id) {
            return Ok(s);
        }
        let stats = std::sync::Arc::new(self.compute_block_field_stats(key, id)?);
        block_stats_cache().insert(id.to_string(), stats.clone());
        Ok(stats)
    }

    /// A block's footer via a cheap ranged tail read (full read for legacy/edge cases),
    /// so callers get `row_count` / `min_ts` etc. without reading the body.
    pub fn read_footer(&self, key: &PartitionKey, id: &str) -> Result<Footer> {
        if let Some(f) = self.try_read_footer(key, id)? {
            return Ok(f);
        }
        Ok(Block::open(self.read_block(key, id)?)?.footer().clone())
    }

    fn compute_block_field_stats(&self, key: &PartitionKey, id: &str) -> Result<BlockFieldStats> {
        // Fast path: footer-only (v2). Any failure (v1 footer, missing size,
        // ranged-read unsupported) falls through to a full block read.
        if let Ok(Some(footer)) = self.try_read_footer(key, id) {
            if let Some(s) = footer_field_stats(&footer) {
                return Ok(s);
            }
        }
        Block::open(self.read_block(key, id)?)?.field_stats()
    }

    /// Read just the footer via a ranged tail read. `Ok(None)` when the object is absent
    /// or the tail isn't block-shaped (caller falls back to a full read).
    fn try_read_footer(&self, key: &PartitionKey, id: &str) -> Result<Option<Footer>> {
        let bkey = Self::block_key(key, id);
        let Some(size) = self.store.size(&bkey)? else {
            return Ok(None);
        };
        if size < 8 {
            return Ok(None);
        }
        let probe = FOOTER_PROBE.min(size);
        let tail = self.store.get_range(&bkey, size - probe, probe)?;
        let n = tail.len();
        if n < 8 || tail[n - 4..] != MAGIC[..] {
            return Ok(None);
        }
        let footer_len =
            u32::from_le_bytes([tail[n - 8], tail[n - 7], tail[n - 6], tail[n - 5]]) as usize;
        let needed = footer_len + 8;
        let footer_bytes: Vec<u8> = if needed <= n {
            tail[n - needed..n - 8].to_vec()
        } else {
            // Footer larger than the probe — one exact follow-up read.
            if needed as u64 > size {
                return Ok(None);
            }
            self.store
                .get_range(&bkey, size - needed as u64, footer_len as u64)?
        };
        Ok(serde_json::from_slice::<Footer>(&footer_bytes).ok())
    }

    /// Atomically swap the live block set: drop `remove`, add `add`, in one CAS.
    /// New blocks must already be written. Unconditional.
    pub fn swap_blocks(&self, key: &PartitionKey, remove: &[String], add: &[String]) -> Result<()> {
        let remove: std::collections::HashSet<&str> = remove.iter().map(String::as_str).collect();
        self.update(key, |m| {
            m.blocks.retain(|b| !remove.contains(b.as_str()));
            for id in add {
                if !m.blocks.contains(id) {
                    m.blocks.push(id.clone());
                }
            }
        })
    }

    /// Guarded swap for compaction: commit only if **every** `remove` id is still live,
    /// else `Ok(false)` — a concurrent compactor won, and the caller discards its orphan.
    pub fn swap_blocks_if_present(
        &self,
        key: &PartitionKey,
        remove: &[String],
        add: &[String],
    ) -> Result<bool> {
        let remove_set: std::collections::HashSet<&str> =
            remove.iter().map(String::as_str).collect();
        self.try_update(key, |m| {
            if !remove.iter().all(|id| m.blocks.contains(id)) {
                return false;
            }
            m.blocks.retain(|b| !remove_set.contains(b.as_str()));
            for id in add {
                if !m.blocks.contains(id) {
                    m.blocks.push(id.clone());
                }
            }
            true
        })
    }

    /// Delete old manifest generations beyond [`KEEP_GENERATIONS`]. Best-effort.
    fn gc_old_generations(&self, key: &PartitionKey) {
        let Ok(keys) = self.store.list(&Self::manifest_prefix(key)) else {
            return;
        };
        let mut gens: Vec<(u64, String)> = keys
            .into_iter()
            .filter_map(|k| {
                let g = Self::basename(&k)
                    .strip_suffix(".json")?
                    .parse::<u64>()
                    .ok()?;
                Some((g, k))
            })
            .collect();
        gens.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
        for (_, k) in gens.into_iter().skip(KEEP_GENERATIONS) {
            let _ = self.store.delete(&k);
        }
    }

    /// Open every live block named by the current manifest. Missing block
    /// objects are skipped (a torn append) rather than failing the whole read.
    pub fn open_blocks(&self, key: &PartitionKey) -> Result<Vec<Block>> {
        let (_, manifest) = self.current(key)?;
        let mut out = Vec::with_capacity(manifest.blocks.len());
        for id in &manifest.blocks {
            match self.read_block(key, id) {
                Ok(bytes) => out.push(Block::open(bytes)?),
                Err(_) => continue,
            }
        }
        Ok(out)
    }

    /// Current live manifest (for compaction / introspection).
    pub fn load_manifest(&self, key: &PartitionKey) -> Result<Manifest> {
        Ok(self.current(key)?.1)
    }

    /// Whether any manifest exists (generation or legacy). Distinguishes "absent from this
    /// store" from "present but empty"; the seeder uses it to find local partitions not in shared.
    pub fn has_manifest(&self, key: &PartitionKey) -> Result<bool> {
        if self.latest_gen(key)?.is_some() {
            return Ok(true);
        }
        Ok(self.store.size(&Self::legacy_manifest_key(key))?.is_some())
    }

    /// Discover all partitions by listing manifest keys and parsing their
    /// `<env>/<index>/<day>/manifest...` prefix — works over a shared store (NFS/S3).
    pub fn list_partitions(&self) -> Result<Vec<PartitionKey>> {
        use std::collections::BTreeSet;
        let mut set: BTreeSet<(String, String, chrono::NaiveDate)> = BTreeSet::new();
        for k in self.store.list("")? {
            let segs: Vec<&str> = k.split('/').collect();
            // env/index/day/manifest/<gen>.json  OR  env/index/day/manifest.json
            let is_manifest = (segs.len() >= 5 && segs[3] == "manifest")
                || (segs.len() == 4 && segs[3] == "manifest.json");
            if !is_manifest {
                continue;
            }
            if let Ok(day) = chrono::NaiveDate::parse_from_str(segs[2], "%Y-%m-%d") {
                set.insert((segs[0].to_string(), segs[1].to_string(), day));
            }
        }
        Ok(set
            .into_iter()
            .map(|(env, index, day)| PartitionKey::new(env, index, day))
            .collect())
    }

    pub fn partition_exists(&self, key: &PartitionKey) -> bool {
        self.latest_gen(key).ok().flatten().is_some()
            || self
                .store
                .size(&Self::legacy_manifest_key(key))
                .ok()
                .flatten()
                .is_some()
    }
}

/// The default local block store rooted at a data dir (filesystem backend).
pub fn local_store(root: &Path) -> BlockStore {
    BlockStore::new(root)
}

/// A local↔shared sync pair (the uploader/puller operate on these directly).
pub struct SyncPair {
    pub local: BlockStore,
    pub shared: BlockStore,
}

/// The block stores for an instance: `engine` (reads/writes, local-first with a shared
/// store) and `sync` (present only with a shared store; drives uploader/puller + compaction).
pub struct BlockSetup {
    pub engine: BlockStore,
    pub sync: Option<SyncPair>,
    pub desc: String,
}

/// Build the block stores for `serve`. With `--shared-store` the engine is local-first
/// and a [`SyncPair`] is returned; without it, a plain local store with nothing to sync.
pub async fn build_block_setup(data_dir: &Path, shared_store: Option<&str>) -> Result<BlockSetup> {
    let local_fs: Arc<dyn ObjectStore> = Arc::new(FsObjectStore::new(data_dir));
    match shared_store {
        Some(s) => {
            let shared_obj = build_object_store(Some(s), data_dir).await?;
            let engine = BlockStore::with_object_store(Arc::new(LocalFirstObjectStore::new(
                local_fs.clone(),
                shared_obj.clone(),
            )));
            let shared = BlockStore::with_object_store(shared_obj);
            let desc = format!(
                "local {} ⇄ shared {}",
                data_dir.display(),
                shared.describe()
            );
            Ok(BlockSetup {
                engine,
                sync: Some(SyncPair {
                    local: BlockStore::with_object_store(local_fs),
                    shared,
                }),
                desc,
            })
        }
        None => Ok(BlockSetup {
            engine: BlockStore::with_object_store(local_fs),
            sync: None,
            desc: format!("local {}", data_dir.display()),
        }),
    }
}
