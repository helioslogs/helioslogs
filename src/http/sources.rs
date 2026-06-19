// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/sources` CRUD — list / create / patch / delete ingestion sources, plus
//! `:id/run` to trigger an immediate poll. Per-user, env-scoped storage on
//! [`crate::control::Control`]; the [`crate::source`] supervisor runs them.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;

use crate::control::sources::{SourceInput, SourcePatch};

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
    /// List every source the caller owns, across all envs (admin Source view).
    #[serde(default)]
    all: bool,
}

pub(super) async fn list_sources_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<EnvParam>,
) -> impl IntoResponse {
    // `?all=true` ignores the active env and returns every owned source so the
    // admin screen can show sources across all environments at once.
    if p.all {
        return match s.control.source_list_all_user(&principal.user_id).await {
            Ok(items) => (StatusCode::OK, Json(json!({ "sources": items }))),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            ),
        };
    }
    let env = match pick_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    match s.control.source_list(&principal.user_id, &env).await {
        Ok(items) => (StatusCode::OK, Json(json!({ "sources": items }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

/// One source plus its ingest checkpoint (per-file byte offsets) so the UI can
/// show what's been consumed and where each file is parked.
pub(super) async fn get_source_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.control.source_get(&principal.user_id, &id).await {
        Ok(Some(src)) => {
            let checkpoint = s
                .control
                .source_checkpoint_get(&id)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            (
                StatusCode::OK,
                Json(json!({ "source": src, "checkpoint": checkpoint })),
            )
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "source not found" })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(Deserialize, Default)]
pub(super) struct BrowseParam {
    path: Option<String>,
}

/// Admin-only server-side directory listing — backs the source-dialog folder picker.
/// Lists immediate sub-directories of `path` (defaults to `/`).
pub(super) async fn browse_handler(
    State(_s): State<AppState>,
    principal: Principal,
    Query(p): Query<BrowseParam>,
) -> impl IntoResponse {
    if !principal.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        );
    }
    let raw = p.path.unwrap_or_default();
    let path = if raw.trim().is_empty() {
        "/".to_string()
    } else {
        raw
    };
    match list_dirs(&path) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

/// Immediate child directories of `path`, hidden ones skipped, name-sorted.
fn list_dirs(path: &str) -> anyhow::Result<serde_json::Value> {
    use std::path::PathBuf;
    let p = PathBuf::from(path);
    let meta = std::fs::metadata(&p).map_err(|e| anyhow::anyhow!("cannot open {path}: {e}"))?;
    if !meta.is_dir() {
        anyhow::bail!("{path} is not a directory");
    }
    let mut dirs: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(&p)? {
        let entry = entry?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        dirs.push((name, entry.path().to_string_lossy().to_string()));
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    let abs = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
    let parent = abs.parent().map(|x| x.to_string_lossy().to_string());
    let entries: Vec<serde_json::Value> = dirs
        .into_iter()
        .map(|(name, path)| json!({ "name": name, "path": path }))
        .collect();
    Ok(json!({ "path": abs.to_string_lossy(), "parent": parent, "dirs": entries }))
}

pub(super) async fn create_source_handler(
    State(s): State<AppState>,
    principal: Principal,
    Query(p): Query<EnvParam>,
    Json(input): Json<SourceInput>,
) -> impl IntoResponse {
    let env = match pick_env(&s.control, p.env.as_deref(), &principal).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    match s
        .control
        .source_create(&principal.user_id, &env, input)
        .await
    {
        Ok(src) => (StatusCode::OK, Json(json!(src))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

pub(super) async fn update_source_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(patch): Json<SourcePatch>,
) -> impl IntoResponse {
    // Moving a source to another env requires access to the target env.
    if let Some(env) = patch.env.as_deref().filter(|e| !e.trim().is_empty()) {
        if let Err(r) = enforce_env_access(&s.control, &principal, env.trim()).await {
            return r;
        }
    }
    match s
        .control
        .source_update(&principal.user_id, &id, patch)
        .await
    {
        Ok(src) => (StatusCode::OK, Json(json!(src))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

pub(super) async fn delete_source_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.control.source_delete(&principal.user_id, &id).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "deleted": id }))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

pub(super) async fn run_source_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.control.source_run_now(&principal.user_id, &id).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "queued": id }))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

pub(super) async fn reset_source_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.control.source_reset(&principal.user_id, &id).await {
        Ok(src) => (StatusCode::OK, Json(json!(src))),
        Err(e) => (status_for(&e), Json(json!({ "error": e.to_string() }))),
    }
}

fn status_for(e: &anyhow::Error) -> StatusCode {
    let msg = e.to_string();
    if msg.contains("not found") {
        StatusCode::NOT_FOUND
    } else if msg.contains("is running") {
        StatusCode::CONFLICT
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}
