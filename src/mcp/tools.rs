// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! MCP tool registry; each tool maps 1:1 to a `crate::search`/`catalog`/`indexer`
//! fn, mirroring the frontend `TOOL_DEFS`. Results are MCP `text` content carrying
//! a stringified JSON payload (sidesteps MCP's lack of a typed-object variant).

use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde_json::{json, Map, Value};
use tokio::task;

use super::McpServer;
use crate::catalog::PartitionKey;
use crate::control::alerts::Alert;
use crate::control::dashboards::{Dashboard, DashboardInput, DashboardPatch};
use crate::control::monitors::{Monitor, MonitorInput, MonitorPatch};
use crate::control::settings::McpSettings;
use crate::engine::PartitionEngine;
use crate::indexer::ingest::event_day;

/// `tools/list` payload, post-filtered by the admin allowlist (returns `[]`
/// when disabled). Order is meaningful — discovery tools come first.
pub(super) async fn list(server: &McpServer) -> Vec<Value> {
    let settings = server.settings().await;
    if !settings.enabled {
        return Vec::new();
    }
    all_tools()
        .into_iter()
        .filter(|t| match t.get("name").and_then(Value::as_str) {
            Some(name) => settings.is_tool_enabled(name),
            None => false,
        })
        .collect()
}

/// Owner id for control-plane rows (dashboards, alerts) created over MCP. MCP
/// authenticates with a shared token (no per-user identity), so all its rows
/// hang off this synthetic user.
const MCP_USER_ID: &str = "mcp";

