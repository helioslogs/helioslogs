// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Push-protocol compatibility shims (HEC, ES `_bulk`, OTLP/HTTP
//! logs, Loki push) so shippers change only their output URL. Each parses into
//! `(index, event)` pairs through the same [`super::ingest::submit_routed`] tail.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::ingest::{authorize_ingest, body_bytes, body_text, submit_routed, IngestKind};
use super::AppState;

#[derive(Deserialize, Default)]
pub(super) struct ShimParams {
    env: Option<String>,
    /// Default target index when the payload doesn't carry one.
    index: Option<String>,
    source: Option<String>,
}

// ---- HEC -------------------------------------------------------------

/// `GET /services/collector/health` — HEC clients probe this before sending.
pub(super) async fn hec_health_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({ "text": "HEC is healthy", "code": 17 })),
    )
}

/// `POST /services/collector` and `/services/collector/event`.
pub(super) async fn hec_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<ShimParams>,
    body: Bytes,
) -> Response {
    let authz = match authorize_ingest(
        &s,
        &headers,
        p.env.as_deref(),
        p.index.as_deref(),
        IngestKind::Shim,
    )
    .await
    {
        Ok(a) => a,
        Err(resp) => return resp.into_response(),
    };
    let text = body_text(&body);
    let (items, parse_errors) = parse_hec(&text, &authz.default_index);
    let out = submit_routed(
        &authz.env,
        items.iter().map(|(i, v)| (i.as_str(), v)),
        p.source.as_deref(),
        authz.allowed_indexes.as_ref(),
        false, // push shims: 429 on full so the shipper retries
    );

    if out.throttled {
        // HEC code 9 = "server is busy"; clients back off and retry.
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "text": "server busy", "code": 9 })),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        Json(json!({
            "text": "Success",
            "code": 0,
            "ingested": out.ingested,
            "errors": out.errors(parse_errors),
        })),
    )
        .into_response()
}

/// Parse a HEC body (JSON objects, whitespace-separated, not strict NDJSON) into
/// `(index, event)` pairs; `parse_errors` counts non-object/undecodable values.
fn parse_hec(body: &str, default_index: &str) -> (Vec<(String, Value)>, usize) {
    let mut items = Vec::new();
    let mut errors = 0usize;
    let stream = serde_json::Deserializer::from_str(body).into_iter::<Value>();
    for v in stream {
        match v {
            Ok(Value::Object(obj)) => items.push(hec_event(obj, default_index)),
            Ok(_) => errors += 1,
            // A malformed value desyncs the stream cursor; stop rather than spin.
            Err(_) => {
                errors += 1;
                break;
            }
        }
    }
    (items, errors)
}

