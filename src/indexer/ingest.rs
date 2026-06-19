// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! JSON-event → block-engine [`Row`](crate::engine::block::Row) conversion.
//! Schema-on-read: only `timestamp`/`message`/`source` get structural treatment
//! (the first two via small alias lists, as they affect storage layout); the rest shred by type.

use anyhow::{anyhow, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use std::collections::HashSet;

use crate::catalog::day_for;

// --- field-alias tables (universal-core only) --------------------------------

const TIMESTAMP_KEYS: &[&str] = &[
    "timestamp",
    "ts",         // zap, GCP
    "time",       // logrus, pino, bunyan, Docker
    "@timestamp", // log4j2 JsonLayout, logback logstash-encoder, ECS
    "Timestamp",
    "eventTime",
    "datetime",
    "@t",      // Serilog CLEF (.NET)
    "asctime", // python-json-logger
];
/// Log-line aliases, kept because `message` has a phrase-position tokenizer.
const MESSAGE_KEYS: &[&str] = &[
    "message", "msg", "Body", "body", "event", "Msg", "log.body", "@m", "@mt",
];
/// `source` is the per-event sub-partition tag — distinct from the partition
/// `index`. Aliased so users can land their existing field name without renaming.
const SOURCE_KEYS: &[&str] = &["source", "log.source", "_source"];

/// JSON event → block-engine [`Row`](crate::engine::block::Row): universal-core
/// extraction plus type-shredding of the rest into dynamic columns (see [`shred_value`]).
pub fn json_to_row(v: &Value, default_source: Option<&str>) -> Result<crate::engine::block::Row> {
    use crate::engine::block::Row;

    let raw = v.as_object().ok_or_else(|| anyhow!("not a JSON object"))?;
    let mut flat = raw.clone();
    for container in ["Attributes", "Resource", "attributes", "resource"] {
        if let Some(Value::Object(inner)) = raw.get(container) {
            flat.remove(container);
            for (k, v) in inner {
                flat.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }
    let obj = &flat;
    let mut consumed: HashSet<&str> = HashSet::new();

    let mut ts_millis = 0i64;
    if let Some((key, val)) = pick(obj, TIMESTAMP_KEYS) {
        if let Some(dt) = parse_timestamp(val) {
            ts_millis = dt.timestamp_millis();
            consumed.insert(key);
        }
    }

    let mut source = None;
    if let Some((key, val)) = pick_str(obj, SOURCE_KEYS) {
        source = Some(val.to_string());
        consumed.insert(key);
    } else if let Some(default) = default_source {
        if !default.is_empty() {
            source = Some(default.to_string());
        }
    }

    let mut message = None;
    if let Some((key, val)) = pick_str(obj, MESSAGE_KEYS) {
        if !val.is_empty() {
            message = Some(val.to_string());
            consumed.insert(key);
        }
    }

    let mut fields = Vec::new();
    for (k, val) in obj {
        if consumed.contains(k.as_str()) {
            continue;
        }
        shred_value(&k.to_lowercase(), val, &mut fields, 0);
    }

    Ok(Row {
        ts_millis,
        message,
        source,
        raw: Some(serde_json::to_string(v).unwrap_or_default()),
        fields,
    })
}

/// Max nesting depth shredded; deeper sub-objects stored as one stringified field.
const MAX_FLATTEN_DEPTH: usize = 16;

/// Max array elements shredded per array; the rest stay only in `raw`.
const MAX_ARRAY_ELEMS: usize = 1000;

/// Shred a JSON value into typed `(path, FieldValue)` leaves: objects recurse to
/// dotted paths, arrays shred each element under the same path (multi-valued).
fn shred_value(
    path: &str,
    val: &Value,
    fields: &mut Vec<(String, crate::engine::block::FieldValue)>,
    depth: usize,
) {
    use crate::engine::block::FieldValue;
    match val {
        Value::Null => {}
        Value::Bool(b) => fields.push((path.to_string(), FieldValue::Bool(*b))),
        Value::Number(n) => fields.push((path.to_string(), number_to_value(n))),
        Value::String(s) => fields.push((path.to_string(), FieldValue::Str(s.clone()))),
        Value::Object(map) if depth < MAX_FLATTEN_DEPTH => {
            for (k, v) in map {
                let child = format!("{path}.{}", k.to_lowercase());
                shred_value(&child, v, fields, depth + 1);
            }
        }
        Value::Array(arr) if depth < MAX_FLATTEN_DEPTH => {
            for elem in arr.iter().take(MAX_ARRAY_ELEMS) {
                shred_value(path, elem, fields, depth + 1);
            }
        }
        // Containers past the depth cap: keep as searchable text.
        other => fields.push((path.to_string(), FieldValue::Str(other.to_string()))),
    }
}

/// JSON number → typed `FieldValue`, keeping i64/f64 distinct (u64 > i64::MAX → f64).
fn number_to_value(n: &serde_json::Number) -> crate::engine::block::FieldValue {
    use crate::engine::block::FieldValue;
    if let Some(i) = n.as_i64() {
        FieldValue::I64(i)
    } else if let Some(u) = n.as_u64() {
        FieldValue::F64(u as f64)
    } else {
        FieldValue::F64(n.as_f64().unwrap_or(0.0))
    }
}

/// Resolve `{{ field }}` placeholders in an index template against one event
/// (sanitized; missing → `unknown`). Fields may be dotted paths into nested objects.
pub fn resolve_index_template(template: &str, event: &Value) -> String {
    if !template.contains("{{") {
        return template.to_string();
    }
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = after[..end].trim();
                let part = lookup_path(event, key)
                    .map(value_to_index_part)
                    .unwrap_or_else(|| "unknown".to_string());
                out.push_str(&part);
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("{{"); // unterminated — keep literal, stop scanning
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Look up a dotted path in the pre-flatten event JSON, trying the whole
/// remaining path as a literal key at each level before descending one segment.
fn lookup_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    if let Some(v) = value.get(path) {
        return Some(v);
    }
    let (head, tail) = path.split_once('.')?;
    lookup_path(value.get(head)?, tail)
}

fn value_to_index_part(v: &Value) -> String {
    let raw = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return "unknown".to_string(),
    };
    // Index names allow `[a-z0-9_-]`; fold everything else to `-`.
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.chars().take(64).collect()
    }
}

/// Extracts a UTC date from an event's timestamp field. Returns None if the
/// timestamp is missing or unparseable; caller decides the fallback.
pub fn event_day(v: &Value) -> Option<NaiveDate> {
    let raw = v.as_object()?;
    let mut flat = raw.clone();
    for container in ["Attributes", "Resource", "attributes", "resource"] {
        if let Some(Value::Object(inner)) = raw.get(container) {
            flat.remove(container);
            for (k, v) in inner {
                flat.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }
    let (_, val) = pick(&flat, TIMESTAMP_KEYS)?;
    Some(day_for(parse_timestamp(val)?))
}

// --- helpers -----------------------------------------------------------------

fn pick<'a>(
    obj: &'a serde_json::Map<String, Value>,
    keys: &[&'static str],
) -> Option<(&'static str, &'a Value)> {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            return Some((k, v));
        }
    }
    None
}

fn pick_str<'a>(
    obj: &'a serde_json::Map<String, Value>,
    keys: &[&'static str],
) -> Option<(&'static str, &'a str)> {
    for k in keys {
        if let Some(s) = obj.get(*k).and_then(Value::as_str) {
            return Some((k, s));
        }
    }
    None
}

fn parse_timestamp(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        Value::String(s) => parse_timestamp_str(s),
        Value::Number(n) => {
            let f = n.as_f64()?;
            Some(unix_number_to_datetime(f))
        }
        _ => None,
    }
}

