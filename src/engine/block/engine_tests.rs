// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Store + `BlockEngine` tests: the CAS manifest, buffered writer, compaction,
//! and the block engine driven through the `PartitionEngine` interface + the
//! public cross-partition `search` entry.

use chrono::NaiveDate;
use tempfile::TempDir;

use crate::catalog::{Catalog, PartitionKey};
use crate::engine::{PartitionEngine, TimeRange};
use crate::schema::build_schema;
use crate::search::query::{parse, Node};

use super::store::BlockStore;
use super::{BlockEngine, BlockWriter, Codec, Row};

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 5, 30).unwrap()
}

fn key() -> PartitionKey {
    PartitionKey::new("default", "orders", day())
}

fn events() -> Vec<serde_json::Value> {
    [
        r#"{"timestamp":"2026-05-30T10:00:00Z","message":"order placed","service":"orders","status":200}"#,
        r#"{"timestamp":"2026-05-30T10:05:00Z","message":"payment-gateway timeout","service":"payments","status":500}"#,
        r#"{"timestamp":"2026-05-30T10:10:00Z","message":"order shipped","service":"orders","status":200}"#,
        r#"{"timestamp":"2026-05-30T10:15:00Z","message":"db connection failed","service":"payments","status":503}"#,
    ]
    .iter()
    .map(|s| serde_json::from_str(s).unwrap())
    .collect()
}

fn node_for(q: &str) -> Node {
    parse(q).unwrap().unwrap_or(Node::All)
}

/// Combined `search_histogram` must be indistinguishable from separate `search` +
/// `histogram` calls — same count, hit order, and buckets.
#[test]
fn search_histogram_matches_separate_calls() {
    let dir = TempDir::new().unwrap();
    let k = key();
    let engine = BlockEngine::new(dir.path(), Codec::Zstd);
    // Two blocks, ingested out of order, so the merge is exercised.
    let evs = events();
    engine.ingest(&k, &evs[2..4], None).unwrap();
    engine.ingest(&k, &evs[0..2], None).unwrap();

    let filter = node_for("service:orders");
    let time = TimeRange::default(); // unbounded ⇒ histogram doesn't pre-fill
    let interval = "1h";

    let (s_count, s_hits) = engine.search(&k, Some(&filter), time, 100).unwrap();
    let h_buckets = engine.histogram(&k, Some(&filter), time, interval).unwrap();
    let (c_count, c_hits, c_buckets) = engine
        .search_histogram(&k, Some(&filter), time, 100, interval)
        .unwrap();

    assert_eq!(c_count, s_count, "match count identical to search()");
    let s_raws: Vec<_> = s_hits.iter().map(|(_, h)| h.raw.clone()).collect();
    let c_raws: Vec<_> = c_hits.iter().map(|(_, h)| h.raw.clone()).collect();
    assert_eq!(c_raws, s_raws, "hit order/content identical to search()");
    assert_eq!(c_buckets, h_buckets, "buckets identical to histogram()");
    // Bucket counts ignore the hit `limit`: limiting hits to 1 still counts both
    // matched rows into the histogram.
    let (_, capped_hits, capped_buckets) = engine
        .search_histogram(&k, Some(&filter), time, 1, interval)
        .unwrap();
    assert_eq!(capped_hits.len(), 1);
    assert_eq!(capped_buckets, h_buckets, "limit bounds hits, not buckets");
}

// store round-trips

#[test]
fn store_appends_and_reopens_blocks() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    assert!(!store.partition_exists(&k));

    let mut w = BlockWriter::new(Codec::Zstd);
    w.push(Row {
        ts_millis: 1,
        message: Some("a".into()),
        source: None,
        raw: Some("{}".into()),
        fields: vec![],
    });
    let id = store.append_block(&k, &w.finish().unwrap()).unwrap();

    assert!(store.partition_exists(&k));
    assert_eq!(store.load_manifest(&k).unwrap().blocks, vec![id]);
    let blocks = store.open_blocks(&k).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].row_count(), 1);
}

