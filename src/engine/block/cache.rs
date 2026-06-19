// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Per-block content-match cache, keyed on `(block_id, filter)` — pure because
//! blocks are immutable, so time-range changes reuse it. Byte-bounded sharded LRU.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use roaring::RoaringBitmap;

/// Cache key: immutable block id + canonical (Debug-formatted) filter AST. The
/// filter string is identical across a query's blocks, so callers compute it once.
pub type Key = (String, String);

struct Entry {
    bm: Arc<RoaringBitmap>,
    bytes: usize,
    last: u64,
}

struct Shard {
    map: HashMap<Key, Entry>,
    bytes: usize,
    cap_bytes: usize,
    tick: u64,
}

impl Shard {
    fn get(&mut self, key: &Key) -> Option<Arc<RoaringBitmap>> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(key).map(|e| {
            e.last = t;
            e.bm.clone()
        })
    }

    fn insert(&mut self, key: Key, bm: Arc<RoaringBitmap>) {
        // Account the bitmap plus rough key/entry overhead.
        let bytes = bm.serialized_size() + key.0.len() + key.1.len() + 64;
        self.tick += 1;
        let last = self.tick;
        if let Some(old) = self.map.insert(key, Entry { bm, bytes, last }) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        if self.bytes > self.cap_bytes {
            self.evict();
        }
    }

    /// Drop the oldest-touched entries down to ~90% of the cap, so we don't
    /// re-sort on the very next insert.
    fn evict(&mut self) {
        let target = self.cap_bytes * 9 / 10;
        let mut by_age: Vec<(u64, Key, usize)> = self
            .map
            .iter()
            .map(|(k, e)| (e.last, k.clone(), e.bytes))
            .collect();
        by_age.sort_unstable_by_key(|(last, _, _)| *last);
        for (_, k, b) in by_age {
            if self.bytes <= target {
                break;
            }
            self.map.remove(&k);
            self.bytes -= b;
        }
    }
}

pub struct BlockEvalCache {
    shards: Vec<Mutex<Shard>>,
    /// Fast no-lock path for the disabled (cap 0) case; also flipped by resizes.
    enabled: AtomicBool,
}

const SHARDS: usize = 16;

impl BlockEvalCache {
    fn new(cap_bytes: usize) -> Self {
        let per = (cap_bytes / SHARDS).max(1);
        let shards = (0..SHARDS)
            .map(|_| {
                Mutex::new(Shard {
                    map: HashMap::new(),
                    bytes: 0,
                    cap_bytes: per,
                    tick: 0,
                })
            })
            .collect();
        Self {
            shards,
            enabled: AtomicBool::new(cap_bytes > 0),
        }
    }

    fn shard(&self, key: &Key) -> &Mutex<Shard> {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        &self.shards[(h.finish() as usize) % SHARDS]
    }

    pub fn get(&self, key: &Key) -> Option<Arc<RoaringBitmap>> {
        if !self.enabled.load(Ordering::Relaxed) {
            return None;
        }
        self.shard(key).lock().unwrap().get(key)
    }

    pub fn insert(&self, key: Key, bm: Arc<RoaringBitmap>) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        self.shard(&key).lock().unwrap().insert(key, bm);
    }

    /// Resize the live cache to a new total byte cap (0 disables + clears it),
    /// evicting down to the new per-shard bound. Called by the runtime-config
    /// refresher so `HELIOS_QUERY_CACHE_MB` / its setting apply without a restart.
    pub fn set_cap_bytes(&self, cap_bytes: usize) {
        let per = (cap_bytes / SHARDS).max(1);
        for sh in &self.shards {
            let mut s = sh.lock().unwrap();
            s.cap_bytes = per;
            if cap_bytes == 0 {
                s.map.clear();
                s.bytes = 0;
            } else if s.bytes > s.cap_bytes {
                s.evict();
            }
        }
        self.enabled.store(cap_bytes > 0, Ordering::Relaxed);
    }
}

// Block field-stats cache: footer-derived coverage keyed by immutable block id, so
// stale entries age out on recompaction. Entry-count-bounded, always on, sharded.

use super::BlockFieldStats;

struct StatsEntry {
    stats: Arc<BlockFieldStats>,
    last: u64,
}

struct StatsShard {
    map: HashMap<String, StatsEntry>,
    cap: usize,
    tick: u64,
}

