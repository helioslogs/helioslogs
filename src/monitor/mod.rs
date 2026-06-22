// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Background scheduler for agent monitors: each tick leases due monitors
//! (CAS, stale-lease stealing) and runs each as its own task in
//! [`ToolMode::MonitorRun`] (alerting enabled), draining the headless event stream.

use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::agent::{AgentEngine, AgentEvent, ToolMode};
use crate::catalog::Catalog;
use crate::control::alerts::AlertInput;
use crate::control::monitors::{Monitor, MonitorKind, ThresholdConfig};
use crate::control::Control;
use crate::schema::Fields;

pub mod prompt;

/// Scheduler poll cadence; finest user schedule is 5 min, so 10s never misses.
const TICK_INTERVAL: Duration = Duration::from_secs(10);

/// Recent alert titles surfaced to the agent for dedup (10 balances noise vs. memory).
const RECENT_ALERTS_FOR_CONTEXT: usize = 10;

/// Spawn the scheduler as a long-running task; never returns, and a failing
/// tick is logged so one bad monitor doesn't stop the rest.
pub async fn run_scheduler(catalog: Catalog, fields: Fields, control: Control) {
    info!(
        tick_interval_secs = TICK_INTERVAL.as_secs(),
        "monitor scheduler started"
    );
    let mut interval = tokio::time::interval(TICK_INTERVAL);
    // `Delay` (not `Burst`) so resuming after a pause doesn't fire a flurry
    // of back-to-back monitor runs.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if let Err(e) = tick(&catalog, &fields, &control).await {
            warn!("monitor scheduler tick failed: {e:#}");
        }
    }
}

async fn tick(catalog: &Catalog, fields: &Fields, control: &Control) -> Result<()> {
    let now_ms = Utc::now().timestamp_millis();
    // AI monitors are gated behind the global agent switch; load it once per tick.
    let agent_enabled = crate::agent::settings::AgentSettings::load(control)
        .await
        .map(|s| s.enabled)
        .unwrap_or(true);
    let monitors = control.monitor_list_all().await?;
    for (owner_user_id, m) in monitors {
        if !m.enabled {
            continue;
        }
        if m.kind == MonitorKind::Ai && !agent_enabled {
            continue;
        }
        if !is_due(&m, now_ms) {
            continue;
        }
        // `try_lease` is the source of truth: lose if another ticker beat
        // us; stale leases (crash mid-run) are stolen automatically.
        if !control.monitor_try_lease(&m.id).await? {
            continue;
        }

        let cat = catalog.clone();
        let fld = *fields;
        let ctl = control.clone();
        let owner = owner_user_id.clone();
        let mid = m.id.clone();
        tokio::spawn(async move {
            let result = run_monitor(&cat, &fld, &ctl, owner, m).await;
            let (status, error, conv_id) = match result {
                Ok(conv_id) => ("ok", None, conv_id),
                Err(e) => {
                    error!(monitor_id = mid, "monitor failed: {e:#}");
                    ("error", Some(format!("{e:#}")), None)
                }
            };
            if let Err(e) = ctl
                .monitor_finish_run(&mid, status, error.as_deref(), conv_id)
                .await
            {
                error!(monitor_id = mid, "finish_run failed: {e:#}");
            }
        });
    }
    Ok(())
}

fn is_due(m: &Monitor, now_ms: i64) -> bool {
    match m.last_run_at {
        None => true,
        Some(t) => (now_ms - t) >= m.interval_seconds * 1000,
    }
}

/// Execute one monitor run; AI monitors return their conversation id (for
/// trace linking), threshold monitors return `None`.
async fn run_monitor(
    catalog: &Catalog,
    fields: &Fields,
    control: &Control,
    owner_user_id: String,
    m: Monitor,
) -> Result<Option<String>> {
    match m.kind {
        MonitorKind::Threshold => {
            run_threshold_monitor(catalog, fields, control, &m).await?;
            Ok(None)
        }
        MonitorKind::Ai => run_ai_monitor(catalog, fields, control, owner_user_id, m)
            .await
            .map(Some),
    }
}