#[test]
fn store_merges_multiple_blocks_in_time_order() {
    let dir = TempDir::new().unwrap();
    let engine = BlockEngine::new(dir.path(), Codec::None);
    let k = key();
    // Two separate ingests ⇒ two blocks; search must merge them newest-first.
    engine.ingest(&k, &events()[..2], None).unwrap();
    engine.ingest(&k, &events()[2..], None).unwrap();

    let (count, hits) = engine.search(&k, None, TimeRange::default(), 10).unwrap();
    assert_eq!(count, 4);
    assert_eq!(hits.len(), 4);
    // Newest first across both blocks.
    assert_eq!(hits[0].1.message.as_deref(), Some("db connection failed"));
    assert_eq!(hits[3].1.message.as_deref(), Some("order placed"));
    // partition label is stamped.
    assert_eq!(hits[0].1.partition, "orders/2026-05-30");
}

fn one_row_block(ts: i64, msg: &str) -> Vec<u8> {
    let mut w = BlockWriter::new(Codec::None);
    w.push(Row {
        ts_millis: ts,
        message: Some(msg.to_string()),
        source: None,
        raw: Some("{}".into()),
        fields: vec![],
    });
    w.finish().unwrap()
}

#[test]
fn concurrent_appends_no_lost_updates() {
    // 12 threads race to append to the SAME partition manifest. The old
    // read-modify-write+rename would lose updates here; CAS must keep all 12.
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    let n = 12i64;

    let handles: Vec<_> = (0..n)
        .map(|i| {
            let store = store.clone();
            let k = k.clone();
            std::thread::spawn(move || store.append_block(&k, &one_row_block(i, "x")).unwrap())
        })
        .collect();
    let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let manifest = store.load_manifest(&k).unwrap();
    assert_eq!(
        manifest.blocks.len(),
        n as usize,
        "every concurrent append must survive in the manifest"
    );
    for id in &ids {
        assert!(manifest.blocks.contains(id), "lost block {id}");
    }
    let total: u32 = store
        .open_blocks(&k)
        .unwrap()
        .iter()
        .map(|b| b.row_count())
        .sum();
    assert_eq!(total, n as u32);
}

#[test]
fn manifest_generations_advance_and_gc() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    for i in 0..5 {
        store.append_block(&k, &one_row_block(i, "x")).unwrap();
    }
    assert_eq!(store.load_manifest(&k).unwrap().blocks.len(), 5);

    // Old generations are GC'd — only the newest few files remain.
    let mdir = dir.path().join("default/orders/2026-05-30/manifest");
    let json_files = std::fs::read_dir(&mdir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .count();
    assert!(
        json_files <= 3,
        "expected ≤3 retained generations, found {json_files}"
    );
}

#[test]
fn swap_blocks_replaces_set() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    let a = store.append_block(&k, &one_row_block(1, "a")).unwrap();
    let b = store.append_block(&k, &one_row_block(2, "b")).unwrap();
    // Compaction-style swap: replace {a, b} with one merged block c.
    let c = store.append_block(&k, &one_row_block(3, "c")).unwrap();
    store
        .swap_blocks(&k, &[a.clone(), b.clone()], &[c.clone()])
        .unwrap();
    let blocks = store.load_manifest(&k).unwrap().blocks;
    assert_eq!(blocks, vec![c]);
    assert!(!blocks.contains(&a) && !blocks.contains(&b));
}

#[test]
fn guarded_swap_aborts_when_inputs_already_merged() {
    // Two compactors race: C1 merged {a,b}→m1, so C2's swap {a,b}→m2 must abort
    // (inputs gone), leaving the manifest {m1} with no duplicated rows.
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    let a = store.append_block(&k, &one_row_block(1, "a")).unwrap();
    let b = store.append_block(&k, &one_row_block(2, "b")).unwrap();

    // C1 wins.
    let m1 = store.write_block(&k, &one_row_block(9, "m1")).unwrap();
    assert!(store
        .swap_blocks_if_present(&k, &[a.clone(), b.clone()], &[m1.clone()])
        .unwrap());

    // C2 loses: its inputs {a,b} are already gone.
    let m2 = store.write_block(&k, &one_row_block(9, "m2")).unwrap();
    let committed = store
        .swap_blocks_if_present(&k, &[a, b], &[m2.clone()])
        .unwrap();
    assert!(!committed, "guarded swap must abort when inputs are gone");
    assert_eq!(
        store.load_manifest(&k).unwrap().blocks,
        vec![m1],
        "manifest must hold only the first merge — no duplication"
    );
    assert!(!store.load_manifest(&k).unwrap().blocks.contains(&m2));
}