/// Dashboard `spec` schema, spliced into the create/update tool descriptions.
/// Mirrors `DashboardSpec` in `frontend/src/api/types.ts`.
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
string — there is no separate index param). Optional per-widget `time`: {\"range\":\"-1h\"} \
overrides the dashboard window. Dashboards are NOT env-scoped: widgets run in the viewer's \
active env at render time.";

/// Static catalog of every tool the MCP server knows how to dispatch.
/// `list` filters this; `dispatch` validates against it.
fn all_tools() -> Vec<Value> {
    vec![
        tool(
            "list_indexes",
            "Enumerate every index currently in the catalog. Cheap orientation: call this first on a new \
             investigation to see what data is even present. Returns the index names \
             (excluding the `_*` internal indexes by default).",
            json!({
                "type": "object",
                "properties": {},
            }),
        ),
        tool(
            "discover_fields",
            "Sample matching events and return the JSON keys present, ranked by coverage × cardinality. \
             Helios is schema-on-read — field names are whatever the events contain — so you should call this \
             BEFORE any aggregation or pipe query, otherwise you're guessing field names. \
             \n\n\
             Each returned field carries: `coverage` (fraction of sampled docs with this key), \
             `cardinality_seen` (distinct values seen; `cardinality_capped=true` means sample hit the cap \
             and true cardinality is higher), `groupable` (safe to pass to `aggregate` or `stats by`), \
             `interesting` (heuristic: low/medium-cardinality groupable field likely useful for \
             aggregation), `value_kind` (string/int/float), and `sample_values`.",
            json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string", "description": "Query to scope the sample. Default '*'." },
                    "index": { "type": "string", "description": "Optional index glob (e.g. 'stripe-*')." },
                    "start": { "type": "string", "description": "Start time. ISO 8601, unix ms, or relative (units: s/m/h/d/w only — e.g. `-15m`, `-24h`, `-7d`, `-2w`). Default '-6h'." },
                    "end":   { "type": "string", "description": "End time. ISO 8601, unix ms, 'now', or relative offset (same units as `start`). Default 'now'." },
                    "sample": { "type": "number", "description": "Docs to sample (1-5000). Default 2000." },
                    "top":   { "type": "number", "description": "Max fields to return (1-100). Default 30." },
                },
            }),
        ),
        tool(
            "query_logs",
            "Search log events. Schema-on-read: any JSON key is queryable as `<key>:value`. Universal \
             fields: `timestamp`, `message`, `raw` (full event JSON), `source` (per-event tag). Returns \
             hits, OR a `table` for pipe queries. Call `discover_fields` first if unsure which keys exist.\n\
             \n\
             GOTCHAS (read first — these silently fail):\n\
             - Wildcards `*`/`?` DO NOT work on dynamic fields: `service:api*` returns nothing. They DO \
             work on bare terms, `message:`/`raw:`, `source:*`, `index:*`. Workaround: `raw:api*` or \
             `discover_fields` for exact values.\n\
             - NOT implemented (don't try): `| where`, `| eval`, `| rex`, `| extract`, `| rename`, \
             `| timechart`. Filter pre-stats inside the search expression.\n\
             - ONE stats-producing stage per pipeline (`stats`/`top`/`rare`); can't chain a second.\n\
             - Grouping by a high-cardinality field can produce thousands of rows and exceed client \
             display limits. Narrow with filters or `| top N FIELD`.\n\
             \n\
             PIPE OPERATORS (analytics):\n\
             - `<search> | stats <agg>[, <agg>...] [by <field>[, ...]]`. Aggs: `count()`, \
             `sum/avg/min/max/p50/p95/p99(F)` (aliases: `mean`→`avg`, `median`→`p50`). Example: \
             `level:error | stats count, p95(latency_ms) by service, host`. Non-numeric values skipped.\n\
             - `| top [N] FIELD` / `| rare [N] FIELD` — stats+sort+head shortcut. Default N=10.\n\
             - `| sort [-]FIELD`, `| head N`, `| tail N` — post-stats only.\n\
             \n\
             QUERY SYNTAX:\n\
             - Field filter `<key>:value` — exact term, case-insensitive (`level:ERROR`). Value \
             tokenized at ingest (`payment-gateway` = one token).\n\
             - Phrases for multi-token exact: `\"upstream call failed\"`, `http.path:\"/api/v1/orders\"`.\n\
             - Numeric range (dynamic fields only): `field:>N`, `>=N`, `<N`, `<=N`. Numeric only.\n\
             - Nested JSON: `error.type:NPE` — dots auto-expand.\n\
             - Bare terms exact-token match `message`+`raw`+`source` (`timeout` = the token `timeout`, \
             not `timeouts`); trailing `*` for prefix (`time*`); quote for a phrase.\n\
             - Booleans: `AND`, `OR`, `NOT`, `-term`; parens group; adjacent = implicit AND.\n\
             - Index glob: `index:stripe-*` (resolved pre-engine).",
            json!({
                "type": "object",
                "properties": {
                    "q":     { "type": "string", "description": "Query. Use '*' for everything. Examples: 'level:ERROR', 'status:>=500', 'severity:error | stats count by service', '* | stats p95(latency_ms) by service', 'service:checkout AND latency_ms:>1000'." },
                    "index": { "type": "string", "description": "Optional index glob filtering the partitions to scan (e.g. 'stripe-*'). Omit to scatter-gather across all indexes. `source` is a per-event tag distinct from `index`." },
                    "start": { "type": "string", "description": "Start time. ISO 8601 ('2026-05-24T06:00:00Z'), unix ms, or relative (units: s/m/h/d/w only — e.g. `-30s`, `-15m`, `-1h`, `-24h`, `-7d`, `-2w`). Default '-6h'." },
                    "end":   { "type": "string", "description": "End time. ISO 8601, unix ms, 'now', or relative offset (same units as `start`). Default 'now'." },
                    "limit": { "type": "number", "description": "Max hits to return (1-200). Default 20. Ignored for pipe queries." },
                },
                "required": ["q"],
            }),
        ),
        tool(
            "histogram",
            "Event counts bucketed over time. Use this to see WHEN things happened — spikes flag \
             incidents, gaps flag ingestion problems. Cheaper than a full search when you just want shape.",
            json!({
                "type": "object",
                "properties": {
                    "q":        { "type": "string", "description": "Query. Default '*'." },
                    "index":    { "type": "string" },
                    "start":    { "type": "string", "description": "Default '-6h'." },
                    "end":      { "type": "string", "description": "Default 'now'." },
                    "interval": { "type": "string", "description": "Bucket size (1m, 5m, 1h, 1d). Auto-picked if omitted." },
                },
            }),
        ),
        tool(
            "aggregate",
            "Top values per field (terms aggregation), returned count-descending. Answers 'which services \
             error most?', 'which hosts are noisy?'. Field names are arbitrary JSON keys — call \
             `discover_fields` first if unsure. NOT aggregatable: `message` and `raw` (text fields, no \
             fast column — use `query_logs` for free-text grouping) and `timestamp` (use `histogram` for \
             time bucketing). The synthetic field `index` returns counts per partition key. `size` caps \
             top-N PER FIELD, so a 5-field request with `size=10` returns up to 50 rows total.",
            json!({
                "type": "object",
                "properties": {
                    "q":      { "type": "string", "description": "Query to scope the aggregation. Default '*'." },
                    "index":  { "type": "string" },
                    "start":  { "type": "string", "description": "Default '-6h'." },
                    "end":    { "type": "string", "description": "Default 'now'." },
                    "fields": { "type": "string", "description": "Comma-separated field names. Required — there is no default." },
                    "size":   { "type": "number", "description": "Top N per field. Default 10." },
                    "approximate": { "type": "boolean", "description": "Stride-sample partitions when there are many (>16). Trades exact counts for latency. Default false." },
                },
                "required": ["fields"],
            }),
        ),
        tool(
            "get_stats",
            "One-shot health check: total documents, segment count, partition count across the entire \
             catalog. Useful to confirm 'is this instance healthy and ingesting?' before drilling in.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_partitions",
            "Enumerate every `(env, index, day)` partition with doc count, segment count, and on-disk byte \
             size. Use when triaging 'is yesterday's data here?' or 'why is partition X huge?'.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "list_environments",
            "List the environments (tenancy boundaries) currently registered in this HeliosLogs instance. \
             Every search / aggregate / histogram tool accepts an optional `env` argument; the default is \
             the active env on the MCP caller's side (typically `default`). Use this tool when the user \
             asks to compare envs (`prod` vs `dev`), when an investigation needs to widen beyond the \
             current env, or to confirm an env exists before scoping a tool call to it.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "get_index_info",
            "Detail for a specific `(index, day)` partition: schema, doc count, segments. Pass both \
             `index` and `day` (YYYY-MM-DD) to scope; omit either for a catalog-wide rollup similar to \
             `get_stats` but with schema attached.",
            json!({
                "type": "object",
                "properties": {
                    "index": { "type": "string", "description": "Partition's index name." },
                    "day":   { "type": "string", "description": "Partition day, YYYY-MM-DD." },
                },
            }),
        ),
        tool(
            "ingest_events",
            "Bulk-ingest events. Pass either `events` (array of JSON objects) or `ndjson` (newline-\
             delimited JSON string) — not both. Events are routed by their timestamp field to the correct \
             `(index, day)` partition; events without a parseable timestamp fall back to today (UTC). \
             Writes a block synchronously before returning, so the new events are queryable immediately. \
             Safe to run while `helioslogs serve` is also writing the same data dir — the block manifest \
             uses compare-and-swap, so concurrent appends don't conflict.",
            json!({
                "type": "object",
                "properties": {
                    "index":  { "type": "string", "description": "Partition key (e.g. 'stripe-webhooks'). Required." },
                    "source": { "type": "string", "description": "Fallback `source` tag for events that don't carry one. Optional." },
                    "events": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "Array of JSON event objects.",
                    },
                    "ndjson": {
                        "type": "string",
                        "description": "Newline-delimited JSON, one event per line. Use instead of `events` for large batches where the array form would be unwieldy.",
                    },
                },
                "required": ["index"],
            }),
        ),
        tool(
            "list_dashboards",
            "List the dashboards visible to MCP: every public dashboard plus the ones created \
             via MCP. Returns id, name, description, owner, visibility, updated_at, and widget \
             count per dashboard — call `get_dashboard` for the full widget spec. Use this first \
             to find an existing dashboard before creating a near-duplicate.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "get_dashboard",
            "Fetch one dashboard's full definition, including its `spec` (widgets, layout, time \
             range). Use before `update_dashboard` — updates replace `spec` wholesale, so read, \
             modify, write back.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Dashboard id (dash_…), from `list_dashboards`." },
                },
                "required": ["id"],
            }),
        ),
        tool(
            "create_dashboard",
            &format!(
                "Create a dashboard. Check `list_dashboards` first to avoid duplicates. Omitting \
                 `spec` creates an empty dashboard (24h window, no widgets). Returns the new id \
                 and a UI path (`/dashboards/<id>`). Always created PUBLIC (visible to and editable \
                 by every user) — MCP can't create private dashboards.\n\n{DASHBOARD_SPEC_DOC}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "name":        { "type": "string", "description": "Display name (required, non-empty)." },
                    "description": { "type": "string", "description": "Optional longer description." },
                    "spec":        { "type": "object", "description": "Widget + layout config; see SPEC FORMAT in the tool description. Default: empty 24h dashboard." },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "update_dashboard",
            &format!(
                "Update a dashboard's name, description, or spec. Only the fields you pass change, \
                 but `spec` is replaced WHOLESALE — call `get_dashboard`, edit the returned spec, \
                 and send the complete result (never a partial widget list). MCP can only update \
                 PUBLIC dashboards (private ones are rejected) and can't change a dashboard's \
                 visibility.\n\n{DASHBOARD_SPEC_DOC}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "id":          { "type": "string", "description": "Dashboard id (dash_…). Required." },
                    "name":        { "type": "string" },
                    "description": { "type": "string" },
                    "spec":        { "type": "object", "description": "Complete replacement spec; see SPEC FORMAT in the tool description." },
                },
                "required": ["id"],
            }),
        ),
        tool(
            "list_alerts",
            "List alerts visible to MCP (public alerts plus MCP-created ones), newest first. \
             Covers monitor-raised alerts and manually created ones. Returns id, severity, \
             title, summary, source (raising monitor name, or `agent`/`mcp` for manual alerts), \
             env, acknowledged state, and timestamps.",
            json!({
                "type": "object",
                "properties": {
                    "unacked_only": { "type": "boolean", "description": "true = inbox view (unacknowledged only). Default false." },
                    "search":  { "type": "string", "description": "Case-insensitive substring match over title / summary / source / env." },
                    "monitor": { "type": "string", "description": "Restrict to alerts raised by one monitor id (mon_…)." },
                    "limit":   { "type": "number", "description": "Max alerts to return (1-200). Default 50." },
                },
            }),
        ),
        tool(
            "acknowledge_alert",
            "Mark an alert acknowledged, clearing it from the inbox. Acknowledgement is shared: \
             acking a public alert clears it for every user, so only ack alerts that have \
             actually been handled or that the user explicitly asked to clear.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Alert id (alt_…), from `list_alerts`." },
                },
                "required": ["id"],
            }),
        ),
        tool(
            "list_monitors",
            "List the scheduled monitors visible to MCP (public ones plus MCP-created), newest-updated \
             first. Monitors are recurring checks: an `ai` monitor hands a prompt to the agent each \
             interval; a `threshold` monitor counts events over a window and alerts when a comparison \
             trips. Returns id, name, kind, schedule, enabled state, env, and last-run status. Call \
             this before `update_monitor` to get the id and current shape.",
            json!({
                "type": "object",
                "properties": {},
            }),
        ),
        tool(
            "create_monitor",
            "Create a recurring monitor. Two kinds:\n\
             - `ai` (default): hands `prompt` to the investigation agent every interval; it queries \
             HeliosLogs and raises an alert when it finds something. Write `prompt` as a self-contained brief \
             ('Every 30 min, compare error rates in orders-api to the prior hour; alert on anomalies.').\n\
             - `threshold`: counts events matching `threshold.query` over `threshold.window_seconds` and \
             alerts when the count is `comparison` the `threshold` value. Deterministic, no LLM.\n\
             Interval floor is 300s (5 min); default 1800s (30 min). Always created PUBLIC (raised \
             alerts visible to all) — MCP can't create private monitors.",
            json!({
                "type": "object",
                "properties": {
                    "name":             { "type": "string", "description": "Short label (1-60 chars). Required." },
                    "description":      { "type": "string", "description": "Optional longer description." },
                    "kind":             { "type": "string", "enum": ["ai", "threshold"], "description": "Monitor type. Default 'ai'." },
                    "prompt":           { "type": "string", "description": "AI monitors only: the agent instruction. Be specific about what to check, the baseline, and the alert criteria." },
                    "threshold":        { "type": "object", "description": "Threshold monitors only. { query, window_seconds, comparison: gt|gte|lt|lte|eq|neq, threshold (number), index?, severity? (low|medium|high) }. `query` is a normal HeliosLogs search (pipes/`index:foo` allowed)." },
                    "interval_seconds": { "type": "number", "description": "How often to run. Default 1800; minimum 300." },
                    "enabled":          { "type": "boolean", "description": "Start enabled? Default true." },
                    "env":              { "type": "string", "description": "Env this monitor runs against. Default 'default'." },
                },
                "required": ["name"],
            }),
        ),
        tool(
            "update_monitor",
            "Edit an existing monitor by id (from `list_monitors`). Only the fields you pass change; \
             omit the rest. Use to retune a schedule, rewrite an AI prompt, adjust a threshold, or \
             pause/resume (`enabled`). Editing a threshold re-arms its edge-triggered alerting. The \
             interval floor (300s) still applies. MCP can only update PUBLIC monitors (private ones \
             are rejected) and can't change a monitor's visibility.",
            json!({
                "type": "object",
                "properties": {
                    "id":               { "type": "string", "description": "Monitor id (mon_…). Required." },
                    "name":             { "type": "string" },
                    "description":      { "type": "string" },
                    "kind":             { "type": "string", "enum": ["ai", "threshold"], "description": "Switching kind requires the new kind's payload (prompt or threshold) to be present/valid." },
                    "prompt":           { "type": "string", "description": "AI monitors: the agent instruction." },
                    "threshold":        { "type": "object", "description": "Threshold monitors: full config object — see create_monitor.threshold." },
                    "interval_seconds": { "type": "number", "description": "New run interval (minimum 300)." },
                    "enabled":          { "type": "boolean", "description": "false = pause, true = resume." },
                },
                "required": ["id"],
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

/// Identifies the API key behind an MCP call, for the `_heliosmcp` audit log.
struct ResolvedKey {
    id: String,
    name: String,
}

/// `tools/call` entry point. Tool errors return as `isError: true` content
/// (recoverable) rather than JSON-RPC errors (which clients treat as fatal).
pub(super) async fn call(
    server: &Arc<McpServer>,
    params: &Value,
    presented_token: Option<&str>,
) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tools/call: missing `name`"))?
        .to_string();
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));

    // Time the dispatch (not the JSON-serialization of the response)
    // so the logged `duration_ms` reflects the tool's actual cost.
    let t0 = Instant::now();
    let settings = server.settings().await;
    // Authenticate up front so the caller's key identity is recorded in the
    // audit log regardless of how the tool itself resolves.
    let auth = authorize_mcp(server, &settings, presented_token).await;
    let (outcome, caller) = match auth {
        Ok(resolved) => (
            dispatch(server.clone(), &name, &args, settings).await,
            resolved,
        ),
        Err(e) => (Err(e), None),
    };
    let duration = t0.elapsed();

    // Mirror to `_heliosmcp`, fire-and-forget — a failed enqueue drops the
    // line silently so self-observability never impacts tool latency.
    let err_msg = outcome.as_ref().err().map(|e| format!("{e:#}"));
    let caller = caller.as_ref().map(|k| (k.id.as_str(), k.name.as_str()));
    crate::self_logs::log_tool_call(&name, &args, duration, err_msg.as_deref(), caller);

    Ok(match outcome {
        Ok(payload) => json!({
            "content": [{ "type": "text", "text": serde_json::to_string(&payload)? }],
            "isError": false,
        }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": format!("{e:#}") }],
            "isError": true,
        }),
    })
}

