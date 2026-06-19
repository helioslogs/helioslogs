// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Ad-hoc block write/compaction benchmarks over `optimization/` sample data.
//! Ignored by default; they print wall-clock numbers, not assertions.

use std::path::PathBuf;
use std::time::Instant;

use super::{Block, BlockWriter, Codec, Row};

/// Filter-eval throughput for a bare term across real blocks — the substring-over-
/// dictionary scan is the hot path shared by search/histogram/aggregate/discover.
#[test]
#[ignore]
fn bench_query() {
    let raw = load_small_blocks(96 * 1024 * 1024);
    assert!(!raw.is_empty(), "no sample blocks found");
    let blocks: Vec<Block> = raw
        .iter()
        .map(|b| Block::open(b.clone()).unwrap())
        .collect();
    let term = std::env::var("HELIOS_BENCH_TERM").unwrap_or_else(|_| "error".to_string());
    let filter = crate::search::query::parse(&term).unwrap();

    let iters = 10;
    let mut total = 0u64;
    let t = Instant::now();
    for _ in 0..iters {
        total = 0;
        for b in &blocks {
            total += super::query::eval(b, filter.as_ref(), None, None)
                .unwrap()
                .len();
        }
    }
    let per = t.elapsed() / iters;
    eprintln!(
        "bench_query [{term:?}]: {} blocks, {total} matches\n  eval: {per:?}/iter",
        blocks.len(),
    );
}

/// Sequential vs. rayon-parallel `eval` across a real on-disk partition.
/// Point `HELIOS_BENCH_PARTITION` at a blocks dir (`HELIOS_BENCH_TERM` for the term).
#[test]
#[ignore]
fn bench_partition() {
    use rayon::prelude::*;
    let dir = std::env::var("HELIOS_BENCH_PARTITION").expect("set HELIOS_BENCH_PARTITION");
    let term = std::env::var("HELIOS_BENCH_TERM").unwrap_or_else(|_| "refused*".to_string());
    let filter = crate::search::query::parse(&term).unwrap();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "hb").unwrap_or(false))
        .collect();
    paths.sort();
    let total_mb: u64 = paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum::<u64>()
        / 1_000_000;
    eprintln!("partition: {} blocks, {total_mb} MB", paths.len());

    let eval_one = |p: &PathBuf| -> u64 {
        let bytes = std::fs::read(p).unwrap();
        let b = Block::open(bytes).unwrap();
        super::query::eval(&b, filter.as_ref(), None, None)
            .unwrap()
            .len()
    };

    // Warm the OS page cache so we compare CPU, not cold disk.
    let _ = paths.iter().map(&eval_one).sum::<u64>();

    let t = Instant::now();
    let seq: u64 = paths.iter().map(&eval_one).sum();
    let seq_t = t.elapsed();

    let t = Instant::now();
    let par: u64 = paths.par_iter().map(&eval_one).sum();
    let par_t = t.elapsed();

    eprintln!(
        "bench_partition [{term:?}]: {seq} matches\n  sequential: {seq_t:?}\n  parallel:   {par_t:?}  ({:.1}x)",
        seq_t.as_secs_f64() / par_t.as_secs_f64(),
    );
    assert_eq!(seq, par);

    // End-to-end search+histogram through the server's BlockStore. Set
    // HELIOS_BENCH_STORE_ROOT (data dir) + HELIOS_BENCH_KEY=env:index:day.
    if let (Ok(root), Ok(keystr)) = (
        std::env::var("HELIOS_BENCH_STORE_ROOT"),
        std::env::var("HELIOS_BENCH_KEY"),
    ) {
        let kp: Vec<&str> = keystr.splitn(3, ':').collect();
        let day = chrono::NaiveDate::parse_from_str(kp[2], "%Y-%m-%d").unwrap();
        let key = crate::catalog::PartitionKey::new(kp[0], kp[1], day);
        let store = super::store::BlockStore::new(&root);
        super::cache::configure_query_cache(512);
        let run = || {
            super::query::search_histogram(
                &store,
                &key,
                filter.as_ref(),
                None,
                None,
                100,
                3_600_000,
            )
            .unwrap()
        };
        let t = Instant::now();
        let (total, hits, buckets) = run();
        let cold = t.elapsed();
        let t = Instant::now();
        let _ = run();
        let warm = t.elapsed();
        eprintln!(
            "  search_histogram: cold {cold:?}, warm(cached) {warm:?}  (total={total}, hits={}, buckets={})",
            hits.len(),
            buckets.len(),
        );
        // Triangulate the warm cost: search-only vs histogram-only.
        let sh = |limit, interval| {
            let t = Instant::now();
            let _ = super::query::search_histogram(
                &store,
                &key,
                filter.as_ref(),
                None,
                None,
                limit,
                interval,
            )
            .unwrap();
            t.elapsed()
        };
        eprintln!(
            "  warm breakdown: limit=0 {:?} | limit=1 {:?} | limit=100 {:?} | limit=1000 {:?} | hist-only(int=1h) {:?}",
            sh(0, 0),
            sh(1, 0),
            sh(100, 0),
            sh(1000, 0),
            sh(0, 3_600_000),
        );

        // How much of the warm path is just reading whole block files (the I/O
        // floor of reading 130 MB blocks when we only need the index/timestamps)?
        let t = Instant::now();
        let whole: u64 = paths
            .iter()
            .map(|p| std::fs::read(p).unwrap().len() as u64)
            .sum();
        let read_t = t.elapsed();
        // Cost of opening each block + decoding ONLY the timestamp section (what
        // a cached-eval candidate-selection pass actually needs).
        let t = Instant::now();
        let mut ts_total = 0usize;
        for p in &paths {
            let b = Block::open(std::fs::read(p).unwrap()).unwrap();
            ts_total += b.timestamps().unwrap().len();
        }
        let ts_t = t.elapsed();
        eprintln!(
            "  read all blocks whole: {read_t:?} ({} MB) | open+timestamps only: {ts_t:?} ({ts_total} ts)",
            whole / 1_000_000,
        );
    }
}

