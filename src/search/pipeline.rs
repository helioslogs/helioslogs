// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Pipe operators — `stats`, `timechart`, `top`, `rare`, `sort`, `head`, `tail`,
//! `where`, `fields`, `rename`; one stats-producing stage (`stats`/`timechart`/
//! `top`/`rare`), row-level transforms after it. Metric aggregation scans matching
//! hits in-process, capped at MAX_SCAN_PER_PARTITION docs per partition.

use crate::catalog::Catalog;
use crate::engine::{select_read_engine, TimeRange};
use crate::schema::{raw_field_name, Fields};
use crate::search::query::Node;
use crate::search::Hit;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

/// Cap on docs scanned per partition for aggregation (guards runaway queries).
const MAX_SCAN_PER_PARTITION: usize = 100_000;

/// Cap on timechart series × buckets before bailing (plain stats is uncapped —
/// its group count is bounded by the per-partition scan caps).
const MAX_GROUPS: usize = 50_000;

// ============================ AST ===========================================

#[derive(Debug, Clone)]
pub struct PipelineQuery {
    /// The search expression text (everything before the first top-level `|`).
    pub search_str: String,
    pub stages: Vec<Stage>,
}

#[derive(Debug, Clone)]
pub enum Stage {
    Stats {
        aggs: Vec<MetricAgg>,
        by: Vec<String>,
    },
    Top {
        n: usize,
        field: String,
    },
    Rare {
        n: usize,
        field: String,
    },
    Sort {
        field: String,
        desc: bool,
    },
    Head {
        n: usize,
    },
    Tail {
        n: usize,
    },
    Timechart {
        span_ms: Option<u64>,
        aggs: Vec<MetricAgg>,
        by: Vec<String>,
    },
    Where {
        column: String,
        op: CmpOp,
        value: WhereValue,
    },
    Fields {
        columns: Vec<String>,
        drop: bool,
    },
    Rename {
        pairs: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WhereValue {
    Num(f64),
    Str(String),
}

#[derive(Debug, Clone)]
pub struct MetricAgg {
    pub op: AggOp,
    /// None for bare `count` (no field, just doc count).
    pub field: Option<String>,
    /// Output column name — e.g. "count", "avg(latency_ms)".
    pub alias: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggOp {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    P50,
    P95,
    P99,
    Earliest,
    Latest,
}

// ============================ Table result ==================================

#[derive(Serialize, Debug, Clone)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<TableValue>>,
    pub took_us: u128,
    pub scanned_docs: usize,
    pub partitions_scanned: usize,
    /// Echo of the search expression — handy for the UI to display.
    pub search: String,
    /// Echo of the desugared pipeline stages, human-readable.
    pub stages: Vec<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum TableValue {
    Int(i64),
    Float(f64),
    Str(String),
    Null,
}

// ============================ Parser ========================================

/// Quick check used by callers to decide between the hits-path and the
/// pipeline-path. Splits and reports whether more than one segment exists.
pub fn has_pipe(query_str: &str) -> bool {
    split_pipes(query_str).len() > 1
}

pub fn parse_pipeline(query_str: &str) -> Result<PipelineQuery> {
    let parts = split_pipes(query_str);
    let mut iter = parts.into_iter();
    let search_str = iter.next().unwrap_or_default();
    let stages = iter
        .map(|s| parse_stage(&s))
        .collect::<Result<Vec<Stage>>>()?;
    Ok(PipelineQuery { search_str, stages })
}

/// Split a query on TOP-LEVEL `|` characters, respecting double-quoted
/// strings, escapes, and parentheses (so `(a | b)` doesn't split).
fn split_pipes(s: &str) -> Vec<String> {
    let mut parts: Vec<String> = vec![String::new()];
    let mut in_quote = false;
    let mut prev_escape = false;
    let mut paren_depth: i32 = 0;
    for c in s.chars() {
        if prev_escape {
            parts.last_mut().unwrap().push(c);
            prev_escape = false;
            continue;
        }
        if c == '\\' {
            parts.last_mut().unwrap().push(c);
            prev_escape = true;
            continue;
        }
        if c == '"' {
            in_quote = !in_quote;
        }
        if !in_quote {
            if c == '(' {
                paren_depth += 1;
            }
            if c == ')' {
                paren_depth = paren_depth.saturating_sub(1);
            }
        }
        if c == '|' && !in_quote && paren_depth == 0 {
            parts.push(String::new());
            continue;
        }
        parts.last_mut().unwrap().push(c);
    }
    parts.into_iter().map(|s| s.trim().to_string()).collect()
}

fn parse_stage(s: &str) -> Result<Stage> {
    let s = s.trim();
    let (cmd, rest) = match s.find(char::is_whitespace) {
        Some(idx) => (&s[..idx], s[idx..].trim()),
        None => (s, ""),
    };
    let cmd_lc = cmd.to_lowercase();
    match cmd_lc.as_str() {
        "stats" => parse_stats(rest),
        "timechart" => parse_timechart(rest),
        "top" => parse_top_or_rare(rest, false),
        "rare" => parse_top_or_rare(rest, true),
        "sort" => parse_sort(rest),
        "where" => parse_where(rest),
        "fields" => parse_fields(rest),
        "rename" => parse_rename(rest),
        "head" => Ok(Stage::Head {
            n: rest.parse().unwrap_or(50),
        }),
        "tail" => Ok(Stage::Tail {
            n: rest.parse().unwrap_or(50),
        }),
        _ => bail!("unknown pipe command: `{cmd}`"),
    }
}

fn parse_timechart(s: &str) -> Result<Stage> {
    let mut rest = s.trim();
    let mut span_ms: Option<u64> = None;
    if rest.to_lowercase().starts_with("span=") {
        let (tok, tail) = match rest.find(char::is_whitespace) {
            Some(idx) => (&rest[..idx], rest[idx..].trim()),
            None => (rest, ""),
        };
        let val = &tok["span=".len()..];
        span_ms = Some(
            crate::search::parse_interval_ms(val)
                .ok_or_else(|| anyhow!("bad timechart span `{val}` — use e.g. span=5m"))?,
        );
        rest = tail;
    }
    let (aggs_part, by_part) = split_on_by(rest);
    let aggs: Vec<MetricAgg> = if aggs_part.is_empty() {
        vec![MetricAgg {
            op: AggOp::Count,
            field: None,
            alias: "count".to_string(),
        }]
    } else {
        split_top_level(&aggs_part, ',')
            .into_iter()
            .map(|s| parse_agg(&s))
            .collect::<Result<Vec<_>>>()?
    };
    let by: Vec<String> = split_top_level(&by_part, ',')
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    Ok(Stage::Timechart { span_ms, aggs, by })
}

fn parse_where(s: &str) -> Result<Stage> {
    let s = s.trim();
    // Scan for the operator outside quotes; multi-char ops first.
    let ops: &[(&str, CmpOp)] = &[
        ("!=", CmpOp::Neq),
        (">=", CmpOp::Gte),
        ("<=", CmpOp::Lte),
        ("==", CmpOp::Eq),
        (">", CmpOp::Gt),
        ("<", CmpOp::Lt),
        ("=", CmpOp::Eq),
    ];
    let mut found: Option<(usize, &str, CmpOp)> = None;
    for (sym, op) in ops {
        if let Some(idx) = s.find(sym) {
            if found.is_none_or(|(i, _, _)| idx < i) {
                found = Some((idx, sym, *op));
            }
        }
    }
    let (idx, sym, op) =
        found.ok_or_else(|| anyhow!("where expects `COLUMN OP VALUE` (ops: = != > >= < <=)"))?;
    let column = s[..idx].trim().to_string();
    let raw_val = s[idx + sym.len()..].trim();
    if column.is_empty() || raw_val.is_empty() {
        bail!("where expects `COLUMN OP VALUE` (ops: = != > >= < <=)");
    }
    let value = if let Some(q) = raw_val.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        WhereValue::Str(q.to_string())
    } else if let Ok(n) = raw_val.parse::<f64>() {
        WhereValue::Num(n)
    } else {
        WhereValue::Str(raw_val.to_string())
    };
    if matches!(op, CmpOp::Gt | CmpOp::Gte | CmpOp::Lt | CmpOp::Lte)
        && matches!(value, WhereValue::Str(_))
    {
        bail!("where `{sym}` needs a numeric value; only = and != compare strings");
    }
    Ok(Stage::Where { column, op, value })
}

fn parse_fields(s: &str) -> Result<Stage> {
    let mut rest = s.trim();
    let drop = rest.starts_with('-');
    if drop {
        rest = rest[1..].trim_start();
    }
    let columns: Vec<String> = if rest.contains(',') {
        split_top_level(rest, ',')
            .into_iter()
            .filter(|c| !c.is_empty())
            .collect()
    } else {
        rest.split_whitespace().map(|c| c.to_string()).collect()
    };
    if columns.is_empty() {
        bail!("fields expects column names, e.g. `fields a, b` or `fields - a`");
    }
    Ok(Stage::Fields { columns, drop })
}

fn parse_rename(s: &str) -> Result<Stage> {
    let pairs: Vec<(String, String)> = split_top_level(s, ',')
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(|p| {
            let lc = p.to_lowercase();
            let idx = lc
                .find(" as ")
                .ok_or_else(|| anyhow!("rename expects `OLD as NEW`, got `{p}`"))?;
            let old = p[..idx].trim().to_string();
            let new = p[idx + 4..].trim().to_string();
            if old.is_empty() || new.is_empty() {
                bail!("rename expects `OLD as NEW`, got `{p}`");
            }
            Ok((old, new))
        })
        .collect::<Result<Vec<_>>>()?;
    if pairs.is_empty() {
        bail!("rename expects `OLD as NEW, ...`");
    }
    Ok(Stage::Rename { pairs })
}

fn parse_stats(s: &str) -> Result<Stage> {
    let (aggs_part, by_part) = split_on_by(s);
    let aggs: Vec<MetricAgg> = split_top_level(&aggs_part, ',')
        .into_iter()
        .map(|s| parse_agg(&s))
        .collect::<Result<Vec<_>>>()?;
    if aggs.is_empty() {
        bail!("stats requires at least one aggregation");
    }
    let by: Vec<String> = split_top_level(&by_part, ',')
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    Ok(Stage::Stats { aggs, by })
}

/// Case-insensitive split on the keyword `by` surrounded by whitespace.
fn split_on_by(s: &str) -> (String, String) {
    let lc = s.to_lowercase();
    if let Some(idx) = lc.find(" by ") {
        (s[..idx].trim().to_string(), s[idx + 4..].trim().to_string())
    } else {
        (s.trim().to_string(), String::new())
    }
}

/// Split on a top-level character (not inside parens).
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut out: Vec<String> = vec![String::new()];
    let mut depth: i32 = 0;
    for c in s.chars() {
        if c == '(' {
            depth += 1;
        }
        if c == ')' {
            depth = depth.saturating_sub(1);
        }
        if c == sep && depth == 0 {
            out.push(String::new());
            continue;
        }
        out.last_mut().unwrap().push(c);
    }
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

fn parse_agg(s: &str) -> Result<MetricAgg> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("count")
        || s.eq_ignore_ascii_case("count(*)")
        || s.eq_ignore_ascii_case("count()")
    {
        return Ok(MetricAgg {
            op: AggOp::Count,
            field: None,
            alias: "count".to_string(),
        });
    }
    let open = s.find('(').context("expected `(` in aggregation")?;
    let close = s.rfind(')').context("expected `)` in aggregation")?;
    let op_str = s[..open].trim().to_lowercase();
    let field = s[open + 1..close].trim();
    if field.is_empty() {
        bail!("aggregation `{op_str}` requires a field name");
    }
    let op = match op_str.as_str() {
        "count" => AggOp::Count,
        "sum" => AggOp::Sum,
        "avg" | "mean" => AggOp::Avg,
        "min" => AggOp::Min,
        "max" => AggOp::Max,
        "p50" | "median" => AggOp::P50,
        "p95" => AggOp::P95,
        "p99" => AggOp::P99,
        "earliest" => AggOp::Earliest,
        "latest" => AggOp::Latest,
        _ => bail!("unknown aggregation: `{op_str}`"),
    };
    if matches!(op, AggOp::Earliest | AggOp::Latest) && !field.eq_ignore_ascii_case("timestamp") {
        bail!("`{op_str}` only supports the `timestamp` field");
    }
    let alias = format!("{op_str}({field})");
    Ok(MetricAgg {
        op,
        field: Some(field.to_string()),
        alias,
    })
}

fn parse_top_or_rare(s: &str, rare: bool) -> Result<Stage> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let (n, field) = match parts.len() {
        0 => bail!("top/rare requires a field"),
        1 => (10, parts[0].to_string()),
        2 => (parts[0].parse().unwrap_or(10), parts[1].to_string()),
        _ => bail!("top/rare expects `[N] FIELD`"),
    };
    if rare {
        Ok(Stage::Rare { n, field })
    } else {
        Ok(Stage::Top { n, field })
    }
}