/// How stale a key's `last_used_at` may get before MCP writes a fresh stamp.
const MCP_KEY_TOUCH_THROTTLE_MS: i64 = 60_000;

/// Resolve + authorize the MCP caller from the presented bearer token. Returns
/// the identifying key on success, or `None` for an allowed-anonymous call (no
/// enabled MCP-scoped key exists yet). An `Err` closes the call.
///
/// Access model: while no enabled MCP-scoped key exists the surface is open
/// (anonymous); once any such key is minted, every call must present a valid,
/// non-expired key carrying the MCP scope.
async fn authorize_mcp(
    server: &Arc<McpServer>,
    settings: &McpSettings,
    presented_token: Option<&str>,
) -> Result<Option<ResolvedKey>> {
    if !settings.enabled {
        return Err(anyhow!(
            "MCP is disabled by admin. Re-enable from the Helios admin UI."
        ));
    }
    let keys = server.control.api_key_list().await?;
    let now_ms = chrono::Utc::now().timestamp_millis();

    if let Some(tok) = presented_token {
        // A token was offered: it must resolve to a usable MCP-scoped key.
        let Some(k) = keys.iter().find(|k| k.enabled && k.token == tok) else {
            return Err(anyhow!("unknown or disabled API key"));
        };
        if k.is_expired(now_ms) {
            return Err(anyhow!("API key '{}' has expired", k.name));
        }
        if !k.scopes.mcp {
            return Err(anyhow!(
                "API key '{}' is not authorized for MCP — enable the MCP scope in Admin → API keys",
                k.name
            ));
        }
        touch_key(server, k, now_ms);
        return Ok(Some(ResolvedKey {
            id: k.id.clone(),
            name: k.name.clone(),
        }));
    }

    // No token: allowed only while no enabled, non-expired MCP-scoped key exists.
    let mcp_key_exists = keys
        .iter()
        .any(|k| k.enabled && k.scopes.mcp && !k.is_expired(now_ms));
    if mcp_key_exists {
        return Err(anyhow!(
            "MCP requires an API key with the MCP scope — pass `Authorization: Bearer hlk_…` \
             from a key created in Admin → API keys"
        ));
    }
    Ok(None)
}