fn blocks_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("optimization/blocks")
}

/// Load every `.hb` under `optimization/blocks` whose on-disk size is below
/// `max_bytes` — i.e. the "small blocks" a compaction pass would merge.
fn load_small_blocks(max_bytes: u64) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let dir = blocks_dir();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {dir:?}: {e}"))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "hb").unwrap_or(false))
        .filter(|e| e.metadata().map(|m| m.len() < max_bytes).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        out.push(std::fs::read(&p).unwrap());
    }
    out
}

fn codec_from_env() -> Codec {
    match std::env::var("HELIOS_BLOCK_COMPRESSION") {
        Ok(v) if matches!(v.as_str(), "off" | "none" | "0") => Codec::None,
        _ => Codec::Zstd,
    }
}

/// Decode a set of small blocks to rows (the compaction read path), then encode
/// them all back into one block (the compaction write path). Reports both legs.
#[test]
#[ignore]
fn bench_compaction() {
    let codec = codec_from_env();
    let raw = load_small_blocks(8 * 1024 * 1024);
    assert!(!raw.is_empty(), "no sample blocks found");
    let total_bytes: usize = raw.iter().map(|b| b.len()).sum();

    // --- decode leg: open + reconstruct rows ---
    let t = Instant::now();
    let mut rows: Vec<Row> = Vec::new();
    for bytes in &raw {
        let block = Block::open(bytes.clone()).unwrap();
        rows.extend(block.rows().unwrap());
    }
    let decode = t.elapsed();
    let n = rows.len();

    // --- encode leg: merge all rows into one block ---
    let t = Instant::now();
    let mut w = BlockWriter::new(codec);
    for r in rows {
        w.push(r);
    }
    let merged = w.finish().unwrap();
    let encode = t.elapsed();

    eprintln!(
        "bench_compaction [{codec:?}]: {} blocks, {:.1} MB in, {n} rows\n  decode (open+rows): {decode:?}\n  encode (merge+finish): {encode:?}\n  merged out: {:.1} MB",
        raw.len(),
        total_bytes as f64 / 1e6,
        merged.len() as f64 / 1e6,
    );
}

/// Single-block write throughput: re-encode one block's rows, averaged over a few
/// iterations — the `BlockWriter::finish` hot path (tokenize → index → serialize) in isolation.
#[test]
#[ignore]
fn bench_write_single() {
    let codec = codec_from_env();
    let raw = load_small_blocks(8 * 1024 * 1024);
    assert!(!raw.is_empty(), "no sample blocks found");
    // Use the largest of the small blocks as the single-block sample.
    let bytes = raw.iter().max_by_key(|b| b.len()).unwrap();
    let rows = Block::open(bytes.clone()).unwrap().rows().unwrap();
    let n = rows.len();

    let iters = 8;
    let mut best = std::time::Duration::MAX;
    let mut total = std::time::Duration::ZERO;
    for _ in 0..iters {
        let sample = rows.clone();
        let t = Instant::now();
        let mut w = BlockWriter::new(codec);
        for r in sample {
            w.push(r);
        }
        let out = w.finish().unwrap();
        let e = t.elapsed();
        std::hint::black_box(out.len());
        best = best.min(e);
        total += e;
    }
    eprintln!(
        "bench_write_single [{codec:?}]: {n} rows, {iters} iters\n  best: {best:?}  avg: {:?}  ({:.2} M rows/s best)",
        total / iters,
        n as f64 / best.as_secs_f64() / 1e6,
    );
}
