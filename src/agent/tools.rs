// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Tool catalog + dispatcher the agent exposes to the LLM (each tool maps 1:1 to a
//! `crate::search`/`crate::catalog` fn on `spawn_blocking`). The toolbelt differs by
//! [`ToolMode`]: chat gets monitor-management tools, a monitor run gets `raise_alert`.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use tokio::task;

use crate::catalog::Catalog;
use crate::control::alerts::AlertInput;
use crate::control::dashboards::{DashboardInput, DashboardPatch};
use crate::control::monitors::{MonitorPatch, DEFAULT_INTERVAL_SECONDS, MIN_INTERVAL_SECONDS};
use crate::control::Control;
use crate::llm::LlmToolDef;
use crate::schema::Fields;

/// What the agent is asked to do this turn — drives the advertised tool
/// surface and what alert/monitor tools may do.
#[derive(Clone, Debug)]
pub enum ToolMode {
    /// Browser-driven user chat; `user_id` scopes monitor-management tools.
    InteractiveChat { user_id: String },
    /// Background scheduled run; ids let `raise_alert` attach to the monitor.
    MonitorRun {
        monitor_id: String,
        user_id: String,
        conversation_id: String,
    },
}

/// Everything a tool needs: data plane (catalog + fields), control plane,
/// run mode, and the conversation's pinned env (default when `env` is omitted).
#[derive(Clone)]
pub struct ToolContext {
    pub catalog: Catalog,
    pub fields: Fields,
    pub control: Control,
    pub mode: ToolMode,
    pub env: String,
    /// Read-only demo instance: write tools are hidden from the model and refused
    /// if called anyway. See [`is_demo_write_tool`].
    pub demo_mode: bool,
}

/// Tools that mutate control-plane / data state — withheld from the demo account
/// in demo mode (other users keep them). `raise_alert` is intentionally absent: it
/// only fires from background monitor runs, not user-driven chat. Keep in sync with
/// the write dispatch arms below.
pub fn is_demo_write_tool(name: &str) -> bool {
    matches!(
        name,
        "ingest_events"
            | "create_monitor"
            | "update_monitor"
            | "delete_monitor"
            | "set_monitor_enabled"
            | "run_monitor_now"
            | "acknowledge_alert"
            | "create_dashboard"
            | "update_dashboard"
    )
}

/// Tool descriptors advertised to the model; search-tool descriptions mirror
/// the MCP server's (`crate::mcp::tools`) so behavior is identical either way.
pub fn tool_defs(mode: &ToolMode) -> Vec<LlmToolDef> {
    let mut tools = base_tools();
    match mode {
        ToolMode::InteractiveChat { .. } => {
            tools.extend(monitor_management_tools());
            tools.extend(dashboard_tools());
            tools.extend(alert_tools());
        }
        ToolMode::MonitorRun { .. } => {
            tools.push(raise_alert_def());
        }
    }
    tools
}