/// Best-effort, throttled `last_used_at` stamp for an MCP key (off the request path).
fn touch_key(server: &Arc<McpServer>, key: &crate::control::api_keys::ApiKey, now_ms: i64) {
    let stale = key
        .last_used_at
        .map(|t| now_ms - t > MCP_KEY_TOUCH_THROTTLE_MS)
        .unwrap_or(true);
    if !stale {
        return;
    }
    let control = server.control.clone();
    let id = key.id.clone();
    tokio::spawn(async move {
        let _ = control.api_key_touch(&id, now_ms).await;
    });
}

async fn dispatch(
    server: Arc<McpServer>,
    name: &str,
    args: &Value,
    settings: McpSettings,
) -> Result<Value> {
    // Engine-free policy gates (auth already cleared in `authorize_mcp`).
    check_gates(&settings, name, args)?;

    // `list_environments` only needs the (async) env catalog — handle it inline
    // rather than dragging the control store into the blocking closure.
    if name == "list_environments" {
        let all = server.control.list_envs(false).await?;
        return list_environments(all, &settings);
    }

    // Dashboard + alert tools are cheap async control-store ops — handled inline.
    match name {
        "list_dashboards" => return list_dashboards(&server).await,
        "get_dashboard" => return get_dashboard(&server, args).await,
        "create_dashboard" => return create_dashboard(&server, args).await,
        "update_dashboard" => return update_dashboard(&server, args).await,
        "list_alerts" => return list_alerts(&server, args).await,
        "acknowledge_alert" => return acknowledge_alert(&server, args).await,
        "list_monitors" => return list_monitors(&server).await,
        "create_monitor" => return create_monitor(&server, args).await,
        "update_monitor" => return update_monitor(&server, args).await,
        _ => {}
    }

    // Ingest validates the target env against the registered (non-system) env
    // list — fetched here (async) and handed to the blocking writer.
    if name == "ingest_events" {
        let valid_envs: std::collections::HashSet<String> = server
            .control
            .list_envs(false)
            .await?
            .into_iter()
            .map(|e| e.name)
            .collect();
        let server = server.clone();
        let args = args.clone();
        return task::spawn_blocking(move || ingest_events(&server, &args, &settings, &valid_envs))
            .await
            .context("tool task panicked")?;
    }

    // Search-path calls are CPU-bound and synchronous (the block engine operates
    // on a blocking thread model). Offload so the HTTP handler isn't held up.
    let name = name.to_string();
    let args = args.clone();
    task::spawn_blocking(move || dispatch_blocking(&server, &name, &args, &settings))
        .await
        .context("tool task panicked")?
}

/// Engine-free policy gates: master switch, tool allowlist, and the `(env, index)`
/// allowlist pre-check (friendlier than scanning nothing). Caller auth is handled
/// separately in [`authorize_mcp`].
fn check_gates(settings: &McpSettings, name: &str, args: &Value) -> Result<()> {
    if !settings.enabled {
        return Err(anyhow!(
            "MCP is disabled by admin. Re-enable from the HeliosLogs admin UI."
        ));
    }
    if !settings.is_tool_enabled(name) {
        return Err(anyhow!("tool '{name}' is disabled by admin"));
    }

    let user_env_provided = arg_str(args, "env").filter(|s| !s.trim().is_empty());
    let user_env = user_env_provided
        .clone()
        .unwrap_or_else(|| crate::catalog::DEFAULT_ENV.to_string());
    let user_index = arg_str(args, "index");
    if !settings.indexes_unrestricted() {
        if let Some(idx) = user_index.as_deref() {
            if !settings.allows(&user_env, idx) {
                return Err(anyhow!(
                    "index '{idx}' in env '{user_env}' is not in the MCP allowlist; \
                     ask the admin to widen the MCP index allowlist or pick something \
                     it covers"
                ));
            }
        } else if user_env_provided.is_some() {
            let env_covered = settings
                .allowed
                .iter()
                .any(|r| r.env == "*" || r.env.eq_ignore_ascii_case(&user_env));
            if !env_covered {
                return Err(anyhow!(
                    "env '{user_env}' is not in the MCP allowlist; ask the admin \
                     to widen it or pick an env it covers"
                ));
            }
        }
    }
    Ok(())
}

fn dispatch_blocking(
    server: &McpServer,
    name: &str,
    args: &Value,
    settings: &McpSettings,
) -> Result<Value> {
    match name {
        "list_indexes" => list_indexes(server, args, settings),
        "discover_fields" => discover_fields(server, args, settings),
        "query_logs" => query_logs(server, args, settings),
        "histogram" => histogram(server, args, settings),
        "aggregate" => aggregate(server, args, settings),
        "get_stats" => get_stats(server, settings),
        "list_partitions" => list_partitions(server, settings),
        "get_index_info" => get_index_info(server, args, settings),
        other => Err(anyhow!("unknown tool: {other}")),
    }
}

// ----------------- per-tool implementations -----------------

