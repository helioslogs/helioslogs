// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! JSON-RPC 2.0 envelope plus the small subset of MCP message shapes we emit,
//! hand-rolled rather than pulling in `rmcp` (six methods, ~150 lines).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Incoming JSON-RPC request *or* notification (the latter omits `id`);
/// callers branch on `id.is_some()`.
#[derive(Deserialize, Debug)]
pub(crate) struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// `None` ⇒ this is a notification (no response expected).
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Serialize)]
pub(crate) struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Serialize)]
pub(crate) struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// --- standard JSON-RPC error codes (subset we ever emit) ---
pub(crate) const PARSE_ERROR: i32 = -32700;
pub(crate) const METHOD_NOT_FOUND: i32 = -32601;
pub(crate) const INTERNAL_ERROR: i32 = -32603;

/// MCP protocol version we negotiate; pinned to what Claude Desktop/Code speak.
pub(super) const PROTOCOL_VERSION: &str = "2024-11-05";
