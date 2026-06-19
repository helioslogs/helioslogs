// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! axum middleware emitting one self-log doc per request (method/path/status/
//! duration + a few headers). `/api/health` is skipped (constant LB probe).

use std::time::Instant;

use axum::body::Body;
use axum::http::{Method, Request, Uri};
use axum::middleware::Next;
use axum::response::Response;

use super::{sender, SelfLogEvent, HTTP_INDEX};

const SKIP_PATHS: &[&str] = &["/api/health"];

pub async fn http_log_middleware(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path().to_string();

    if SKIP_PATHS.contains(&path.as_str()) {
        return next.run(req).await;
    }

    let user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let xff = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string());

    let start = Instant::now();
    let resp = next.run(req).await;
    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

    emit(
        &method,
        &uri,
        &path,
        resp.status().as_u16(),
        duration_ms,
        user_agent,
        xff,
    );

    resp
}

fn emit(
    method: &Method,
    uri: &Uri,
    path: &str,
    status: u16,
    duration_ms: f64,
    user_agent: Option<String>,
    client_ip: Option<String>,
) {
    let Some(tx) = sender() else { return };

    let mut doc = serde_json::Map::new();
    doc.insert(
        "timestamp".into(),
        serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    // `source` mirrors the tracing layer's `<crate>.<module>` convention so
    // `source:helioslogs.http` cross-cuts the access log and http-module tracing.
    doc.insert(
        "source".into(),
        serde_json::Value::String("helioslogs.http".to_string()),
    );
    doc.insert(
        "method".into(),
        serde_json::Value::String(method.to_string()),
    );
    doc.insert("path".into(), serde_json::Value::String(path.to_string()));
    if let Some(q) = uri.query() {
        // Querystrings may carry sensitive params on misuse; accepted for the
        // operational value of seeing actual filters/queries.
        doc.insert("query".into(), serde_json::Value::String(q.to_string()));
    }
    doc.insert("status".into(), serde_json::Value::Number(status.into()));
    doc.insert(
        "duration_ms".into(),
        serde_json::Number::from_f64((duration_ms * 100.0).round() / 100.0)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
    );
    // Bucket the status as `status_class` (`2xx`, `5xx`, ...) — a clean
    // low-cardinality sidebar facet.
    doc.insert(
        "status_class".into(),
        serde_json::Value::String(format!("{}xx", status / 100)),
    );
    // Severity heuristic — 5xx error, 4xx warn, else info — so the row badge
    // picks a colour without a literal `level` per request.
    let level = if status >= 500 {
        "ERROR"
    } else if status >= 400 {
        "WARN"
    } else {
        "INFO"
    };
    doc.insert("level".into(), serde_json::Value::String(level.into()));
    if let Some(ua) = user_agent {
        doc.insert("user_agent".into(), serde_json::Value::String(ua));
    }
    if let Some(ip) = client_ip {
        doc.insert("client_ip".into(), serde_json::Value::String(ip));
    }
    // Synthesise a message field so the row preview reads as a one-liner.
    doc.insert(
        "message".into(),
        serde_json::Value::String(format!(
            "{} {} → {} ({:.1}ms)",
            method, path, status, duration_ms
        )),
    );

    let _ = tx.send(SelfLogEvent {
        index: HTTP_INDEX,
        doc: serde_json::Value::Object(doc),
    });
}