// compaction (size-driven)

#[test]
fn compaction_merges_small_blocks_under_large_target() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    for i in 0..5 {
        store
            .append_block(&k, &one_row_block(i, &format!("m{i}")))
            .unwrap();
    }
    // Huge target ⇒ all 5 small blocks pack into one group → one merged block.
    let stats =
        super::compact::compact_partition(&store, Codec::Zstd, &k, 1 << 30, 4, 0, usize::MAX)
            .unwrap()
            .unwrap();
    assert_eq!(stats.merged_blocks, 5);
    assert_eq!(stats.new_blocks, 1);

    let blocks = store.open_blocks(&k).unwrap();
    assert_eq!(blocks.len(), 1, "5 small blocks → 1");
    assert_eq!(blocks.iter().map(|b| b.row_count()).sum::<u32>(), 5);
    // Merged files were deleted — only the compacted block remains on disk.
    let n_files = std::fs::read_dir(dir.path().join("default/orders/2026-05-30/blocks"))
        .unwrap()
        .count();
    assert_eq!(n_files, 1);
}

#[test]
fn compaction_groups_to_target_size() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    // Six byte-identical blocks ⇒ identical on-disk size S.
    let blk = one_row_block(1, "x");
    for _ in 0..6 {
        store.append_block(&k, &blk).unwrap();
    }
    let s = store
        .block_size(&k, &store.load_manifest(&k).unwrap().blocks[0])
        .unwrap();
    // Target = S+1 ⇒ a group seals after the 2nd block (2S ≥ S+1) → pairs.
    let stats = super::compact::compact_partition(&store, Codec::None, &k, s + 1, 2, 0, usize::MAX)
        .unwrap()
        .unwrap();
    assert_eq!(stats.new_blocks, 3, "6 blocks → 3 target-sized pairs");
    assert_eq!(stats.merged_blocks, 6);

    let blocks = store.open_blocks(&k).unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks.iter().map(|b| b.row_count()).sum::<u32>(), 6);
}

#[test]
fn compaction_skips_when_too_few_small_blocks() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    for i in 0..3 {
        store.append_block(&k, &one_row_block(i, "x")).unwrap();
    }
    // Below the min-small-blocks threshold ⇒ no-op.
    let res = super::compact::compact_partition(&store, Codec::Zstd, &k, 1 << 30, 4, 0, usize::MAX)
        .unwrap();
    assert!(res.is_none());
    assert_eq!(store.load_manifest(&k).unwrap().blocks.len(), 3);
}

#[test]
fn compaction_skips_when_group_below_min_compact_size() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    // Five tiny blocks: enough to clear min-small-blocks, but their combined
    // size is far under a 5 MB floor ⇒ not worth rewriting.
    for i in 0..5 {
        store
            .append_block(&k, &one_row_block(i, &format!("m{i}")))
            .unwrap();
    }
    let res = super::compact::compact_partition(
        &store,
        Codec::Zstd,
        &k,
        1 << 30,
        4,
        5 * 1024 * 1024,
        usize::MAX,
    )
    .unwrap();
    assert!(res.is_none(), "group under the floor must not be rewritten");
    assert_eq!(
        store.load_manifest(&k).unwrap().blocks.len(),
        5,
        "blocks left untouched until enough data accumulates"
    );
}

#[test]
fn compaction_waives_floor_when_too_many_small_blocks() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    // Ten tiny blocks whose total is far under the 5 MB floor — normally a no-op,
    // but the small-block count hits the override (10) so they get packed anyway.
    for i in 0..10 {
        store
            .append_block(&k, &one_row_block(i, &format!("m{i}")))
            .unwrap();
    }
    let stats =
        super::compact::compact_partition(&store, Codec::Zstd, &k, 1 << 30, 4, 5 * 1024 * 1024, 10)
            .unwrap()
            .expect("too many small blocks must force a merge despite the floor");
    assert_eq!(stats.merged_blocks, 10);
    assert_eq!(
        store.load_manifest(&k).unwrap().blocks.len(),
        1,
        "the swarm of tiny blocks is packed into one"
    );
}

