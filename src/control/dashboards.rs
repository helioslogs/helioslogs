// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Dashboard wire shapes (storage/CRUD live on [`crate::control::Control`]); the
//! widget/layout config is an opaque `spec` blob the backend never inspects.

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Widget kinds the frontend can render (mirrors `WidgetKind` in
/// `frontend/src/api/types.ts`, minus the deprecated kinds).
pub const WIDGET_KINDS: &[&str] = &[
    "timeseries",
    "stat",
    "topn",
    "search_results",
    "alerts",
    "saved_searches",
];

/// Validate an LLM-authored `spec` (agent/MCP tools). The HTTP path keeps the
/// blob opaque — this only guards tool callers against hallucinated shapes
/// that would render as a broken dashboard.
pub fn validate_spec(spec: &Value) -> Result<()> {
    let obj = spec
        .as_object()
        .ok_or_else(|| anyhow!("spec must be a JSON object"))?;
    if !matches!(obj.get("time_range"), Some(Value::String(s)) if !s.is_empty()) {
        bail!("spec.time_range is required (relative range string like \"-24h\")");
    }
    let widgets = obj
        .get("widgets")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("spec.widgets must be an array"))?;
    for (i, w) in widgets.iter().enumerate() {
        let wo = w
            .as_object()
            .ok_or_else(|| anyhow!("spec.widgets[{i}] must be an object"))?;
        let kind = wo.get("kind").and_then(Value::as_str).unwrap_or("");
        if !WIDGET_KINDS.contains(&kind) {
            bail!("spec.widgets[{i}].kind '{kind}' is not one of {WIDGET_KINDS:?}");
        }
        if wo
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            bail!("spec.widgets[{i}].id is required (any unique string, e.g. \"w1\")");
        }
        if wo.get("title").and_then(Value::as_str).is_none() {
            bail!("spec.widgets[{i}].title is required (string; may be empty)");
        }
        let layout = wo.get("layout").and_then(Value::as_object).ok_or_else(|| {
            anyhow!("spec.widgets[{i}].layout is required: {{x,y,w,h}} in 12-column grid units")
        })?;
        for k in ["x", "y", "w", "h"] {
            if !layout.get(k).is_some_and(Value::is_number) {
                bail!("spec.widgets[{i}].layout.{k} must be a number");
            }
        }
    }
    Ok(())
}

/// A saved dashboard: typed metadata plus an opaque `spec`. User-owned
/// (+ optional public); not env-pinned — widgets follow the active env at view time.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Dashboard {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Widget + layout config, opaque to the backend: `{ time_range, refresh_secs, widgets:[…] }`.
    #[serde(default)]
    pub spec: Value,
    /// `true` = visible to all users and editable by anyone. `false` = owner-only.
    #[serde(default)]
    pub public: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Owner display label, set only in the admin "view all" listing. Never
    /// persisted (skipped when `None`, which it always is on the stored doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// Payload for `POST /api/dashboards`. Server fills in id + timestamps.
#[derive(Deserialize, Debug, Default)]
pub struct DashboardInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub spec: Value,
    /// Defaults to public when the client omits it (new dashboards are shared).
    #[serde(default = "default_true")]
    pub public: bool,
}

fn default_true() -> bool {
    true
}

/// Payload for `PATCH /api/dashboards/:id`. All fields optional — only those
/// present are applied.
#[derive(Deserialize, Debug, Default)]
pub struct DashboardPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub spec: Option<Value>,
    pub public: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn widget() -> Value {
        json!({
            "id": "w1",
            "kind": "timeseries",
            "title": "Errors",
            "layout": { "x": 0, "y": 0, "w": 6, "h": 8 },
            "series": [{ "id": "s1", "label": "errors", "query": "level:error", "color": "#ef4444" }],
        })
    }

    #[test]
    fn validate_spec_accepts_well_formed() {
        let spec = json!({ "time_range": "-24h", "widgets": [widget()] });
        assert!(validate_spec(&spec).is_ok());
        // Empty widget list is a valid (blank) dashboard.
        assert!(validate_spec(&json!({ "time_range": "-1h", "widgets": [] })).is_ok());
    }

    #[test]
    fn validate_spec_rejects_bad_shapes() {
        assert!(validate_spec(&json!("nope")).is_err());
        assert!(validate_spec(&json!({ "widgets": [] })).is_err()); // no time_range
        assert!(validate_spec(&json!({ "time_range": "-24h" })).is_err()); // no widgets

        let mut w = widget();
        w["kind"] = json!("piechart");
        assert!(validate_spec(&json!({ "time_range": "-24h", "widgets": [w] })).is_err());

        let mut w = widget();
        w.as_object_mut().unwrap().remove("id");
        assert!(validate_spec(&json!({ "time_range": "-24h", "widgets": [w] })).is_err());

        let mut w = widget();
        w["layout"] = json!({ "x": 0, "y": 0, "w": 6 }); // missing h
        assert!(validate_spec(&json!({ "time_range": "-24h", "widgets": [w] })).is_err());
    }
}
