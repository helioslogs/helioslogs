// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Self-observability: tracing/HTTP/MCP events index back into the reserved
//! `_helioslogs`/`_helioshttp`/`_heliosmcp` indexes (under `_system` env) via a
//! `OnceLock` sender → writer task; layers no-op until `install_sender` (serve only).

use std::sync::OnceLock;
use tokio::sync::mpsc;

pub mod http_layer;
pub mod mcp_layer;
pub mod tracing_layer;
pub mod writer;

pub use http_layer::http_log_middleware;
pub use mcp_layer::log_tool_call;
pub use tracing_layer::SelfLogsLayer;
pub use writer::run_writer;

/// Reserved index names. All three have an underscore prefix so the scatter
/// planner can recognise them as internal.
pub const LOGS_INDEX: &str = "_helioslogs";
pub const HTTP_INDEX: &str = "_helioshttp";
pub const MCP_INDEX: &str = "_heliosmcp";

/// One event handed to the background writer task. Pre-rendered to JSON so the
/// writer reuses the standard `json_to_doc` schema-on-read pipeline.
pub struct SelfLogEvent {
    pub index: &'static str,
    pub doc: serde_json::Value,
}

/// Identity of the node emitting self-logs, stamped onto every doc as flat
/// `node_id` / `node_host` / `node_port` fields.
#[derive(Clone)]
pub struct NodeInfo {
    pub id: String,
    pub host: String,
    pub port: u16,
}

impl NodeInfo {
    /// Read the machine hostname once at startup (falling back to `"unknown"`),
    /// pairing it with the per-process `node_id` and bind port.
    pub fn new(id: String, port: u16) -> Self {
        let host = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "unknown".into());
        Self { id, host, port }
    }

    /// Flat scalar fields stamped onto each self-log doc — kept flat (not nested)
    /// so they stay queryable per-key; port stays numeric for range/agg.
    pub fn fields(&self) -> [(&'static str, serde_json::Value); 3] {
        [
            ("node_id", serde_json::Value::String(self.id.clone())),
            ("node_host", serde_json::Value::String(self.host.clone())),
            ("node_port", serde_json::Value::Number(self.port.into())),
        ]
    }
}

static SENDER: OnceLock<mpsc::UnboundedSender<SelfLogEvent>> = OnceLock::new();

/// Installs the global sender once at `serve` start (idempotent). After this,
/// events begin flowing to the writer task.
pub fn install_sender(tx: mpsc::UnboundedSender<SelfLogEvent>) {
    let _ = SENDER.set(tx);
}

/// Borrow the sender (`None` before `install_sender`, so layers no-op). Hot
/// path: a single lock-free atomic read.
pub(crate) fn sender() -> Option<&'static mpsc::UnboundedSender<SelfLogEvent>> {
    SENDER.get()
}