#[test]
fn compaction_preserves_query_results() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    // Four events as four separate blocks.
    for ev in events() {
        let row = crate::indexer::ingest::json_to_row(&ev, None).unwrap();
        let mut w = BlockWriter::new(Codec::Zstd);
        w.push(row);
        store.append_block(&k, &w.finish().unwrap()).unwrap();
    }
    let engine = BlockEngine::new(dir.path(), Codec::Zstd);
    let q = node_for("service:orders");
    let before = engine
        .search(&k, Some(&q), TimeRange::default(), 100)
        .unwrap();

    super::compact::compact_partition(&store, Codec::Zstd, &k, 1 << 30, 4, 0, usize::MAX)
        .unwrap()
        .unwrap();

    let after = engine
        .search(&k, Some(&q), TimeRange::default(), 100)
        .unwrap();
    assert_eq!(store.load_manifest(&k).unwrap().blocks.len(), 1);
    assert_eq!(before.0, after.0, "count unchanged by compaction");
    let b_raws: Vec<_> = before.1.iter().filter_map(|(_, h)| h.raw.clone()).collect();
    let a_raws: Vec<_> = after.1.iter().filter_map(|(_, h)| h.raw.clone()).collect();
    assert_eq!(b_raws, a_raws, "same rows, same order after compaction");
}

#[test]
fn array_of_objects_multivalue_and_leaf_resolution() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    // An order with two line-items, and a refund with its own `qty` — separate
    // blocks so we also exercise cross-block resolution.
    let evs = [
        r#"{"timestamp":"2026-05-30T10:00:00Z","message":"order placed","items":[{"sku":"sku-A0006","qty":1,"price_cents":3999},{"sku":"sku-C0006","qty":2,"price_cents":9999}]}"#,
        r#"{"timestamp":"2026-05-30T10:05:00Z","message":"refund issued","refunds":[{"qty":5}]}"#,
    ];
    for ev in evs {
        let v: serde_json::Value = serde_json::from_str(ev).unwrap();
        let row = crate::indexer::ingest::json_to_row(&v, None).unwrap();
        let mut w = BlockWriter::new(Codec::Zstd);
        w.push(row);
        store.append_block(&k, &w.finish().unwrap()).unwrap();
    }
    let engine = BlockEngine::new(dir.path(), Codec::Zstd);
    let count = |q: &str| {
        let node = node_for(q);
        engine
            .search(&k, Some(&node), TimeRange::default(), 100)
            .unwrap()
            .0
    };

    // Multi-value numeric: BOTH the first element (value column) and later
    // elements (term index) match equality.
    assert_eq!(count("items.qty:1"), 1, "first-element qty");
    assert_eq!(count("items.qty:2"), 1, "second-element qty via term index");
    assert_eq!(count("items.price_cents:9999"), 1, "second-element price");
    assert_eq!(count("items.qty:99"), 0, "absent value matches nothing");
    // Multi-value string element.
    assert_eq!(count("items.sku:sku-C0006"), 1, "second-element sku");
    // Leaf resolution: bare `qty` reaches both `items.qty` and `refunds.qty`.
    assert_eq!(count("qty:1"), 1, "qty:1 → the order via items.qty");
    assert_eq!(count("qty:5"), 1, "qty:5 → the refund via refunds.qty");
    // A dotted path stays exact — `items.qty` must not leak into `refunds.qty`.
    assert_eq!(count("items.qty:5"), 0, "exact path doesn't cross siblings");
}