fn base_tools() -> Vec<LlmToolDef> {
    vec![
        LlmToolDef {
            name: "query_logs".into(),
            description: "Search log events. Helios is schema-on-read: any JSON key present in the ingested events is queryable as `<key>:value` — there is no fixed schema. Supports pipe operators for analytics (e.g. 'level:error | stats count by service'). Returns hit metadata plus the first N events; each event's full original JSON is in `event`. For analytics queries with pipes, returns a `table` field instead of individual hits.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string", "description": "Query. Use '*' for everything. Field names are whatever keys the events contain — call `discover_fields` first if unsure. Examples: 'level:ERROR', 'status:500', 'severity:error | stats count by service', '* | stats p95(latency_ms) by service'." },
                    "env":   { "type": "string", "description": "Override the conversation's active env (see system prompt). Omit to use the active env. Use `list_environments` to discover names. Cross-env queries are intentionally rare." },
                    "index": { "type": "string", "description": "Optional index glob (e.g. 'stripe-*', '*webhooks') filtering the partitions to scan. Omit to scatter-gather across all indexes in the active env." },
                    "start": { "type": "string", "description": "Start time. ISO 8601, unix ms, or relative ('-1h', '-24h', '-7d'). Default '-6h'." },
                    "end":   { "type": "string", "description": "End time. ISO 8601, unix ms, 'now', or relative. Default 'now'." },
                    "limit": { "type": "number", "description": "Max hits to return (1-50). Default 20. Ignored for pipe queries." },
                },
                "required": ["q"],
            }),
        },
        LlmToolDef {
            name: "discover_fields".into(),
            description: "Sample the current result set and return the JSON keys present in the matching events, ranked by coverage × cardinality. Use BEFORE running aggregations or pipe queries to learn what fields exist — Helios has no fixed schema, so field names are per-event.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string", "description": "Query to scope the sample. Default '*'." },
                    "env":   { "type": "string", "description": "Override active env. See `query_logs.env`." },
                    "index": { "type": "string", "description": "Optional index glob." },
                    "start": { "type": "string", "description": "Default '-6h'." },
                    "end":   { "type": "string", "description": "Default 'now'." },
                    "top":   { "type": "number", "description": "Max fields to return (1-100). Default 20." },
                },
            }),
        },
        LlmToolDef {
            name: "histogram".into(),
            description: "Event counts bucketed over time. Use to see when things happened — spikes flag incidents, gaps flag ingestion problems.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string", "description": "Query. Default '*'." },
                    "env":   { "type": "string", "description": "Override active env. See `query_logs.env`." },
                    "index": { "type": "string", "description": "Optional index glob." },
                    "start": { "type": "string", "description": "Default '-6h'." },
                    "end":   { "type": "string", "description": "Default 'now'." },
                    "interval": { "type": "string", "description": "Bucket size (1m, 5m, 1h, 1d). Auto-picked if omitted." },
                },
            }),
        },
        LlmToolDef {
            name: "aggregate".into(),
            description: "Top values per field (terms aggregation). Answers 'which services have the most errors?', 'which hosts are noisy?'. Field names are arbitrary JSON keys from the events — call `discover_fields` first if you don't know what's available; it returns a `groupable` flag identifying fields safe to aggregate on. NOT aggregatable: `message` and `raw` (text, not fast-columnar) and `timestamp` (use the `histogram` tool instead). The synthetic field `index` returns counts per partition.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string", "description": "Query to scope the aggregation. Default '*'." },
                    "env":   { "type": "string", "description": "Override active env. See `query_logs.env`." },
                    "index": { "type": "string", "description": "Optional index glob." },
                    "start": { "type": "string", "description": "Default '-6h'." },
                    "end":   { "type": "string", "description": "Default 'now'." },
                    "fields": { "type": "string", "description": "Comma-separated field names. Use `index` for partition-key counts, any other name for terms-agg on `dynamic.<name>`. Required." },
                    "size":   { "type": "number", "description": "Top N per field. Default 10." },
                },
                "required": ["fields"],
            }),
        },
        LlmToolDef {
            name: "list_indexes".into(),
            description: "Enumerate every index currently in the catalog. Use when the user references an index name to confirm it exists, or to discover what's available.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        LlmToolDef {
            name: "list_environments".into(),
            description: "List the environments (tenancy boundaries) registered in this HeliosLogs instance — `default` plus any admin-defined envs like `dev`, `test`, `prod`. Use this when the user asks to compare envs, or before scoping a tool call to a non-default env. Search / aggregate / histogram tools accept `env: \"<name>\"` to override the conversation's active env (visible in the system prompt).".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        LlmToolDef {
            name: "suggest_followups".into(),
            description: "Offer the user 2-4 clickable follow-up prompts. Two use cases: (1) POST-ANALYSIS: after you've delivered findings, call this with natural next investigations. (2) CLARIFY MID-ANALYSIS: when you need a decision from the user before continuing, state the question in your assistant text and put the answer options in `prompts`. Calling this tool ends your turn. Write prompts from the user's perspective, short (5-12 words), specific.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "2-4 follow-up prompts; each becomes a clickable button.",
                    },
                    "label": {
                        "type": "string",
                        "description": "Optional short heading shown above the buttons. Defaults to 'Try next'.",
                    },
                },
                "required": ["prompts"],
            }),
        },
    ]
}

fn raise_alert_def() -> LlmToolDef {
    LlmToolDef {
        name: "raise_alert".into(),
        description: "Record a finding from this monitor run as an alert in the user's inbox. ONLY call this when you've found something worth surfacing to a human — anomalous error rate, novel error pattern, ingestion gap, service-level outage, etc. Do NOT raise an alert just because errors exist; baseline rates of errors are normal. The user will see the title in their inbox and can click through to this conversation to see the evidence. If you've already raised an alert about this exact situation in a recent run (recent alerts are listed in your system prompt), DO NOT raise it again unless the situation has materially changed.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "severity": {
                    "type": "string",
                    "enum": ["low", "medium", "high"],
                    "description": "low = noteworthy but not urgent; medium = should be looked at today; high = page someone now (production-impacting)."
                },
                "title": {
                    "type": "string",
                    "description": "One-line headline (10-80 chars). Specific enough to be actionable: 'auth-svc 5xx rate 3x baseline' not 'errors detected'."
                },
                "summary": {
                    "type": "string",
                    "description": "2-4 sentence explanation: what changed, when, the magnitude, the most likely cause if you have one. **Renders as GitHub-flavored markdown** — use `**bold**` for the key number, `code` for field names/queries, lists if helpful. When a search would let the user dig in, embed an HTML anchor with a HeliosLogs search URL: <a href=\"/search?q=QUERY&range=-1h\">short label</a>. The query goes only in the href (quotes can hold spaces, pipes, parens — no escaping needed); use a short human label. Time window: &range=-1h/-24h/-7d for relative, or &start=ISO&end=ISO for absolute. Add &index=NAME to scope. One or two links inline is good; don't pile them up."
                },
                "evidence": {
                    "type": "object",
                    "description": "Optional structured evidence — counts, percentages, top offenders, time windows. Free-form JSON; will be shown as a small key/value block under the summary."
                }
            },
            "required": ["severity", "title", "summary"]
        }),
    }
}

