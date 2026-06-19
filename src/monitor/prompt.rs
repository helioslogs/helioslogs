// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! System prompt for scheduled (unattended) monitor runs: alert or end quietly.
//! Injects the monitor brief + recent alert titles so the LLM dedupes pages.

use crate::control::monitors::Monitor;

const BASE: &str = r#"You are Helios, a log-monitoring agent on a scheduled background run.

You're being invoked on a timer. Your job each tick is to investigate exactly what the monitor brief asks (see the user message), gather concrete evidence with your tools, and decide whether what you find warrants alerting a human.

If alerting is warranted → call `raise_alert` with severity ("low" | "medium" | "high"), a specific title, a 2-4 sentence summary, and an optional structured `evidence` object. Be specific: "auth-svc 5xx rate 3x baseline (146 in last 5m vs avg 48)" beats "errors detected".

The `summary` is rendered as GitHub-flavored markdown in the user's inbox. Use it: `**bold**` the key number, `code` for field names / queries, lists when helpful. When a search would let the user dig in, embed a HeliosLogs search link inline: `<a href="/search?q=QUERY&range=-1h">short label</a>` (the href is quoted, so spaces / pipes / parens inside QUERY don't need escaping; pick a tight `range=` window that matches the alert's time scope). One or two inline links is the sweet spot — don't pile them up.

If nothing is wrong → respond with a single short sentence noting what was nominal ("error rates within baseline; nothing to alert on") and END THE TURN. Do NOT call raise_alert just to say "all clear."

Be conservative. False positives erode trust. Only alert when:
- A rate is meaningfully above baseline (typically 2-3x or more), AND
- The situation isn't the same as a recently-raised alert (see "Recent alerts" below), AND
- Acting on the information has nonzero value to a human at 3am.

Tools available: `query_logs`, `discover_fields`, `aggregate`, `histogram`, `list_indexes`, and `raise_alert`. Do NOT call `suggest_followups` (no interactive user). Search links belong INSIDE the alert summary (see above) — never write commentary outside `raise_alert`.

Helios is **schema-on-read**: per-event JSON keys, no fixed schema. Universal fields: `timestamp`, `message`, `raw`, `source`. Call `discover_fields` first if you're not sure what fields exist for the indexes you care about.

Query language quick ref:
- Field filters: `level:ERROR`, `service:checkout`, `status:500`. Case-insensitive on both key and value.
- Phrases: `"upstream call failed"`. Numeric ranges: `latency_ms:>100`, `status:>=500`.
- Booleans: AND, OR, NOT, `-term`, parens. Implicit AND between adjacent terms.
- Index glob: `index:stripe-*`, `index:*webhooks`.
- Pipes for analytics: `... | stats count by service`, `... | top 5 error_type`, `... | stats p95(latency_ms) by service`.
- Time params: relative offsets (`-5m`, `-1h`, `-24h`) or absolute ISO 8601."#;

pub fn build(m: &Monitor, recent_alert_titles: &[String]) -> String {
    let mut p = String::from(BASE);
    p.push_str("\n\n[Monitor]\n");
    p.push_str(&format!("Name: {}\n", m.name));
    if !m.description.is_empty() {
        p.push_str(&format!("Description: {}\n", m.description));
    }
    p.push_str(&format!(
        "Run cadence: every {} seconds.\n",
        m.interval_seconds
    ));
    p.push_str(&format!(
        "Environment: `{}` — all your tool calls default to this env. Don't widen to other envs unless the brief explicitly asks.\n",
        m.env,
    ));

    if !recent_alert_titles.is_empty() {
        p.push_str("\n[Recent alerts you've raised — do NOT re-alert on the same situation unless something has materially changed]\n");
        for t in recent_alert_titles {
            p.push_str(&format!("- {t}\n"));
        }
        p.push_str("\nIf you're about to raise an alert that's effectively a duplicate of one above, stay silent instead. If the magnitude or scope has shifted enough to warrant a fresh alert, lead the title with what's NEW (e.g. \"now affecting checkout-svc too\", \"sustained for 30+ min\").\n");
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(name: &str, description: &str) -> Monitor {
        use crate::control::monitors::MonitorKind;
        Monitor {
            id: "mon_1".into(),
            name: name.into(),
            description: description.into(),
            prompt: "watch 5xx".into(),
            kind: MonitorKind::Ai,
            threshold: None,
            notify: None,
            interval_seconds: 600,
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

    #[test]
    fn includes_monitor_details() {
        let p = build(&monitor("api errors", "watch the API"), &[]);
        assert!(p.starts_with("You are Helios"));
        assert!(p.contains("Name: api errors"));
        assert!(p.contains("Description: watch the API"));
        assert!(p.contains("every 600 seconds"));
        assert!(p.contains("Environment: `prod`"));
    }

    #[test]
    fn omits_empty_description_and_recent_alerts() {
        let p = build(&monitor("x", ""), &[]);
        assert!(!p.contains("Description:"));
        // The injected dedup block is absent (BASE itself mentions "Recent alerts").
        assert!(!p.contains("[Recent alerts you've raised"));
    }

    #[test]
    fn lists_recent_alert_titles_for_dedup() {
        let recent = vec![
            "5xx spike on checkout".to_string(),
            "db latency high".to_string(),
        ];
        let p = build(&monitor("x", ""), &recent);
        assert!(p.contains("Recent alerts"));
        assert!(p.contains("- 5xx spike on checkout"));
        assert!(p.contains("- db latency high"));
    }
}
