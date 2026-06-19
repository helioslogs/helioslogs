// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Admin API for the raw syslog listener config (`/api/admin/syslog`). Read/write
//! the control-plane settings the syslog supervisor polls. Admin-only access is
//! enforced by the `/api/admin/*` auth middleware.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::json;

use crate::control::settings::{
    SyslogRoute, KEY_SYSLOG_BIND, KEY_SYSLOG_DEFAULT_ENV, KEY_SYSLOG_DEFAULT_INDEX,
    KEY_SYSLOG_ENABLED, KEY_SYSLOG_ROUTES, KEY_SYSLOG_TCP_PORT, KEY_SYSLOG_UDP_PORT,
    SYSLOG_ROUTE_FIELDS, SYSLOG_ROUTE_OPS,
};

use super::AppState;

/// `GET /api/admin/syslog` — current listener config plus the field/op vocabularies
/// the UI uses to build route editors.
pub(super) async fn get_config_handler(State(s): State<AppState>) -> Response {
    let c = match s.control.syslog_settings().await {
        Ok(c) => c,
        Err(e) => return internal_err(e),
    };
    (
        StatusCode::OK,
        Json(json!({
            "enabled": c.enabled,
            "bind": c.bind,
            "udp_port": c.udp_port,
            "tcp_port": c.tcp_port,
            "default_env": c.default_env,
            "default_index": c.default_index,
            "routes": c.routes,
            "route_fields": SYSLOG_ROUTE_FIELDS,
            "route_ops": SYSLOG_ROUTE_OPS,
            // When set via `--syslog-port`, the listener ignores the ports above and
            // binds this one (UDP + TCP) instead. Surfaced so the UI can say so.
            "port_override": s.syslog_port,
        })),
    )
        .into_response()
}

#[derive(Deserialize, Default)]
pub(super) struct SyslogConfigPatch {
    enabled: Option<bool>,
    bind: Option<String>,
    udp_port: Option<u16>,
    tcp_port: Option<u16>,
    default_env: Option<String>,
    default_index: Option<String>,
    /// Full replacement of the route list when present.
    routes: Option<Vec<SyslogRoute>>,
}

/// `POST /api/admin/syslog` — update config. Validates routes (known field/op,
/// non-empty value, compilable regex) before persisting anything.
pub(super) async fn post_config_handler(
    State(s): State<AppState>,
    Json(p): Json<SyslogConfigPatch>,
) -> Response {
    if let Some(routes) = &p.routes {
        for (i, r) in routes.iter().enumerate() {
            if !r.is_valid() {
                return bad_req(&format!(
                    "route {i}: field must be one of {SYSLOG_ROUTE_FIELDS:?}, op one of {SYSLOG_ROUTE_OPS:?}, value non-empty",
                ));
            }
            if r.op == "regex" {
                if let Err(e) = regex::Regex::new(&r.value) {
                    return bad_req(&format!("route {i}: invalid regex: {e}"));
                }
            }
        }
    }

    if let Some(b) = p.enabled {
        if let Err(e) = s
            .control
            .set_setting(KEY_SYSLOG_ENABLED, &b.to_string())
            .await
        {
            return internal_err(e);
        }
    }
    // Strings: empty clears the override (reverts to the built-in default).
    for (key, val) in [
        (KEY_SYSLOG_BIND, p.bind),
        (KEY_SYSLOG_DEFAULT_ENV, p.default_env),
        (KEY_SYSLOG_DEFAULT_INDEX, p.default_index),
    ] {
        let Some(v) = val else { continue };
        let res = if v.trim().is_empty() {
            s.control.unset_setting(key).await
        } else {
            s.control.set_setting(key, v.trim()).await
        };
        if let Err(e) = res {
            return internal_err(e);
        }
    }
    // Ports: 0 is a valid value (disables that transport), so always write when present.
    for (key, val) in [
        (KEY_SYSLOG_UDP_PORT, p.udp_port),
        (KEY_SYSLOG_TCP_PORT, p.tcp_port),
    ] {
        let Some(port) = val else { continue };
        if let Err(e) = s.control.set_setting(key, &port.to_string()).await {
            return internal_err(e);
        }
    }
    if let Some(routes) = p.routes {
        let body = serde_json::to_string(&routes).unwrap_or_else(|_| "[]".into());
        if let Err(e) = s.control.set_setting(KEY_SYSLOG_ROUTES, &body).await {
            return internal_err(e);
        }
    }
    get_config_handler(State(s)).await
}

fn internal_err(e: anyhow::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}

fn bad_req(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}