fn monitor_management_tools() -> Vec<LlmToolDef> {
    vec![
        LlmToolDef {
            name: "create_monitor".into(),
            description: "Propose a new scheduled agent monitor for the user. This does NOT create it directly — it returns a draft that the user has to confirm with a click in the UI. Use when the user asks to 'watch for X' or 'alert me when Y'. The `prompt` field is the instruction that will be handed to the agent every interval; write it as a self-contained investigation brief ('Every 5 minutes, check error rates in orders-api vs the prior hour baseline. Raise an alert if anything looks anomalous.'). Default interval: 30 minutes; minimum: 5 minutes.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name":             { "type": "string", "description": "Short human label (1-60 chars). Becomes the row label in /saved." },
                    "description":      { "type": "string", "description": "Optional longer description shown on hover." },
                    "prompt":           { "type": "string", "description": "The agent instruction. Be specific about what to check, the comparison baseline, and the alert criteria." },
                    "interval_seconds": { "type": "number", "description": "How often to run, in seconds. Default 1800 (30 min); minimum 300 (5 min)." }
                },
                "required": ["name", "prompt"]
            }),
        },
        LlmToolDef {
            name: "list_monitors".into(),
            description: "List the user's scheduled monitors with status (enabled, last run time, last status).".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        LlmToolDef {
            name: "update_monitor".into(),
            description: "Edit an existing monitor by id (from `list_monitors`). Only the fields you pass change; omit the rest. Use to retune the schedule, rewrite an AI monitor's `prompt`, pause/resume it (`enabled`), or change visibility (`public`). Unlike `create_monitor` this applies immediately — confirm a non-trivial change with the user first. Interval floor is 5 minutes (300s).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id":               { "type": "string", "description": "Monitor id (mon_…)." },
                    "name":             { "type": "string", "description": "New short label." },
                    "description":      { "type": "string", "description": "New description." },
                    "prompt":           { "type": "string", "description": "New agent instruction (AI monitors)." },
                    "interval_seconds": { "type": "number", "description": "New run interval in seconds (minimum 300)." },
                    "enabled":          { "type": "boolean", "description": "false = pause, true = resume." },
                    "public":           { "type": "boolean", "description": "Alert visibility: true = all users, false = owner-only." }
                },
                "required": ["id"]
            }),
        },
        LlmToolDef {
            name: "delete_monitor".into(),
            description: "Delete a monitor by id. Irreversible — confirm with the user before calling.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "Monitor id (mon_…)." } },
                "required": ["id"]
            }),
        },
        LlmToolDef {
            name: "set_monitor_enabled".into(),
            description: "Pause (enabled=false) or resume (enabled=true) a monitor without deleting it.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id":      { "type": "string", "description": "Monitor id." },
                    "enabled": { "type": "boolean" }
                },
                "required": ["id", "enabled"]
            }),
        },
        LlmToolDef {
            name: "run_monitor_now".into(),
            description: "Trigger a monitor to run immediately (in the background) without waiting for its next scheduled tick. Useful for testing a freshly-created monitor's prompt.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        },
    ]
}

fn alert_tools() -> Vec<LlmToolDef> {
    vec![
        LlmToolDef {
            name: "list_alerts".into(),
            description: "List the alerts visible to the user (their own + public ones), newest first — both monitor-raised and manually created. Returns id, severity, title, summary, source (monitor name or `agent`/`mcp`), env, acknowledged state, and timestamps. Use when the user asks what's in their inbox, what fired recently, or before acknowledging.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "unacked_only": { "type": "boolean", "description": "true = inbox view (unacknowledged only). Default false." },
                    "search":  { "type": "string", "description": "Case-insensitive substring match over title / summary / source / env." },
                    "monitor": { "type": "string", "description": "Restrict to alerts raised by one monitor id (mon_…)." },
                    "limit":   { "type": "number", "description": "Max alerts to return (1-200). Default 50." },
                },
            }),
        },
        LlmToolDef {
            name: "acknowledge_alert".into(),
            description: "Mark an alert acknowledged, clearing it from the inbox. Acknowledgement is shared — acking a public alert clears it for every user — so only ack when the user asked to, or after they've confirmed the finding is handled.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Alert id (alt_…), from `list_alerts`." },
                },
                "required": ["id"],
            }),
        },
    ]
}

/// Dashboard `spec` schema, spliced into the create/update tool descriptions.
/// Mirrors `DashboardSpec` in `frontend/src/api/types.ts` and the MCP twin.
const DASHBOARD_SPEC_DOC: &str = "\
SPEC FORMAT: { \"time_range\": \"-24h\", \"refresh_secs\": 0, \"widgets\": [...] } — \
`time_range` is the default relative window for every widget; `refresh_secs` 0/omitted = manual refresh.\n\
Each widget: { \"id\": \"w1\" (any unique string), \"kind\": ..., \"title\": ..., \
\"layout\": {\"x\":0,\"y\":0,\"w\":6,\"h\":8} } — 12-column grid; typical sizes: chart 6x8, \
stat 3x4, full-width table 12x8. Kinds + their extra fields:\n\
- `timeseries` — counts over time. `series`: 1-4 of {\"id\",\"label\",\"query\",\"color\" (hex)}; \
optional `chart`: line|bar|area.\n\
- `stat` — one big number per series (total match count). `series` as above.\n\
- `topn` — top values of `field` among events matching `series[0].query`; optional `size` (rows).\n\
- `search_results` — live table of events matching `series[0].query`; optional `limit` (rows).\n\
- `alerts` / `saved_searches` — recent alerts / saved searches; optional `limit`. No series.\n\
Series queries are normal HeliosLogs searches incl. pipes (scope with `index:foo` INSIDE the query \
string — there is no separate index param). Run `discover_fields` / test queries with `query_logs` \
BEFORE wiring them into widgets, so the dashboard isn't built on guessed field names. Optional \
per-widget `time`: {\"range\":\"-1h\"} overrides the dashboard window. Dashboards are NOT \
env-scoped: widgets run in the viewer's active env at render time.";