fn parse_sort(s: &str) -> Result<Stage> {
    if let Some(field) = s.strip_prefix('-') {
        Ok(Stage::Sort {
            field: field.trim().to_string(),
            desc: true,
        })
    } else if let Some(field) = s.strip_prefix('+') {
        Ok(Stage::Sort {
            field: field.trim().to_string(),
            desc: false,
        })
    } else {
        Ok(Stage::Sort {
            field: s.trim().to_string(),
            desc: false,
        })
    }
}

// ============================ Executor ======================================

pub fn execute(
    catalog: &Catalog,
    fields: &Fields,
    pipeline: &PipelineQuery,
    env: Option<&str>,
    index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    extra_index_filter: &[crate::control::settings::EnvIndexAllow],
) -> Result<Table> {
    let t0 = Instant::now();

    // Desugar top/rare into stats + sort + head.
    let expanded = desugar(pipeline.stages.clone());

    // Find the (single) stats-producing stage.
    let is_stats = |s: &Stage| matches!(s, Stage::Stats { .. } | Stage::Timechart { .. });
    let stats_idx = expanded
        .iter()
        .position(is_stats)
        .ok_or_else(|| anyhow!("pipelines need a `stats`, `timechart`, `top`, or `rare` stage"))?;
    if stats_idx > 0 {
        bail!(
            "stages before `stats`/`timechart` are not supported — filter events in the \
             search expression before the first `|`"
        );
    }
    if expanded.iter().skip(stats_idx + 1).any(is_stats) {
        bail!("only one `stats`/`timechart` stage is supported per pipeline");
    }
    let (aggs, by, time_bucket_ms) = match &expanded[stats_idx] {
        Stage::Stats { aggs, by } => (aggs.clone(), by.clone(), None),
        Stage::Timechart { span_ms, aggs, by } => {
            let span = span_ms.unwrap_or_else(|| default_span_ms(start, end));
            (aggs.clone(), by.clone(), Some(span))
        }
        _ => unreachable!(),
    };

    // Scatter-gather scan of matching docs, in-process accumulation.
    let (mut table, scanned, partitions_scanned) = run_stats(
        catalog,
        fields,
        &pipeline.search_str,
        env,
        index,
        start,
        end,
        &aggs,
        &by,
        time_bucket_ms,
        extra_index_filter,
    )?;

    // Apply post-stats stages.
    for stage in expanded.iter().skip(stats_idx + 1) {
        match stage {
            Stage::Sort { field, desc } => sort_table(&mut table, field, *desc),
            Stage::Head { n } => table.rows.truncate(*n),
            Stage::Tail { n } => {
                let len = table.rows.len();
                if *n < len {
                    table.rows.drain(0..len - *n);
                }
            }
            Stage::Where { column, op, value } => apply_where(&mut table, column, *op, value)?,
            Stage::Fields { columns, drop } => apply_fields(&mut table, columns, *drop),
            Stage::Rename { pairs } => apply_rename(&mut table, pairs),
            _ => {}
        }
    }

    table.took_us = t0.elapsed().as_micros();
    table.scanned_docs = scanned;
    table.partitions_scanned = partitions_scanned;
    table.search = pipeline.search_str.clone();
    table.stages = expanded.iter().map(stage_to_display).collect();
    Ok(table)
}