/// RFC3339 first, then common zone-less logger layouts (assumed UTC). A parse here
/// only ever upgrades the "no timestamp → today" fallback, so it can't misroute.
fn parse_timestamp_str(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Some(dt);
    }
    // Offset-bearing, non-RFC3339: the Apache/nginx Common Log Format date,
    // e.g. `10/Oct/2024:13:55:36 +0000` (grok `HTTPDATE`).
    if let Ok(dt) = DateTime::parse_from_str(s, "%d/%b/%Y:%H:%M:%S %z") {
        return Some(dt.with_timezone(&Utc));
    }
    const ZONELESS: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S%.f",  // ISO without an offset
        "%Y-%m-%d %H:%M:%S%.f",  // space separator (optional fraction)
        "%Y-%m-%d %H:%M:%S,%3f", // comma millis — python-json-logger / log4j default
    ];
    for fmt in ZONELESS {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ndt.and_utc());
        }
    }
    None
}

fn unix_number_to_datetime(n: f64) -> DateTime<Utc> {
    let abs = n.abs();
    if abs < 1e10 {
        let secs = n.trunc() as i64;
        let nanos = ((n.fract().abs()) * 1e9) as u32;
        DateTime::from_timestamp(secs, nanos).unwrap_or_else(Utc::now)
    } else if abs < 1e13 {
        DateTime::from_timestamp_millis(n as i64).unwrap_or_else(Utc::now)
    } else if abs < 1e16 {
        DateTime::from_timestamp_micros(n as i64).unwrap_or_else(Utc::now)
    } else {
        let ns = n as i64;
        let secs = ns / 1_000_000_000;
        let nanos = (ns % 1_000_000_000) as u32;
        DateTime::from_timestamp(secs, nanos).unwrap_or_else(Utc::now)
    }
}