fn dashboard_tools() -> Vec<LlmToolDef> {
    vec![
        LlmToolDef {
            name: "list_dashboards".into(),
            description: "List the dashboards visible to the user (their own + public ones): id, name, description, owner, visibility, updated_at, widget count. Call `get_dashboard` for the full widget spec. Check here before creating a near-duplicate dashboard.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        LlmToolDef {
            name: "get_dashboard".into(),
            description: "Fetch one dashboard's full definition including its `spec` (widgets, layout, time range). Always call this before `update_dashboard` — updates replace the spec wholesale.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Dashboard id (dash_…), from `list_dashboards`." },
                },
                "required": ["id"],
            }),
        },
        LlmToolDef {
            name: "create_dashboard".into(),
            description: format!("Create a dashboard for the user. This creates it immediately — no draft step — so confirm the widgets they want first if the request is vague. Returns the id and UI path; link it in your reply as [name](/dashboards/<id>).\n\n{DASHBOARD_SPEC_DOC}"),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name":        { "type": "string", "description": "Display name (required, non-empty)." },
                    "description": { "type": "string", "description": "Optional longer description." },
                    "spec":        { "type": "object", "description": "Widget + layout config; see SPEC FORMAT in the tool description. Default: empty 24h dashboard." },
                    "public":      { "type": "boolean", "description": "true (default) = visible to and editable by every user; false = private to this user." },
                },
                "required": ["name"],
            }),
        },
        LlmToolDef {
            name: "update_dashboard".into(),
            description: format!("Update a dashboard's name, description, spec, or visibility. Only the fields you pass change, but `spec` is replaced WHOLESALE — call `get_dashboard`, edit the returned spec, and send the complete result (never a partial widget list). Editable: the user's own dashboards and public ones.\n\n{DASHBOARD_SPEC_DOC}"),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id":          { "type": "string", "description": "Dashboard id (dash_…). Required." },
                    "name":        { "type": "string" },
                    "description": { "type": "string" },
                    "spec":        { "type": "object", "description": "Complete replacement spec; see SPEC FORMAT in the tool description." },
                    "public":      { "type": "boolean" },
                },
                "required": ["id"],
            }),
        },
    ]
}

/// `true` when calling this tool ends the assistant's turn (i.e. the agent
/// loop should not iterate further). Currently only `suggest_followups`.
pub fn is_terminal(tool_name: &str) -> bool {
    tool_name == "suggest_followups"
}

/// Execute a tool by name; engine queries run on `spawn_blocking`. Returns the
/// JSON payload fed back as the next `role: "tool"` message content.
pub async fn execute(ctx: &ToolContext, name: &str, args: &Value) -> Result<Value> {
    // Defense-in-depth: write tools are already filtered out of the catalog in
    // demo mode (see `run_turn_with_mode`), but refuse them here too in case the
    // model hallucinates the name. The error is fed back as a tool result.
    if ctx.demo_mode && is_demo_write_tool(name) {
        return Err(anyhow!(
            "'{name}' is disabled on this read-only demo instance"
        ));
    }
    // Cheap UI-signal tool — no IO, no blocking work.
    if name == "suggest_followups" {
        return Ok(suggest_followups(args));
    }
    if name == "raise_alert" {
        return raise_alert(ctx, args).await;
    }
    // Control-plane tools — cheap async control-store ops, run inline rather
    // than through the spawn_blocking path used for engine queries.
    match name {
        "create_monitor" => return create_monitor(ctx, args),
        "list_monitors" => return list_monitors(ctx).await,
        "update_monitor" => return update_monitor(ctx, args).await,
        "delete_monitor" => return delete_monitor(ctx, args).await,
        "set_monitor_enabled" => return set_monitor_enabled(ctx, args).await,
        "run_monitor_now" => return run_monitor_now(ctx, args).await,
        "list_environments" => return list_environments(ctx).await,
        "list_dashboards" => return list_dashboards(ctx).await,
        "get_dashboard" => return get_dashboard(ctx, args).await,
        "create_dashboard" => return create_dashboard(ctx, args).await,
        "update_dashboard" => return update_dashboard(ctx, args).await,
        "list_alerts" => return list_alerts(ctx, args).await,
        "acknowledge_alert" => return acknowledge_alert(ctx, args).await,
        _ => {}
    }

    let name = name.to_string();
    let args = args.clone();
    let catalog = ctx.catalog.clone();
    let fields = ctx.fields;
    let ctx_env = ctx.env.clone();
    task::spawn_blocking(move || run_blocking(&catalog, &fields, &ctx_env, &name, &args))
        .await
        .map_err(|e| anyhow!("tool task panicked: {e}"))?
}

fn run_blocking(
    catalog: &Catalog,
    fields: &Fields,
    ctx_env: &str,
    name: &str,
    args: &Value,
) -> Result<Value> {
    match name {
        "query_logs" => query_logs(catalog, fields, ctx_env, args),
        "discover_fields" => discover_fields(catalog, fields, ctx_env, args),
        "histogram" => histogram(catalog, fields, ctx_env, args),
        "aggregate" => aggregate(catalog, fields, ctx_env, args),
        "list_indexes" => list_indexes(catalog),
        other => Err(anyhow!("unknown tool: {other}")),
    }
}

