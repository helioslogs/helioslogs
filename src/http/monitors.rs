// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/monitors` + `/api/alerts` HTTP surfaces (storage on [`crate::control::Control`]).
//! Monitors aren't env-scoped (listing spans all envs) but run against the caller's active env.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::agent::AgentEvent;
use crate::control::monitors::{MonitorInput, MonitorKind, MonitorPatch};

use super::auth::{enforce_env_access, resolve_request_env, Principal};
use super::AppState;

#[derive(Deserialize, Default)]
pub(super) struct EnvParam {
    env: Option<String>,
    /// Admin-only: include every user's monitors. Ignored for non-admins.
    #[serde(default)]
    all: bool,
}

async fn pick_env(
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
    enforce_env_access(control, principal, &env).await?;
    Ok(env)
}

// ---- monitors -----------------------------------------------------------

pub(super) async fn list_monitors_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<EnvParam>,
) -> impl IntoResponse {
    let result = if p.all && principal.is_admin {
        s.control.monitor_list_admin().await
    } else {
        s.control.monitor_list(&principal.user_id).await
    };
    match result {
        Ok(items) => (StatusCode::OK, Json(json!({ "monitors": items }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn create_monitor_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<EnvParam>,
    Json(input): Json<MonitorInput>,
) -> impl IntoResponse {
    let env = match pick_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    match s
        .control
        .monitor_create(&principal.user_id, &env, input)
        .await
    {
        Ok(m) => (StatusCode::OK, Json(json!(m))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn get_monitor_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s
        .control
        .monitor_get(&principal.user_id, &id, principal.is_admin)
        .await
    {
        Ok(Some(m)) => (StatusCode::OK, Json(json!(m))),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn update_monitor_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(patch): Json<MonitorPatch>,
) -> impl IntoResponse {
    match s
        .control
        .monitor_update(&principal.user_id, &id, patch, principal.is_admin)
        .await
    {
        Ok(m) => (StatusCode::OK, Json(json!(m))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

pub(super) async fn delete_monitor_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s
        .control
        .monitor_delete(&principal.user_id, &id, principal.is_admin)
        .await
    {
        Ok(()) => (StatusCode::OK, Json(json!({ "deleted": id }))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

/// "Run now" trigger — clears `last_run_at` so the next scheduler tick runs it.
/// Allowed for the owner or any public monitor; 404 if not visible.
pub(super) async fn run_monitor_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let owns = match s.control.monitor_get(&principal.user_id, &id, false).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        }
    };
    if owns.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" })));
    }
    match s.control.monitor_clear_last_run(&id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "id": id,
                "message": "Will run on the next scheduler tick (within 10s)."
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

/// "Run & watch" — runs an AI monitor immediately and streams the agent's
/// investigation trace over SSE. The first event carries the conversation id;
/// the rest are the normal `AgentEvent`s the chat panel already renders.
pub(super) async fn run_monitor_live_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let err_resp = |code: StatusCode, msg: String| -> Response {
        (code, Json(json!({ "error": msg }))).into_response()
    };

    let monitor = match s.control.monitor_get(&principal.user_id, &id, false).await {
        Ok(Some(m)) => m,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "not found".into()),
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if monitor.kind != MonitorKind::Ai {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "Only AI monitors produce a live agent trace.".into(),
        );
    }
    match crate::agent::settings::AgentSettings::load(&s.control).await {
        Ok(cfg) if !cfg.enabled => {
            return err_resp(
                StatusCode::CONFLICT,
                "AI agent functionality is disabled by an administrator.".into(),
            )
        }
        Ok(_) => {}
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }

    // Lease so we never double-run with the scheduler; finish_run releases it.
    match s.control.monitor_try_lease(&id).await {
        Ok(true) => {}
        Ok(false) => return err_resp(StatusCode::CONFLICT, "Monitor is already running.".into()),
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }

    // Create the trace conversation up front so we can stream its id first.
    let conv_id =
        match crate::monitor::create_monitor_conversation(&s.control, &principal.user_id, &monitor)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                let _ = s
                    .control
                    .monitor_finish_run(&id, "error", Some(&format!("{e:#}")), None)
                    .await;
                return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
        };

    let (tx, rx) = mpsc::channel::<AgentEvent>(64);

    let catalog = s.catalog.clone();
    let fields = s.fields.clone();
    let control = s.control.clone();
    let owner = principal.user_id.clone();
    let mid = id.clone();
    let run_conv = conv_id.clone();
    tokio::spawn(async move {
        let result = crate::monitor::run_ai_monitor_into(
            &catalog,
            &fields,
            &control,
            &owner,
            &monitor,
            run_conv.clone(),
            tx.clone(),
        )
        .await;
        let (status, error) = match &result {
            Ok(()) => ("ok", None),
            Err(e) => ("error", Some(format!("{e:#}"))),
        };
        if let Err(e) = control
            .monitor_finish_run(&mid, status, error.as_deref(), Some(run_conv))
            .await
        {
            tracing::error!(monitor_id = mid, "live finish_run failed: {e:#}");
        }
        if let Err(e) = result {
            let _ = tx
                .send(AgentEvent::Error {
                    message: format!("{e:#}"),
                })
                .await;
        }
    });

    // Lead with the conversation id, then forward the agent event stream.
    let head = stream::once(async move {
        Ok::<_, Infallible>(
            Event::default().data(json!({ "type": "conversation", "id": conv_id }).to_string()),
        )
    });
    let body = ReceiverStream::new(rx).map(|evt| {
        let payload = serde_json::to_string(&evt).unwrap_or_else(|_| "{}".into());
        Ok::<_, Infallible>(Event::default().data(payload))
    });
    Sse::new(head.chain(body))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

pub(super) async fn list_monitor_alerts_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Verify ownership first so we don't leak existence.
    let owns = match s.control.monitor_get(&principal.user_id, &id, false).await {
        Ok(v) => v.is_some(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        }
    };
    if !owns {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" })));
    }
    match s
        .control
        .alert_list_for_monitor(&principal.user_id, &id)
        .await
    {
        Ok(items) => (StatusCode::OK, Json(json!({ "alerts": items }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

// ---- alerts -------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub(super) struct ListAlertsQuery {
    #[serde(default)]
    pub unacked: bool,
    /// Case-insensitive substring over title / summary / monitor / env.
    pub q: Option<String>,
    /// Restrict to one monitor's alerts.
    pub monitor: Option<String>,
    /// Cap on returned rows (default 100, max 500).
    pub limit: Option<usize>,
}

pub(super) async fn list_alerts_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(q): Query<ListAlertsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    match s
        .control
        .alert_list(
            &principal.user_id,
            q.unacked,
            q.monitor.as_deref(),
            q.q.as_deref(),
            limit,
        )
        .await
    {
        Ok(items) => (StatusCode::OK, Json(json!({ "alerts": items }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn alerts_unacked_count_handler(
    State(s): State<AppState>,
    principal: Principal,
) -> impl IntoResponse {
    match s.control.alert_unacked_count(&principal.user_id).await {
        Ok(n) => (StatusCode::OK, Json(json!({ "unacked": n }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
pub(super) struct AlertPatch {
    #[serde(default)]
    pub acknowledged: Option<bool>,
    /// Toast dismissal — only `true` is meaningful (toasts don't un-dismiss).
    #[serde(default)]
    pub dismissed: Option<bool>,
}

pub(super) async fn patch_alert_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(patch): Json<AlertPatch>,
) -> impl IntoResponse {
    // Pick the requested transition. `dismissed` is independent of `acknowledged`;
    // a single PATCH carries one or the other.
    let result = if patch.dismissed == Some(true) {
        s.control.alert_dismiss(&principal.user_id, &id).await
    } else if patch.acknowledged == Some(true) {
        s.control.alert_acknowledge(&principal.user_id, &id).await
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "set acknowledged=true or dismissed=true" })),
        );
    };
    match result {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true, "id": id }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn dismiss_all_alerts_handler(
    State(s): State<AppState>,
    principal: Principal,
) -> impl IntoResponse {
    match s.control.alert_dismiss_all(&principal.user_id).await {
        Ok(n) => (StatusCode::OK, Json(json!({ "ok": true, "dismissed": n }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

fn status_for(e: &anyhow::Error) -> StatusCode {
    if e.to_string().contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}
