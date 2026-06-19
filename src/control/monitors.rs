// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Scheduled-monitor types + cadence constants. Storage + CRUD + the CAS run
//! lease live on [`crate::control::Control`] (`backend.rs`, per-entity files).

use serde::{Deserialize, Serialize};

/// Cadence floor: each tick spends LLM tokens, so sub-5-min is a foot-gun.
pub const MIN_INTERVAL_SECONDS: i64 = 300;
pub const DEFAULT_INTERVAL_SECONDS: i64 = 1800;

/// A run idle past this is treated as stuck (crash); its lease is stolen next tick.
pub const STUCK_LEASE_SECS: i64 = 600;

/// Threshold count window: the count query's lookback, distinct from `interval_seconds`.
pub const DEFAULT_WINDOW_SECONDS: i64 = 900;
pub const MIN_WINDOW_SECONDS: i64 = 60;

/// What a monitor *is*: LLM-driven investigation (`Ai`) or count-threshold
/// check (`Threshold`). Pre-existing stored monitors deserialize as `Ai`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MonitorKind {
    #[default]
    Ai,
    Threshold,
}

/// Comparison applied between the observed count and the threshold.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Comparison {
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
    Neq,
}

impl Comparison {
    /// Does `count` breach `threshold` under this comparison?
    pub fn breached(self, count: i64, threshold: i64) -> bool {
        match self {
            Comparison::Gt => count > threshold,
            Comparison::Gte => count >= threshold,
            Comparison::Lt => count < threshold,
            Comparison::Lte => count <= threshold,
            Comparison::Eq => count == threshold,
            Comparison::Neq => count != threshold,
        }
    }

    /// Human phrasing for alert titles/summaries (reads after the count,
    /// e.g. "296 … is **above** the threshold of 50").
    pub fn describe(self) -> &'static str {
        match self {
            Comparison::Gt => "above",
            Comparison::Gte => "at or above",
            Comparison::Lt => "below",
            Comparison::Lte => "at or below",
            Comparison::Eq => "equal to",
            Comparison::Neq => "not equal to",
        }
    }
}

/// Config for a `Threshold` monitor. Absent on `Ai` monitors.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ThresholdConfig {
    /// Pipelined query the count is taken over. `index:foo` works here
    /// too; `index` below is an additional convenience filter.
    #[serde(default)]
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    /// Lookback window the count spans, ending at evaluation time.
    pub window_seconds: i64,
    pub comparison: Comparison,
    pub threshold: i64,
    /// Severity stamped on the alert when the threshold is breached.
    #[serde(default = "default_severity")]
    pub severity: String,
}

fn default_severity() -> String {
    "medium".to_string()
}

/// Per-monitor webhook override; when set it replaces the global alert
/// webhook target for this monitor's alerts.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NotifyOverride {
    pub webhook_url: String,
    /// "generic" (default) or "slack".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Monitor {
    pub id: String,
    pub name: String,
    pub description: String,
    /// AI monitors only — the instruction handed to the agent each tick.
    /// Empty for threshold monitors.
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub kind: MonitorKind,
    /// Threshold monitors only — the count/comparison config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<ThresholdConfig>,
    /// Per-monitor alert webhook override; None = global settings target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<NotifyOverride>,
    pub interval_seconds: i64,
    pub enabled: bool,
    pub last_run_at: Option<i64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    /// Conversation id of the last run's trace (`conv_*`), for click-into-trace
    /// from the alert inbox. Always null for threshold monitors (no agent run).
    pub last_conversation_id: Option<String>,
    /// Threshold monitors only — whether the last evaluation was breaching.
    /// Drives edge-triggered alerting (fire on false→true, not every tick).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_breaching: Option<bool>,
    pub running: bool,
    pub running_since: Option<i64>,
    /// `true` (default) = raised alerts are visible/ackable by all; `false` = owner-only.
    #[serde(default = "default_public")]
    pub public: bool,
    /// Env this monitor's run targets. Stamped at create time.
    pub env: String,
    pub created_at: String,
    pub updated_at: String,
    /// Owner display label, set only in the admin "view all" listing. Never
    /// persisted (skipped when `None`, which it always is on the stored doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

fn default_public() -> bool {
    true
}

/// Payload for `POST /api/monitors`. Owner is taken from the session.
#[derive(Deserialize, Debug, Default)]
pub struct MonitorInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: MonitorKind,
    /// Required for `Ai` monitors; ignored for `Threshold`.
    #[serde(default)]
    pub prompt: String,
    /// Required for `Threshold` monitors; ignored for `Ai`.
    #[serde(default)]
    pub threshold: Option<ThresholdConfig>,
    #[serde(default)]
    pub notify: Option<NotifyOverride>,
    #[serde(default)]
    pub interval_seconds: Option<i64>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Alert visibility. Defaults to public when omitted by the client.
    #[serde(default = "default_public")]
    pub public: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Deserialize, Debug, Default)]
pub struct MonitorPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub kind: Option<MonitorKind>,
    pub prompt: Option<String>,
    pub threshold: Option<ThresholdConfig>,
    /// `Some` with an empty `webhook_url` clears the override.
    pub notify: Option<NotifyOverride>,
    pub interval_seconds: Option<i64>,
    pub enabled: Option<bool>,
    pub public: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_breached() {
        assert!(Comparison::Gt.breached(296, 50));
        assert!(!Comparison::Gt.breached(50, 50));
        assert!(Comparison::Gte.breached(50, 50));
        assert!(Comparison::Lt.breached(3, 10));
        assert!(Comparison::Lte.breached(10, 10));
        assert!(Comparison::Eq.breached(0, 0));
        assert!(Comparison::Neq.breached(1, 0));
        assert!(!Comparison::Neq.breached(0, 0));
    }

    #[test]
    fn comparison_describe_phrasing() {
        assert_eq!(Comparison::Gt.describe(), "above");
        assert_eq!(Comparison::Gte.describe(), "at or above");
        assert_eq!(Comparison::Lt.describe(), "below");
        assert_eq!(Comparison::Lte.describe(), "at or below");
        assert_eq!(Comparison::Eq.describe(), "equal to");
        assert_eq!(Comparison::Neq.describe(), "not equal to");
    }

    #[test]
    fn comparison_serde_lowercase() {
        assert_eq!(
            serde_json::to_value(Comparison::Gte).unwrap(),
            serde_json::json!("gte")
        );
        let c: Comparison = serde_json::from_value(serde_json::json!("neq")).unwrap();
        assert_eq!(c, Comparison::Neq);
    }

    #[test]
    fn kind_defaults_to_ai_when_absent() {
        // Monitors written before `kind` existed must read back as AI.
        let m: Monitor = serde_json::from_value(serde_json::json!({
            "id": "mon_x",
            "name": "legacy",
            "description": "",
            "prompt": "watch errors",
            "interval_seconds": 1800,
            "enabled": true,
            "last_run_at": null,
            "last_status": null,
            "last_error": null,
            "last_conversation_id": null,
            "running": false,
            "running_since": null,
            "env": "default",
            "created_at": "t",
            "updated_at": "t"
        }))
        .unwrap();
        assert_eq!(m.kind, MonitorKind::Ai);
        assert!(m.threshold.is_none());
    }
}
