// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Round-trip + property tests for the block format: writes read back exactly,
//! term lookups are exact, the bloom never false-negatives, mixed-types shred.

use super::codec::*;
use super::*;

fn row(ts: i64, msg: &str, fields: Vec<(&str, FieldValue)>) -> Row {
    Row {
        ts_millis: ts,
        message: Some(msg.to_string()),
        source: Some("orders".to_string()),
        raw: Some(format!("{{\"message\":\"{msg}\"}}")),
        fields: fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    }
}

fn build(rows: Vec<Row>, codec: Codec) -> Block {
    let mut w = BlockWriter::new(codec);
    for r in rows {
        w.push(r);
    }
    Block::open(w.finish().unwrap()).unwrap()
}

// low-level codecs

#[test]
fn opt_strings_subset_matches_full_decode() {
    let vals: Vec<Option<&str>> = vec![
        Some("alpha"),
        None,
        Some("gamma"),
        Some(""),
        None,
        Some("zeta"),
    ];
    let buf = encode_opt_strings(&vals);
    let full = decode_opt_strings(&buf, vals.len()).unwrap();

    // Subsets of every shape: middle gaps, the absent rows, first+last, empty.
    for wanted in [
        vec![0u32, 2, 5],
        vec![1, 4],
        vec![0, 5],
        vec![3],
        vec![0, 1, 2, 3, 4, 5],
        vec![],
    ] {
        let got = decode_opt_strings_subset(&buf, vals.len(), &wanted).unwrap();
        let want: Vec<Option<String>> = wanted.iter().map(|&i| full[i as usize].clone()).collect();
        assert_eq!(got, want, "subset {wanted:?}");
    }
}

#[test]
fn display_rows_returns_only_requested_rows() {
    let rows = vec![
        row(1, "first", vec![]),
        row(2, "second", vec![]),
        row(3, "third", vec![]),
    ];
    let b = build(rows, Codec::Zstd);
    let got = b.display_rows(&[0, 2]).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0.as_deref(), Some("first"));
    assert_eq!(got[1].0.as_deref(), Some("third"));
}

#[test]
fn uvarint_roundtrips_edge_values() {
    for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
        let mut buf = Vec::new();
        write_uvarint(&mut buf, v);
        let mut slice = buf.as_slice();
        assert_eq!(read_uvarint(&mut slice).unwrap(), v);
        assert!(slice.is_empty());
    }
}

#[test]
fn zigzag_roundtrips_signed() {
    for v in [0i64, -1, 1, -1000, 1000, i64::MIN, i64::MAX] {
        assert_eq!(unzigzag(zigzag(v)), v);
    }
}

#[test]
fn i64_delta_roundtrips() {
    let vals = vec![100, 100, 101, 105, 105, 200, 5_000_000_000];
    let enc = encode_i64_delta(&vals);
    assert_eq!(decode_i64_delta(&enc, vals.len()).unwrap(), vals);
}

#[test]
fn opt_strings_roundtrip_with_gaps() {
    let vals = vec![Some("a"), None, Some(""), Some("héllo"), None];
    let enc = encode_opt_strings(&vals);
    let dec = decode_opt_strings(&enc, vals.len()).unwrap();
    let expect: Vec<Option<String>> = vals.iter().map(|v| v.map(|s| s.to_string())).collect();
    assert_eq!(dec, expect);
}

#[test]
fn bloom_no_false_negatives() {
    let terms: Vec<String> = (0..500).map(|i| format!("term-{i}")).collect();
    let mut b = Bloom::with_capacity(terms.len());
    for t in &terms {
        b.insert(t);
    }
    for t in &terms {
        assert!(b.contains(t), "false negative on {t}");
    }
    // Serialized form behaves identically.
    let b2 = Bloom::deserialize(&b.serialize()).unwrap();
    for t in &terms {
        assert!(b2.contains(t));
    }
}

// block round-trips