/// Map a HEC envelope to `(index, event)`. An `event` key means a real envelope (unwrap +
/// fold in `fields`/`time`/`host`/...); otherwise the object itself is the event.
fn hec_event(obj: Map<String, Value>, default_index: &str) -> (String, Value) {
    let Some(event) = obj.get("event") else {
        return (default_index.to_string(), Value::Object(obj));
    };
    let mut out = match event {
        Value::Object(m) => m.clone(),
        Value::String(s) => Map::from_iter([("message".to_string(), json!(s))]),
        other => Map::from_iter([("message".to_string(), json!(other.to_string()))]),
    };
    if let Some(Value::Object(fields)) = obj.get("fields") {
        for (k, v) in fields {
            out.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    // `time` (epoch seconds, possibly fractional) → timestamp; json_to_row's
    // numeric/string timestamp parsing handles it.
    if let Some(t) = obj.get("time") {
        out.entry("timestamp".to_string())
            .or_insert_with(|| t.clone());
    }
    for (src_key, dst_key) in [
        ("host", "host"),
        ("source", "source"),
        ("sourcetype", "sourcetype"),
    ] {
        if let Some(Value::String(v)) = obj.get(src_key) {
            out.entry(dst_key.to_string()).or_insert_with(|| json!(v));
        }
    }
    let index = obj
        .get("index")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(default_index)
        .to_string();
    (index, Value::Object(out))
}

// ---- Elasticsearch bulk -----------------------------------------------------

/// `POST /_bulk` and `/api/es/_bulk`. Maps to a minimal ES bulk response so
/// Filebeat / Logstash / Vector's `elasticsearch` sink treat it as success.
pub(super) async fn es_bulk_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<ShimParams>,
    body: Bytes,
) -> Response {
    let authz = match authorize_ingest(
        &s,
        &headers,
        p.env.as_deref(),
        p.index.as_deref(),
        IngestKind::Shim,
    )
    .await
    {
        Ok(a) => a,
        Err(resp) => return resp.into_response(),
    };
    let text = body_text(&body);
    let (items, _parse_errors) = parse_es_bulk(&text, &authz.default_index);
    let out = submit_routed(
        &authz.env,
        items.iter().map(|(i, v)| (i.as_str(), v)),
        p.source.as_deref(),
        authz.allowed_indexes.as_ref(),
        false, // push shims: 429 on full so the shipper retries
    );

    let status = if out.throttled {
        StatusCode::TOO_MANY_REQUESTS
    } else {
        StatusCode::OK
    };
    // One success item per ingested doc. Full per-item fidelity (echoing _id,
    // partial-failure statuses) is a follow-up; shippers key off `errors`.
    let resp_items: Vec<Value> = (0..out.ingested)
        .map(|_| json!({ "index": { "status": 201 } }))
        .collect();
    let errors = out.throttled || out.row_errors > 0 || !out.write_errors.is_empty();
    (
        status,
        Json(json!({ "took": 0, "errors": errors, "items": resp_items })),
    )
        .into_response()
}

/// Parse an ES bulk body (NDJSON: action line, then a source line for
/// index/create/update). `_index` on the action routes per-doc.
fn parse_es_bulk(body: &str, default_index: &str) -> (Vec<(String, Value)>, usize) {
    let mut items = Vec::new();
    let mut errors = 0usize;
    // Some(index, is_update) means the next non-blank line is this action's doc.
    let mut pending: Option<(String, bool)> = None;

    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                errors += 1;
                continue;
            }
        };

        if let Some((idx, is_update)) = pending.take() {
            // `update` wraps the partial doc in `{ "doc": {...} }`.
            let ev = if is_update {
                v.get("doc").cloned().unwrap_or(v)
            } else {
                v
            };
            if ev.is_object() {
                items.push((idx, ev));
            } else {
                errors += 1;
            }
            continue;
        }

        // Action line: { "<action>": { "_index": "...", ... } }.
        let Some((action, meta)) = v.as_object().and_then(|o| o.iter().next()) else {
            errors += 1;
            continue;
        };
        let target = meta
            .get("_index")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(default_index)
            .to_string();
        match action.as_str() {
            "index" | "create" => pending = Some((target, false)),
            "update" => pending = Some((target, true)),
            "delete" => {} // no doc line follows
            _ => errors += 1,
        }
    }
    (items, errors)
}

// ---- OTLP/HTTP logs (JSON + protobuf) ---------------------------------------

/// `POST /v1/logs` and `/api/otlp/v1/logs` (OTLP/HTTP). Accepts `application/x-protobuf`
/// (the OTel default) and `application/json`; both flatten to the same event shape.
pub(super) async fn otlp_logs_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<ShimParams>,
    body: Bytes,
) -> Response {
    let authz = match authorize_ingest(
        &s,
        &headers,
        p.env.as_deref(),
        p.index.as_deref(),
        IngestKind::Shim,
    )
    .await
    {
        Ok(a) => a,
        Err(resp) => return resp.into_response(),
    };
    let is_proto = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.contains("protobuf"));
    let raw = body_bytes(&body);
    let parsed = if is_proto {
        super::otlp_proto::parse_otlp_logs_proto(&raw, &authz.default_index)
    } else {
        parse_otlp_logs(&String::from_utf8_lossy(&raw), &authz.default_index)
    };
    let items = match parsed {
        Ok(items) => items,
        Err(e) => return shim_bad_request(&e),
    };
    let out = submit_routed(
        &authz.env,
        items.iter().map(|(i, v)| (i.as_str(), v)),
        p.source.as_deref(),
        authz.allowed_indexes.as_ref(),
        false, // push shims: 429 on full so the shipper retries
    );
    if out.throttled {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({}))).into_response();
    }
    // ExportLogsServiceResponse — empty object on full success.
    (StatusCode::OK, Json(json!({}))).into_response()
}