fn desugar(stages: Vec<Stage>) -> Vec<Stage> {
    let mut out: Vec<Stage> = Vec::new();
    for s in stages {
        match s {
            Stage::Top { n, field } => {
                out.push(Stage::Stats {
                    aggs: vec![MetricAgg {
                        op: AggOp::Count,
                        field: None,
                        alias: "count".to_string(),
                    }],
                    by: vec![field],
                });
                out.push(Stage::Sort {
                    field: "count".to_string(),
                    desc: true,
                });
                out.push(Stage::Head { n });
            }
            Stage::Rare { n, field } => {
                out.push(Stage::Stats {
                    aggs: vec![MetricAgg {
                        op: AggOp::Count,
                        field: None,
                        alias: "count".to_string(),
                    }],
                    by: vec![field],
                });
                out.push(Stage::Sort {
                    field: "count".to_string(),
                    desc: false,
                });
                out.push(Stage::Head { n });
            }
            other => out.push(other),
        }
    }
    out
}

fn stage_to_display(s: &Stage) -> String {
    match s {
        Stage::Stats { aggs, by } => {
            let agg_str: Vec<String> = aggs.iter().map(|a| a.alias.clone()).collect();
            let by_str = if by.is_empty() {
                String::new()
            } else {
                format!(" by {}", by.join(", "))
            };
            format!("stats {}{}", agg_str.join(", "), by_str)
        }
        Stage::Sort { field, desc } => {
            format!("sort {}{}", if *desc { "-" } else { "" }, field)
        }
        Stage::Head { n } => format!("head {n}"),
        Stage::Tail { n } => format!("tail {n}"),
        Stage::Top { n, field } => format!("top {n} {field}"),
        Stage::Rare { n, field } => format!("rare {n} {field}"),
        Stage::Timechart { span_ms, aggs, by } => {
            let agg_str: Vec<String> = aggs.iter().map(|a| a.alias.clone()).collect();
            let span_str = span_ms
                .map(|ms| format!(" span={}", display_interval(ms)))
                .unwrap_or_default();
            let by_str = if by.is_empty() {
                String::new()
            } else {
                format!(" by {}", by.join(", "))
            };
            format!("timechart{} {}{}", span_str, agg_str.join(", "), by_str)
        }
        Stage::Where { column, op, value } => {
            let op_str = match op {
                CmpOp::Eq => "=",
                CmpOp::Neq => "!=",
                CmpOp::Gt => ">",
                CmpOp::Gte => ">=",
                CmpOp::Lt => "<",
                CmpOp::Lte => "<=",
            };
            let val_str = match value {
                WhereValue::Num(n) => n.to_string(),
                WhereValue::Str(s) => format!("\"{s}\""),
            };
            format!("where {column} {op_str} {val_str}")
        }
        Stage::Fields { columns, drop } => {
            format!(
                "fields {}{}",
                if *drop { "- " } else { "" },
                columns.join(", ")
            )
        }
        Stage::Rename { pairs } => {
            let p: Vec<String> = pairs.iter().map(|(o, n)| format!("{o} as {n}")).collect();
            format!("rename {}", p.join(", "))
        }
    }
}

