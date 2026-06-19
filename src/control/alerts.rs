// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Alert types + severity normalization. Storage + CRUD live on
//! [`crate::control::Control`] (`backend.rs`, owner-scoped per-entity files).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Alert {
    pub id: String,
    pub monitor_id: String,
    /// Monitor name, denormalized at create time so the inbox needs no lookup.
    pub monitor_name: String,
    /// Env the raising monitor targets, denormalized at create time.
    /// `#[serde(default)]` so alerts written before this field stay readable.
    #[serde(default)]
    pub env: String,
    /// Conversation id of the monitor run that raised this (`conv_*`).
    pub conversation_id: Option<String>,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub evidence: Option<Value>,
    /// `true` = visible to all users; any user may acknowledge it. `false` =
    /// owner-only. Denormalized from the raising monitor at create time.
    #[serde(default)]
    pub public: bool,
    pub acknowledged: bool,
    pub acknowledged_at: Option<i64>,
    /// Per-request toast-dismissed flag, computed from `dismissed_by` on read (not persisted as-is).
    #[serde(default)]
    pub dismissed: bool,
    pub created_at: i64,
}

/// Payload constructed server-side from a `raise_alert` tool call.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AlertInput {
    pub monitor_id: String,
    pub conversation_id: Option<String>,
    pub severity: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub evidence: Option<Value>,
}

/// Payload for a monitor-less alert (manually filed via a UI/API path).
/// `source` becomes the inbox's origin label (the `monitor_name` slot).
#[derive(Debug, Clone, Default)]
pub struct ManualAlertInput {
    pub source: String,
    pub env: String,
    pub public: bool,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub evidence: Option<Value>,
}

pub(crate) fn normalize_severity(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "high" | "critical" | "error" => "high".into(),
        "medium" | "warn" | "warning" => "medium".into(),
        _ => "low".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_severity;

    #[test]
    fn high_aliases() {
        for s in ["high", "HIGH", "critical", "error", "  Error "] {
            assert_eq!(normalize_severity(s), "high", "input {s:?}");
        }
    }

    #[test]
    fn medium_aliases() {
        for s in ["medium", "warn", "Warning"] {
            assert_eq!(normalize_severity(s), "medium", "input {s:?}");
        }
    }

    #[test]
    fn unknown_defaults_to_low() {
        for s in ["low", "info", "", "garbage"] {
            assert_eq!(normalize_severity(s), "low", "input {s:?}");
        }
    }
}
