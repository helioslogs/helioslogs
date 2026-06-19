// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/admin/api-keys` — admin CRUD for API keys. The secret is returned in full
//! once on create; listings show a masked hint. A key authenticates REST calls
//! through the same bearer path as a JWT (see [`super::auth`]) and/or the MCP server,
//! per its multi-select scope (standard API, admin API, MCP).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::control::api_keys::ApiKeyInput;

use super::auth::Principal;
use super::AppState;

fn err(status: StatusCode, msg: String) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

pub(super) async fn list_handler(State(s): State<AppState>) -> impl IntoResponse {
    match s.control.api_key_list().await {
        Ok(keys) => {
            let keys: Vec<_> = keys.iter().map(|k| k.view()).collect();
            (StatusCode::OK, Json(json!({ "keys": keys })))
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub(super) async fn create_handler(
    State(s): State<AppState>,
    principal: Principal,
    Json(input): Json<ApiKeyInput>,
) -> impl IntoResponse {
    if input.name.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "name is required".to_string());
    }
    if !input.scopes.any() {
        return err(
            StatusCode::BAD_REQUEST,
            "select at least one scope".to_string(),
        );
    }
    // A positive lifetime sets an expiry; absent / 0 / negative = never expires.
    let expires_at = input
        .expires_in_days
        .filter(|d| *d > 0)
        .map(|d| chrono::Utc::now().timestamp_millis() + d * 86_400_000);
    match s
        .control
        .api_key_create(
            &input.name,
            &input.description,
            input.scopes,
            expires_at,
            &principal.user_id,
        )
        .await
    {
        // Full secret returned ONCE — the UI must surface it now.
        Ok(k) => (
            StatusCode::OK,
            Json(json!({
                "id": k.id,
                "name": k.name,
                "description": k.description,
                "scopes": k.scopes,
                "expires_at": k.expires_at,
                "token": k.token,
            })),
        ),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
pub(super) struct EnabledPatch {
    enabled: bool,
}

pub(super) async fn patch_handler(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(p): Json<EnabledPatch>,
) -> impl IntoResponse {
    match s.control.api_key_set_enabled(&id, p.enabled).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub(super) async fn delete_handler(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.control.api_key_delete(&id).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "deleted": id }))),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