#[test]
fn timestamps_sorted_ascending_and_bounds_correct() {
    let blk = build(
        vec![
            row(300, "c", vec![]),
            row(100, "a", vec![]),
            row(200, "b", vec![]),
        ],
        Codec::Zstd,
    );
    assert_eq!(blk.row_count(), 3);
    assert_eq!(blk.timestamps().unwrap(), vec![100, 200, 300]);
    assert_eq!(blk.min_ts(), 100);
    assert_eq!(blk.max_ts(), 300);
    // Stored columns follow the same sorted order.
    let msgs: Vec<String> = blk
        .messages()
        .unwrap()
        .into_iter()
        .map(|m| m.unwrap())
        .collect();
    assert_eq!(msgs, vec!["a", "b", "c"]);
}

#[test]
fn empty_block_is_valid() {
    let blk = build(vec![], Codec::None);
    assert_eq!(blk.row_count(), 0);
    assert!(blk.timestamps().unwrap().is_empty());
    assert!(blk.term_postings("message", "x").unwrap().is_none());
}

#[test]
fn term_postings_tokenizes_message() {
    let blk = build(
        vec![
            row(1, "payment-gateway timeout", vec![]),
            row(2, "db connection ok", vec![]),
        ],
        Codec::Zstd,
    );
    // Minor-token match: "gateway" should hit the row with "payment-gateway".
    let bm = blk.term_postings("message", "gateway").unwrap().unwrap();
    assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0]);
    // Major token too.
    let bm = blk
        .term_postings("message", "payment-gateway")
        .unwrap()
        .unwrap();
    assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0]);
    // Absent term.
    assert!(blk.term_postings("message", "nope").unwrap().is_none());
}

#[test]
fn source_is_exact_lowercased_term() {
    let blk = build(vec![row(1, "x", vec![])], Codec::None);
    assert!(blk.term_postings("source", "orders").unwrap().is_some());
    // Not tokenized — a substring isn't a term.
    assert!(blk.term_postings("source", "order").unwrap().is_none());
}

#[test]
fn bare_terms_are_exact_tokens_not_substrings() {
    let blk = build(
        vec![
            row(1, "request error", vec![]),
            row(2, "many errors here", vec![]),
            row(3, "status 500 ok", vec![]),
            row(4, "size 1500 bytes", vec![]),
        ],
        Codec::Zstd,
    );
    let matches = |q: &str| -> Vec<u32> {
        let f = crate::search::query::parse(q).unwrap();
        super::query::eval(&blk, f.as_ref(), None, None)
            .unwrap()
            .iter()
            .collect()
    };
    // Bare `error` is the token `error`, not the substring of `errors`.
    assert_eq!(matches("error"), vec![0]);
    // A trailing wildcard restores the prefix/substring scan.
    assert_eq!(matches("error*"), vec![0, 1]);
    // Numeric token `500` doesn't match `1500`.
    assert_eq!(matches("500"), vec![2]);
}

#[test]
fn numeric_value_column_roundtrips_with_bounds() {
    let blk = build(
        vec![
            row(1, "x", vec![("amount", FieldValue::F64(10.5))]),
            row(2, "y", vec![("amount", FieldValue::F64(-3.0))]),
            row(3, "z", vec![("amount", FieldValue::F64(99.0))]),
        ],
        Codec::Zstd,
    );
    let col = blk.value_column("amount", LogType::F64).unwrap().unwrap();
    assert_eq!(col.present.len(), 3);
    assert_eq!(col.min, -3.0);
    assert_eq!(col.max, 99.0);
    match col.values {
        super::reader::ColumnValues::F64(v) => assert_eq!(v, vec![10.5, -3.0, 99.0]),
        _ => panic!("wrong column type"),
    }
}