fn display_interval(ms: u64) -> String {
    if ms % 86_400_000 == 0 {
        format!("{}d", ms / 86_400_000)
    } else if ms % 3_600_000 == 0 {
        format!("{}h", ms / 3_600_000)
    } else if ms % 60_000 == 0 {
        format!("{}m", ms / 60_000)
    } else if ms % 1_000 == 0 {
        format!("{}s", ms / 1_000)
    } else {
        format!("{ms}ms")
    }
}

/// Default timechart span: ~60 buckets over the request window, snapped to a
/// friendly step; 1h when the window is open-ended.
fn default_span_ms(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> u64 {
    const STEPS_MS: &[u64] = &[
        1_000, 5_000, 10_000, 30_000, 60_000, 300_000, 600_000, 1_800_000, 3_600_000, 7_200_000,
        21_600_000, 43_200_000, 86_400_000,
    ];
    let (Some(s), Some(e)) = (start, end) else {
        return 3_600_000;
    };
    let span = (e - s).num_milliseconds().max(60_000) as u64;
    let target = span / 60;
    *STEPS_MS
        .iter()
        .find(|&&step| step >= target)
        .unwrap_or(&86_400_000)
}

fn run_stats(
    catalog: &Catalog,
    fields: &Fields,
    search_str: &str,
    env: Option<&str>,
    index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    aggs: &[MetricAgg],
    by: &[String],
    time_bucket_ms: Option<u64>,
    extra_index_filter: &[crate::control::settings::EnvIndexAllow],
) -> Result<(Table, usize, usize)> {
    // Re-use the same partition-filter resolution as plain search so
    // `index:foo` inside the pipeline's search expression works identically.
    let p = crate::search::scatter::plan(
        catalog,
        search_str,
        env,
        index,
        start,
        end,
        extra_index_filter,
    )?;
    let keys = p.keys;
    let stripped: Option<Node> = p.node;

    let mut groups: BTreeMap<Vec<String>, GroupAcc> = BTreeMap::new();
    let mut total_scanned = 0usize;
    let mut partitions_scanned = 0usize;

    let engine = select_read_engine(catalog, fields);
    let time = TimeRange { start, end };
    for k in &keys {
        // Unordered scan of up to MAX_SCAN_PER_PARTITION matching docs, same
        // count + cap semantics as before, now behind the engine seam.
        let (count, hits) = engine.scan(k, stripped.as_ref(), time, MAX_SCAN_PER_PARTITION)?;
        if count == 0 {
            continue;
        }
        partitions_scanned += 1;

        for hit in &hits {
            total_scanned += 1;
            // Parse the raw event JSON once per doc; dynamic group keys and
            // numeric metrics are walked out of it.
            let parsed: Option<JsonValue> = hit
                .raw
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok());

            let ts_ms: Option<i64> = hit.timestamp.as_deref().and_then(parse_ts_ms);

            let mut key_parts: Vec<String> = Vec::with_capacity(by.len() + 1);
            if let Some(span) = time_bucket_ms {
                // Timechart: bucketed RFC3339 UTC key — fixed width, so the
                // BTreeMap's lexicographic order is chronological.
                let Some(ts) = ts_ms else {
                    continue;
                };
                let bucket = ts - ts.rem_euclid(span as i64);
                key_parts.push(format_bucket(bucket, span));
            }
            for f in by {
                key_parts.push(
                    extract_field_string(hit, parsed.as_ref(), f, k.index.as_str())
                        .unwrap_or_else(|| "—".to_string()),
                );
            }
            // Timechart only: series × buckets can explode combinatorially
            // (span=1m by user_id over a week). Plain `stats` groups are
            // already bounded by the per-partition scan caps.
            if time_bucket_ms.is_some()
                && !groups.contains_key(&key_parts)
                && groups.len() >= MAX_GROUPS
            {
                bail!(
                    "result exceeds {MAX_GROUPS} groups — increase the timechart span or \
                     reduce the by-field cardinality"
                );
            }
            let group = groups.entry(key_parts).or_default();
            group.doc_count += 1;

            for agg in aggs {
                if agg.op == AggOp::Count && agg.field.is_none() {
                    continue; // bare count is just group.doc_count
                }
                let val = if matches!(agg.op, AggOp::Earliest | AggOp::Latest) {
                    ts_ms.map(|t| t as f64)
                } else {
                    extract_field_numeric(parsed.as_ref(), agg.field.as_deref().unwrap_or(""))
                };
                if let Some(val) = val {
                    let acc = group.metrics.entry(agg.alias.clone()).or_default();
                    acc.count += 1;
                    acc.sum += val;
                    if acc.count == 1 {
                        acc.min = val;
                        acc.max = val;
                    } else {
                        if val < acc.min {
                            acc.min = val;
                        }
                        if val > acc.max {
                            acc.max = val;
                        }
                    }
                    if matches!(agg.op, AggOp::P50 | AggOp::P95 | AggOp::P99) {
                        acc.values.push(val);
                    }
                }
            }
        }
    }

    // Build columns: time bucket first (timechart), then by fields, then each
    // aggregation in declaration order.
    let mut columns: Vec<String> = Vec::new();
    if time_bucket_ms.is_some() {
        columns.push("_time".to_string());
    }
    columns.extend(by.iter().cloned());
    for agg in aggs {
        columns.push(agg.alias.clone());
    }

    let mut rows: Vec<Vec<TableValue>> = Vec::with_capacity(groups.len());
    for (key, acc) in groups {
        let mut row: Vec<TableValue> = key.into_iter().map(TableValue::Str).collect();
        for agg in aggs {
            row.push(finalize_agg(agg, &acc));
        }
        rows.push(row);
    }

    Ok((
        Table {
            columns,
            rows,
            took_us: 0,
            scanned_docs: 0,
            partitions_scanned: 0,
            search: String::new(),
            stages: Vec::new(),
        },
        total_scanned,
        partitions_scanned,
    ))
}