#[test]
fn bare_wildcard_matches_source_like_scoped() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    // Two events whose `source` ends in `.gz`, one that doesn't.
    let evs = [
        r#"{"timestamp":"2026-05-30T10:00:00Z","message":"a","source":"loadgen/apache-20260606.ndjson.gz"}"#,
        r#"{"timestamp":"2026-05-30T10:01:00Z","message":"b","source":"loadgen/otel-20260606.ndjson.gz"}"#,
        r#"{"timestamp":"2026-05-30T10:02:00Z","message":"c","source":"loadgen/syslog-20260606.log"}"#,
    ];
    for ev in evs {
        let v: serde_json::Value = serde_json::from_str(ev).unwrap();
        let row = crate::indexer::ingest::json_to_row(&v, None).unwrap();
        let mut w = BlockWriter::new(Codec::Zstd);
        w.push(row);
        store.append_block(&k, &w.finish().unwrap()).unwrap();
    }
    let engine = BlockEngine::new(dir.path(), Codec::Zstd);
    let count = |q: &str| {
        let node = node_for(q);
        engine
            .search(&k, Some(&node), TimeRange::default(), 100)
            .unwrap()
            .0
    };

    // The bug: `source:*gz` matched but bare `*gz` did not. Both must now find
    // the two gz sources, and exact source lookups still work both ways.
    assert_eq!(count("source:*gz"), 2, "scoped wildcard");
    assert_eq!(count("*gz"), 2, "bare wildcard matches source too");
    assert_eq!(
        count("loadgen/apache-20260606.ndjson.gz"),
        1,
        "bare exact source still matches"
    );
    assert_eq!(count("*log"), 1, "bare wildcard hits the non-gz source");
}

/// Richer event set covering every column kind compaction rebuilds, including a
/// mixed-type `amount` (numeric on some rows, string on others).
fn rich_events() -> Vec<serde_json::Value> {
    [
        r#"{"timestamp":"2026-05-30T10:00:00Z","message":"payment gateway timeout","service":"payments","status":500,"latency":12.5,"ok":false,"amount":10.0}"#,
        r#"{"timestamp":"2026-05-30T10:01:00Z","message":"order placed","service":"orders","status":200,"latency":3.0,"ok":true,"amount":"-"}"#,
        r#"{"timestamp":"2026-05-30T10:02:00Z","message":"order shipped fast","service":"orders","status":200,"latency":1.5,"ok":true,"amount":20.0}"#,
        r#"{"timestamp":"2026-05-30T10:03:00Z","message":"db connection failed","service":"payments","status":503,"latency":7.0,"ok":false,"amount":5.0}"#,
        r#"{"timestamp":"2026-05-30T10:04:00Z","message":"payment gateway recovered","service":"payments","status":200,"latency":2.0,"ok":true,"amount":15.0}"#,
        r#"{"timestamp":"2026-05-30T10:05:00Z","message":"order placed again","service":"orders","status":201,"latency":4.0,"ok":true,"amount":"n/a"}"#,
    ]
    .iter()
    .map(|s| serde_json::from_str(s).unwrap())
    .collect()
}

/// Canonicalize rows (sort fields, then sort rows) so two block sets compare
/// equal regardless of block boundaries or column emit order.
fn normalize_rows(mut rows: Vec<Row>) -> Vec<Row> {
    for r in rows.iter_mut() {
        r.fields
            .sort_by(|a, b| (&a.0, format!("{:?}", a.1)).cmp(&(&b.0, format!("{:?}", b.1))));
    }
    rows.sort_by(|a, b| (a.ts_millis, &a.message, &a.raw).cmp(&(b.ts_millis, &b.message, &b.raw)));
    rows
}

fn all_rows(store: &BlockStore, k: &PartitionKey) -> Vec<Row> {
    let rows: Vec<Row> = store
        .open_blocks(k)
        .unwrap()
        .iter()
        .flat_map(|b| b.rows().unwrap())
        .collect();
    normalize_rows(rows)
}

