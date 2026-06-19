// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/admin/ingest-tokens` — admin CRUD for scoped push tokens plus the `require`
//! switch. The secret is returned in full once on create; listings show a masked hint.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::control::ingest_tokens::PushTokenInput;

use super::AppState;

fn err(status: StatusCode, msg: String) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

pub(super) async fn list_handler(State(s): State<AppState>) -> impl IntoResponse {
    match s.control.ingest_auth().await {
        Ok(a) => {
            let tokens: Vec<_> = a.tokens.iter().map(|t| t.view()).collect();
            (
                StatusCode::OK,
                Json(json!({
                    "require": a.require,
                    "api_enabled": a.api_enabled,
                    "shims_enabled": a.shims_enabled,
                    "tokens": tokens,
                })),
            )
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub(super) async fn create_handler(
    State(s): State<AppState>,
    Json(input): Json<PushTokenInput>,
) -> impl IntoResponse {
    let env = input.env.trim().to_string();
    if env.is_empty() {
        return err(StatusCode::BAD_REQUEST, "env is required".to_string());
    }
    if env.starts_with('_') {
        return err(
            StatusCode::BAD_REQUEST,
            "env names starting with '_' are reserved for the system".to_string(),
        );
    }
    match s.control.env_exists(&env).await {
        Ok(true) => {}
        Ok(false) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("unknown environment '{env}'"),
            );
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    match s
        .control
        .ingest_token_create(&input.name, &env, input.indexes)
        .await
    {
        // Full secret returned ONCE — the UI must surface it now.
        Ok(t) => (
            StatusCode::OK,
            Json(json!({
                "id": t.id,
                "name": t.name,
                "env": t.env,
                "indexes": t.indexes,
                "token": t.token,
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
    match s.control.ingest_token_set_enabled(&id, p.enabled).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub(super) async fn delete_handler(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.control.ingest_token_delete(&id).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "deleted": id }))),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Patch the ingest policy: the token `require` switch and/or the per-class
/// enable toggles. Each field is optional so callers can set just one.
#[derive(Deserialize)]
pub(super) struct PolicyPut {
    require: Option<bool>,
    api_enabled: Option<bool>,
    shims_enabled: Option<bool>,
}

pub(super) async fn set_require_handler(
    State(s): State<AppState>,
    Json(p): Json<PolicyPut>,
) -> impl IntoResponse {
    if let Some(require) = p.require {
        if let Err(e) = s.control.ingest_set_require(require).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    if p.api_enabled.is_some() || p.shims_enabled.is_some() {
        if let Err(e) = s
            .control
            .ingest_set_endpoints(p.api_enabled, p.shims_enabled)
            .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    match s.control.ingest_auth().await {
        Ok(a) => (
            StatusCode::OK,
            Json(json!({
                "require": a.require,
                "api_enabled": a.api_enabled,
                "shims_enabled": a.shims_enabled,
            })),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