async fn list_environments(ctx: &ToolContext) -> Result<Value> {
    let envs = ctx.control.list_envs(false).await?;
    Ok(json!({ "environments": envs }))
}

// ----------------- monitor / alert tools ---------------------------------

async fn raise_alert(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let (monitor_id, conv_id) = match &ctx.mode {
        ToolMode::MonitorRun {
            monitor_id,
            conversation_id,
            ..
        } => (monitor_id.clone(), conversation_id.clone()),
        ToolMode::InteractiveChat { .. } => {
            return Err(anyhow!(
                "raise_alert is only available inside a scheduled monitor run"
            ));
        }
    };
    let severity = arg_str(args, "severity").unwrap_or_else(|| "medium".to_string());
    let title = arg_str(args, "title").ok_or_else(|| anyhow!("title is required"))?;
    let summary = arg_str(args, "summary").unwrap_or_default();
    let evidence = args.get("evidence").cloned();

    let alert = ctx
        .control
        .alert_create(AlertInput {
            monitor_id,
            conversation_id: Some(conv_id),
            severity,
            title,
            summary,
            evidence,
        })
        .await?;
    Ok(json!({
        "ok": true,
        "alert_id": alert.id,
        "severity": alert.severity,
        "title": alert.title,
    }))
}

fn create_monitor(_ctx: &ToolContext, args: &Value) -> Result<Value> {
    // Drafts only — actual creation goes through the UI's Create button
    // (POST /api/monitors); we just return the proposed shape.
    let name = arg_str(args, "name").ok_or_else(|| anyhow!("name is required"))?;
    let prompt = arg_str(args, "prompt").ok_or_else(|| anyhow!("prompt is required"))?;
    let description = arg_str(args, "description").unwrap_or_default();
    let interval =
        arg_i64(args, "interval_seconds", DEFAULT_INTERVAL_SECONDS).max(MIN_INTERVAL_SECONDS);
    Ok(json!({
        "status": "draft",
        "monitor": {
            "name": name,
            "description": description,
            "prompt": prompt,
            "interval_seconds": interval,
        },
        "note": "This is a draft. The user must click Create in the UI to actually schedule it."
    }))
}

fn user_id_for_ctx(ctx: &ToolContext) -> Result<&str> {
    match &ctx.mode {
        ToolMode::InteractiveChat { user_id } => Ok(user_id),
        ToolMode::MonitorRun { user_id, .. } => Ok(user_id),
    }
}

async fn list_monitors(ctx: &ToolContext) -> Result<Value> {
    let uid = user_id_for_ctx(ctx)?;
    // Monitors are not env-scoped — list all of the user's monitors.
    let items = ctx.control.monitor_list(uid).await?;
    let summaries: Vec<Value> = items
        .into_iter()
        .map(|m| {
            json!({
                "id": m.id,
                "name": m.name,
                "description": m.description,
                "interval_seconds": m.interval_seconds,
                "enabled": m.enabled,
                "last_run_at": m.last_run_at,
                "last_status": m.last_status,
            })
        })
        .collect();
    Ok(json!({ "monitors": summaries }))
}

async fn update_monitor(ctx: &ToolContext, args: &Value) -> Result<Value> {
    if !matches!(ctx.mode, ToolMode::InteractiveChat { .. }) {
        return Err(anyhow!(
            "update_monitor is only available in interactive chat"
        ));
    }
    let uid = user_id_for_ctx(ctx)?;
    let id = arg_str(args, "id").ok_or_else(|| anyhow!("id is required"))?;
    let patch = MonitorPatch {
        name: arg_str(args, "name"),
        // Raw read so an explicit "" clears the description.
        description: args
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt: arg_str(args, "prompt"),
        interval_seconds: args.get("interval_seconds").and_then(Value::as_i64),
        enabled: args.get("enabled").and_then(Value::as_bool),
        public: args.get("public").and_then(Value::as_bool),
        ..Default::default()
    };
    let m = ctx.control.monitor_update(uid, &id, patch, false).await?;
    Ok(json!({
        "ok": true,
        "id": m.id,
        "name": m.name,
        "enabled": m.enabled,
        "interval_seconds": m.interval_seconds,
    }))
}

async fn delete_monitor(ctx: &ToolContext, args: &Value) -> Result<Value> {
    if !matches!(ctx.mode, ToolMode::InteractiveChat { .. }) {
        return Err(anyhow!(
            "delete_monitor is only available in interactive chat"
        ));
    }
    let uid = user_id_for_ctx(ctx)?;
    let id = arg_str(args, "id").ok_or_else(|| anyhow!("id is required"))?;
    ctx.control.monitor_delete(uid, &id, false).await?;
    Ok(json!({ "ok": true, "deleted": id }))
}

async fn set_monitor_enabled(ctx: &ToolContext, args: &Value) -> Result<Value> {
    if !matches!(ctx.mode, ToolMode::InteractiveChat { .. }) {
        return Err(anyhow!(
            "set_monitor_enabled is only available in interactive chat"
        ));
    }
    let uid = user_id_for_ctx(ctx)?;
    let id = arg_str(args, "id").ok_or_else(|| anyhow!("id is required"))?;
    let enabled = args
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("enabled is required (boolean)"))?;
    let updated = ctx
        .control
        .monitor_update(
            uid,
            &id,
            MonitorPatch {
                enabled: Some(enabled),
                ..Default::default()
            },
            false,
        )
        .await?;
    Ok(json!({
        "ok": true,
        "id": updated.id,
        "enabled": updated.enabled,
    }))
}