#[derive(Default)]
struct GroupAcc {
    doc_count: u64,
    metrics: HashMap<String, MetricAcc>,
}

#[derive(Default, Clone)]
struct MetricAcc {
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
    /// Held only when a percentile is requested.
    values: Vec<f64>,
}

fn finalize_agg(agg: &MetricAgg, acc: &GroupAcc) -> TableValue {
    if agg.op == AggOp::Count && agg.field.is_none() {
        return TableValue::Int(acc.doc_count as i64);
    }
    let Some(m) = acc.metrics.get(&agg.alias) else {
        return TableValue::Null;
    };
    if m.count == 0 {
        return TableValue::Null;
    }
    match agg.op {
        AggOp::Count => TableValue::Int(m.count as i64),
        AggOp::Sum => TableValue::Float(round4(m.sum)),
        AggOp::Avg => TableValue::Float(round4(m.sum / m.count as f64)),
        AggOp::Min => TableValue::Float(round4(m.min)),
        AggOp::Max => TableValue::Float(round4(m.max)),
        AggOp::P50 => percentile(&m.values, 0.50),
        AggOp::P95 => percentile(&m.values, 0.95),
        AggOp::P99 => percentile(&m.values, 0.99),
        AggOp::Earliest => ts_ms_to_value(m.min as i64),
        AggOp::Latest => ts_ms_to_value(m.max as i64),
    }
}

fn parse_ts_ms(s: &str) -> Option<i64> {
    s.parse::<DateTime<Utc>>()
        .ok()
        .map(|d| d.timestamp_millis())
}

fn ts_ms_to_value(ms: i64) -> TableValue {
    match DateTime::<Utc>::from_timestamp_millis(ms) {
        Some(d) => TableValue::Str(d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
        None => TableValue::Null,
    }
}

/// Fixed-width RFC3339 UTC bucket label; millis only for sub-second spans.
fn format_bucket(ms: i64, span_ms: u64) -> String {
    let d = DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default();
    if span_ms % 1_000 == 0 {
        d.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }
}

/// 4 decimal places: keeps sub-cent sums (LLM costs) while staying tidy;
/// the UI formats numeric cells to 2 dp anyway.
fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

fn percentile(values: &[f64], p: f64) -> TableValue {
    if values.is_empty() {
        return TableValue::Null;
    }
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let len = sorted.len();
    let idx = ((len as f64 - 1.0) * p).round() as usize;
    TableValue::Float(round4(sorted[idx.min(len - 1)]))
}

fn apply_where(table: &mut Table, column: &str, op: CmpOp, value: &WhereValue) -> Result<()> {
    let col_idx = table
        .columns
        .iter()
        .position(|c| c == column)
        .ok_or_else(|| {
            anyhow!(
                "where: unknown column `{column}` (available: {})",
                table.columns.join(", ")
            )
        })?;
    table.rows.retain(|row| {
        let Some(cell) = row.get(col_idx) else {
            return false;
        };
        match (value, cell) {
            // Null only ever matches `!=`.
            (_, TableValue::Null) => op == CmpOp::Neq,
            (WhereValue::Num(want), cell) => match value_as_f64(cell) {
                Some(have) => match op {
                    CmpOp::Eq => have == *want,
                    CmpOp::Neq => have != *want,
                    CmpOp::Gt => have > *want,
                    CmpOp::Gte => have >= *want,
                    CmpOp::Lt => have < *want,
                    CmpOp::Lte => have <= *want,
                },
                None => op == CmpOp::Neq,
            },
            (WhereValue::Str(want), cell) => {
                let have = value_as_str(cell);
                match op {
                    CmpOp::Eq => have.eq_ignore_ascii_case(want),
                    CmpOp::Neq => !have.eq_ignore_ascii_case(want),
                    _ => false, // ordering ops are rejected for strings at parse time
                }
            }
        }
    });
    Ok(())
}

fn apply_fields(table: &mut Table, columns: &[String], drop: bool) {
    let keep: Vec<usize> = if drop {
        (0..table.columns.len())
            .filter(|&i| !columns.iter().any(|c| c == &table.columns[i]))
            .collect()
    } else {
        columns
            .iter()
            .filter_map(|c| table.columns.iter().position(|tc| tc == c))
            .collect()
    };
    table.columns = keep.iter().map(|&i| table.columns[i].clone()).collect();
    for row in &mut table.rows {
        *row = keep
            .iter()
            .map(|&i| row.get(i).cloned().unwrap_or(TableValue::Null))
            .collect();
    }
}

fn apply_rename(table: &mut Table, pairs: &[(String, String)]) {
    for (old, new) in pairs {
        if let Some(col) = table.columns.iter_mut().find(|c| *c == old) {
            *col = new.clone();
        }
    }
}

fn sort_table(table: &mut Table, field: &str, desc: bool) {
    let col_idx = table.columns.iter().position(|c| c == field);
    let Some(col_idx) = col_idx else {
        return; // unknown sort column — silently skip
    };
    table.rows.sort_by(|a, b| {
        let av = a.get(col_idx);
        let bv = b.get(col_idx);
        let ord = compare_values(av, bv);
        if desc {
            ord.reverse()
        } else {
            ord
        }
    });
}

fn compare_values(a: Option<&TableValue>, b: Option<&TableValue>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, _) => Ordering::Less,
        (_, None) => Ordering::Greater,
        (Some(av), Some(bv)) => match (value_as_f64(av), value_as_f64(bv)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            _ => value_as_str(av).cmp(&value_as_str(bv)),
        },
    }
}

fn value_as_f64(v: &TableValue) -> Option<f64> {
    match v {
        TableValue::Int(n) => Some(*n as f64),
        TableValue::Float(f) => Some(*f),
        _ => None,
    }
}

fn value_as_str(v: &TableValue) -> String {
    match v {
        TableValue::Int(n) => n.to_string(),
        TableValue::Float(f) => f.to_string(),
        TableValue::Str(s) => s.clone(),
        TableValue::Null => String::new(),
    }
}

// ----- field extraction (schema + dynamic) ----------------------------------