#[test]
fn mixed_type_path_shreds_into_parallel_columns() {
    // "amount" is numeric on two rows and the string "-" on one — the dirty-log
    // case. It must shred: an F64 value column + a str term index, disjoint rows.
    let blk = build(
        vec![
            row(1, "a", vec![("amount", FieldValue::F64(10.0))]),
            row(2, "b", vec![("amount", FieldValue::Str("-".to_string()))]),
            row(3, "c", vec![("amount", FieldValue::F64(20.0))]),
        ],
        Codec::Zstd,
    );
    // Numeric side: only rows 0 and 2.
    let col = blk.value_column("amount", LogType::F64).unwrap().unwrap();
    assert_eq!(col.present.iter().collect::<Vec<_>>(), vec![0, 2]);
    // String side: only row 1, via the term index.
    let bm = blk.term_postings("amount", "-").unwrap().unwrap();
    assert_eq!(bm.iter().collect::<Vec<_>>(), vec![1]);
    // Range pruning bounds ignore the "-" row entirely.
    assert_eq!(col.min, 10.0);
    assert_eq!(col.max, 20.0);
    // Discovery reports both shred types under one logical name.
    let mut paths = blk.dynamic_paths();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            ("amount".to_string(), LogType::F64),
            ("amount".to_string(), LogType::Str),
        ]
    );
}

#[test]
fn bool_column_roundtrips() {
    let blk = build(
        vec![
            row(1, "a", vec![("ok", FieldValue::Bool(true))]),
            row(2, "b", vec![("ok", FieldValue::Bool(false))]),
            row(3, "c", vec![("ok", FieldValue::Bool(true))]),
        ],
        Codec::None,
    );
    let col = blk.value_column("ok", LogType::Bool).unwrap().unwrap();
    assert_eq!(col.present.iter().collect::<Vec<_>>(), vec![0, 1, 2]);
    match col.values {
        super::reader::ColumnValues::Bool(truth) => {
            assert_eq!(truth.iter().collect::<Vec<_>>(), vec![0, 2]);
        }
        _ => panic!("wrong column type"),
    }
}

#[test]
fn field_postings_gives_terms_agg_input() {
    let blk = build(
        vec![
            row(1, "x", vec![("svc", FieldValue::Str("api".to_string()))]),
            row(2, "y", vec![("svc", FieldValue::Str("api".to_string()))]),
            row(3, "z", vec![("svc", FieldValue::Str("web".to_string()))]),
        ],
        Codec::Zstd,
    );
    let mut counts: Vec<(String, u64)> = blk
        .field_postings("svc")
        .unwrap()
        .into_iter()
        .map(|(t, bm)| (t, bm.len()))
        .collect();
    counts.sort();
    assert_eq!(counts, vec![("api".to_string(), 2), ("web".to_string(), 1)]);
}

#[test]
fn compression_on_off_are_value_equivalent() {
    let rows = || {
        vec![
            row(
                1,
                "payment-gateway up",
                vec![("amount", FieldValue::I64(42))],
            ),
            row(2, "db down", vec![("amount", FieldValue::I64(7))]),
        ]
    };
    let on = build(rows(), Codec::Zstd);
    let off = build(rows(), Codec::None);

    assert_eq!(on.timestamps().unwrap(), off.timestamps().unwrap());
    assert_eq!(on.messages().unwrap(), off.messages().unwrap());
    assert_eq!(
        on.term_postings("message", "gateway").unwrap(),
        off.term_postings("message", "gateway").unwrap()
    );
    let ca = on.value_column("amount", LogType::I64).unwrap().unwrap();
    let cb = off.value_column("amount", LogType::I64).unwrap().unwrap();
    assert_eq!(ca.present, cb.present);
    assert_eq!((ca.min, ca.max), (cb.min, cb.max));
}

#[test]
fn time_overlap_pruning() {
    let blk = build(
        vec![row(1000, "a", vec![]), row(2000, "b", vec![])],
        Codec::None,
    );
    assert!(blk.overlaps(Some(1500), Some(3000)));
    assert!(blk.overlaps(None, None));
    assert!(!blk.overlaps(Some(3000), None)); // all rows older than window start
    assert!(!blk.overlaps(None, Some(500))); // all rows newer than window end
}

#[test]
fn corrupt_magic_is_rejected() {
    let mut w = BlockWriter::new(Codec::None);
    w.push(row(1, "x", vec![]));
    let mut bytes = w.finish().unwrap();
    bytes[0] = b'X';
    assert!(Block::open(bytes).is_err());
}