/// Count-threshold monitor; alerts only on the not-breaching → breaching edge
/// (no re-fire while it holds) and persists the new breaching state either way.
async fn run_threshold_monitor(
    catalog: &Catalog,
    fields: &Fields,
    control: &Control,
    m: &Monitor,
) -> Result<()> {
    let cfg = m
        .threshold
        .clone()
        .ok_or_else(|| anyhow::anyhow!("threshold monitor {} has no config", m.id))?;

    let end = Utc::now();
    let start = end - chrono::Duration::seconds(cfg.window_seconds);

    // Block-engine queries are sync + CPU-bound — keep them off the async
    // worker, mirroring how the agent tools offload via spawn_blocking.
    let (cat, fld, q, env, index) = (
        catalog.clone(),
        *fields,
        cfg.query.clone(),
        m.env.clone(),
        cfg.index.clone(),
    );
    let total = tokio::task::spawn_blocking(move || {
        crate::search::search(
            &cat,
            &fld,
            &q,
            Some(env.as_str()),
            index.as_deref(),
            Some(start),
            Some(end),
            0,
            1,
            &[],
            None,
        )
        .map(|r| r.total)
    })
    .await??;

    let count = total as i64;
    let breaching = cfg.comparison.breached(count, cfg.threshold);
    let was_breaching = m.last_breaching == Some(true);

    if breaching && !was_breaching {
        let (title, summary, evidence) = threshold_alert_content(m, &cfg, count);
        control
            .alert_create(AlertInput {
                monitor_id: m.id.clone(),
                conversation_id: None,
                severity: cfg.severity.clone(),
                title,
                summary,
                evidence: Some(evidence),
            })
            .await?;
    }

    control.monitor_set_breaching(&m.id, breaching).await?;
    Ok(())
}

/// Build the alert title/summary/evidence for a breached threshold monitor.
fn threshold_alert_content(
    m: &Monitor,
    cfg: &ThresholdConfig,
    count: i64,
) -> (String, String, serde_json::Value) {
    let window = format_window(cfg.window_seconds);
    let desc = cfg.comparison.describe();
    let title = format!("{}: {} matches in last {}", m.name, count, window);
    let where_idx = cfg
        .index
        .as_deref()
        .map(|i| format!(" in index `{i}`"))
        .unwrap_or_default();
    let summary = format!(
        "Query `{}`{} returned **{}** matches in the last {} — {} the threshold of **{}**.",
        cfg.query, where_idx, count, window, desc, cfg.threshold
    );
    let evidence = serde_json::json!({
        "count": count,
        "threshold": cfg.threshold,
        "comparison": cfg.comparison,
        "window_seconds": cfg.window_seconds,
        "query": cfg.query,
        "index": cfg.index,
    });
    (title, summary, evidence)
}

/// "15m" / "2h" / "1d" — compact human window label for alert text.
fn format_window(seconds: i64) -> String {
    if seconds < 3600 {
        format!("{}m", (seconds / 60).max(1))
    } else if seconds < 86400 {
        let h = seconds as f64 / 3600.0;
        if h.fract() == 0.0 {
            format!("{}h", h as i64)
        } else {
            format!("{h:.1}h")
        }
    } else {
        let d = seconds as f64 / 86400.0;
        if d.fract() == 0.0 {
            format!("{}d", d as i64)
        } else {
            format!("{d:.1}d")
        }
    }
}

/// AI monitor run: create the conversation, build the dedup-aware prompt,
/// Create the monitor's trace conversation (user-scoped, kind `monitor`).
pub async fn create_monitor_conversation(
    control: &Control,
    owner_user_id: &str,
    m: &Monitor,
) -> Result<String> {
    let title = format!("[monitor] {}", m.name);
    // Conversations are user-scoped (not env-scoped); the run still targets the
    // monitor's env via the agent's tool-call scoping below.
    let conv = control
        .conv_create_with_kind(owner_user_id, &title, "monitor")
        .await?;
    Ok(conv.id)
}

