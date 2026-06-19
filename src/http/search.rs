// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/stats`, `/api/search`, `/api/aggregate`, `/api/histogram` — a pure HTTP
//! shell over [`crate::search`] (request parsing, response shaping, error mapping).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;

use crate::control::settings::EnvIndexAllow;
use crate::search;
use crate::search::discover;

use super::auth::{enforce_env_access, resolve_request_env, Principal};
use super::time::{auto_interval, parse_time};
use super::AppState;

/// Returns the per-user partition allowlist passed to scatter as
/// `extra_allow`. Admins bypass (empty slice = no extra restriction).
async fn user_allow(
    control: &crate::control::Control,
    principal: &Principal,
) -> Result<Vec<EnvIndexAllow>, (StatusCode, Json<serde_json::Value>)> {
    if principal.is_admin {
        return Ok(Vec::new());
    }
    control.user_allowed(&principal.user_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })
}

/// Env scope for a search-side request: explicit `?env=` wins, else the session's
/// active env. Enforces RBAC on the override; errors bubble as 400/403.
async fn pick_search_env(
    control: &crate::control::Control,
    query_env: Option<&str>,
    principal: &Principal,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let env = resolve_request_env(query_env, principal).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
    })?;
    // Enforce on both paths (URL override + session-active) so a revoked grant
    // takes effect on the next request, not only on an explicit env switch.
    enforce_env_access(control, principal, &env).await?;
    Ok(env)
}

#[derive(Deserialize, Default)]
pub(super) struct SearchParams {
    #[serde(default)]
    q: String,
    /// Environment scope; absent = scan every env (pre-env-picker behavior).
    env: Option<String>,
    index: Option<String>,
    start: Option<String>,
    end: Option<String>,
    /// 0-based offset into the merged result set for page-style pagination.
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
    /// Comma-separated `env:index:yyyy-mm-dd` triples bounding the scatter (still AND-ed with RBAC); drives day-by-day streaming.
    partitions: Option<String>,
}

fn default_limit() -> usize {
    50
}

#[derive(Deserialize, Default)]
pub(super) struct SearchHistogramParams {
    #[serde(default)]
    q: String,
    env: Option<String>,
    index: Option<String>,
    start: Option<String>,
    end: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
    interval: Option<String>,
    /// See `SearchParams::partitions`.
    partitions: Option<String>,
}

#[derive(Deserialize, Default)]
pub(super) struct HistogramParams {
    #[serde(default)]
    q: String,
    env: Option<String>,
    index: Option<String>,
    start: Option<String>,
    end: Option<String>,
    interval: Option<String>,
    /// See `SearchParams::partitions`.
    partitions: Option<String>,
}

/// Parses the comma-separated `partitions=` param (`env:index:yyyy-mm-dd` per key) into
/// `Vec<PartitionKey>`; `Ok(None)` when absent/empty means "no bound, use the planned set".
fn parse_partitions_param(
    raw: Option<&str>,
) -> Result<Option<Vec<crate::catalog::PartitionKey>>, String> {
    let s = match raw {
        Some(s) if !s.trim().is_empty() => s,
        _ => return Ok(None),
    };
    let mut out = Vec::new();
    for tok in s.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        let parts: Vec<&str> = tok.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(format!(
                "invalid partition `{tok}`: expected `env:index:yyyy-mm-dd`"
            ));
        }
        let day = chrono::NaiveDate::parse_from_str(parts[2], "%Y-%m-%d")
            .map_err(|e| format!("invalid date in partition `{tok}`: {e}"))?;
        out.push(crate::catalog::PartitionKey::new(parts[0], parts[1], day));
    }
    Ok(Some(out))
}

#[derive(Deserialize, Default)]
pub(super) struct SearchPartitionsParams {
    #[serde(default)]
    q: String,
    env: Option<String>,
    index: Option<String>,
    start: Option<String>,
    end: Option<String>,
}

#[derive(Deserialize, Default)]
pub(super) struct AggregateParams {
    #[serde(default)]
    q: String,
    env: Option<String>,
    index: Option<String>,
    start: Option<String>,
    end: Option<String>,
    fields: Option<String>,
    #[serde(default = "default_agg_size")]
    size: u32,
    /// Opt into partition sampling past `HELIOS_AGG_MAX_PARTITIONS` (stride-sample + scale up); response carries `sampled` flags.
    #[serde(default)]
    approximate: bool,
}

fn default_agg_size() -> u32 {
    10
}

#[derive(Deserialize, Default)]
pub(super) struct DiscoverParams {
    #[serde(default)]
    q: String,
    env: Option<String>,
    index: Option<String>,
    start: Option<String>,
    end: Option<String>,
    /// Max number of fields to return (clamped to [1, MAX_TOP]).
    top: Option<usize>,
}

/// Maps a search fn's `anyhow::Result<T>` onto the handler tuple, also unwrapping the
/// `spawn_blocking` `JoinError` so a worker panic becomes a 500, not a hang.
fn search_result_to_response<T: serde::Serialize>(
    joined: Result<anyhow::Result<T>, tokio::task::JoinError>,
) -> (StatusCode, Json<serde_json::Value>) {
    match joined {
        Ok(Ok(r)) => (StatusCode::OK, Json(json!(r))),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("search worker panicked: {e}") })),
        ),
    }
}

pub(super) async fn stats_handler(State(s): State<AppState>) -> impl IntoResponse {
    let joined = tokio::task::spawn_blocking(move || search::stats(&s.catalog)).await;
    // stats() failures are 500s (catalog-internal), not 400s.
    match joined {
        Ok(Ok(st)) => (StatusCode::OK, Json(json!(st))),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("stats worker panicked: {e}") })),
        ),
    }
}

