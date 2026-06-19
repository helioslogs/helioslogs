// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/searches` CRUD — list / create / patch / delete saved searches.
//! Per-user storage in [`crate::control::saved::SavedSearchStore`].

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;

use crate::control::saved::{SavedSearchInput, SavedSearchPatch};

use super::auth::{enforce_env_access, resolve_request_env, Principal};
use super::AppState;

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

#[derive(Deserialize, Default)]
pub(super) struct EnvParam {
    env: Option<String>,
    /// Admin-only: include every user's searches (incl. private). Ignored for
    /// non-admins.
    #[serde(default)]
    all: bool,
}

pub(super) async fn list_searches_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<EnvParam>,
) -> impl IntoResponse {
    let env = match pick_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let result = if p.all && principal.is_admin {
        s.control.saved_list_all(&env).await
    } else {
        s.control.saved_list(&principal.user_id, &env).await
    };
    match result {
        Ok(items) => (StatusCode::OK, Json(json!({ "searches": items }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn create_search_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<EnvParam>,
    Json(input): Json<SavedSearchInput>,
) -> impl IntoResponse {
    let env = match pick_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    match s
        .control
        .saved_create(&principal.user_id, &env, input)
        .await
    {
        Ok(ss) => (StatusCode::OK, Json(json!(ss))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn update_search_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(patch): Json<SavedSearchPatch>,
) -> impl IntoResponse {
    match s
        .control
        .saved_update(&principal.user_id, &id, patch, principal.is_admin)
        .await
    {
        Ok(ss) => (StatusCode::OK, Json(json!(ss))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

pub(super) async fn delete_search_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s
        .control
        .saved_delete(&principal.user_id, &id, principal.is_admin)
        .await
    {
        Ok(()) => (StatusCode::OK, Json(json!({ "deleted": id }))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

/// Map a store error to an HTTP status. 404 only for "not found" (also covers private-row
/// ownership mismatches, by design); everything else is 500 so real bugs aren't hidden.
fn status_for(e: &anyhow::Error) -> StatusCode {
    if e.to_string().contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}
