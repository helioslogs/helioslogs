// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Admin user management — `/api/admin/users/*` (admin-only path prefix). Handlers
//! receive a [`Principal`] to refuse self-destructive actions like deleting yourself.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::json;

use crate::control::users::generate_password;

use super::auth::Principal;
use super::AppState;

#[derive(Deserialize)]
pub(super) struct CreateUserRequest {
    pub userid: String,
    pub email: String,
    pub display_name: String,
    #[serde(default)]
    pub is_admin: bool,
}

pub(super) async fn list_handler(State(s): State<AppState>, _principal: Principal) -> Response {
    match s.control.list_users().await {
        Ok(users) => (StatusCode::OK, Json(json!({ "users": users }))).into_response(),
        Err(e) => internal_error(e),
    }
}

/// Creates a user with a server-generated random password, returned **once** in the
/// response for the admin to hand off.
pub(super) async fn create_handler(
    State(s): State<AppState>,
    _principal: Principal,
    Json(req): Json<CreateUserRequest>,
) -> Response {
    let password = generate_password();
    match s
        .control
        .create_user(
            &req.userid,
            &req.email,
            &req.display_name,
            &password,
            req.is_admin,
        )
        .await
    {
        Ok(user) => (
            StatusCode::OK,
            Json(json!({ "user": user, "password": password })),
        )
            .into_response(),
        Err(e) => bad_request(e),
    }
}

#[derive(Deserialize, Default)]
pub(super) struct UpdateUserRequest {
    /// All fields optional. Omitted fields are left untouched.
    /// `userid` is intentionally not updatable — see [`crate::control::users::ControlDb::update_user`].
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub is_admin: Option<bool>,
}

pub(super) async fn update_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(req): Json<UpdateUserRequest>,
) -> Response {
    // Refuse to demote your own account (anti-lockout); other admins can demote you,
    // same posture as delete.
    if id == principal.user_id && req.is_admin == Some(false) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "you cannot remove your own admin role" })),
        )
            .into_response();
    }
    match s
        .control
        .update_user(
            &id,
            req.email.as_deref(),
            req.display_name.as_deref(),
            req.is_admin,
        )
        .await
    {
        Ok(user) => (StatusCode::OK, Json(json!({ "user": user }))).into_response(),
        Err(e) => {
            let msg = e.to_string();
            let code = if msg.contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (code, Json(json!({ "error": msg }))).into_response()
        }
    }
}

pub(super) async fn delete_handler(
    State(s): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    if id == principal.user_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "you cannot delete your own account" })),
        )
            .into_response();
    }
    match s.control.delete_user(&id).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "deleted": id }))).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("user {id} not found") })),
        )
            .into_response(),
        Err(e) => internal_error(e),
    }
}

/// Regenerates a user's password and invalidates their sessions, returning the new
/// plaintext — the admin's lockout-recovery path (no email flow by design).
pub(super) async fn regenerate_password_handler(
    State(s): State<AppState>,
    _principal: Principal,
    Path(id): Path<String>,
) -> Response {
    // Surface a clean 404 rather than the "user … not found" anyhow message
    // that set_password would produce.
    match s.control.get_user_by_id(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("user {id} not found") })),
            )
                .into_response();
        }
        Err(e) => return internal_error(e),
    }
    match s.control.regenerate_password(&id).await {
        Ok(password) => (StatusCode::OK, Json(json!({ "password": password }))).into_response(),
        Err(e) => internal_error(e),
    }
}

fn bad_request(e: anyhow::Error) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}

fn internal_error(e: anyhow::Error) -> Response {
    tracing::error!("users: {e:#}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal error" })),
    )
        .into_response()
}