/// Golden net for compaction refactors: a compacted partition must be indistinguishable
/// from the un-compacted one. Built from out-of-order blocks to exercise the merge re-sort.
#[test]
fn compaction_is_equivalent_across_field_types_and_blocks() {
    let dir = TempDir::new().unwrap();
    let k = key();
    let engine = BlockEngine::new(dir.path(), Codec::Zstd);
    let evs = rich_events();
    // Three batches → three blocks, middle batch ingested first (out of order).
    engine.ingest(&k, &evs[2..4], None).unwrap();
    engine.ingest(&k, &evs[0..2], None).unwrap();
    engine.ingest(&k, &evs[4..6], None).unwrap();

    let store = BlockStore::new(dir.path());
    assert_eq!(store.load_manifest(&k).unwrap().blocks.len(), 3);

    let queries = [
        "*",
        "service:orders",
        "service:payments",
        "message:\"payment gateway\"", // phrase → exercises positions
        "status:>=500",
        "NOT service:orders",
        "message:order",
        "amount:>=15", // numeric side of the mixed-type path
        "amount:-",    // string side of the mixed-type path
    ];
    let snapshot = |eng: &BlockEngine| -> Vec<(String, u64, Vec<String>)> {
        queries
            .iter()
            .map(|q| {
                let (count, hits) = eng
                    .search(&k, Some(&node_for(q)), TimeRange::default(), 100)
                    .unwrap();
                let raws: Vec<String> = hits.iter().filter_map(|(_, h)| h.raw.clone()).collect();
                (q.to_string(), count, raws)
            })
            .collect()
    };
    let terms_snapshot = |eng: &BlockEngine| -> Vec<(String, String, u64)> {
        let (_, t) = eng
            .terms(
                &k,
                None,
                TimeRange::default(),
                &["service".to_string(), "status".to_string()],
                50,
                false,
            )
            .unwrap();
        let mut out = Vec::new();
        for (field, terms) in t {
            for (term, val) in terms {
                out.push((field.clone(), term, val.0));
            }
        }
        out.sort();
        out
    };

    let before_q = snapshot(&engine);
    let before_terms = terms_snapshot(&engine);
    let before_rows = all_rows(&store, &k);

    // Huge target + min-small=2 ⇒ all three blocks pack into one merged block.
    let stats =
        super::compact::compact_partition(&store, Codec::Zstd, &k, 1 << 30, 2, 0, usize::MAX)
            .unwrap()
            .unwrap();
    assert_eq!(stats.rows, 6);
    assert_eq!(
        store.load_manifest(&k).unwrap().blocks.len(),
        1,
        "all three blocks should pack into one"
    );

    assert_eq!(
        before_q,
        snapshot(&engine),
        "queries differ after compaction"
    );
    assert_eq!(
        before_terms,
        terms_snapshot(&engine),
        "aggregations differ after compaction"
    );
    assert_eq!(
        before_rows,
        all_rows(&store, &k),
        "decoded rows differ after compaction"
    );
}

#[test]
fn compaction_resorts_out_of_order_blocks() {
    let dir = TempDir::new().unwrap();
    let store = BlockStore::new(dir.path());
    let k = key();
    // Later timestamps appended first, earlier ones after (the late-data case a
    // column-level merge must globally re-sort, not just concatenate).
    store
        .append_block(&k, &one_row_block(300, "late-a"))
        .unwrap();
    store
        .append_block(&k, &one_row_block(400, "late-b"))
        .unwrap();
    store
        .append_block(&k, &one_row_block(100, "early-a"))
        .unwrap();
    store
        .append_block(&k, &one_row_block(200, "early-b"))
        .unwrap();

    super::compact::compact_partition(&store, Codec::None, &k, 1 << 30, 2, 0, usize::MAX)
        .unwrap()
        .unwrap();
    let blocks = store.open_blocks(&k).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(
        blocks[0].timestamps().unwrap(),
        vec![100, 200, 300, 400],
        "merged block must be globally time-sorted"
    );

    // And the engine serves the merged block newest-first.
    let engine = BlockEngine::new(dir.path(), Codec::None);
    let (_, hits) = engine.search(&k, None, TimeRange::default(), 10).unwrap();
    let msgs: Vec<_> = hits.iter().filter_map(|(_, h)| h.message.clone()).collect();
    assert_eq!(msgs, vec!["late-b", "late-a", "early-b", "early-a"]);
}

// buffered writer

#[tokio::test]
async fn buffered_writer_batches_and_flushes_on_close() {
    use super::ingest::{run_writer, BlockIngestEvent};
    use tokio::sync::mpsc;

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let (tx, rx) = mpsc::channel(1024);
    let handle = tokio::spawn(run_writer(BlockStore::new(&root), Codec::Zstd, rx));

    let k = key();
    // Three rows, well under the flush threshold — they stay buffered until the
    // channel closes, then land as a single block (one flush, not three).
    for (i, ev) in events().iter().take(3).enumerate() {
        let row = crate::indexer::ingest::json_to_row(ev, None).unwrap();
        let _ = i;
        tx.send(BlockIngestEvent {
            key: k.clone(),
            row,
        })
        .await
        .unwrap();
    }
    drop(tx); // close channel → final flush + task exit
    handle.await.unwrap();

    let store = BlockStore::new(&root);
    let blocks = store.open_blocks(&k).unwrap();
    assert_eq!(blocks.len(), 1, "buffered rows should flush as one block");
    let total: u32 = blocks.iter().map(|b| b.row_count()).sum();
    assert_eq!(total, 3);
}