/// Flatten OTLP `resourceLogs[].scopeLogs[].logRecords[]` into events, merging
/// resource + record attributes and mapping `body`/`timeUnixNano`/`severityText`.
fn parse_otlp_logs(text: &str, default_index: &str) -> Result<Vec<(String, Value)>, String> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| format!("OTLP/JSON parse failed (protobuf isn't supported yet): {e}"))?;
    let mut out = Vec::new();
    let empty: Vec<Value> = Vec::new();
    let resource_logs = root.get("resourceLogs").and_then(|v| v.as_array());
    for rl in resource_logs.unwrap_or(&empty) {
        let mut base = Map::new();
        if let Some(attrs) = rl
            .pointer("/resource/attributes")
            .and_then(|v| v.as_array())
        {
            flatten_otlp_attrs(attrs, &mut base);
        }
        for sl in rl
            .get("scopeLogs")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty)
        {
            for rec in sl
                .get("logRecords")
                .and_then(|v| v.as_array())
                .unwrap_or(&empty)
            {
                let mut ev = base.clone();
                if let Some(attrs) = rec.get("attributes").and_then(|v| v.as_array()) {
                    flatten_otlp_attrs(attrs, &mut ev);
                }
                if let Some(body) = rec.get("body") {
                    ev.insert("message".into(), otlp_any_value(body));
                }
                if let Some(t) = rec
                    .get("timeUnixNano")
                    .or_else(|| rec.get("observedTimeUnixNano"))
                {
                    if let Some(n) = otlp_unix_nano(t) {
                        ev.insert("timestamp".into(), json!(n));
                    }
                }
                if let Some(sev) = rec.get("severityText").and_then(|v| v.as_str()) {
                    ev.entry("severity".to_string())
                        .or_insert_with(|| json!(sev));
                }
                for k in ["traceId", "spanId"] {
                    if let Some(Value::String(v)) = rec.get(k) {
                        ev.entry(k.to_string()).or_insert_with(|| json!(v));
                    }
                }
                out.push((default_index.to_string(), Value::Object(ev)));
            }
        }
    }
    Ok(out)
}

/// OTLP attribute list `[{key, value: AnyValue}]` → flat map entries.
fn flatten_otlp_attrs(attrs: &[Value], into: &mut Map<String, Value>) {
    for a in attrs {
        if let (Some(k), Some(v)) = (a.get("key").and_then(|k| k.as_str()), a.get("value")) {
            into.insert(k.to_string(), otlp_any_value(v));
        }
    }
}

/// OTLP `AnyValue` → plain JSON scalar. `intValue` arrives as a numeric string.
fn otlp_any_value(v: &Value) -> Value {
    let Some(o) = v.as_object() else {
        return v.clone();
    };
    if let Some(s) = o.get("stringValue") {
        return s.clone();
    }
    if let Some(i) = o.get("intValue") {
        return i
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .map_or_else(|| i.clone(), |n| json!(n));
    }
    if let Some(b) = o.get("boolValue") {
        return b.clone();
    }
    if let Some(d) = o.get("doubleValue") {
        return d.clone();
    }
    // arrayValue / kvlistValue / bytesValue: stringify so they stay searchable.
    json!(v.to_string())
}