#[cfg(test)]
mod tests {
    //! Coverage for the popular app-logger shapes. The JSON loggers below all
    //! flow through `json_to_row`; the text/logfmt ones (logrus-text, log4j
    //! `PatternLayout`) go through [`crate::indexer::parse`] and are tested there.

    use super::*;
    use crate::engine::block::FieldValue;
    use serde_json::json;

    fn row(v: Value) -> crate::engine::block::Row {
        json_to_row(&v, None).unwrap()
    }

    fn field<'a>(r: &'a crate::engine::block::Row, key: &str) -> Option<&'a FieldValue> {
        r.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// All values shredded under `key` (in row order) — for multi-valued fields.
    fn fields_for<'a>(r: &'a crate::engine::block::Row, key: &str) -> Vec<&'a FieldValue> {
        r.fields
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v)
            .collect()
    }

    /// timestamp parsed (not the epoch fallback) + message extracted.
    fn assert_core(r: &crate::engine::block::Row, msg: &str) {
        assert!(
            r.ts_millis > 0,
            "timestamp should parse, got {}",
            r.ts_millis
        );
        assert_eq!(r.message.as_deref(), Some(msg));
    }

    #[test]
    fn zap_production_json() {
        // go.uber.org/zap NewProduction: float-seconds `ts`, `msg`.
        let r = row(json!({
            "level":"error","ts":1609459200.123,"caller":"svc/x.go:42",
            "msg":"db timeout","stacktrace":"goroutine..."
        }));
        assert_core(&r, "db timeout");
        assert_eq!(field(&r, "level"), Some(&FieldValue::Str("error".into())));
        assert_eq!(
            field(&r, "caller"),
            Some(&FieldValue::Str("svc/x.go:42".into()))
        );
    }

    #[test]
    fn logrus_json() {
        let r = row(json!({
            "level":"info","msg":"started","time":"2021-01-01T00:00:00Z","app":"api"
        }));
        assert_core(&r, "started");
        assert_eq!(field(&r, "app"), Some(&FieldValue::Str("api".into())));
    }

    #[test]
    fn pino_numeric_level_and_ms_time() {
        // pino: numeric level, ms-epoch `time`, `msg`.
        let r = row(json!({
            "level":30,"time":1609459200000i64,"pid":1,"hostname":"h","msg":"request done"
        }));
        assert_core(&r, "request done");
        assert_eq!(field(&r, "level"), Some(&FieldValue::I64(30)));
    }

    #[test]
    fn bunyan_json() {
        let r = row(json!({
            "name":"app","hostname":"h","pid":1,"level":30,
            "msg":"hi","time":"2021-01-01T00:00:00.000Z","v":0
        }));
        assert_core(&r, "hi");
    }

    #[test]
    fn winston_default_space_timestamp() {
        // winston format.timestamp() emits "YYYY-MM-DD HH:mm:ss" (no T, no zone).
        let r = row(json!({
            "level":"info","message":"served","timestamp":"2021-01-01 00:00:00"
        }));
        assert_core(&r, "served");
    }

    #[test]
    fn log4j2_json_layout() {
        let r = row(json!({
            "@timestamp":"2021-01-01T00:00:00.000Z","level":"ERROR",
            "loggerName":"com.acme.Svc","thread":"main","message":"boom"
        }));
        assert_core(&r, "boom");
        assert_eq!(
            field(&r, "loggername"),
            Some(&FieldValue::Str("com.acme.Svc".into()))
        );
    }

    #[test]
    fn logback_logstash_encoder() {
        let r = row(json!({
            "@timestamp":"2021-01-01T00:00:00.000+00:00","level":"WARN",
            "logger_name":"com.acme.Svc","thread_name":"main","message":"slow","level_value":30000
        }));
        assert_core(&r, "slow");
        assert_eq!(field(&r, "level_value"), Some(&FieldValue::I64(30000)));
    }

    #[test]
    fn serilog_clef_dotnet() {
        // Compact Log Event Format: `@t` time, `@m` rendered message, `@mt` template.
        let r = row(json!({
            "@t":"2021-01-01T00:00:00.0000000Z","@l":"Error",
            "@m":"User bob logged in","@mt":"User {User} logged in","User":"bob"
        }));
        assert_core(&r, "User bob logged in");
        assert_eq!(field(&r, "user"), Some(&FieldValue::Str("bob".into())));
    }

    #[test]
    fn python_json_logger_asctime() {
        // python-json-logger default: `asctime` with comma millis, `message`.
        let r = row(json!({
            "asctime":"2024-01-15 10:30:45,123","levelname":"ERROR",
            "name":"app","message":"db down"
        }));
        assert_core(&r, "db down");
        assert_eq!(
            field(&r, "levelname"),
            Some(&FieldValue::Str("ERROR".into()))
        );
    }

    #[test]
    fn gcp_structured_logging() {
        let r = row(json!({
            "severity":"ERROR","timestamp":"2021-01-01T00:00:00Z",
            "message":"handler failed","logging.googleapis.com/trace":"projects/p/traces/t"
        }));
        assert_core(&r, "handler failed");
        assert_eq!(
            field(&r, "severity"),
            Some(&FieldValue::Str("ERROR".into()))
        );
    }

    #[test]
    fn apache_clf_timestamp_parses() {
        // grok HTTPDATE capture flows in as a `timestamp` string.
        let r = row(json!({"timestamp":"10/Oct/2024:13:55:36 +0000","message":"GET /"}));
        assert!(r.ts_millis > 0);
        assert_eq!(r.message.as_deref(), Some("GET /"));
    }

    #[test]
    fn index_template_resolves_and_sanitizes() {
        let ev = json!({"service":"Checkout API","level":"error"});
        assert_eq!(
            resolve_index_template("app-{{ service }}", &ev),
            "app-checkout-api"
        );
        // literal passes through untouched (fast path)
        assert_eq!(resolve_index_template("static", &ev), "static");
        // missing field → "unknown"
        assert_eq!(resolve_index_template("x-{{ nope }}", &ev), "x-unknown");
        // numbers stringify
        let n = json!({"shard": 3});
        assert_eq!(resolve_index_template("s{{shard}}", &n), "s3");
    }

    #[test]
    fn index_template_resolves_dotted_paths() {
        // k8s shape from the Fluent Bit `kubernetes` filter: nested metadata.
        let ev = json!({
            "kubernetes": {
                "namespace_name": "payments",
                "labels": { "app": "checkout", "app.kubernetes.io/name": "checkout-web" }
            }
        });
        assert_eq!(
            resolve_index_template("logs-{{kubernetes.namespace_name}}", &ev),
            "logs-payments"
        );
        assert_eq!(
            resolve_index_template("logs-{{kubernetes.labels.app}}", &ev),
            "logs-checkout"
        );
        // A dotted *key* at the leaf still resolves (literal-key-first per level).
        assert_eq!(
            resolve_index_template("logs-{{kubernetes.labels.app.kubernetes.io/name}}", &ev),
            "logs-checkout-web"
        );
        // Missing nested path → "unknown".
        assert_eq!(
            resolve_index_template("logs-{{kubernetes.pod_name}}", &ev),
            "logs-unknown"
        );
    }

    #[test]
    fn otel_attributes_are_flattened() {
        // OTel/ECS shape: nested Attributes/Resource flattened to top level.
        let r = row(json!({
            "Timestamp":"2021-01-01T00:00:00Z","Body":"span event",
            "Attributes":{"http.method":"GET"},"Resource":{"service.name":"checkout"}
        }));
        assert_core(&r, "span event");
        assert_eq!(
            field(&r, "http.method"),
            Some(&FieldValue::Str("GET".into()))
        );
        assert_eq!(
            field(&r, "service.name"),
            Some(&FieldValue::Str("checkout".into()))
        );
    }

    #[test]
    fn nested_objects_flatten_to_dotted_paths() {
        let r = row(json!({
            "message":"sync",
            "metrics":{"syncAccuracy":200,"detail":{"ok":true}},
            "node":{"port":7300,"host":"h1"}
        }));
        // Nested scalars become typed, queryable dotted columns (keys lowercased
        // to match the query parser).
        assert_eq!(
            field(&r, "metrics.syncaccuracy"),
            Some(&FieldValue::I64(200))
        );
        assert_eq!(
            field(&r, "metrics.detail.ok"),
            Some(&FieldValue::Bool(true))
        );
        assert_eq!(field(&r, "node.port"), Some(&FieldValue::I64(7300)));
        assert_eq!(field(&r, "node.host"), Some(&FieldValue::Str("h1".into())));
        // The container is shredded away, not kept as a stringified blob.
        assert!(field(&r, "node").is_none());
        assert!(field(&r, "metrics").is_none());
    }

    #[test]
    fn arrays_of_scalars_become_multivalue() {
        let r = row(json!({ "message":"x", "tags":["a","b"], "ports":[1,2] }));
        assert_eq!(
            fields_for(&r, "tags"),
            vec![&FieldValue::Str("a".into()), &FieldValue::Str("b".into())]
        );
        assert_eq!(
            fields_for(&r, "ports"),
            vec![&FieldValue::I64(1), &FieldValue::I64(2)]
        );
    }

    #[test]
    fn array_of_objects_flattens_to_repeated_dotted_paths() {
        // The order/line-items shape: one repeated column per leaf, one value
        // per element.
        let r = row(json!({
            "message":"order placed",
            "items":[
                {"sku":"sku-A0006","qty":1,"price_cents":3999},
                {"sku":"sku-C0006","qty":2,"price_cents":9999}
            ]
        }));
        assert_eq!(
            fields_for(&r, "items.sku"),
            vec![
                &FieldValue::Str("sku-A0006".into()),
                &FieldValue::Str("sku-C0006".into())
            ]
        );
        assert_eq!(
            fields_for(&r, "items.qty"),
            vec![&FieldValue::I64(1), &FieldValue::I64(2)]
        );
        assert_eq!(
            fields_for(&r, "items.price_cents"),
            vec![&FieldValue::I64(3999), &FieldValue::I64(9999)]
        );
    }

    #[test]
    fn deep_nesting_is_capped() {
        // Build an object nested past MAX_FLATTEN_DEPTH.
        let mut v = json!({ "leaf": 1 });
        for _ in 0..MAX_FLATTEN_DEPTH + 4 {
            v = json!({ "n": v });
        }
        let r = row(json!({ "message":"x", "root": v }));
        // The deep leaf never gets its own column — the sub-object at the cap is
        // stringified instead, so nothing is lost (still full-text searchable).
        assert!(
            !r.fields.iter().any(|(k, _)| k.ends_with(".leaf")),
            "deep leaf must not become its own dotted column"
        );
        assert!(
            r.fields
                .iter()
                .any(|(_, val)| matches!(val, FieldValue::Str(s) if s.contains("leaf"))),
            "capped sub-object should be retained as searchable text"
        );
    }

    #[test]
    fn default_source_fallback_and_precedence() {
        // No source key in the event -> the shipper default is applied.
        let r = json_to_row(&json!({"message":"x"}), Some("shipper-1")).unwrap();
        assert_eq!(r.source.as_deref(), Some("shipper-1"));
        // The event's own source wins over the default.
        let r = json_to_row(&json!({"message":"x","source":"in-doc"}), Some("shipper-1")).unwrap();
        assert_eq!(r.source.as_deref(), Some("in-doc"));
        // An empty default is ignored (no source set).
        let r = json_to_row(&json!({"message":"x"}), Some("")).unwrap();
        assert_eq!(r.source, None);
    }

    #[test]
    fn null_values_are_dropped() {
        let r = row(json!({
            "message":"x","a":null,"b":{"c":null},"tags":[null,"keep",null]
        }));
        assert!(field(&r, "a").is_none());
        assert!(field(&r, "b.c").is_none());
        // Nulls inside an array are skipped; the non-null element survives.
        assert_eq!(
            fields_for(&r, "tags"),
            vec![&FieldValue::Str("keep".into())]
        );
    }

    #[test]
    fn mixed_type_array_shreds_each_element() {
        let r = row(json!({"message":"x","mixed":[1,"two",{"nested":3}]}));
        // Scalars accumulate under the array path...
        assert_eq!(
            fields_for(&r, "mixed"),
            vec![&FieldValue::I64(1), &FieldValue::Str("two".into())]
        );
        // ...and an object element recurses into a dotted sub-path.
        assert_eq!(field(&r, "mixed.nested"), Some(&FieldValue::I64(3)));
    }

    #[test]
    fn empty_message_is_treated_as_absent() {
        let r = row(json!({"message":"","timestamp":"2021-01-01T00:00:00Z"}));
        assert_eq!(r.message, None);
    }

    #[test]
    fn raw_is_preserved_verbatim() {
        let v = json!({"message":"x","a":1});
        let r = json_to_row(&v, None).unwrap();
        assert_eq!(r.raw, Some(serde_json::to_string(&v).unwrap()));
    }

    #[test]
    fn event_day_extracts_date_or_none() {
        assert_eq!(
            event_day(&json!({"timestamp":"2026-06-07T12:00:00Z"})),
            NaiveDate::from_ymd_opt(2026, 6, 7)
        );
        assert_eq!(event_day(&json!({"message":"no timestamp"})), None);
    }

    #[test]
    fn index_template_multiple_placeholders_and_unterminated() {
        let ev = json!({"svc":"api","shard":2,"on":true});
        assert_eq!(resolve_index_template("{{svc}}-{{shard}}", &ev), "api-2");
        assert_eq!(resolve_index_template("flag-{{on}}", &ev), "flag-true");
        // An unterminated placeholder is kept literal and scanning stops.
        assert_eq!(resolve_index_template("x-{{svc", &ev), "x-{{svc");
    }
}