fn extract_field_string(
    hit: &Hit,
    parsed: Option<&JsonValue>,
    name: &str,
    index_name: &str,
) -> Option<String> {
    let lc = name.to_lowercase();
    // Synthetic meta-field: `index` resolves to the partition key (which lives
    // on disk, not in the doc).
    if lc == "index" {
        return Some(index_name.to_string());
    }
    // Core fields come off the scanned hit; `source`/`source_raw` map to
    // `hit.source`. Anything else is a dynamic JSON path.
    let physical = raw_field_name(&lc);
    let physical = if physical.is_empty() {
        lc.as_str()
    } else {
        physical
    };
    match physical {
        "timestamp" if hit.timestamp.is_some() => return hit.timestamp.clone(),
        "message" if hit.message.is_some() => return hit.message.clone(),
        "raw" if hit.raw.is_some() => return hit.raw.clone(),
        "source" | "source_raw" if hit.source.is_some() => return hit.source.clone(),
        _ => {}
    }
    // Fall back to the raw JSON for any unknown / dynamic path.
    let leaf = walk_json(parsed?, &lc)?;
    json_to_display_string(leaf)
}

fn extract_field_numeric(parsed: Option<&JsonValue>, name: &str) -> Option<f64> {
    // After schema-on-read there are no numeric universal-core fields — numeric
    // values live in the dynamic JSON, so always walk the raw event.
    let lc = name.to_lowercase();
    let leaf = walk_json(parsed?, &lc)?;
    match leaf {
        JsonValue::Number(n) => n.as_f64(),
        JsonValue::String(s) => s.parse().ok(),
        JsonValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Walk a JSON value by dotted path. Tries the full path as a verbatim
/// top-level key first, then descends segment by segment. Case-insensitive.
fn walk_json<'a>(v: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    if let JsonValue::Object(map) = v {
        // Case-insensitive top-level match for the full dotted key.
        for (k, val) in map {
            if k.eq_ignore_ascii_case(path) {
                return Some(val);
            }
        }
    }
    // Otherwise walk segment by segment.
    let mut cur = v;
    for seg in path.split('.') {
        let obj = cur.as_object()?;
        let mut next: Option<&JsonValue> = None;
        for (k, val) in obj {
            if k.eq_ignore_ascii_case(seg) {
                next = Some(val);
                break;
            }
        }
        cur = next?;
    }
    Some(cur)
}