pub(super) async fn search_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<SearchParams>,
) -> impl IntoResponse {
    let env = match pick_search_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let allow = match user_allow(&s.control, &principal).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let explicit_partitions = match parse_partitions_param(p.partitions.as_deref()) {
        Ok(v) => v,
        Err(msg) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))),
    };
    let start = parse_time(p.start.as_deref());
    let end = parse_time(p.end.as_deref());
    let joined = tokio::task::spawn_blocking(move || {
        search::search(
            &s.catalog,
            &s.fields,
            &p.q,
            Some(env.as_str()),
            p.index.as_deref(),
            start,
            end,
            p.offset,
            p.limit,
            &allow,
            explicit_partitions,
        )
    })
    .await;
    search_result_to_response(joined)
}

pub(super) async fn aggregate_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<AggregateParams>,
) -> impl IntoResponse {
    let env = match pick_search_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let allow = match user_allow(&s.control, &principal).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let start = parse_time(p.start.as_deref());
    let end = parse_time(p.end.as_deref());
    // Schema-on-read: the caller must specify which fields to aggregate
    // (driven by field discovery); there is no hardcoded default.
    let fields: Vec<String> = match p.fields.as_deref() {
        Some(s) if !s.is_empty() => s.split(',').map(|x| x.trim().to_string()).collect(),
        _ => Vec::new(),
    };
    let joined = tokio::task::spawn_blocking(move || {
        search::aggregate(
            &s.catalog,
            &s.fields,
            &p.q,
            Some(env.as_str()),
            p.index.as_deref(),
            start,
            end,
            &fields,
            p.size,
            p.approximate,
            &allow,
        )
    })
    .await;
    search_result_to_response(joined)
}

pub(super) async fn discover_fields_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<DiscoverParams>,
) -> impl IntoResponse {
    let env = match pick_search_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let allow = match user_allow(&s.control, &principal).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let start = parse_time(p.start.as_deref());
    let end = parse_time(p.end.as_deref());
    let top = p.top.unwrap_or(discover::DEFAULT_TOP);
    // Footer-derived field list: the true (path, type) columns over the window, stable and
    // independent of the predicate. (Agent/MCP use the sampling `discover_fields` instead.)
    let joined = tokio::task::spawn_blocking(move || {
        discover::field_catalog(
            &s.catalog,
            &p.q,
            Some(env.as_str()),
            p.index.as_deref(),
            start,
            end,
            top,
            &allow,
        )
    })
    .await;
    search_result_to_response(joined)
}

pub(super) async fn histogram_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<HistogramParams>,
) -> impl IntoResponse {
    let env = match pick_search_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let allow = match user_allow(&s.control, &principal).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let explicit_partitions = match parse_partitions_param(p.partitions.as_deref()) {
        Ok(v) => v,
        Err(msg) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))),
    };
    let start = parse_time(p.start.as_deref());
    let end = parse_time(p.end.as_deref());
    let interval = p
        .interval
        .clone()
        .unwrap_or_else(|| auto_interval(start, end));
    let joined = tokio::task::spawn_blocking(move || {
        search::histogram(
            &s.catalog,
            &s.fields,
            &p.q,
            Some(env.as_str()),
            p.index.as_deref(),
            start,
            end,
            &interval,
            &allow,
            explicit_partitions,
        )
    })
    .await;
    search_result_to_response(joined)
}

/// `GET /api/search_histogram` — hits page + histogram in one fan-out (filter evaluated
/// once per partition vs. a separate pair). Same params as `/search` plus `interval`.
pub(super) async fn search_histogram_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<SearchHistogramParams>,
) -> impl IntoResponse {
    let env = match pick_search_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let allow = match user_allow(&s.control, &principal).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let explicit_partitions = match parse_partitions_param(p.partitions.as_deref()) {
        Ok(v) => v,
        Err(msg) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))),
    };
    let start = parse_time(p.start.as_deref());
    let end = parse_time(p.end.as_deref());
    let interval = p
        .interval
        .clone()
        .unwrap_or_else(|| auto_interval(start, end));
    let joined = tokio::task::spawn_blocking(move || {
        search::search_histogram(
            &s.catalog,
            &s.fields,
            &p.q,
            Some(env.as_str()),
            p.index.as_deref(),
            start,
            end,
            p.offset,
            p.limit,
            &interval,
            &allow,
            explicit_partitions,
        )
    })
    .await;
    search_result_to_response(joined)
}

/// `GET /api/search_partitions` — ordered list of partitions the query+range would scan,
/// most recent day first. The frontend plans per-day streaming calls from this.
pub(super) async fn search_partitions_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<SearchPartitionsParams>,
) -> impl IntoResponse {
    let env = match pick_search_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let allow = match user_allow(&s.control, &principal).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let start = parse_time(p.start.as_deref());
    let end = parse_time(p.end.as_deref());
    let joined = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let plan = crate::search::scatter::plan(
            &s.catalog,
            &p.q,
            Some(env.as_str()),
            p.index.as_deref(),
            start,
            end,
            &allow,
        )?;
        // Most-recent day first so the streaming UI surfaces freshest results soonest;
        // ties within a day break by (env, index) for stable ordering.
        let mut keys = plan.keys;
        keys.sort_by(|a, b| {
            b.day
                .cmp(&a.day)
                .then_with(|| a.env.cmp(&b.env))
                .then_with(|| a.index.cmp(&b.index))
        });
        let total = keys.len();
        let partitions: Vec<serde_json::Value> = keys
            .into_iter()
            .map(|k| {
                json!({
                    "env": k.env,
                    "index": k.index,
                    "day": k.day_string(),
                })
            })
            .collect();
        Ok(json!({ "total": total, "partitions": partitions }))
    })
    .await;
    match joined {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("worker panicked: {e}") })),
        ),
    }
}