/// Run an AI monitor's agent loop into an existing conversation, streaming
/// events to `tx`. Shared by the scheduler (which drains `tx`) and the manual
/// "run & watch" endpoint (which forwards `tx` over SSE).
pub async fn run_ai_monitor_into(
    catalog: &Catalog,
    fields: &Fields,
    control: &Control,
    owner_user_id: &str,
    m: &Monitor,
    conv_id: String,
    tx: mpsc::Sender<AgentEvent>,
) -> Result<()> {
    let recent_titles = control
        .alert_recent_titles_for_monitor(owner_user_id, &m.id, RECENT_ALERTS_FOR_CONTEXT)
        .await?;
    let system_prompt = prompt::build(m, &recent_titles);

    let engine = AgentEngine {
        catalog: catalog.clone(),
        fields: *fields,
        control: control.clone(),
        demo_mode: false,
    };
    let mode = ToolMode::MonitorRun {
        monitor_id: m.id.clone(),
        user_id: owner_user_id.to_string(),
        conversation_id: conv_id.clone(),
    };
    let user_msg = format!(
        "Run this monitor and decide whether to raise an alert.\n\n[Monitor brief]\n{}",
        m.prompt
    );
    engine
        .run_turn_with_mode(
            mode,
            system_prompt,
            conv_id,
            owner_user_id,
            &m.env,
            &user_msg,
            None,
            tx,
        )
        .await
}

/// Scheduler entry point: create the conversation, drain the headless event
/// stream, and return the conversation id for trace linking.
async fn run_ai_monitor(
    catalog: &Catalog,
    fields: &Fields,
    control: &Control,
    owner_user_id: String,
    m: Monitor,
) -> Result<String> {
    let conv_id = create_monitor_conversation(control, &owner_user_id, &m).await?;

    // Drain agent events into the void — no SSE consumer for scheduled runs;
    // the agent loop still persists the trace to the conversation.
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    run_ai_monitor_into(
        catalog,
        fields,
        control,
        &owner_user_id,
        &m,
        conv_id.clone(),
        tx,
    )
    .await?;

    let _ = drain.await;
    Ok(conv_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::monitors::Comparison;

    fn monitor() -> Monitor {
        Monitor {
            id: "mon_1".into(),
            name: "5xx watch".into(),
            description: String::new(),
            prompt: String::new(),
            kind: MonitorKind::Threshold,
            threshold: None,
            notify: None,
            interval_seconds: 300,
            enabled: true,
            last_run_at: None,
            last_status: None,
            last_error: None,
            last_conversation_id: None,
            last_breaching: None,
            running: false,
            running_since: None,
            public: true,
            env: "prod".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
            owner: None,
        }
    }

    fn threshold_cfg() -> ThresholdConfig {
        ThresholdConfig {
            query: "status:>=500".into(),
            index: Some("web".into()),
            window_seconds: 900,
            comparison: Comparison::Gt,
            threshold: 50,
            severity: "high".into(),
        }
    }

    #[test]
    fn is_due_when_never_run() {
        assert!(is_due(&monitor(), 1_000_000));
    }

    #[test]
    fn is_due_respects_interval() {
        let mut m = monitor();
        m.interval_seconds = 300; // 300_000 ms
        m.last_run_at = Some(1_000_000);
        assert!(!is_due(&m, 1_000_000 + 299_000)); // not yet
        assert!(is_due(&m, 1_000_000 + 300_000)); // exactly due
        assert!(is_due(&m, 1_000_000 + 500_000)); // overdue
    }

    #[test]
    fn format_window_units() {
        assert_eq!(format_window(900), "15m");
        assert_eq!(format_window(60), "1m");
        assert_eq!(format_window(30), "1m"); // floors to at least 1m
        assert_eq!(format_window(7200), "2h");
        assert_eq!(format_window(5400), "1.5h"); // fractional hours
        assert_eq!(format_window(86400), "1d");
        assert_eq!(format_window(129600), "1.5d"); // fractional days
    }

    #[test]
    fn threshold_alert_content_shape() {
        let m = monitor();
        let cfg = threshold_cfg();
        let (title, summary, evidence) = threshold_alert_content(&m, &cfg, 296);
        assert_eq!(title, "5xx watch: 296 matches in last 15m");
        assert!(summary.contains("`status:>=500`"));
        assert!(summary.contains("in index `web`"));
        assert!(summary.contains("above the threshold of **50**"));
        assert_eq!(evidence["count"], 296);
        assert_eq!(evidence["threshold"], 50);
        assert_eq!(evidence["query"], "status:>=500");
    }

    #[test]
    fn threshold_alert_content_without_index() {
        let m = monitor();
        let mut cfg = threshold_cfg();
        cfg.index = None;
        let (_, summary, evidence) = threshold_alert_content(&m, &cfg, 10);
        assert!(!summary.contains("in index"));
        assert!(evidence["index"].is_null());
    }
}
