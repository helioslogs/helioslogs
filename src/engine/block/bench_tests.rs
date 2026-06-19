// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Assertion-backed benchmarks for the memory/IO work:
//!   * read amplification — a selective query must read only the sections it
//!     touches (ranged reads), not whole blocks. Measured via [`CountingStore`].
//!   * compaction memory — peak `allocated` scales with the target (group) size,
//!     not the partition size. Measured via a jemalloc peak sampler (`#[ignore]`,
//!     run with `--ignored --test-threads=1`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use chrono::NaiveDate;
use tempfile::TempDir;

use crate::catalog::PartitionKey;
use crate::search::query::parse;

use super::objstore::{FsObjectStore, ObjectStore};
use super::store::BlockStore;
use super::{Block, BlockWriter, Codec, Row};

// An ObjectStore decorator that tallies the bytes every read actually returns, so a
// test can assert how much a query pulled off the backing store (get + get_range).
struct CountingStore {
    inner: Arc<dyn ObjectStore>,
    bytes: AtomicU64,
    gets: AtomicU64,
    ranges: AtomicU64,
}

impl CountingStore {
    fn new(inner: Arc<dyn ObjectStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            bytes: AtomicU64::new(0),
            gets: AtomicU64::new(0),
            ranges: AtomicU64::new(0),
        })
    }
    fn reset(&self) {
        self.bytes.store(0, Ordering::Relaxed);
        self.gets.store(0, Ordering::Relaxed);
        self.ranges.store(0, Ordering::Relaxed);
    }
    fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
}

impl ObjectStore for CountingStore {
    fn get(&self, key: &str) -> Result<Vec<u8>> {
        let b = self.inner.get(key)?;
        self.bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
        self.gets.fetch_add(1, Ordering::Relaxed);
        Ok(b)
    }
    fn get_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let b = self.inner.get_range(key, offset, len)?;
        self.bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
        self.ranges.fetch_add(1, Ordering::Relaxed);
        Ok(b)
    }
    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.inner.put(key, bytes)
    }
    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
        self.inner.put_if_absent(key, bytes)
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix)
    }
    fn size(&self, key: &str) -> Result<Option<u64>> {
        self.inner.size(key)
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 5, 30).unwrap()
}
fn key() -> PartitionKey {
    PartitionKey::new("default", "orders", day())
}

/// One block of `rows` events; every row carries a chunky, high-entropy `raw`
/// payload (so the raw/string sections genuinely dominate the block, and blocks are
/// large enough that the 64 KB footer probe is negligible — as in production). When
/// `needle` is set, row 0 embeds it in the message so exactly that block carries it.
fn write_block(
    store: &BlockStore,
    k: &PartitionKey,
    base_ts: i64,
    rows: usize,
    needle: Option<&str>,
    codec: Codec,
) {
    let mut w = BlockWriter::new(codec);
    let mut seed: u64 = base_ts as u64 | 1;
    for i in 0..rows {
        // Cheap LCG hex so raw doesn't compress to nothing (mirrors real log entropy).
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let entropy = format!("{:016x}{:016x}", seed, seed.rotate_left(17));
        let msg = match needle {
            Some(n) if i == 0 => format!("request completed {n} status ok"),
            _ => "request completed status ok".to_string(),
        };
        let raw = format!(
            "{{\"msg\":\"{msg}\",\"path\":\"/api/v1/resource/{i}\",\"trace\":\"{entropy}\",\"i\":{i}}}"
        );
        w.push(Row {
            ts_millis: base_ts + i as i64,
            message: Some(msg),
            source: Some("/var/log/app.log".to_string()),
            raw: Some(raw),
            fields: vec![],
        });
    }
    store.append_block(k, &w.finish().unwrap()).unwrap();
}