// codec gaps (plain i64, f64, dict, decompress guard)

#[test]
fn i64_plain_roundtrips() {
    let vals = vec![0i64, -1, 1, i64::MIN, i64::MAX, 12_345, -9_999];
    let enc = encode_i64_plain(&vals);
    assert_eq!(decode_i64_plain(&enc, vals.len()).unwrap(), vals);
}

#[test]
fn f64_roundtrips() {
    let vals = vec![
        0.0f64,
        -3.5,
        1e20,
        -1e-20,
        99.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    let enc = encode_f64(&vals);
    assert_eq!(decode_f64(&enc, vals.len()).unwrap(), vals);
}

#[test]
fn dict_roundtrips_with_unicode() {
    let terms = vec!["a".to_string(), "bb".to_string(), "héllo".to_string()];
    let enc = encode_dict(&terms);
    assert_eq!(decode_dict(&enc).unwrap(), terms);
}

#[test]
fn decompress_detects_length_mismatch() {
    let raw = b"the quick brown fox jumps";
    let comp = compress(Codec::Zstd, raw).unwrap();
    // A wrong raw_len footer must error, not silently return truncated/garbage.
    assert!(decompress(Codec::Zstd, &comp, raw.len() as u32 + 5).is_err());
    // The honest length still round-trips.
    assert_eq!(
        decompress(Codec::Zstd, &comp, raw.len() as u32).unwrap(),
        raw
    );
}

// compaction-critical roundtrips (rows / positions / str values)

/// Sort a row's fields into a canonical order so `rows()` output (column order)
/// compares equal to hand-written input regardless of field ordering.
fn norm_fields(mut r: Row) -> Row {
    r.fields
        .sort_by(|a, b| (&a.0, format!("{:?}", a.1)).cmp(&(&b.0, format!("{:?}", b.1))));
    r
}

#[test]
fn rows_roundtrip_reconstructs_input() {
    // The exact input compaction decodes: heterogeneous field types, optional
    // message/source/raw, and out-of-order timestamps.
    let input = vec![
        Row {
            ts_millis: 300,
            message: Some("gamma".into()),
            source: Some("svc-a".into()),
            raw: Some("r3".into()),
            fields: vec![
                ("name".into(), FieldValue::Str("API".into())),
                ("n".into(), FieldValue::I64(7)),
                ("f".into(), FieldValue::F64(1.5)),
                ("ok".into(), FieldValue::Bool(true)),
            ],
        },
        Row {
            ts_millis: 100,
            message: Some("alpha".into()),
            source: None,
            raw: Some("r1".into()),
            fields: vec![
                ("n".into(), FieldValue::I64(-3)),
                ("ok".into(), FieldValue::Bool(false)),
            ],
        },
        Row {
            ts_millis: 200,
            message: None,
            source: Some("svc-b".into()),
            raw: None,
            fields: vec![("name".into(), FieldValue::Str("web".into()))],
        },
    ];
    let blk = build(input.clone(), Codec::Zstd);
    let got: Vec<Row> = blk.rows().unwrap().into_iter().map(norm_fields).collect();

    let mut want: Vec<Row> = input.into_iter().map(norm_fields).collect();
    want.sort_by_key(|r| r.ts_millis); // rows() returns ascending-ts order
    assert_eq!(got, want);
}

#[test]
fn message_positions_roundtrip_including_repeats() {
    // Repeated tokens in one row must keep all their positions, in order — the
    // case the per-term posting accumulator has to group correctly.
    let blk = build(
        vec![
            row(1, "alpha beta alpha", vec![]),
            row(2, "beta gamma", vec![]),
        ],
        Codec::Zstd,
    );
    let fi = blk.load_field_index("message").unwrap().unwrap();
    let idx = |t: &str| fi.terms.iter().position(|x| x == t).unwrap();
    assert_eq!(fi.positions_for(idx("alpha"), 0), &[0u32, 2]);
    assert_eq!(fi.positions_for(idx("beta"), 0), &[1u32]);
    assert_eq!(fi.positions_for(idx("beta"), 1), &[0u32]);
    assert_eq!(fi.positions_for(idx("gamma"), 1), &[1u32]);
    // A term absent from a row yields no positions.
    assert_eq!(fi.positions_for(idx("gamma"), 0), &[] as &[u32]);
}

#[test]
fn str_value_column_preserves_case_with_gaps() {
    let blk = build(
        vec![
            row(1, "a", vec![("name", FieldValue::Str("API".into()))]),
            row(2, "b", vec![]),
            row(3, "c", vec![("name", FieldValue::Str("Web".into()))]),
        ],
        Codec::Zstd,
    );
    // Value column keeps original case, one slot per row with None gaps.
    let col = blk.str_column("name").unwrap().unwrap();
    assert_eq!(
        col,
        vec![Some("API".to_string()), None, Some("Web".to_string())]
    );
    // The term index for the same path is lowercased + searchable.
    assert!(blk.term_postings("name", "api").unwrap().is_some());
    assert!(blk.term_postings("name", "API").unwrap().is_none());
}

#[test]
fn large_block_many_rows_roundtrips() {
    let mut rows = Vec::new();
    for i in 0..5000 {
        rows.push(row(
            i as i64,
            &format!("event number {i} service-{}", i % 7),
            vec![("latency_ms", FieldValue::I64((i % 500) as i64))],
        ));
    }
    let blk = build(rows, Codec::Zstd);
    assert_eq!(blk.row_count(), 5000);
    let ts = blk.timestamps().unwrap();
    assert_eq!(ts.first(), Some(&0));
    assert_eq!(ts.last(), Some(&4999));
    // "service-3" appears on rows where i % 7 == 3.
    let bm = blk.term_postings("message", "service-3").unwrap().unwrap();
    assert_eq!(bm.len(), (0..5000).filter(|i| i % 7 == 3).count() as u64);
}

#[test]
fn footer_field_stats_match_columns() {
    // 4 rows: `level` (str, all, 2 distinct), `code` (i64, 3), `flag` (bool, 1).
    // Footer-derived stats must report exact rows + cardinality with no body reads.
    let rows = vec![
        row(
            1,
            "a",
            vec![
                ("level", FieldValue::Str("info".into())),
                ("code", FieldValue::I64(200)),
            ],
        ),
        row(
            2,
            "b",
            vec![
                ("level", FieldValue::Str("error".into())),
                ("code", FieldValue::I64(500)),
            ],
        ),
        row(
            3,
            "c",
            vec![
                ("level", FieldValue::Str("info".into())),
                ("code", FieldValue::I64(200)),
            ],
        ),
        row(
            4,
            "d",
            vec![
                ("level", FieldValue::Str("info".into())),
                ("flag", FieldValue::Bool(true)),
            ],
        ),
    ];
    let blk = build(rows, Codec::Zstd);
    assert_eq!(blk.footer().format_version, FORMAT_VERSION);

    // Footer-only fast path (no section reads) and the full path must agree.
    let from_footer = footer_field_stats(blk.footer()).expect("v2 footer");
    let from_block = blk.field_stats().unwrap();
    assert_eq!(from_footer.row_count, 4);
    assert_eq!(from_block.row_count, 4);

    let find = |s: &BlockFieldStats, name: &str| {
        s.fields.iter().find(|f| f.path == name).cloned().unwrap()
    };
    let level = find(&from_footer, "level");
    assert_eq!(level.kind, FieldKind::Str);
    assert_eq!(level.rows, 4);
    assert_eq!(level.cardinality, 2); // info, error
    let code = find(&from_footer, "code");
    assert_eq!(code.kind, FieldKind::Int);
    assert_eq!(code.rows, 3);
    let flag = find(&from_footer, "flag");
    assert_eq!(flag.kind, FieldKind::Bool);
    assert_eq!(flag.rows, 1);
}