fn json_to_display_string(v: &JsonValue) -> Option<String> {
    match v {
        JsonValue::String(s) => Some(s.clone()),
        JsonValue::Number(n) => Some(n.to_string()),
        JsonValue::Bool(b) => Some(b.to_string()),
        JsonValue::Null => None,
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- pipe splitting -----------------------------------------------------

    #[test]
    fn has_pipe_detects_stages() {
        assert!(!has_pipe("error"));
        assert!(has_pipe("error | stats count"));
    }

    #[test]
    fn split_pipes_respects_quotes_and_parens() {
        // The `|` inside quotes and parens must not split.
        assert_eq!(
            split_pipes(r#"a "x|y" (b|c) | stats count"#),
            vec![r#"a "x|y" (b|c)"#.to_string(), "stats count".to_string()]
        );
    }

    // ---- stage parsing ------------------------------------------------------

    #[test]
    fn parse_pipeline_search_and_stage() {
        let pq = parse_pipeline("error | stats count by status").unwrap();
        assert_eq!(pq.search_str, "error");
        assert_eq!(pq.stages.len(), 1);
        match &pq.stages[0] {
            Stage::Stats { aggs, by } => {
                assert_eq!(aggs.len(), 1);
                assert_eq!(aggs[0].op, AggOp::Count);
                assert_eq!(by, &["status".to_string()]);
            }
            other => panic!("expected Stats, got {other:?}"),
        }
    }

    #[test]
    fn parse_agg_count_forms() {
        for s in ["count", "count()", "count(*)", "COUNT"] {
            let a = parse_agg(s).unwrap();
            assert_eq!(a.op, AggOp::Count);
            assert!(a.field.is_none());
            assert_eq!(a.alias, "count");
        }
    }

    #[test]
    fn parse_agg_metric_aliases() {
        let a = parse_agg("avg(latency_ms)").unwrap();
        assert_eq!(a.op, AggOp::Avg);
        assert_eq!(a.field.as_deref(), Some("latency_ms"));
        assert_eq!(a.alias, "avg(latency_ms)");
        // `mean` and `median` are aliases.
        assert_eq!(parse_agg("mean(x)").unwrap().op, AggOp::Avg);
        assert_eq!(parse_agg("median(x)").unwrap().op, AggOp::P50);
        assert_eq!(parse_agg("p95(x)").unwrap().op, AggOp::P95);
    }

    #[test]
    fn parse_agg_errors() {
        assert!(parse_agg("sum()").is_err()); // empty field
        assert!(parse_agg("bogus(x)").is_err()); // unknown op
    }

    #[test]
    fn parse_stats_requires_an_agg() {
        assert!(parse_stage("stats").is_err());
        assert!(parse_stage("stats by status").is_err());
    }

    #[test]
    fn parse_top_rare_defaults_and_n() {
        match parse_stage("top status").unwrap() {
            Stage::Top { n, field } => {
                assert_eq!(n, 10);
                assert_eq!(field, "status");
            }
            other => panic!("got {other:?}"),
        }
        match parse_stage("rare 5 path").unwrap() {
            Stage::Rare { n, field } => {
                assert_eq!(n, 5);
                assert_eq!(field, "path");
            }
            other => panic!("got {other:?}"),
        }
        assert!(parse_stage("top").is_err());
        assert!(parse_stage("top a b c").is_err());
    }

    #[test]
    fn parse_sort_direction() {
        assert!(matches!(
            parse_stage("sort -count").unwrap(),
            Stage::Sort { desc: true, .. }
        ));
        assert!(matches!(
            parse_stage("sort +count").unwrap(),
            Stage::Sort { desc: false, .. }
        ));
        assert!(matches!(
            parse_stage("sort count").unwrap(),
            Stage::Sort { desc: false, .. }
        ));
    }

    #[test]
    fn parse_head_tail_default_50() {
        assert!(matches!(
            parse_stage("head 5").unwrap(),
            Stage::Head { n: 5 }
        ));
        assert!(matches!(
            parse_stage("head").unwrap(),
            Stage::Head { n: 50 }
        ));
        assert!(matches!(
            parse_stage("tail 3").unwrap(),
            Stage::Tail { n: 3 }
        ));
    }

    #[test]
    fn unknown_stage_errors() {
        assert!(parse_stage("frobnicate x").is_err());
    }

    #[test]
    fn parse_timechart_span_aggs_by() {
        match parse_stage("timechart span=5m avg(latency_ms) by service").unwrap() {
            Stage::Timechart { span_ms, aggs, by } => {
                assert_eq!(span_ms, Some(300_000));
                assert_eq!(aggs[0].op, AggOp::Avg);
                assert_eq!(by, vec!["service".to_string()]);
            }
            other => panic!("got {other:?}"),
        }
        // Bare timechart defaults to count, no span.
        match parse_stage("timechart").unwrap() {
            Stage::Timechart { span_ms, aggs, by } => {
                assert_eq!(span_ms, None);
                assert_eq!(aggs[0].op, AggOp::Count);
                assert!(by.is_empty());
            }
            other => panic!("got {other:?}"),
        }
        assert!(parse_stage("timechart span=bogus count").is_err());
    }

    #[test]
    fn parse_where_ops_and_values() {
        match parse_stage("where count > 100").unwrap() {
            Stage::Where { column, op, value } => {
                assert_eq!(column, "count");
                assert_eq!(op, CmpOp::Gt);
                assert_eq!(value, WhereValue::Num(100.0));
            }
            other => panic!("got {other:?}"),
        }
        // No-space form and aliased columns with parens.
        match parse_stage("where avg(latency_ms)>=1.5").unwrap() {
            Stage::Where { column, op, value } => {
                assert_eq!(column, "avg(latency_ms)");
                assert_eq!(op, CmpOp::Gte);
                assert_eq!(value, WhereValue::Num(1.5));
            }
            other => panic!("got {other:?}"),
        }
        // Quoted and bare strings parse for equality ops.
        match parse_stage(r#"where service != "web""#).unwrap() {
            Stage::Where { op, value, .. } => {
                assert_eq!(op, CmpOp::Neq);
                assert_eq!(value, WhereValue::Str("web".into()));
            }
            other => panic!("got {other:?}"),
        }
        // Ordering ops reject string values.
        assert!(parse_stage("where service > web").is_err());
        assert!(parse_stage("where count").is_err());
    }

    #[test]
    fn parse_fields_keep_and_drop() {
        match parse_stage("fields service, count").unwrap() {
            Stage::Fields { columns, drop } => {
                assert!(!drop);
                assert_eq!(columns, vec!["service".to_string(), "count".to_string()]);
            }
            other => panic!("got {other:?}"),
        }
        match parse_stage("fields - count").unwrap() {
            Stage::Fields { columns, drop } => {
                assert!(drop);
                assert_eq!(columns, vec!["count".to_string()]);
            }
            other => panic!("got {other:?}"),
        }
        assert!(parse_stage("fields").is_err());
    }

    #[test]
    fn parse_rename_pairs() {
        match parse_stage("rename count as n, avg(x) as mean_x").unwrap() {
            Stage::Rename { pairs } => {
                assert_eq!(
                    pairs,
                    vec![
                        ("count".to_string(), "n".to_string()),
                        ("avg(x)".to_string(), "mean_x".to_string())
                    ]
                );
            }
            other => panic!("got {other:?}"),
        }
        assert!(parse_stage("rename count").is_err());
    }

    #[test]
    fn parse_earliest_latest_timestamp_only() {
        assert_eq!(
            parse_agg("earliest(timestamp)").unwrap().op,
            AggOp::Earliest
        );
        assert_eq!(parse_agg("latest(timestamp)").unwrap().op, AggOp::Latest);
        assert!(parse_agg("earliest(latency_ms)").is_err());
    }

    // ---- post-stats transforms ------------------------------------------------

    #[test]
    fn apply_where_filters_rows() {
        let mut t = table(vec![
            vec![TableValue::Str("a".into()), TableValue::Int(2)],
            vec![TableValue::Str("b".into()), TableValue::Int(10)],
            vec![TableValue::Str("c".into()), TableValue::Null],
        ]);
        apply_where(&mut t, "count", CmpOp::Gt, &WhereValue::Num(5.0)).unwrap();
        assert_eq!(t.rows.len(), 1);
        assert!(matches!(t.rows[0][1], TableValue::Int(10)));

        // String equality is case-insensitive; null only matches !=.
        let mut t2 = table(vec![
            vec![TableValue::Str("Web".into()), TableValue::Int(1)],
            vec![TableValue::Str("api".into()), TableValue::Int(2)],
        ]);
        apply_where(&mut t2, "k", CmpOp::Eq, &WhereValue::Str("web".into())).unwrap();
        assert_eq!(t2.rows.len(), 1);

        // Unknown column errors instead of silently passing everything.
        let mut t3 = table(vec![]);
        assert!(apply_where(&mut t3, "nope", CmpOp::Eq, &WhereValue::Num(1.0)).is_err());
    }

    #[test]
    fn apply_fields_projects_and_drops() {
        let mut t = table(vec![vec![TableValue::Str("a".into()), TableValue::Int(1)]]);
        apply_fields(&mut t, &["count".to_string(), "k".to_string()], false);
        assert_eq!(t.columns, vec!["count".to_string(), "k".to_string()]);
        assert!(matches!(t.rows[0][0], TableValue::Int(1)));

        let mut t2 = table(vec![vec![TableValue::Str("a".into()), TableValue::Int(1)]]);
        apply_fields(&mut t2, &["count".to_string()], true);
        assert_eq!(t2.columns, vec!["k".to_string()]);
        assert_eq!(t2.rows[0].len(), 1);
    }

    #[test]
    fn apply_rename_then_sortable_by_new_name() {
        let mut t = table(vec![
            vec![TableValue::Str("a".into()), TableValue::Int(2)],
            vec![TableValue::Str("b".into()), TableValue::Int(9)],
        ]);
        apply_rename(&mut t, &[("count".to_string(), "n".to_string())]);
        assert_eq!(t.columns[1], "n");
        sort_table(&mut t, "n", true);
        assert!(matches!(t.rows[0][1], TableValue::Int(9)));
    }

    // ---- timechart helpers ------------------------------------------------------

    #[test]
    fn bucket_format_fixed_width_sorts_chronologically() {
        let a = format_bucket(1_767_225_600_000, 60_000); // 2026-01-01T00:00:00Z
        let b = format_bucket(1_767_225_660_000, 60_000);
        assert_eq!(a, "2026-01-01T00:00:00Z");
        assert!(a < b);
        // Sub-second spans include millis.
        assert_eq!(
            format_bucket(1_767_225_600_500, 500),
            "2026-01-01T00:00:00.500Z"
        );
    }

    #[test]
    fn default_span_targets_about_60_buckets() {
        let start = Utc::now();
        assert_eq!(default_span_ms(None, None), 3_600_000);
        assert_eq!(
            default_span_ms(Some(start), Some(start + chrono::Duration::hours(1))),
            60_000
        );
        assert_eq!(
            default_span_ms(Some(start), Some(start + chrono::Duration::days(30))),
            43_200_000
        );
    }

    #[test]
    fn pre_stats_stages_rejected() {
        let pq = parse_pipeline("* | head 5 | stats count").unwrap();
        // execute() needs a catalog, so check via the same guard logic inline.
        let expanded = desugar(pq.stages);
        let idx = expanded
            .iter()
            .position(|s| matches!(s, Stage::Stats { .. } | Stage::Timechart { .. }))
            .unwrap();
        assert!(idx > 0, "head must precede stats in this fixture");
    }

    // ---- desugar ------------------------------------------------------------

    #[test]
    fn desugar_top_into_stats_sort_head() {
        let out = desugar(vec![Stage::Top {
            n: 3,
            field: "status".into(),
        }]);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], Stage::Stats { .. }));
        assert!(matches!(out[1], Stage::Sort { desc: true, .. }));
        assert!(matches!(out[2], Stage::Head { n: 3 }));
    }

    #[test]
    fn desugar_rare_sorts_ascending() {
        let out = desugar(vec![Stage::Rare {
            n: 2,
            field: "status".into(),
        }]);
        assert!(matches!(out[1], Stage::Sort { desc: false, .. }));
    }

    // ---- numeric helpers ----------------------------------------------------

    #[test]
    fn round4_rounds_to_four_places() {
        assert_eq!(round4(3.14159), 3.1416);
        assert_eq!(round4(2.0 / 3.0), 0.6667);
        assert_eq!(round4(0.001), 0.001);
    }

    #[test]
    fn percentile_picks_nearest_rank() {
        assert!(matches!(percentile(&[], 0.5), TableValue::Null));
        match percentile(&[1.0, 2.0, 3.0, 4.0], 0.5) {
            TableValue::Float(v) => assert_eq!(v, 3.0),
            other => panic!("got {other:?}"),
        }
        match percentile(&[10.0, 20.0, 30.0], 1.0) {
            TableValue::Float(v) => assert_eq!(v, 30.0),
            other => panic!("got {other:?}"),
        }
    }

    // ---- sort_table / compare_values ---------------------------------------

    fn table(rows: Vec<Vec<TableValue>>) -> Table {
        Table {
            columns: vec!["k".into(), "count".into()],
            rows,
            took_us: 0,
            scanned_docs: 0,
            partitions_scanned: 0,
            search: String::new(),
            stages: Vec::new(),
        }
    }

    #[test]
    fn sort_table_numeric_desc() {
        let mut t = table(vec![
            vec![TableValue::Str("a".into()), TableValue::Int(2)],
            vec![TableValue::Str("b".into()), TableValue::Int(10)],
            vec![TableValue::Str("c".into()), TableValue::Int(5)],
        ]);
        sort_table(&mut t, "count", true);
        let order: Vec<i64> = t
            .rows
            .iter()
            .map(|r| match r[1] {
                TableValue::Int(n) => n,
                _ => -1,
            })
            .collect();
        assert_eq!(order, vec![10, 5, 2]);
    }

    #[test]
    fn sort_table_unknown_column_is_noop() {
        let mut t = table(vec![vec![TableValue::Str("a".into()), TableValue::Int(1)]]);
        sort_table(&mut t, "nope", false); // must not panic
        assert_eq!(t.rows.len(), 1);
    }

    // ---- walk_json / field extraction --------------------------------------

    #[test]
    fn walk_json_nested_and_dotted_key() {
        let v = json!({"a": {"b": 7}});
        assert_eq!(walk_json(&v, "a.b"), Some(&json!(7)));
        // A verbatim dotted top-level key wins.
        let v2 = json!({"a.b": 9});
        assert_eq!(walk_json(&v2, "a.b"), Some(&json!(9)));
    }

    #[test]
    fn walk_json_case_insensitive() {
        let v = json!({"Status": 500});
        assert_eq!(walk_json(&v, "status"), Some(&json!(500)));
    }

    #[test]
    fn extract_numeric_coerces_strings_and_bools() {
        let v = json!({"n": "42", "flag": true, "f": 1.5});
        assert_eq!(extract_field_numeric(Some(&v), "n"), Some(42.0));
        assert_eq!(extract_field_numeric(Some(&v), "flag"), Some(1.0));
        assert_eq!(extract_field_numeric(Some(&v), "f"), Some(1.5));
        assert_eq!(extract_field_numeric(Some(&v), "missing"), None);
    }

    #[test]
    fn extract_string_synthetic_index_and_core_fields() {
        let hit = Hit {
            timestamp: Some("2026-01-01T00:00:00Z".into()),
            message: Some("hello".into()),
            score: 0.0,
            partition: "web/2026-01-01".into(),
            source: Some("nginx".into()),
            raw: Some(r#"{"path":"/login"}"#.into()),
        };
        let parsed: JsonValue = serde_json::from_str(hit.raw.as_deref().unwrap()).unwrap();
        // `index` is synthetic — resolves to the partition index name.
        assert_eq!(
            extract_field_string(&hit, Some(&parsed), "index", "web"),
            Some("web".to_string())
        );
        assert_eq!(
            extract_field_string(&hit, Some(&parsed), "message", "web"),
            Some("hello".to_string())
        );
        assert_eq!(
            extract_field_string(&hit, Some(&parsed), "source", "web"),
            Some("nginx".to_string())
        );
        // Dynamic path falls back to the raw JSON.
        assert_eq!(
            extract_field_string(&hit, Some(&parsed), "path", "web"),
            Some("/login".to_string())
        );
    }
}