/// Read amplification: a selective query must read far fewer bytes than the blocks
/// hold. An absent term should bloom-skip every block (footer + bloom only); a
/// rare term should read its one block's sections, never all of them.
#[test]
fn ranged_reads_avoid_amplification() {
    let dir = TempDir::new().unwrap();
    let k = key();
    let writer = BlockStore::new(dir.path());
    // 8 blocks, each a few MB (so the footer probe is negligible, as in prod); the
    // rare term lives in exactly one of them.
    for b in 0..8 {
        let needle = (b == 5).then_some("zebraneedle");
        write_block(
            &writer,
            &k,
            1_000_000 + b * 100_000,
            20_000,
            needle,
            Codec::Zstd,
        );
    }

    // Total on-disk block bytes — the floor the old whole-block read path paid.
    let ids = writer.load_manifest(&k).unwrap().blocks;
    let total: u64 = ids.iter().filter_map(|id| writer.block_size(&k, id)).sum();
    assert!(total > 100_000, "fixture should be sizeable, got {total}");

    let counting = CountingStore::new(Arc::new(FsObjectStore::new(dir.path())));
    let store = BlockStore::with_object_store(counting.clone());
    let run = |q: &str| {
        let filter = parse(q).unwrap();
        super::query::search_histogram(&store, &k, filter.as_ref(), None, None, 100, 0).unwrap()
    };

    // Absent term: bloom-gated, so only footer + bloom of each block is touched.
    counting.reset();
    let (absent_total, _, _) = run("qqzzxxnotpresent");
    let absent_bytes = counting.bytes();

    // Rare term: matches one block. We read that block's sections + every block's
    // footer/bloom — still well under the whole-partition size.
    counting.reset();
    let (rare_total, hits, _) = run("zebraneedle");
    let rare_bytes = counting.bytes();

    eprintln!(
        "read-amplification: total-on-disk {total} | absent-term {absent_bytes} ({:.1}%) | \
         rare-term {rare_bytes} ({:.1}%)",
        100.0 * absent_bytes as f64 / total as f64,
        100.0 * rare_bytes as f64 / total as f64,
    );

    assert_eq!(absent_total, 0);
    assert!(
        absent_bytes < total / 4,
        "absent-term scan read {absent_bytes} of {total} bytes (want <25%) — bloom skip not working"
    );
    assert_eq!(rare_total, 1, "exactly one row carries the needle");
    assert_eq!(hits.len(), 1);
    assert!(
        rare_bytes < total / 2,
        "rare-term scan read {rare_bytes} of {total} bytes (want <50%)"
    );
}

