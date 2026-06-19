// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `tracing_subscriber::Layer` turning each event into a JSON doc shipped to the
//! writer task. Drops `helioslogs::self_logs::*`-targeted events to avoid unbounded
//! self-recursion (block-engine events are allowed; their feedback is bounded).

use std::fmt;

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use super::{sender, SelfLogEvent, LOGS_INDEX};

pub struct SelfLogsLayer;

impl<S> Layer<S> for SelfLogsLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let Some(tx) = sender() else { return };

        let meta = event.metadata();
        let target = meta.target();
        // Drop only self_logs-internal events — they'd recurse unboundedly via
        // the channel they're written to. Catalog feedback is bounded; see module doc.
        if target.starts_with("helioslogs::self_logs") {
            return;
        }

        let mut collector = FieldCollector::default();
        event.record(&mut collector);

        let mut doc = collector.extras;
        doc.insert(
            "timestamp".into(),
            serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
        );
        doc.insert(
            "level".into(),
            serde_json::Value::String(meta.level().to_string()),
        );
        doc.insert(
            "target".into(),
            serde_json::Value::String(target.to_string()),
        );
        // `source` is the origin tag: tracing target as `<crate>.<module>`, matching
        // the HTTP middleware so `source:helioslogs.http` cross-cuts both indexes.
        doc.insert(
            "source".into(),
            serde_json::Value::String(source_from_target(target)),
        );
        if let Some(file) = meta.file() {
            doc.insert("file".into(), serde_json::Value::String(file.to_string()));
        }
        if let Some(line) = meta.line() {
            doc.insert("line".into(), serde_json::Value::Number(line.into()));
        }
        if let Some(msg) = collector.message {
            doc.insert("message".into(), serde_json::Value::String(msg));
        }

        // Unbounded send — never blocks the event-emitting thread. If the
        // writer task is gone (shutdown), the send returns Err and we drop.
        let _ = tx.send(SelfLogEvent {
            index: LOGS_INDEX,
            doc: serde_json::Value::Object(doc),
        });
    }
}

/// `crate::module::submodule` → `crate.module`. Two segments distinguishes
/// `helioslogs.http` from `helioslogs.catalog` without per-submodule source explosion.
fn source_from_target(target: &str) -> String {
    let mut parts = target.split("::");
    let crate_name = parts.next().unwrap_or("helioslogs");
    match parts.next() {
        Some(module) => format!("{}.{}", crate_name, module),
        None => crate_name.to_string(),
    }
}

/// Walks event fields, peeling off the synthetic `message` field and capturing
/// the rest as a flat JSON map (numeric/bool stay typed, else stringified).
#[derive(Default)]
struct FieldCollector {
    message: Option<String>,
    extras: serde_json::Map<String, serde_json::Value>,
}

impl Visit for FieldCollector {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.extras.insert(
                field.name().to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let s = format!("{:?}", value);
        if field.name() == "message" {
            self.message = Some(s);
        } else {
            self.extras
                .insert(field.name().to_string(), serde_json::Value::String(s));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.extras.insert(field.name().to_string(), value.into());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.extras.insert(field.name().to_string(), value.into());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.extras.insert(field.name().to_string(), value.into());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.extras.insert(
            field.name().to_string(),
            serde_json::Number::from_f64(value)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
        );
    }
}