impl StatsShard {
    fn get(&mut self, id: &str) -> Option<Arc<BlockFieldStats>> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(id).map(|e| {
            e.last = t;
            e.stats.clone()
        })
    }

    fn insert(&mut self, id: String, stats: Arc<BlockFieldStats>) {
        self.tick += 1;
        let last = self.tick;
        self.map.insert(id, StatsEntry { stats, last });
        if self.map.len() > self.cap {
            self.evict();
        }
    }

    fn evict(&mut self) {
        let target = self.cap * 9 / 10;
        let mut by_age: Vec<(u64, String)> =
            self.map.iter().map(|(k, e)| (e.last, k.clone())).collect();
        by_age.sort_unstable_by_key(|(last, _)| *last);
        for (_, k) in by_age {
            if self.map.len() <= target {
                break;
            }
            self.map.remove(&k);
        }
    }
}

pub struct BlockStatsCache {
    shards: Vec<Mutex<StatsShard>>,
}

impl BlockStatsCache {
    fn new(total_cap: usize) -> Self {
        let per = (total_cap / SHARDS).max(1);
        let shards = (0..SHARDS)
            .map(|_| {
                Mutex::new(StatsShard {
                    map: HashMap::new(),
                    cap: per,
                    tick: 0,
                })
            })
            .collect();
        Self { shards }
    }

    fn shard(&self, id: &str) -> &Mutex<StatsShard> {
        let mut h = DefaultHasher::new();
        id.hash(&mut h);
        &self.shards[(h.finish() as usize) % SHARDS]
    }

    pub fn get(&self, id: &str) -> Option<Arc<BlockFieldStats>> {
        self.shard(id).lock().unwrap().get(id)
    }

    pub fn insert(&self, id: String, stats: Arc<BlockFieldStats>) {
        self.shard(&id).lock().unwrap().insert(id, stats);
    }
}

static STATS_CACHE: OnceLock<BlockStatsCache> = OnceLock::new();

/// The block field-stats cache — always on (immutable, count-bounded). Cap is
/// generous: ~200k blocks' worth of small stat structs.
pub fn block_stats_cache() -> &'static BlockStatsCache {
    STATS_CACHE.get_or_init(|| BlockStatsCache::new(200_000))
}

static CACHE: OnceLock<BlockEvalCache> = OnceLock::new();

/// Install the cache at serve startup. The instance is always created (so it can
/// be resized live); `cap_mb == 0` starts it disabled — every lookup is a miss.
pub fn configure_query_cache(cap_mb: usize) {
    CACHE
        .get_or_init(|| BlockEvalCache::new(cap_mb * 1024 * 1024))
        .set_cap_bytes(cap_mb * 1024 * 1024);
}

/// Resize the installed cache (MB) at runtime. No-op before [`configure_query_cache`]
/// (tests / CLI search never install one).
pub fn resize_query_cache(cap_mb: usize) {
    if let Some(c) = CACHE.get() {
        c.set_cap_bytes(cap_mb * 1024 * 1024);
    }
}

/// The installed cache, or `None` when not configured (tests, CLI search). A
/// disabled (cap 0) cache still returns `Some` but misses every lookup.
pub fn query_cache() -> Option<&'static BlockEvalCache> {
    CACHE.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    impl BlockEvalCache {
        fn total_bytes(&self) -> usize {
            self.shards.iter().map(|s| s.lock().unwrap().bytes).sum()
        }
    }

    fn bm(rows: &[u32]) -> Arc<RoaringBitmap> {
        Arc::new(rows.iter().copied().collect())
    }

    #[test]
    fn roundtrips_and_keys_on_block_plus_filter() {
        let c = BlockEvalCache::new(64 * 1024 * 1024);
        let k1 = ("blk1".to_string(), "Term{error}".to_string());
        let k2 = ("blk1".to_string(), "Term{warn}".to_string());
        c.insert(k1.clone(), bm(&[1, 2, 3]));
        c.insert(k2.clone(), bm(&[9]));
        assert_eq!(
            c.get(&k1).unwrap().iter().collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(c.get(&k2).unwrap().len(), 1);
        // A different block id is a miss even with the same filter.
        assert!(c
            .get(&("blk2".to_string(), "Term{error}".to_string()))
            .is_none());
    }

    #[test]
    fn eviction_keeps_bytes_under_cap() {
        // Tiny cap; insert far more than fits and confirm the bound holds and
        // the most-recently-touched entry survives.
        let cap = 16 * 1024;
        let c = BlockEvalCache::new(cap);
        for i in 0..2000u32 {
            c.insert(
                (format!("blk{i}"), "f".to_string()),
                bm(&(0..50).map(|r| r + i * 100).collect::<Vec<_>>()),
            );
        }
        let hot = ("blkHOT".to_string(), "f".to_string());
        c.insert(hot.clone(), bm(&[7]));
        assert!(c.get(&hot).is_some(), "just-inserted entry must survive");
        assert!(
            c.total_bytes() <= cap,
            "cache bytes {} exceeded cap {cap}",
            c.total_bytes()
        );
    }
}