fn list_indexes(server: &McpServer, _args: &Value, settings: &McpSettings) -> Result<Value> {
    let mut indexes: Vec<String> = Vec::new();
    // Env-aware allowlist: an index passes if any `(env, index)` partition
    // is permitted. Iterate partitions, dedupe by index name.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for k in server.catalog.list_partitions() {
        // System envs (`_system`) are reachable only by scoping to them
        // explicitly — never surfaced in this cross-env index list.
        if k.env.starts_with('_') {
            continue;
        }
        if !settings.indexes_unrestricted() && !settings.allows(&k.env, &k.index) {
            continue;
        }
        if seen.insert(k.index.clone()) {
            indexes.push(k.index);
        }
    }
    indexes.sort();
    Ok(json!({ "indexes": indexes }))
}

/// Lists the non-system envs the allowlist permits: all of them when open,
/// else the intersection of registered envs and the allowlist's named envs.
fn list_environments(
    all: Vec<crate::control::envs::EnvRow>,
    settings: &McpSettings,
) -> Result<Value> {
    let filtered = match settings.allowed_envs() {
        None => all,
        Some(allowed) => {
            let allowed_lc: std::collections::HashSet<String> =
                allowed.iter().map(|s| s.to_ascii_lowercase()).collect();
            all.into_iter()
                .filter(|e| allowed_lc.contains(&e.name.to_ascii_lowercase()))
                .collect()
        }
    };
    Ok(json!({ "environments": filtered }))
}

fn discover_fields(server: &McpServer, args: &Value, settings: &McpSettings) -> Result<Value> {
    let q = arg_str(args, "q").unwrap_or_else(|| "*".to_string());
    let env = arg_str(args, "env");
    let index = arg_str(args, "index");
    let (start, end) = arg_range(args, "-6h", "now")?;
    let sample = arg_usize(args, "sample", 2000);
    let top = arg_usize(args, "top", 30);
    let r = crate::search::discover::discover_fields(
        &server.catalog,
        &server.fields,
        &q,
        env.as_deref(),
        index.as_deref(),
        start,
        end,
        sample,
        top,
        &settings.allowed,
    )?;
    Ok(serde_json::to_value(r)?)
}

fn query_logs(server: &McpServer, args: &Value, settings: &McpSettings) -> Result<Value> {
    let q = arg_str(args, "q").context("query_logs requires `q`")?;
    let env = arg_str(args, "env");
    let index = arg_str(args, "index");
    let (start, end) = arg_range(args, "-6h", "now")?;
    let limit = arg_usize(args, "limit", 20).clamp(1, 200);
    let r = crate::search::search(
        &server.catalog,
        &server.fields,
        &q,
        env.as_deref(),
        index.as_deref(),
        start,
        end,
        0,
        limit,
        &settings.allowed,
        None,
    )?;
    // Parse `raw` inline so the LLM sees event fields without paying for
    // both the verbatim string and the parsed object.
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

fn histogram(server: &McpServer, args: &Value, settings: &McpSettings) -> Result<Value> {
    let q = arg_str(args, "q").unwrap_or_else(|| "*".to_string());
    let env = arg_str(args, "env");
    let index = arg_str(args, "index");
    let (start, end) = arg_range(args, "-6h", "now")?;
    let interval = arg_str(args, "interval").unwrap_or_else(|| auto_interval(start, end));
    let r = crate::search::histogram(
        &server.catalog,
        &server.fields,
        &q,
        env.as_deref(),
        index.as_deref(),
        start,
        end,
        &interval,
        &settings.allowed,
        None,
    )?;
    Ok(serde_json::to_value(r)?)
}

fn aggregate(server: &McpServer, args: &Value, settings: &McpSettings) -> Result<Value> {
    let q = arg_str(args, "q").unwrap_or_else(|| "*".to_string());
    let env = arg_str(args, "env");
    let index = arg_str(args, "index");
    let (start, end) = arg_range(args, "-6h", "now")?;
    let fields_str = arg_str(args, "fields").context("aggregate requires `fields`")?;
    let field_names: Vec<String> = fields_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if field_names.is_empty() {
        return Err(anyhow!("aggregate: `fields` is empty"));
    }
    let size = arg_u32(args, "size", 10);
    let approximate = arg_bool(args, "approximate", false);
    let r = crate::search::aggregate(
        &server.catalog,
        &server.fields,
        &q,
        env.as_deref(),
        index.as_deref(),
        start,
        end,
        &field_names,
        size,
        approximate,
        &settings.allowed,
    )?;
    Ok(serde_json::to_value(r)?)
}

/// Catalog-wide stats restricted to the allowlist; can't use
/// `crate::search::stats()` as it counts every partition unconditionally.
fn get_stats(server: &McpServer, settings: &McpSettings) -> Result<Value> {
    if settings.indexes_unrestricted() {
        let s = crate::search::stats(&server.catalog)?;
        return Ok(serde_json::to_value(s)?);
    }
    let engine = crate::engine::block::BlockEngine::with_store(
        crate::engine::block::configured_store(server.catalog.root()),
        crate::engine::block_codec(),
    );
    let mut docs = 0u64;
    let mut segs = 0usize;
    let mut parts = 0usize;
    for k in server.catalog.list_partitions() {
        if !settings.allows(&k.env, &k.index) {
            continue;
        }
        let Some((d, sg)) = engine.partition_stats(&k)? else {
            continue;
        };
        docs += d;
        segs += sg;
        parts += 1;
    }
    Ok(json!({
        "num_docs": docs,
        "num_segments": segs,
        "num_partitions": parts,
    }))
}

fn list_partitions(server: &McpServer, settings: &McpSettings) -> Result<Value> {
    let engine = crate::engine::block::BlockEngine::with_store(
        crate::engine::block::configured_store(server.catalog.root()),
        crate::engine::block_codec(),
    );
    let mut out: Vec<Value> = Vec::new();
    for k in server.catalog.list_partitions() {
        if !settings.allows(&k.env, &k.index) {
            continue;
        }
        let Some((docs, segs)) = engine.partition_stats(&k)? else {
            continue;
        };
        let dir = server.catalog.partition_path(&k);
        let (bytes, files) = directory_size(&dir);
        out.push(json!({
            "env": k.env,
            "index": k.index,
            "day": k.day_string(),
            "num_docs": docs,
            "num_segments": segs,
            "byte_size": bytes,
            "file_count": files,
        }));
    }
    Ok(json!({ "partitions": out }))
}

fn get_index_info(server: &McpServer, args: &Value, settings: &McpSettings) -> Result<Value> {
    let index = arg_str(args, "index");
    let day = arg_str(args, "day");
    match (index, day) {
        (Some(idx_name), Some(day_s)) => {
            let env_name = crate::catalog::env_or_default(arg_str(args, "env").as_deref())?;
            if !settings.allows(&env_name, &idx_name) {
                return Err(anyhow!(
                    "index '{idx_name}' in env '{env_name}' not in the MCP allowlist"
                ));
            }
            let day = NaiveDate::parse_from_str(&day_s, "%Y-%m-%d")
                .with_context(|| format!("bad day '{day_s}', expected YYYY-MM-DD"))?;
            let key = PartitionKey::new(&env_name, &idx_name, day);
            let engine = crate::engine::block::BlockEngine::with_store(
                crate::engine::block::configured_store(server.catalog.root()),
                crate::engine::block_codec(),
            );
            let (docs, segs) = engine.partition_stats(&key)?.ok_or_else(|| {
                anyhow!(
                    "no such partition: {}",
                    crate::catalog::partition_label(&key)
                )
            })?;
            let dir = server.catalog.partition_path(&key);
            let (bytes, files) = directory_size(&dir);
            Ok(json!({
                "scope": "partition",
                "env": key.env,
                "index": key.index,
                "day": key.day_string(),
                "directory": dir.display().to_string(),
                "num_docs": docs,
                "num_segments": segs,
                "byte_size": bytes,
                "file_count": files,
            }))
        }
        _ => {
            // Catalog-wide rollup. Same shape as `list_partitions` + global
            // totals + schema — useful as a single "describe everything" call.
            let engine = crate::engine::block::BlockEngine::with_store(
                crate::engine::block::configured_store(server.catalog.root()),
                crate::engine::block_codec(),
            );
            let mut total_docs = 0u64;
            let mut total_segments = 0usize;
            let mut total_bytes = 0u64;
            let mut parts: Vec<Value> = Vec::new();
            for k in server.catalog.list_partitions() {
                if !settings.allows(&k.env, &k.index) {
                    continue;
                }
                let Some((docs, segs)) = engine.partition_stats(&k)? else {
                    continue;
                };
                let dir = server.catalog.partition_path(&k);
                let (bytes, _) = directory_size(&dir);
                total_docs += docs;
                total_segments += segs;
                total_bytes += bytes;
                parts.push(json!({
                    "env": k.env, "index": k.index, "day": k.day_string(),
                    "num_docs": docs, "num_segments": segs, "byte_size": bytes,
                }));
            }
            // Schema-on-read: only the universal core is fixed; everything
            // else is per-event (use `discover_fields`).
            let schema_json = json!([
                { "name": "timestamp", "type": "Date" },
                { "name": "message", "type": "Str" },
                { "name": "raw", "type": "Str" },
                { "name": "source", "type": "Str" },
                { "name": "dynamic", "type": "Json" },
            ]);
            Ok(json!({
                "scope": "catalog",
                "num_docs": total_docs,
                "num_segments": total_segments,
                "byte_size": total_bytes,
                "partitions": parts,
                "schema": schema_json,
            }))
        }
    }
}

// ----------------- dashboard tools -----------------

/// Compact list row: everything but the spec (which can be large).
fn dashboard_summary(d: &Dashboard) -> Value {
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
}

async fn list_dashboards(server: &McpServer) -> Result<Value> {
    let items = server.control.dashboard_list(MCP_USER_ID).await?;
    let out: Vec<Value> = items.iter().map(dashboard_summary).collect();
    Ok(json!({ "dashboards": out }))
}

async fn get_dashboard(server: &McpServer, args: &Value) -> Result<Value> {
    let id = arg_str(args, "id").context("get_dashboard requires `id`")?;
    let d = server
        .control
        .dashboard_get(MCP_USER_ID, &id, false)
        .await?;
    let mut v = serde_json::to_value(&d)?;
    v["url"] = json!(format!("/dashboards/{}", d.id));
    Ok(v)
}

async fn create_dashboard(server: &McpServer, args: &Value) -> Result<Value> {
    let name = arg_str(args, "name").context("create_dashboard requires `name`")?;
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
        // MCP creates public-only — it has no per-user identity to own private rows.
        public: true,
    };
    let d = server.control.dashboard_create(MCP_USER_ID, input).await?;
    Ok(json!({
        "ok": true,
        "id": d.id,
        "name": d.name,
        "public": d.public,
        "url": format!("/dashboards/{}", d.id),
    }))
}