fn otlp_unix_nano(t: &Value) -> Option<i64> {
    match t {
        Value::String(s) => s.parse::<i64>().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

// ---- Loki push (JSON) -------------------------------------------------------

/// `POST /loki/api/v1/push` (JSON). Stream labels become fields on every line, and a
/// JSON-object line is shredded into fields (see [`parse_loki_push`]).
pub(super) async fn loki_push_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<ShimParams>,
    body: Bytes,
) -> Response {
    let authz = match authorize_ingest(
        &s,
        &headers,
        p.env.as_deref(),
        p.index.as_deref(),
        IngestKind::Shim,
    )
    .await
    {
        Ok(a) => a,
        Err(resp) => return resp.into_response(),
    };
    let text = body_text(&body);
    let items = match parse_loki_push(&text, &authz.default_index) {
        Ok(items) => items,
        Err(e) => return shim_bad_request(&e),
    };
    let out = submit_routed(
        &authz.env,
        items.iter().map(|(i, v)| (i.as_str(), v)),
        p.source.as_deref(),
        authz.allowed_indexes.as_ref(),
        false, // push shims: 429 on full so the shipper retries
    );
    if out.throttled {
        return (StatusCode::TOO_MANY_REQUESTS, "rate limited").into_response();
    }
    // Loki replies 204 No Content on success.
    StatusCode::NO_CONTENT.into_response()
}

/// Loki `{streams:[{stream:{labels}, values:[[ts_ns, line, meta?]]}]}` → events.
fn parse_loki_push(text: &str, default_index: &str) -> Result<Vec<(String, Value)>, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| {
        format!("Loki JSON parse failed (protobuf/snappy isn't supported yet): {e}")
    })?;
    let mut out = Vec::new();
    let empty: Vec<Value> = Vec::new();
    for stream in root
        .get("streams")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty)
    {
        let mut labels = Map::new();
        if let Some(obj) = stream.get("stream").and_then(|v| v.as_object()) {
            for (k, v) in obj {
                labels.insert(k.clone(), v.clone());
            }
        }
        for entry in stream
            .get("values")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty)
        {
            let Some(arr) = entry.as_array() else {
                continue;
            };
            // A JSON-object line is shredded into fields (like the object-based shims);
            // otherwise the line stays as the message.
            let mut ev = match arr.get(1).and_then(|v| v.as_str()) {
                Some(line) => match serde_json::from_str::<Value>(line) {
                    Ok(Value::Object(m)) => m,
                    _ => Map::from_iter([("message".to_string(), json!(line))]),
                },
                None => Map::new(),
            };
            // Loki's tuple timestamp (ns); the event's own `timestamp` wins.
            if let Some(ts) = arr.first().and_then(otlp_unix_nano) {
                ev.entry("timestamp".to_string())
                    .or_insert_with(|| json!(ts));
            }
            // Stream labels are external metadata — fill in without clobbering
            // the event's own fields.
            for (k, v) in &labels {
                ev.entry(k.clone()).or_insert_with(|| v.clone());
            }
            // Optional 3rd element: structured metadata.
            if let Some(meta) = arr.get(2).and_then(|v| v.as_object()) {
                for (k, v) in meta {
                    ev.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            out.push((default_index.to_string(), Value::Object(ev)));
        }
    }
    Ok(out)
}

