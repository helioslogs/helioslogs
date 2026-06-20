// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! HTTP transport for the MCP dispatcher: a single `POST /mcp` JSON-RPC endpoint
//! (Streamable-HTTP shape, no SSE upgrade). Auth is an `Authorization: Bearer hlk_…`
//! API key with the MCP scope (open until the first MCP-scoped key exists); tool
//! errors return `isError` so the LLM can recover.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;
use std::sync::Arc;

use crate::mcp::{
    self, JsonRpcError, JsonRpcResponse, McpServer, Request, INTERNAL_ERROR, PARSE_ERROR,
};

use super::AppState;

/// `POST /mcp` — JSON-RPC over HTTP. The body is one JSON-RPC message (no batch support).
pub(super) async fn handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Parse the JSON-RPC envelope first. A bad envelope means we
    // can't recover the `id`, so the error response carries `null`.
    let req: Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return rpc_error(Value::Null, PARSE_ERROR, format!("parse error: {e}")),
    };

    // Per-request `McpServer` (cheap — each field is Arc-Clone). Wrapped in `Arc`
    // because `tools::call` moves it into `spawn_blocking`.
    let server = Arc::new(McpServer {
        catalog: state.catalog.clone(),
        fields: state.fields,
        control: state.control.clone(),
    });

    let bearer = bearer_token(&headers);
    let id = req.id.clone();
    let is_notification = id.is_none();
    let outcome = mcp::handle_request(&server, &req, bearer.as_deref()).await;

    // Notifications get an empty 202 — JSON-RPC §4.1 wants no body, but HTTP needs
    // a status; 202 is conventional per the MCP HTTP transport guidance.
    if is_notification {
        if let Err(e) = outcome {
            tracing::warn!(method = %req.method, error = %e, "mcp/http: notification failed");
        }
        return (StatusCode::ACCEPTED, ()).into_response();
    }

    let id = id.unwrap_or(Value::Null);
    match outcome {
        Ok(value) => (
            StatusCode::OK,
            Json(JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(value),
                error: None,
            }),
        )
            .into_response(),
        Err(e) => rpc_error(id, mcp::error_code_for(&e), format!("{e:#}")),
    }
}

/// Extract a Bearer token from `Authorization`; `None` for missing/malformed/non-Bearer.
/// The dispatcher then accepts (no auth configured) or rejects (auth configured) the call.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = raw.split_once(' ')?;

    if scheme.eq_ignore_ascii_case("Bearer") {
        Some(token.trim().to_string())
    } else {
        None
    }
}

fn rpc_error(id: Value, code: i32, message: String) -> Response {
    let body = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message,
            data: None,
        }),
    };
    // JSON-RPC errors are still 200-level HTTP; HTTP status codes are reserved for
    // genuine transport problems. The MCP client reads the `error` field instead.
    let http_status = if code == INTERNAL_ERROR {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::OK
    };
    (http_status, Json(body)).into_response()
}