/// Reject a private dashboard: MCP (no per-user identity) may only edit public
/// ones. Errors if the dashboard is missing, not visible, or private.
async fn require_public_dashboard(server: &McpServer, id: &str) -> Result<()> {
    let d = server.control.dashboard_get(MCP_USER_ID, id, false).await?;
    if !d.public {
        return Err(anyhow!(
            "dashboard {id} is private; MCP can only update public dashboards"
        ));
    }
    Ok(())
}

async fn update_dashboard(server: &McpServer, args: &Value) -> Result<Value> {
    let id = arg_str(args, "id").context("update_dashboard requires `id`")?;
    // MCP manages public dashboards only — reject private ones (and never flips
    // visibility, so `public` is not part of the patch).
    require_public_dashboard(server, &id).await?;
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
        public: None,
    };
    let d = server
        .control
        .dashboard_update(MCP_USER_ID, &id, patch, false)
        .await?;
    Ok(json!({
        "ok": true,
        "id": d.id,
        "name": d.name,
        "public": d.public,
        "updated_at": d.updated_at,
        "url": format!("/dashboards/{}", d.id),
    }))
}

// ----------------- alert tools -----------------

/// Tool-facing alert row: ms timestamps converted to ISO for LLM friendliness.
fn alert_row(a: &Alert) -> Value {
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
}

fn ms_to_iso(ms: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(ms).map(|t| t.to_rfc3339())
}

async fn list_alerts(server: &McpServer, args: &Value) -> Result<Value> {
    let unacked_only = arg_bool(args, "unacked_only", false);
    let search = arg_str(args, "search");
    let monitor = arg_str(args, "monitor");
    let limit = arg_usize(args, "limit", 50).clamp(1, 200);
    let items = server
        .control
        .alert_list(
            MCP_USER_ID,
            unacked_only,
            monitor.as_deref(),
            search.as_deref(),
            limit,
        )
        .await?;
    let rows: Vec<Value> = items.iter().map(alert_row).collect();
    Ok(json!({ "alerts": rows }))
}

async fn acknowledge_alert(server: &McpServer, args: &Value) -> Result<Value> {
    let id = arg_str(args, "id").context("acknowledge_alert requires `id`")?;
    if !server.control.alert_acknowledge(MCP_USER_ID, &id).await? {
        return Err(anyhow!("alert {id} not found"));
    }
    Ok(json!({ "ok": true, "id": id, "acknowledged": true }))
}

// ----------------- monitor tools -----------------