async fn run_monitor_now(ctx: &ToolContext, args: &Value) -> Result<Value> {
    // Fire-and-forget against the scheduler — clear `last_run_at` so the next
    // tick (within 10s) picks it up immediately.
    if !matches!(ctx.mode, ToolMode::InteractiveChat { .. }) {
        return Err(anyhow!(
            "run_monitor_now is only available in interactive chat"
        ));
    }
    let uid = user_id_for_ctx(ctx)?;
    let id = arg_str(args, "id").ok_or_else(|| anyhow!("id is required"))?;
    // Verify ownership first (clean error if not theirs).
    ctx.control
        .monitor_get(uid, &id, false)
        .await?
        .ok_or_else(|| anyhow!("monitor {id} not found"))?;
    ctx.control.monitor_clear_last_run(&id).await?;
    Ok(json!({
        "ok": true,
        "id": id,
        "message": "Will run on the next scheduler tick (within 10s)."
    }))
}

// ----------------- dashboard tools ----------------------------------------

async fn list_dashboards(ctx: &ToolContext) -> Result<Value> {
    let uid = user_id_for_ctx(ctx)?;
    // Dashboards are not env-scoped — own + public rows across the instance.
    let items = ctx.control.dashboard_list(uid).await?;
    let summaries: Vec<Value> = items
        .iter()
        .map(|d| {
            let widget_count = d
                .spec
                .get("widgets")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            json!({
                "id": d.id,
                "name": d.name,
                "description": d.description,
                "owner": d.owner,
                "public": d.public,
                "updated_at": d.updated_at,
                "widget_count": widget_count,
                "url": format!("/dashboards/{}", d.id),
            })
        })
        .collect();
    Ok(json!({ "dashboards": summaries }))
}

async fn get_dashboard(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let uid = user_id_for_ctx(ctx)?;
    let id = arg_str(args, "id").ok_or_else(|| anyhow!("id is required"))?;
    let d = ctx.control.dashboard_get(uid, &id, false).await?;
    let mut v = serde_json::to_value(&d)?;
    v["url"] = json!(format!("/dashboards/{}", d.id));
    Ok(v)
}

async fn create_dashboard(ctx: &ToolContext, args: &Value) -> Result<Value> {
    if !matches!(ctx.mode, ToolMode::InteractiveChat { .. }) {
        return Err(anyhow!(
            "create_dashboard is only available in interactive chat"
        ));
    }
    let uid = user_id_for_ctx(ctx)?;
    let name = arg_str(args, "name").ok_or_else(|| anyhow!("name is required"))?;
    let spec = match args.get("spec") {
        Some(s) if !s.is_null() => {
            crate::control::dashboards::validate_spec(s)?;
            s.clone()
        }
        _ => json!({ "time_range": "-24h", "widgets": [] }),
    };
    let input = DashboardInput {
        name,
        description: arg_str(args, "description").unwrap_or_default(),
        spec,
        public: args.get("public").and_then(Value::as_bool).unwrap_or(true),
    };
    let d = ctx.control.dashboard_create(uid, input).await?;
    Ok(json!({
        "ok": true,
        "id": d.id,
        "name": d.name,
        "public": d.public,
        "url": format!("/dashboards/{}", d.id),
    }))
}

async fn update_dashboard(ctx: &ToolContext, args: &Value) -> Result<Value> {
    if !matches!(ctx.mode, ToolMode::InteractiveChat { .. }) {
        return Err(anyhow!(
            "update_dashboard is only available in interactive chat"
        ));
    }
    let uid = user_id_for_ctx(ctx)?;
    let id = arg_str(args, "id").ok_or_else(|| anyhow!("id is required"))?;
    let spec = match args.get("spec") {
        Some(s) if !s.is_null() => {
            crate::control::dashboards::validate_spec(s)?;
            Some(s.clone())
        }
        _ => None,
    };
    let patch = DashboardPatch {
        name: arg_str(args, "name"),
        // Raw read (not `arg_str`) so an explicit "" clears the description.
        description: args
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        spec,
        public: args.get("public").and_then(Value::as_bool),
    };
    let d = ctx.control.dashboard_update(uid, &id, patch, false).await?;
    Ok(json!({
        "ok": true,
        "id": d.id,
        "name": d.name,
        "public": d.public,
        "updated_at": d.updated_at,
        "url": format!("/dashboards/{}", d.id),
    }))
}

// ----------------- alert inbox tools ---------------------------------------

fn ms_to_iso(ms: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(ms).map(|t| t.to_rfc3339())
}

async fn list_alerts(ctx: &ToolContext, args: &Value) -> Result<Value> {
    let uid = user_id_for_ctx(ctx)?;
    let unacked_only = args
        .get("unacked_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let search = arg_str(args, "search");
    let monitor = arg_str(args, "monitor");
    let limit = arg_usize(args, "limit", 50).clamp(1, 200);
    let items = ctx
        .control
        .alert_list(
            uid,
            unacked_only,
            monitor.as_deref(),
            search.as_deref(),
            limit,
        )
        .await?;
    let rows: Vec<Value> = items
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "severity": a.severity,
                "title": a.title,
                "summary": a.summary,
                "evidence": a.evidence,
                "source": a.monitor_name,
                "monitor_id": if a.monitor_id.is_empty() { Value::Null } else { json!(a.monitor_id) },
                "env": a.env,
                "public": a.public,
                "acknowledged": a.acknowledged,
                "acknowledged_at": a.acknowledged_at.and_then(ms_to_iso),
                "created_at": ms_to_iso(a.created_at),
            })
        })
        .collect();
    Ok(json!({ "alerts": rows }))
}

