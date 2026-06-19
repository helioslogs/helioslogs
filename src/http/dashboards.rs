// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/dashboards` CRUD — list / create / get / patch / delete dashboards.
//! Per-user storage in [`crate::control`] (`dashboards/<id>.json`). Not
//! env-scoped: query widgets follow the caller's active env at view time.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;

use crate::control::dashboards::{DashboardInput, DashboardPatch};

use super::auth::Principal;
use super::AppState;

#[derive(Deserialize, Default)]
pub(super) struct ListParam {
    /// Admin-only: include every user's dashboards. Ignored for non-admins.
    #[serde(default)]
    all: bool,
}

pub(super) async fn list_dashboards_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<ListParam>,
) -> impl IntoResponse {
    let result = if p.all && principal.is_admin {
        s.control.dashboard_list_all().await
    } else {
        s.control.dashboard_list(&principal.user_id).await
    };
    match result {
        Ok(items) => (StatusCode::OK, Json(json!({ "dashboards": items }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn create_dashboard_handler(
    State(s): State<AppState>,
    principal: Principal,
    Json(input): Json<DashboardInput>,
) -> impl IntoResponse {
    match s.control.dashboard_create(&principal.user_id, input).await {
        Ok(d) => (StatusCode::OK, Json(json!(d))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn get_dashboard_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s
        .control
        .dashboard_get(&principal.user_id, &id, principal.is_admin)
        .await
    {
        Ok(d) => (StatusCode::OK, Json(json!(d))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

pub(super) async fn update_dashboard_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(patch): Json<DashboardPatch>,
) -> impl IntoResponse {
    match s
        .control
        .dashboard_update(&principal.user_id, &id, patch, principal.is_admin)
        .await
    {
        Ok(d) => (StatusCode::OK, Json(json!(d))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

pub(super) async fn delete_dashboard_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s
        .control
        .dashboard_delete(&principal.user_id, &id, principal.is_admin)
        .await
    {
        Ok(()) => (StatusCode::OK, Json(json!({ "deleted": id }))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

/// 404 only for the "not found" path (which also covers ownership mismatches on
/// private rows, by design); everything else surfaces as 500.
fn status_for(e: &anyhow::Error) -> StatusCode {
    if e.to_string().contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}