/// Tool-facing monitor row: ms timestamps converted to ISO; threshold config
/// inlined for threshold monitors, prompt for AI ones.
fn monitor_row(m: &Monitor) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "description": m.description,
        "kind": m.kind,
        "prompt": if m.prompt.is_empty() { Value::Null } else { json!(m.prompt) },
        "threshold": m.threshold,
        "interval_seconds": m.interval_seconds,
        "enabled": m.enabled,
        "public": m.public,
        "env": m.env,
        "last_run_at": m.last_run_at.and_then(ms_to_iso),
        "last_status": m.last_status,
        "url": "/monitors",
    })
}

async fn list_monitors(server: &McpServer) -> Result<Value> {
    // Not env-scoped: own (mcp) + public monitors across the instance.
    let items = server.control.monitor_list(MCP_USER_ID).await?;
    let rows: Vec<Value> = items.iter().map(monitor_row).collect();
    Ok(json!({ "monitors": rows }))
}

/// Reject a private monitor: MCP may only edit public ones. Errors if missing,
/// not visible, or private.
async fn require_public_monitor(server: &McpServer, id: &str) -> Result<()> {
    let m = server
        .control
        .monitor_get(MCP_USER_ID, id, false)
        .await?
        .ok_or_else(|| anyhow!("monitor {id} not found"))?;
    if !m.public {
        return Err(anyhow!(
            "monitor {id} is private; MCP can only update public monitors"
        ));
    }
    Ok(())
}

async fn create_monitor(server: &McpServer, args: &Value) -> Result<Value> {
    // MonitorInput mirrors POST /api/monitors; `env` is separate (the run-target).
    let mut input: MonitorInput =
        serde_json::from_value(args.clone()).context("invalid monitor fields")?;
    // MCP creates public-only — it has no per-user identity to own private rows.
    input.public = true;
    let env = crate::catalog::env_or_default(arg_str(args, "env").as_deref())?;
    let m = server
        .control
        .monitor_create(MCP_USER_ID, &env, input)
        .await?;
    Ok(monitor_row(&m))
}

async fn update_monitor(server: &McpServer, args: &Value) -> Result<Value> {
    let id = arg_str(args, "id").context("update_monitor requires `id`")?;
    // MCP manages public monitors only — reject private ones.
    require_public_monitor(server, &id).await?;
    let mut patch: MonitorPatch =
        serde_json::from_value(args.clone()).context("invalid monitor patch")?;
    // Never let MCP flip visibility (would orphan it from MCP management).
    patch.public = None;
    let m = server
        .control
        .monitor_update(MCP_USER_ID, &id, patch, false)
        .await?;
    Ok(monitor_row(&m))
}

/// Synchronous ingest that commits inline (no background committer outside
/// `helioslogs serve`), so events are queryable as soon as the call returns.
fn ingest_events(
    server: &McpServer,
    args: &Value,
    settings: &McpSettings,
    valid_envs: &std::collections::HashSet<String>,
) -> Result<Value> {
    let index = arg_str(args, "index").context("ingest_events requires `index`")?;
    if !crate::catalog::valid_index_name(&index) {
        return Err(anyhow!(
            "invalid index name '{index}' (lowercase letters, digits, '-', '_'; max 64 chars)"
        ));
    }
    // Writes must commit to a concrete env — default to `default` when the
    // caller doesn't supply one.
    let env = crate::catalog::env_or_default(arg_str(args, "env").as_deref())?;
    // `_`-prefixed *envs* are reserved for the system; index names are
    // unrestricted (the `_` prefix is just a naming convention).
    if env.starts_with('_') {
        return Err(anyhow!(
            "env names starting with '_' are reserved for the system"
        ));
    }
    // Only ingest into a registered environment (same rule as HTTP ingest).
    if !valid_envs.contains(&env) {
        return Err(anyhow!(
            "unknown environment '{env}' — create it first via Admin → Environments"
        ));
    }
    // Writes share the read allowlist; the realistic use case is scoping MCP
    // to a sandbox index where both surfaces need the same restriction.
    if !settings.allows(&env, &index) {
        return Err(anyhow!(
            "index '{index}' in env '{env}' not in the MCP allowlist; cannot ingest"
        ));
    }
    let default_source = arg_str(args, "source");
    let default_source_ref = default_source.as_deref();

    let mut events: Vec<Value> = Vec::new();
    if let Some(arr) = args.get("events").and_then(Value::as_array) {
        events.extend(arr.iter().cloned());
    }
    if let Some(s) = args.get("ndjson").and_then(Value::as_str) {
        for (lineno, line) in s.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(line)
                .with_context(|| format!("ndjson line {} parse error", lineno + 1))?;
            events.push(v);
        }
    }
    if events.is_empty() {
        return Err(anyhow!(
            "ingest_events: neither `events` nor `ndjson` was provided (or both empty)"
        ));
    }

    let t0 = Instant::now();
    let today = Utc::now().date_naive();
    let mut by_day: std::collections::HashMap<NaiveDate, Vec<Value>> =
        std::collections::HashMap::new();
    for v in events {
        let day = event_day(&v).unwrap_or(today);
        by_day.entry(day).or_default().push(v);
    }

    let mut ingested = 0usize;
    let mut parse_errors = 0usize;
    let mut partitions_touched: Vec<String> = Vec::new();

    // Immediate write (read-your-writes): the block engine appends a block at once.
    let block = crate::engine::block::BlockEngine::with_store(
        crate::engine::block::configured_store(server.catalog.root()),
        crate::engine::block_codec(),
    );
    for (day, evs) in by_day {
        let key = PartitionKey::new(&env, &index, day);
        let (added, errs) = block.ingest(&key, &evs, default_source_ref)?;
        ingested += added;
        parse_errors += errs;
        partitions_touched.push(format!("{}/{}", key.index, key.day_string()));
    }

    Ok(json!({
        "ingested": ingested,
        "parse_errors": parse_errors,
        "partitions": partitions_touched,
        "took_ms": t0.elapsed().as_millis() as u64,
    }))
}

// ----------------- arg helpers -----------------

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn arg_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
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

