// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Best-effort alert webhook delivery. Resolves the effective target (monitor
//! override, else global settings), builds the payload, POSTs with one retry.
//! Failures are logged to tracing (searchable in `_helioslogs`), never
//! propagated — alert creation must always succeed.

use crate::control::alerts::Alert;
use crate::control::monitors::NotifyOverride;
use crate::control::settings::{
    KEY_ALERT_WEBHOOK_ENABLED, KEY_ALERT_WEBHOOK_FORMAT, KEY_ALERT_WEBHOOK_URL,
};
use crate::control::Control;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WebhookFormat {
    #[default]
    Generic,
    Slack,
}

impl WebhookFormat {
    pub fn parse(s: &str) -> Self {
        if s.trim().eq_ignore_ascii_case("slack") {
            WebhookFormat::Slack
        } else {
            WebhookFormat::Generic
        }
    }
}

pub fn build_payload(format: WebhookFormat, alert: &Alert) -> Value {
    match format {
        WebhookFormat::Generic => json!({
            "event": "alert.created",
            "alert": alert,
        }),
        WebhookFormat::Slack => {
            let sev = alert.severity.to_uppercase();
            let mut text = format!("*[{sev}]* {}: {}", alert.monitor_name, alert.title);
            if !alert.summary.is_empty() {
                text.push('\n');
                text.push_str(&alert.summary);
            }
            json!({ "text": text })
        }
    }
}

/// Fire-and-forget dispatch; call after the alert is durably stored. No-op
/// when no tokio runtime is available (sync test contexts).
pub fn spawn_dispatch(control: Control, alert: Alert, override_target: Option<NotifyOverride>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        match resolve_target(&control, override_target).await {
            Ok(Some((url, format))) => deliver(&url, format, &alert).await,
            Ok(None) => {}
            Err(e) => tracing::warn!(alert_id = %alert.id, "alert webhook config: {e}"),
        }
    });
}

/// Monitor override wins; else the global settings target when enabled.
async fn resolve_target(
    control: &Control,
    override_target: Option<NotifyOverride>,
) -> anyhow::Result<Option<(String, WebhookFormat)>> {
    if let Some(o) = override_target {
        if !o.webhook_url.trim().is_empty() {
            let format = WebhookFormat::parse(o.format.as_deref().unwrap_or(""));
            return Ok(Some((o.webhook_url, format)));
        }
    }
    let enabled = control
        .get_setting(KEY_ALERT_WEBHOOK_ENABLED)
        .await?
        .map(|v| v == "true")
        .unwrap_or(false);
    if !enabled {
        return Ok(None);
    }
    let Some(url) = control.get_setting(KEY_ALERT_WEBHOOK_URL).await? else {
        return Ok(None);
    };
    if url.trim().is_empty() {
        return Ok(None);
    }
    let format = WebhookFormat::parse(
        &control
            .get_setting(KEY_ALERT_WEBHOOK_FORMAT)
            .await?
            .unwrap_or_default(),
    );
    Ok(Some((url, format)))
}

async fn deliver(url: &str, format: WebhookFormat, alert: &Alert) {
    let payload = build_payload(format, alert);
    for attempt in 0..2 {
        match send_webhook(url, &payload).await {
            Ok(status) if status.is_success() => return,
            Ok(status) => {
                tracing::warn!(alert_id = %alert.id, %status, attempt, "alert webhook rejected");
            }
            Err(e) => {
                tracing::warn!(alert_id = %alert.id, attempt, "alert webhook failed: {e}");
            }
        }
        if attempt == 0 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
}

/// Sends a test/synthetic payload; used by the admin "send test" endpoint.
pub async fn send_webhook(url: &str, payload: &Value) -> anyhow::Result<reqwest::StatusCode> {
    let parsed = crate::outbound::validate_outbound_url(url)?;
    let resp = crate::outbound::client()
        .post(parsed)
        .json(payload)
        .send()
        .await?;
    Ok(resp.status())
}

pub fn test_alert() -> Alert {
    Alert {
        id: "alt_test".into(),
        monitor_id: "mon_test".into(),
        monitor_name: "Test monitor".into(),
        env: "default".into(),
        conversation_id: None,
        severity: "low".into(),
        title: "Test alert from Helios".into(),
        summary: "This is a test delivery — your webhook is wired up.".into(),
        evidence: None,
        public: true,
        acknowledged: false,
        acknowledged_at: None,
        dismissed: false,
        created_at: chrono::Utc::now().timestamp_millis(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_parsing_defaults_to_generic() {
        assert_eq!(WebhookFormat::parse("slack"), WebhookFormat::Slack);
        assert_eq!(WebhookFormat::parse("SLACK"), WebhookFormat::Slack);
        assert_eq!(WebhookFormat::parse("generic"), WebhookFormat::Generic);
        assert_eq!(WebhookFormat::parse(""), WebhookFormat::Generic);
        assert_eq!(WebhookFormat::parse("bogus"), WebhookFormat::Generic);
    }

    #[test]
    fn generic_payload_carries_full_alert() {
        let a = test_alert();
        let p = build_payload(WebhookFormat::Generic, &a);
        assert_eq!(p["event"], "alert.created");
        assert_eq!(p["alert"]["id"], "alt_test");
        assert_eq!(p["alert"]["severity"], "low");
        assert_eq!(p["alert"]["monitor_name"], "Test monitor");
        assert!(p["alert"]["created_at"].is_i64());
    }

    #[test]
    fn slack_payload_text_formatting() {
        let mut a = test_alert();
        a.severity = "high".into();
        let p = build_payload(WebhookFormat::Slack, &a);
        let text = p["text"].as_str().unwrap();
        assert!(text.starts_with("*[HIGH]* Test monitor: Test alert from Helios"));
        assert!(text.contains("test delivery"));
        // Summary-less alerts get a single line.
        a.summary = String::new();
        let p2 = build_payload(WebhookFormat::Slack, &a);
        assert!(!p2["text"].as_str().unwrap().contains('\n'));
    }
}