async fn acknowledge_alert(ctx: &ToolContext, args: &Value) -> Result<Value> {
    if !matches!(ctx.mode, ToolMode::InteractiveChat { .. }) {
        return Err(anyhow!(
            "acknowledge_alert is only available in interactive chat"
        ));
    }
    let uid = user_id_for_ctx(ctx)?;
    let id = arg_str(args, "id").ok_or_else(|| anyhow!("id is required"))?;
    if !ctx.control.alert_acknowledge(uid, &id).await? {
        return Err(anyhow!("alert {id} not found"));
    }
    Ok(json!({ "ok": true, "id": id, "acknowledged": true }))
}

// ----------------- search tools ------------------------------------------

fn query_logs(catalog: &Catalog, fields: &Fields, ctx_env: &str, args: &Value) -> Result<Value> {
    let q = arg_str(args, "q").unwrap_or_else(|| "*".to_string());
    // Fall back to the conversation's pinned env when the LLM omits it —
    // otherwise the tool would silently scan every env's data.
    let env = arg_str(args, "env").unwrap_or_else(|| ctx_env.to_string());
    let index = arg_str(args, "index");
    let (start, end) = arg_range(args, "-6h", "now");
    let limit = arg_usize(args, "limit", 20).clamp(1, 50);

    let r = crate::search::search(
        catalog,
        fields,
        &q,
        Some(env.as_str()),
        index.as_deref(),
        start,
        end,
        0,
        limit,
        &[],
        None,
    )?;

    // Mirror MCP: parse `raw` inline so the LLM sees event fields without
    // also paying the token cost of the verbatim JSON string.
    let mut hits_out = Vec::with_capacity(r.hits.len());
    for h in &r.hits {
        let event: Option<Value> = h
            .raw
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .filter(|v| v.is_object());
        hits_out.push(json!({
            "timestamp": h.timestamp,
            "message": h.message,
            "source": h.source,
            "partition": h.partition,
            "event": event,
        }));
    }
    Ok(json!({
        "total": r.total,
        "took_us": r.took_us,
        "partitions_scanned": r.partitions_scanned,
        "hits": hits_out,
        "table": r.table,
    }))
}

fn discover_fields(
    catalog: &Catalog,
    fields: &Fields,
    ctx_env: &str,
    args: &Value,
) -> Result<Value> {
    let q = arg_str(args, "q").unwrap_or_else(|| "*".to_string());
    let env = arg_str(args, "env").unwrap_or_else(|| ctx_env.to_string());
    let index = arg_str(args, "index");
    let (start, end) = arg_range(args, "-6h", "now");
    let top = arg_usize(args, "top", 20).clamp(1, 100);
    let r = crate::search::discover::discover_fields(
        catalog,
        fields,
        &q,
        Some(env.as_str()),
        index.as_deref(),
        start,
        end,
        2000,
        top,
        &[],
    )?;
    Ok(serde_json::to_value(r)?)
}

fn histogram(catalog: &Catalog, fields: &Fields, ctx_env: &str, args: &Value) -> Result<Value> {
    let q = arg_str(args, "q").unwrap_or_else(|| "*".to_string());
    let env = arg_str(args, "env").unwrap_or_else(|| ctx_env.to_string());
    let index = arg_str(args, "index");
    let (start, end) = arg_range(args, "-6h", "now");
    let interval = arg_str(args, "interval").unwrap_or_else(|| auto_interval(start, end));
    let r = crate::search::histogram(
        catalog,
        fields,
        &q,
        Some(env.as_str()),
        index.as_deref(),
        start,
        end,
        &interval,
        &[],
        None,
    )?;
    Ok(serde_json::to_value(r)?)
}

fn aggregate(catalog: &Catalog, fields: &Fields, ctx_env: &str, args: &Value) -> Result<Value> {
    let q = arg_str(args, "q").unwrap_or_else(|| "*".to_string());
    let env = arg_str(args, "env").unwrap_or_else(|| ctx_env.to_string());
    let index = arg_str(args, "index");
    let (start, end) = arg_range(args, "-6h", "now");
    let fields_str =
        arg_str(args, "fields").ok_or_else(|| anyhow!("aggregate requires `fields`"))?;
    let field_names: Vec<String> = fields_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if field_names.is_empty() {
        return Err(anyhow!("aggregate: `fields` is empty"));
    }
    let size = arg_u32(args, "size", 10);
    let r = crate::search::aggregate(
        catalog,
        fields,
        &q,
        Some(env.as_str()),
        index.as_deref(),
        start,
        end,
        &field_names,
        size,
        false,
        &[],
    )?;
    Ok(serde_json::to_value(r)?)
}

fn list_indexes(catalog: &Catalog) -> Result<Value> {
    // Index names across user envs; system envs (`_system`) are excluded and
    // reached only by scoping to them explicitly.
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for k in catalog.list_partitions() {
        if k.env.starts_with('_') {
            continue;
        }
        seen.insert(k.index);
    }
    Ok(json!({ "indexes": seen.into_iter().collect::<Vec<_>>() }))
}

