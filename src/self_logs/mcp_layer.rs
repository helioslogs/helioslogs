// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Emits one self-log doc per MCP tool call into `_heliosmcp` (tool/arguments/
//! duration/status). Shares the `SelfLogEvent` channel with `http_layer.rs`.

use std::time::Duration;

use serde_json::{Map, Value};

use super::{sender, SelfLogEvent, MCP_INDEX};

/// Logs a completed tool call (from `mcp::tools::call`). No-op when no sender
/// is installed. `api_key` is the `(id, name)` of the key that authorized the
/// call, or `None` for an anonymous call (no MCP-scoped key exists yet).
pub fn log_tool_call(
    tool: &str,
    arguments: &Value,
    duration: Duration,
    error: Option<&str>,
    api_key: Option<(&str, &str)>,
) {
    let Some(tx) = sender() else { return };

    let mut doc = Map::new();
    doc.insert(
        "timestamp".into(),
        Value::String(chrono::Utc::now().to_rfc3339()),
    );
    doc.insert("source".into(), Value::String("helioslogs.mcp".into()));
    doc.insert("tool".into(), Value::String(tool.to_string()));
    // Attribute the call to its API key so MCP activity is per-caller auditable.
    if let Some((id, name)) = api_key {
        doc.insert("api_key_id".into(), Value::String(id.to_string()));
        doc.insert("api_key_name".into(), Value::String(name.to_string()));
    }
    if !arguments.is_null() {
        doc.insert("arguments".into(), arguments.clone());
    }
    let duration_ms = duration.as_secs_f64() * 1000.0;
    doc.insert(
        "duration_ms".into(),
        serde_json::Number::from_f64((duration_ms * 100.0).round() / 100.0)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    );
    let is_error = error.is_some();
    doc.insert(
        "status".into(),
        Value::String(if is_error {
            "error".into()
        } else {
            "ok".into()
        }),
    );
    doc.insert(
        "level".into(),
        Value::String(if is_error {
            "ERROR".into()
        } else {
            "INFO".into()
        }),
    );
    if let Some(err) = error {
        doc.insert("error".into(), Value::String(err.to_string()));
    }
    doc.insert(
        "message".into(),
        Value::String(format!(
            "mcp {} {} ({:.1}ms){}{}",
            tool,
            if is_error { "→ error" } else { "→ ok" },
            duration_ms,
            api_key
                .map(|(_, name)| format!(" [key: {name}]"))
                .unwrap_or_default(),
            error.map(|e| format!(": {e}")).unwrap_or_default(),
        )),
    );

    let _ = tx.send(SelfLogEvent {
        index: MCP_INDEX,
        doc: Value::Object(doc),
    });
}