// block engine via the PartitionEngine interface + public search

#[test]
fn block_engine_search_and_stats() {
    let dir = TempDir::new().unwrap();
    let k = key();
    let engine = BlockEngine::new(dir.path(), Codec::Zstd);
    engine.ingest(&k, &events(), None).unwrap();

    // partition_stats: 4 docs; missing partition ⇒ None.
    let (docs, _blocks) = engine.partition_stats(&k).unwrap().unwrap();
    assert_eq!(docs, 4);
    let absent = PartitionKey::new("default", "nope", day());
    assert!(engine.partition_stats(&absent).unwrap().is_none());

    // Representative query counts.
    for (q, want) in [
        ("*", 4u64),
        ("service:orders", 2),
        ("service:payments", 2),
        ("status:>=500", 2),
        ("message:gateway", 1),
        ("NOT service:orders", 2),
    ] {
        let (count, _) = engine
            .search(&k, Some(&node_for(q)), TimeRange::default(), 100)
            .unwrap();
        assert_eq!(count, want, "count for `{q}`");
    }

    // Numeric terms agg on status: 200×2, 500×1, 503×1.
    let (_, terms) = engine
        .terms(
            &k,
            None,
            TimeRange::default(),
            &["status".to_string()],
            10,
            false,
        )
        .unwrap();
    assert_eq!(terms["status"]["200"].0, 2);
    assert_eq!(terms["status"]["500"].0, 1);
    assert_eq!(terms["status"]["503"].0, 1);
}

#[test]
fn live_search_path_serves_from_blocks() {
    // The public cross-partition `search` entry serves from the block store —
    // exactly what the HTTP handler calls.
    let dir = TempDir::new().unwrap();
    let catalog = Catalog::open(dir.path().to_path_buf()).unwrap();
    let k = key();
    BlockEngine::new(catalog.root().to_path_buf(), Codec::Zstd)
        .ingest(&k, &events(), None)
        .unwrap();

    let fields = build_schema();
    let resp = crate::search::search(
        &catalog,
        &fields,
        "service:orders",
        Some("default"),
        None,
        None,
        None,
        0,
        10,
        &[],
        None,
    )
    .unwrap();
    assert_eq!(resp.total, 2, "block engine should serve 2 orders rows");
    assert!(resp
        .hits
        .iter()
        .all(|h| h.raw.as_deref().unwrap_or("").contains("orders")));
}

/// The footer-derived field catalog: exact coverage straight from block
/// metadata, universal-core excluded, numeric vs categorical classified.
#[test]
fn field_catalog_from_footers() {
    let dir = TempDir::new().unwrap();
    let k = key();
    let engine = BlockEngine::new(dir.path(), Codec::Zstd);
    engine.ingest(&k, &events(), None).unwrap();

    let catalog = Catalog::open(dir.path().to_path_buf()).unwrap();
    let resp = crate::search::discover::field_catalog(
        &catalog,
        "*",
        Some("default"),
        None,
        None,
        None,
        50,
        &[],
    )
    .unwrap();

    assert_eq!(resp.total_rows, 4);

    let find = |name: &str| resp.fields.iter().find(|f| f.name == name);
    let service = find("service").expect("service in catalog");
    assert!(
        (service.coverage - 1.0).abs() < 1e-9,
        "present in all 4 rows"
    );
    assert_eq!(
        service.value_kind,
        crate::search::discover::ValueKind::String
    );
    assert_eq!(service.cardinality, 2); // orders, payments
    assert!(
        service.interesting,
        "low-card categorical in every row auto-pins"
    );

    let status = find("status").expect("status in catalog");
    assert!((status.coverage - 1.0).abs() < 1e-9);
    assert_eq!(status.cardinality, 0, "numeric cardinality untracked");
    assert!(!status.interesting, "numeric isn't an auto-pin candidate");

    // Universal-core fields are rendered structurally, never in the catalog.
    assert!(find("message").is_none());
    assert!(find("timestamp").is_none());
}