/// Parse `start`/`end` with the same rules as the HTTP layer; duplicated
/// here to avoid a dotted-line dep on `crate::http::time`.
#[allow(clippy::type_complexity)]
fn arg_range(
    args: &Value,
    default_start: &str,
    default_end: &str,
) -> Result<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> {
    let start = parse_time(arg_str(args, "start").as_deref().unwrap_or(default_start));
    let end = parse_time(arg_str(args, "end").as_deref().unwrap_or(default_end));
    Ok((start, end))
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

/// Picks a sensible histogram bucket for a window — mirrors `http::time::auto_interval`.
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

fn directory_size(dir: &std::path::Path) -> (u64, usize) {
    let mut total = 0u64;
    let mut files = 0usize;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    for e in entries.flatten() {
        if let Ok(meta) = e.metadata() {
            if meta.is_file() {
                total += meta.len();
                files += 1;
            }
        }
    }
    (total, files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::settings::EnvIndexAllow;

    // ---- arg extraction -----------------------------------------------------

    #[test]
    fn arg_helpers_extract_with_defaults() {
        let args = json!({"s": "x", "empty": "", "n": 7, "flag": true});
        assert_eq!(arg_str(&args, "s"), Some("x".to_string()));
        assert_eq!(arg_str(&args, "empty"), None); // empty filtered
        assert_eq!(arg_str(&args, "missing"), None);
        assert!(arg_bool(&args, "flag", false));
        assert!(arg_bool(&args, "missing", true)); // default
        assert_eq!(arg_usize(&args, "n", 0), 7);
        assert_eq!(arg_usize(&args, "missing", 20), 20);
        assert_eq!(arg_u32(&args, "n", 0), 7);
        assert_eq!(arg_u32(&args, "missing", 50), 50);
    }

    // ---- time parsing (note: MCP supports weeks `w`) ------------------------

    #[test]
    fn parse_duration_includes_weeks() {
        assert_eq!(parse_duration("2w"), Some(Duration::weeks(2)));
        assert_eq!(parse_duration("30s"), Some(Duration::seconds(30)));
        assert_eq!(parse_duration("bad"), None);
    }

    #[test]
    fn parse_time_handles_formats() {
        assert!(parse_time("").is_none());
        assert_eq!(parse_time("0").unwrap().timestamp_millis(), 0);
        assert!(parse_time("now").is_some());
        assert!(parse_time("-1h").is_some());
        assert!(parse_time("2026-01-01T00:00:00Z").is_some());
    }

    #[test]
    fn auto_interval_buckets() {
        assert_eq!(auto_interval(None, None), "60m"); // 1h default -> 60 1m buckets
        let now = Utc::now();
        assert_eq!(
            auto_interval(Some(now), Some(now + Duration::minutes(1))),
            "10s"
        );
    }

    // ---- tool descriptor ----------------------------------------------------

    #[test]
    fn tool_descriptor_uses_input_schema_key() {
        let t = tool("query_logs", "search", json!({"type": "object"}));
        assert_eq!(t["name"], "query_logs");
        assert_eq!(t["description"], "search");
        assert_eq!(t["inputSchema"]["type"], "object");
    }

    #[test]
    fn catalog_includes_dashboard_and_alert_tools() {
        let tools = all_tools();
        for n in [
            "list_dashboards",
            "get_dashboard",
            "create_dashboard",
            "update_dashboard",
            "list_alerts",
            "acknowledge_alert",
            "list_monitors",
            "create_monitor",
            "update_monitor",
        ] {
            assert!(
                tools.iter().any(|t| t["name"] == n),
                "missing tool descriptor: {n}"
            );
        }
    }

    async fn test_server() -> McpServer {
        use crate::control::store::build_control_store;
        let cd = tempfile::TempDir::new().unwrap();
        let store = build_control_store(None, cd.path()).await.unwrap();
        let crypto = Arc::new(crate::control::crypto::Crypto::new(false).unwrap());
        let control = crate::control::Control::new(store, crypto);
        control.upsert_env("default", false).await.unwrap();
        // Leak the temp dir so the control store outlives the test body.
        std::mem::forget(cd);
        McpServer {
            catalog: crate::catalog::Catalog::open(std::env::temp_dir()).unwrap(),
            fields: crate::schema::build_schema(),
            control,
        }
    }

    #[tokio::test]
    async fn mcp_monitors_are_public_only() {
        let server = test_server().await;

        // create forces public even when the caller asks for private.
        let m = create_monitor(
            &server,
            &json!({ "name": "m", "prompt": "watch errors", "public": false }),
        )
        .await
        .unwrap();
        assert_eq!(m["public"], json!(true));
        let pub_id = m["id"].as_str().unwrap().to_string();

        // updating a public monitor works, and can't flip it private.
        let upd = update_monitor(
            &server,
            &json!({ "id": pub_id, "name": "renamed", "public": false }),
        )
        .await
        .unwrap();
        assert_eq!(upd["name"], json!("renamed"));
        assert_eq!(upd["public"], json!(true));

        // a private monitor (owned by mcp) is rejected on update.
        let priv_mon = server
            .control
            .monitor_create(
                MCP_USER_ID,
                "default",
                MonitorInput {
                    name: "priv".into(),
                    prompt: "x".into(),
                    public: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let err = update_monitor(&server, &json!({ "id": priv_mon.id, "name": "n" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("private"), "got: {err}");
    }

    #[tokio::test]
    async fn mcp_dashboards_are_public_only() {
        let server = test_server().await;

        // create forces public.
        let d = create_dashboard(&server, &json!({ "name": "d", "public": false }))
            .await
            .unwrap();
        assert_eq!(d["public"], json!(true));

        // a private dashboard (owned by mcp) is rejected on update.
        let priv_dash = server
            .control
            .dashboard_create(
                MCP_USER_ID,
                DashboardInput {
                    name: "priv".into(),
                    description: String::new(),
                    spec: json!({ "widgets": [] }),
                    public: false,
                },
            )
            .await
            .unwrap();
        let err = update_dashboard(&server, &json!({ "id": priv_dash.id, "name": "n" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("private"), "got: {err}");
    }

    // ---- gate enforcement ---------------------------------------------------

    fn settings(enabled: bool, allowed: Vec<EnvIndexAllow>) -> McpSettings {
        McpSettings {
            enabled,
            allowed,
            enabled_tools: vec!["*".into()],
        }
    }

    fn open() -> Vec<EnvIndexAllow> {
        vec![EnvIndexAllow {
            env: "*".into(),
            indexes: vec!["*".into()],
        }]
    }

    #[test]
    fn gate_rejects_when_disabled() {
        let s = settings(false, open());
        assert!(check_gates(&s, "query_logs", &json!({})).is_err());
    }

    #[test]
    fn gate_enforces_tool_allowlist() {
        let mut s = settings(true, open());
        s.enabled_tools = vec!["histogram".into()];
        assert!(check_gates(&s, "query_logs", &json!({})).is_err());
        assert!(check_gates(&s, "histogram", &json!({})).is_ok());
    }

    #[test]
    fn gate_enforces_index_allowlist() {
        let allowed = vec![EnvIndexAllow {
            env: "prod".into(),
            indexes: vec!["orders-*".into()],
        }];
        let s = settings(true, allowed);
        // Allowed index in allowed env -> ok.
        assert!(check_gates(
            &s,
            "query_logs",
            &json!({"env": "prod", "index": "orders-eu"})
        )
        .is_ok());
        // Disallowed index -> err.
        assert!(check_gates(
            &s,
            "query_logs",
            &json!({"env": "prod", "index": "billing"})
        )
        .is_err());
        // Env not covered -> err.
        assert!(check_gates(&s, "query_logs", &json!({"env": "dev"})).is_err());
    }
}