// jemalloc peak-`allocated` during `f`, minus the baseline at entry. A background
// sampler advances the epoch and tracks the max; only meaningful run serially
// (`--test-threads=1`), since `allocated` is process-wide.
fn peak_allocated_during<T>(f: impl FnOnce() -> T) -> (T, u64) {
    use std::sync::atomic::AtomicBool;
    use tikv_jemalloc_ctl::{epoch, stats};

    let stop = Arc::new(AtomicBool::new(false));
    let peak = Arc::new(AtomicU64::new(0));
    let sampler = {
        let stop = stop.clone();
        let peak = peak.clone();
        std::thread::spawn(move || {
            let (Ok(e), Ok(alloc)) = (epoch::mib(), stats::allocated::mib()) else {
                return;
            };
            let _ = e.advance();
            let baseline = alloc.read().unwrap_or(0) as u64;
            while !stop.load(Ordering::Relaxed) {
                let _ = e.advance();
                let cur = alloc.read().unwrap_or(0) as u64;
                peak.fetch_max(cur.saturating_sub(baseline), Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        })
    };
    // Give the sampler a moment to record the baseline before the work starts.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let out = f();
    stop.store(true, Ordering::Relaxed);
    sampler.join().unwrap();
    (out, peak.load(Ordering::Relaxed))
}

/// Compaction memory is bounded by the *target block size* (the group the writer
/// accumulates), not the whole partition — `compact_partition` streams one group at
/// a time and drops each source block as it goes. This benchmark proves the bound by
/// compacting the same fixture two ways: one big group vs. small (paired) groups,
/// and showing the paired run's peak is markedly lower.
///
/// (Aside: `for_each_row` vs. a per-block `Vec<Row>` is memory-neutral here — the
/// peak is the writer's group accumulation, not the per-block decode — so the lever
/// for compaction RSS is the target size, not the row-streaming path.)
#[test]
#[ignore = "memory benchmark; run with --ignored --test-threads=1"]
fn compaction_peak_scales_with_target() {
    // Build an identical 8-block fixture in its own dir, returning (store, key, sizes).
    let build = || {
        let dir = TempDir::new().unwrap();
        let k = key();
        let store = BlockStore::new(dir.path());
        for b in 0..8 {
            write_block(
                &store,
                &k,
                1_000_000 + b * 100_000,
                5_000,
                None,
                Codec::Zstd,
            );
        }
        let total: u64 = store
            .load_manifest(&k)
            .unwrap()
            .blocks
            .iter()
            .filter_map(|id| store.block_size(&k, id))
            .sum();
        let one = store
            .block_size(&k, &store.load_manifest(&k).unwrap().blocks[0])
            .unwrap();
        (dir, store, k, total, one)
    };

    // Whole partition as ONE group (huge target) — writer accumulates all 8 blocks.
    let (_d1, store1, k1, total, one) = build();
    let (_, peak_one_group) = peak_allocated_during(|| {
        super::compact::compact_partition(&store1, Codec::Zstd, &k1, 1 << 30, 2, 0, usize::MAX)
            .unwrap()
            .unwrap()
    });

    // Paired groups (target ≈ 2 blocks) — writer never holds more than ~2 blocks.
    let (_d2, store2, k2, _, _) = build();
    let (_, peak_paired) = peak_allocated_during(|| {
        super::compact::compact_partition(&store2, Codec::Zstd, &k2, one * 2 + 1, 2, 0, usize::MAX)
            .unwrap()
            .unwrap()
    });

    eprintln!(
        "compaction peak allocated ({:.1} MB partition on disk, {:.2} MB/block): \
         one-group {:.1} MB | paired-groups {:.1} MB",
        total as f64 / 1e6,
        one as f64 / 1e6,
        peak_one_group as f64 / 1e6,
        peak_paired as f64 / 1e6,
    );
    assert!(
        peak_paired < peak_one_group,
        "paired-group peak {peak_paired} should be below one-group peak {peak_one_group} \
         — compaction memory must scale with target, not partition size"
    );
}

/// Decompose the one-group compaction peak: holding all source rows vs. the full
/// finish() (rows + term-index build + body). Tells us whether a columnar merge
/// (which never holds rows + all indexes at once) can actually help.
#[test]
#[ignore = "memory decomposition; run with --ignored --test-threads=1"]
fn compaction_peak_decomposition() {
    let dir = TempDir::new().unwrap();
    let k = key();
    let store = BlockStore::new(dir.path());
    for b in 0..8 {
        write_block(
            &store,
            &k,
            1_000_000 + b * 100_000,
            5_000,
            None,
            Codec::Zstd,
        );
    }
    let ids = store.load_manifest(&k).unwrap().blocks;
    let total: u64 = ids.iter().filter_map(|id| store.block_size(&k, id)).sum();

    // (a) peak just to decode + hold all source rows as Vec<Row>.
    let (rows, peak_rows) = peak_allocated_during(|| {
        let mut all: Vec<Row> = Vec::new();
        for id in &ids {
            let b = Block::open(store.read_block(&k, id).unwrap()).unwrap();
            b.for_each_row(|r| {
                all.push(r);
                Ok(())
            })
            .unwrap();
        }
        all
    });
    let n = rows.len();
    drop(rows);

    // (b) peak of the full row-based finish (push all rows + build columns/indexes).
    let (_, peak_finish) = peak_allocated_during(|| {
        let mut w = BlockWriter::new(Codec::Zstd);
        for id in &ids {
            let b = Block::open(store.read_block(&k, id).unwrap()).unwrap();
            b.for_each_row(|r| {
                w.push(r);
                Ok(())
            })
            .unwrap();
        }
        w.finish().unwrap()
    });

    // (c) peak to decode + hold all source blocks' `raw` term indexes at once — the
    // irreducible working set a columnar merge MUST hold to merge the dominant field
    // (postings + positions), before it can even write the merged output index.
    let (idxs, peak_raw_idx) = peak_allocated_during(|| {
        let mut held = Vec::new();
        for id in &ids {
            let b = Block::open(store.read_block(&k, id).unwrap()).unwrap();
            if let Some(fi) = b.load_field_index("raw").unwrap() {
                held.push(fi);
            }
        }
        held
    });
    drop(idxs);

    eprintln!(
        "compaction decomposition ({:.1} MB on disk, {n} rows): hold-rows {:.1} MB | \
         full-finish {:.1} MB | finish-overhead {:.1} MB | source raw term-indexes {:.1} MB",
        total as f64 / 1e6,
        peak_rows as f64 / 1e6,
        peak_finish as f64 / 1e6,
        (peak_finish as f64 - peak_rows as f64) / 1e6,
        peak_raw_idx as f64 / 1e6,
    );
}