fn suggest_followups(args: &Value) -> Value {
    let prompts: Vec<String> = args
        .get("prompts")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .take(4)
                .collect()
        })
        .unwrap_or_default();
    let label = args
        .get("label")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string());
    json!({ "prompts": prompts, "label": label })
}

// ----------------- arg helpers (mirror crate::mcp::tools) -----------------

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn arg_usize(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(default)
}

fn arg_u32(args: &Value, key: &str, default: u32) -> u32 {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|n| n as u32)
        .unwrap_or(default)
}

fn arg_i64(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn arg_range(
    args: &Value,
    default_start: &str,
    default_end: &str,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let start = parse_time(arg_str(args, "start").as_deref().unwrap_or(default_start));
    let end = parse_time(arg_str(args, "end").as_deref().unwrap_or(default_end));
    (start, end)
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s == "now" {
        return Some(Utc::now());
    }
    if let Some(rest) = s.strip_prefix('-') {
        return Some(Utc::now() - parse_duration(rest)?);
    }
    if let Ok(n) = s.parse::<i64>() {
        return DateTime::<Utc>::from_timestamp_millis(n);
    }
    s.parse::<DateTime<Utc>>().ok()
}

fn parse_duration(s: &str) -> Option<Duration> {
    let (num, unit) = s.split_at(s.find(|c: char| c.is_alphabetic())?);
    let n: i64 = num.parse().ok()?;
    Some(match unit {
        "s" => Duration::seconds(n),
        "m" => Duration::minutes(n),
        "h" => Duration::hours(n),
        "d" => Duration::days(n),
        "w" => Duration::weeks(n),
        _ => return None,
    })
}

fn auto_interval(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> String {
    let span = match (start, end) {
        (Some(s), Some(e)) => (e - s).num_seconds().max(60),
        _ => 3600,
    };
    let mins = (span / 60).max(1);
    if mins < 10 {
        "10s".into()
    } else if mins < 60 {
        format!("{}s", ((mins + 5) / 10 * 10).max(10))
    } else if mins < 3600 {
        format!("{}m", mins)
    } else {
        format!("{}h", (mins / 60).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_helpers() {
        let args = json!({"s": "x", "empty": "", "n": 7, "neg": -3});
        assert_eq!(arg_str(&args, "s"), Some("x".to_string()));
        assert_eq!(arg_str(&args, "empty"), None);
        assert_eq!(arg_usize(&args, "n", 0), 7);
        assert_eq!(arg_usize(&args, "missing", 20), 20);
        assert_eq!(arg_u32(&args, "n", 0), 7);
        assert_eq!(arg_i64(&args, "neg", 0), -3);
        assert_eq!(arg_i64(&args, "missing", 99), 99);
    }

    #[test]
    fn is_terminal_only_suggest_followups() {
        assert!(is_terminal("suggest_followups"));
        assert!(!is_terminal("query_logs"));
    }

    #[test]
    fn suggest_followups_caps_and_trims() {
        let out = suggest_followups(&json!({
            "prompts": ["  a ", "", "b", "c", "d", "e"],
            "label": "  More "
        }));
        // Empties dropped, trimmed, capped at 4.
        assert_eq!(out["prompts"], json!(["a", "b", "c", "d"]));
        assert_eq!(out["label"], "More");
    }

    #[test]
    fn suggest_followups_missing_fields() {
        let out = suggest_followups(&json!({}));
        assert_eq!(out["prompts"], json!([]));
        assert!(out["label"].is_null());
    }

    fn names(defs: &[LlmToolDef]) -> Vec<String> {
        defs.iter().map(|d| d.name.clone()).collect()
    }

    #[test]
    fn tool_defs_interactive_chat_has_monitor_mgmt_not_raise_alert() {
        let defs = tool_defs(&ToolMode::InteractiveChat {
            user_id: "u1".into(),
        });
        let n = names(&defs);
        assert!(n.contains(&"query_logs".to_string()));
        assert!(n.contains(&"create_monitor".to_string()));
        assert!(n.contains(&"list_monitors".to_string()));
        assert!(n.contains(&"update_monitor".to_string()));
        assert!(n.contains(&"create_dashboard".to_string()));
        assert!(n.contains(&"list_dashboards".to_string()));
        assert!(n.contains(&"list_alerts".to_string()));
        assert!(n.contains(&"acknowledge_alert".to_string()));
        // create_alert was removed in favor of the monitor tools.
        assert!(!n.contains(&"create_alert".to_string()));
        assert!(!n.contains(&"raise_alert".to_string()));
    }

    #[test]
    fn tool_defs_monitor_run_has_raise_alert_not_monitor_mgmt() {
        let defs = tool_defs(&ToolMode::MonitorRun {
            monitor_id: "m1".into(),
            user_id: "u1".into(),
            conversation_id: "c1".into(),
        });
        let n = names(&defs);
        assert!(n.contains(&"query_logs".to_string()));
        assert!(n.contains(&"raise_alert".to_string()));
        assert!(!n.contains(&"create_monitor".to_string()));
        assert!(!n.contains(&"update_monitor".to_string()));
        assert!(!n.contains(&"create_dashboard".to_string()));
    }
}