fn shim_bad_request(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hec_envelope_with_event_object() {
        let body = r#"{"time":1433188255.253,"host":"web01","source":"/var/log/app.log","sourcetype":"json","index":"app","event":{"level":"error","msg":"boom"},"fields":{"dc":"us-east"}}"#;
        let (items, errs) = parse_hec(body, "default");
        assert_eq!(errs, 0);
        assert_eq!(items.len(), 1);
        let (idx, ev) = &items[0];
        assert_eq!(idx, "app");
        assert_eq!(ev["level"], "error");
        assert_eq!(ev["dc"], "us-east"); // merged field
        assert_eq!(ev["host"], "web01");
        assert_eq!(ev["source"], "/var/log/app.log");
        assert_eq!(ev["timestamp"], json!(1433188255.253));
    }

    #[test]
    fn hec_string_event_becomes_message() {
        let body = r#"{"event":"plain text line"}{"event":"second"}"#;
        let (items, errs) = parse_hec(body, "default");
        assert_eq!(errs, 0);
        assert_eq!(items.len(), 2); // concatenated objects
        assert_eq!(items[0].1["message"], "plain text line");
        assert_eq!(items[0].0, "default"); // falls back to default index
    }

    #[test]
    fn hec_raw_object_without_event_key() {
        let (items, _) = parse_hec(r#"{"a":1,"b":2}"#, "raw");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "raw");
        assert_eq!(items[0].1["a"], 1);
    }

    #[test]
    fn es_bulk_index_and_create() {
        let body = concat!(
            "{\"index\":{\"_index\":\"logs\",\"_id\":\"1\"}}\n",
            "{\"message\":\"one\",\"@timestamp\":\"2026-06-01T00:00:00Z\"}\n",
            "{\"create\":{\"_index\":\"audit\"}}\n",
            "{\"message\":\"two\"}\n",
        );
        let (items, errs) = parse_es_bulk(body, "default");
        assert_eq!(errs, 0);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "logs");
        assert_eq!(items[0].1["message"], "one");
        assert_eq!(items[1].0, "audit");
    }

    #[test]
    fn es_bulk_default_index_and_delete_skipped() {
        let body = concat!(
            "{\"index\":{}}\n",
            "{\"message\":\"uses default\"}\n",
            "{\"delete\":{\"_index\":\"logs\",\"_id\":\"9\"}}\n", // no doc line
            "{\"update\":{\"_index\":\"logs\"}}\n",
            "{\"doc\":{\"message\":\"patched\"}}\n",
        );
        let (items, errs) = parse_es_bulk(body, "fallback");
        assert_eq!(errs, 0);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "fallback");
        assert_eq!(items[1].0, "logs");
        assert_eq!(items[1].1["message"], "patched"); // unwrapped from {doc:...}
    }

    #[test]
    fn otlp_logs_flattens_resource_and_record() {
        let body = r#"{
          "resourceLogs":[{
            "resource":{"attributes":[{"key":"service.name","value":{"stringValue":"checkout"}}]},
            "scopeLogs":[{"logRecords":[
              {"timeUnixNano":"1544712660300000000","severityText":"ERROR",
               "body":{"stringValue":"db timeout"},
               "attributes":[{"key":"http.status","value":{"intValue":"500"}}]}
            ]}]
          }]
        }"#;
        let items = parse_otlp_logs(body, "otel").unwrap();
        assert_eq!(items.len(), 1);
        let (idx, ev) = &items[0];
        assert_eq!(idx, "otel");
        assert_eq!(ev["service.name"], "checkout");
        assert_eq!(ev["message"], "db timeout");
        assert_eq!(ev["severity"], "ERROR");
        assert_eq!(ev["http.status"], 500); // intValue string → number
        assert_eq!(ev["timestamp"], json!(1544712660300000000i64));
    }

    #[test]
    fn otlp_non_json_errors_cleanly() {
        assert!(parse_otlp_logs("\u{0}\u{1}protobuf-bytes", "x").is_err());
    }

    #[test]
    fn loki_push_expands_streams_and_labels() {
        let body = r#"{"streams":[
          {"stream":{"app":"api","level":"warn"},
           "values":[["1544712660300000000","line one"],
                     ["1544712660400000000","line two",{"trace_id":"abc"}]]}
        ]}"#;
        let items = parse_loki_push(body, "loki").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "loki");
        assert_eq!(items[0].1["app"], "api");
        assert_eq!(items[0].1["level"], "warn");
        assert_eq!(items[0].1["message"], "line one");
        assert_eq!(items[0].1["timestamp"], json!(1544712660300000000i64));
        assert_eq!(items[1].1["trace_id"], "abc"); // structured metadata merged
    }

    #[test]
    fn loki_json_line_is_shredded_into_fields() {
        // A JSON-object line (e.g. Fluent Bit `line_format json`) is parsed into
        // fields, not stored as a string message; labels fill in around it.
        let body = r#"{"streams":[
          {"stream":{"job":"fluentbit","service":"checkout","level":"info"},
           "values":[["1780766215412556300",
             "{\"message\":\"order processed\",\"http\":{\"method\":\"POST\",\"status\":200},\"latency_ms\":42}"]]}
        ]}"#;
        let items = parse_loki_push(body, "loki").unwrap();
        assert_eq!(items.len(), 1);
        let ev = &items[0].1;
        // Shredded payload fields.
        assert_eq!(ev["message"], "order processed");
        assert_eq!(ev["http"]["status"], json!(200));
        assert_eq!(ev["latency_ms"], json!(42));
        // Labels merged in alongside.
        assert_eq!(ev["service"], "checkout");
        assert_eq!(ev["job"], "fluentbit");
        // Loki tuple timestamp used (the line carried none).
        assert_eq!(ev["timestamp"], json!(1780766215412556300i64));
    }

    #[test]
    fn loki_event_field_wins_over_label() {
        // When the JSON line and a label share a key, the event's own value wins.
        let body = r#"{"streams":[
          {"stream":{"service":"label-svc"},
           "values":[["1","{\"service\":\"event-svc\",\"message\":\"hi\"}"]]}
        ]}"#;
        let items = parse_loki_push(body, "loki").unwrap();
        assert_eq!(items[0].1["service"], "event-svc");
    }
}
