// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/envs` + `/api/admin/envs` — env catalog endpoints (list/create/delete) and
//! per-user grant sets. Listing is RBAC-scoped; admin actions reject reserved envs.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;

use super::auth::Principal;
use super::AppState;

#[derive(Deserialize, Default)]
pub(super) struct ListEnvsParams {
    /// Include reserved system envs (`_system`). Admin-only (ignored for non-admins); used by the MCP allowlist UI.
    #[serde(default)]
    include_system: bool,
}

pub(super) async fn list_envs_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<ListEnvsParams>,
) -> impl IntoResponse {
    let want_system = p.include_system && principal.is_admin;
    let all = match s.control.list_envs(want_system).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            );
        }
    };
    // The admin-set login default rides along so the picker / admin page can
    // mark it; `None` when unset or pointing at a deleted env.
    let default_env = s.control.default_env().await.ok().flatten();
    // Admins always see every (non-system) env. Non-admins filter to
    // their explicit grants.
    if principal.is_admin {
        return (
            StatusCode::OK,
            Json(json!({ "envs": all, "default_env": default_env })),
        );
    }
    let rules = match s.control.user_allowed(&principal.user_id).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            );
        }
    };
    // Empty allowlist is the "unrestricted" sentinel — same semantics
    // as enforce_env_access. Return every non-system env.
    if rules.is_empty() {
        return (
            StatusCode::OK,
            Json(json!({ "envs": all, "default_env": default_env })),
        );
    }
    let granted: HashSet<String> = rules.into_iter().map(|r| r.env).collect();
    let filtered: Vec<_> = all
        .into_iter()
        .filter(|e| granted.contains(&e.name))
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "envs": filtered, "default_env": default_env })),
    )
}

#[derive(Deserialize)]
pub(super) struct CreateEnvRequest {
    pub name: String,
}

pub(super) async fn create_env_handler(
    State(s): State<AppState>,
    Json(req): Json<CreateEnvRequest>,
) -> impl IntoResponse {
    match s.control.create_env(&req.name).await {
        Ok(env) => (StatusCode::OK, Json(json!({ "env": env }))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
pub(super) struct ReorderEnvsRequest {
    /// User env names in the desired picker order (ascending).
    pub names: Vec<String>,
}

pub(super) async fn reorder_envs_handler(
    State(s): State<AppState>,
    Json(req): Json<ReorderEnvsRequest>,
) -> impl IntoResponse {
    match s.control.reorder_envs(&req.names).await {
        Ok(envs) => (StatusCode::OK, Json(json!({ "envs": envs }))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
pub(super) struct SetDefaultEnvRequest {
    pub name: String,
}

pub(super) async fn set_default_env_handler(
    State(s): State<AppState>,
    Json(req): Json<SetDefaultEnvRequest>,
) -> impl IntoResponse {
    match s.control.set_default_env(&req.name).await {
        Ok(name) => (StatusCode::OK, Json(json!({ "default_env": name }))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn clear_default_env_handler(State(s): State<AppState>) -> impl IntoResponse {
    match s.control.clear_default_env().await {
        Ok(()) => (StatusCode::OK, Json(json!({ "default_env": null }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn list_user_allowed_handler(
    State(s): State<AppState>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    match s.control.user_allowed(&user_id).await {
        Ok(allowed) => (StatusCode::OK, Json(json!({ "allowed": allowed }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
pub(super) struct SetUserAllowedRequest {
    pub allowed: Vec<crate::control::settings::EnvIndexAllow>,
}

pub(super) async fn set_user_allowed_handler(
    State(s): State<AppState>,
    Path(user_id): Path<String>,
    Json(req): Json<SetUserAllowedRequest>,
) -> impl IntoResponse {
    // Validate each env exists before persisting — surfaces a cleaner
    // error than the FK violation we'd otherwise get.
    for r in &req.allowed {
        match s.control.env_exists(&r.env).await {
            Ok(true) => {}
            Ok(false) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("env '{}' does not exist", r.env) })),
                );
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                );
            }
        }
    }
    match s.control.set_user_allowed(&user_id, &req.allowed).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "allowed": req.allowed }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
pub(super) struct PatchEnvRequest {
    /// Days to keep this env's partitions; null/0 clears the override
    /// (falls back to the global default).
    pub retention_days: Option<i64>,
}

pub(super) async fn patch_env_handler(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<PatchEnvRequest>,
) -> impl IntoResponse {
    match s.control.set_env_retention(&name, req.retention_days).await {
        Ok(env) => (StatusCode::OK, Json(json!({ "env": env }))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn delete_env_handler(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    // Disk check first — partitions still on disk would be orphaned if we
    // deleted the control row (they'd show as "unknown" in the env catalog).
    let on_disk: Vec<_> = s
        .catalog
        .list_partitions()
        .into_iter()
        .filter(|k| k.env == name)
        .collect();
    if !on_disk.is_empty() {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!(
                    "env '{name}' still owns {} partition(s) on disk; \
                     delete the partitions first via /api/admin/partitions",
                    on_disk.len()
                ),
            })),
        );
    }
    match s.control.delete_env_if_no_control_rows(&name).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}
