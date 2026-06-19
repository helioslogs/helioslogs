// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Transport-agnostic MCP (Model Context Protocol) dispatcher: owns the protocol
//! shape and tool catalog (HTTP transport lives in `src/http/mcp_http.rs`). Each
//! call is offloaded onto `spawn_blocking` since block-engine search is CPU-bound.

pub(crate) mod protocol;
mod tools;

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

use crate::catalog::Catalog;
use crate::control::settings::McpSettings;
use crate::control::Control;
use crate::schema::Fields;
use protocol::{JsonRpcRequest, METHOD_NOT_FOUND, PROTOCOL_VERSION};

pub(crate) use protocol::{
    JsonRpcError, JsonRpcRequest as Request, JsonRpcResponse, INTERNAL_ERROR, PARSE_ERROR,
};

/// Per-request dependency bundle; cheap to build (all fields are
/// `Arc`-backed handles shared across the process).
pub(crate) struct McpServer {
    pub(crate) catalog: Catalog,
    pub(crate) fields: Fields,
    pub(crate) control: Control,
}

impl McpServer {
    /// Load MCP settings; on failure fall back to defaults (`enabled = false`),
    /// so a corrupt store safely closes the surface rather than opening it.
    pub(crate) async fn settings(&self) -> McpSettings {
        match self.control.mcp_settings().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "mcp: settings load failed, using defaults");
                McpSettings::default()
            }
        }
    }
}

/// Top-level JSON-RPC method dispatch, transport-agnostic. `presented_token`
/// is the client's offered bearer credential, checked by `tools::call`.
pub(crate) async fn handle_request(
    server: &Arc<McpServer>,
    req: &JsonRpcRequest,
    presented_token: Option<&str>,
) -> Result<Value> {
    match req.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "helioslogs",
                "version": env!("CARGO_PKG_VERSION"),
            },
        })),
        // Notifications per spec; the transport uses our return value
        // only to decide whether to log a failure.
        "notifications/initialized" | "notifications/cancelled" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::list(server).await })),
        "tools/call" => tools::call(server, &req.params, presented_token).await,
        other => Err(MethodNotFound(other.to_string()).into()),
    }
}

/// Sentinel error so the transport can map "method not found" to the proper
/// JSON-RPC code (-32601) instead of bucketing it as `INTERNAL_ERROR`.
#[derive(Debug)]
pub(crate) struct MethodNotFound(pub String);
impl std::fmt::Display for MethodNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "method not found: {}", self.0)
    }
}
impl std::error::Error for MethodNotFound {}

pub(crate) fn error_code_for(e: &anyhow::Error) -> i32 {
    if e.downcast_ref::<MethodNotFound>().is_some() {
        METHOD_NOT_FOUND
    } else {
        INTERNAL_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_not_found_maps_to_jsonrpc_code() {
        let e = anyhow::Error::from(MethodNotFound("frob".into()));
        assert_eq!(error_code_for(&e), METHOD_NOT_FOUND);
    }

    #[test]
    fn other_errors_map_to_internal() {
        let e = anyhow::anyhow!("something blew up");
        assert_eq!(error_code_for(&e), INTERNAL_ERROR);
    }

    #[test]
    fn method_not_found_display() {
        assert_eq!(
            MethodNotFound("x".into()).to_string(),
            "method not found: x"
        );
    }
}
