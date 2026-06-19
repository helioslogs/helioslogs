// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Ingestion-source wire shapes + scheduling constants (storage lives on
//! [`crate::control::Control`]). A Source says where to find logs and how to parse
//! them; only local-filesystem `pull` is implemented (`s3`/`watch`/`event` later).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Floor on the poll interval — sub-second polling just hammers the disk.
pub const MIN_INTERVAL_SECONDS: i64 = 5;
/// Default poll cadence when a source doesn't specify one.
pub const DEFAULT_INTERVAL_SECONDS: i64 = 10;
/// A lease held longer than this is assumed stuck (crash mid-run) and stealable.
pub const STUCK_LEASE_SECS: i64 = 300;

/// A configured ingestion source plus its run status. `kind`/`mode` are strings
/// (not enums) so older docs stay readable as new shapes are added.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Source {
    pub id: String,
    pub name: String,
    /// Workspace this source ingests into.
    pub env: String,
    /// Storage partition (`index:` filter) events land in.
    pub index: String,
    /// `fs` is the only implemented backend; `s3` is reserved.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// `pull` (poll a glob) is implemented; `watch`/`event` are reserved.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Glob the source matches, e.g. `/var/log/app/**/*.log`.
    pub path: String,
    /// Glob patterns to skip (matched against the absolute path).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Parse format: `auto` | `ndjson` | `json` | `text` | `syslog`.
    #[serde(default = "default_format")]
    pub format: String,
    /// `auto` (sniff/extension) | `none` | `gzip` | `zstd`.
    #[serde(default = "default_compression")]
    pub compression: String,
    pub multiline_pattern: Option<String>,
    pub multiline_max_lines: Option<usize>,
    /// Grok / named-capture-regex pattern (or a preset name) for `format=grok`.
    pub grok_pattern: Option<String>,
    /// Poll interval for `pull` mode.
    #[serde(default = "default_interval")]
    pub interval_seconds: i64,
    /// Default `source` tag for events that don't carry one.
    pub source_tag: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    // ---- run status (managed by the supervisor) ----
    pub last_run_at: Option<i64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    #[serde(default)]
    pub total_ingested: u64,
    #[serde(default)]
    pub running: bool,
    pub running_since: Option<i64>,
    /// Live rows ingested this run; reset on lease start, cleared on finish.
    #[serde(default)]
    pub progress_ingested: u64,
    /// File/key currently being read this run, surfaced as live status.
    pub progress_file: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn default_kind() -> String {
    "fs".to_string()
}
fn default_mode() -> String {
    "pull".to_string()
}
fn default_format() -> String {
    "auto".to_string()
}
fn default_compression() -> String {
    "auto".to_string()
}
fn default_interval() -> i64 {
    DEFAULT_INTERVAL_SECONDS
}
fn default_enabled() -> bool {
    true
}

/// Payload for `POST /api/sources`. Server fills in id + timestamps; `env` is
/// stamped from the caller's active env.
#[derive(Deserialize, Debug, Default)]
pub struct SourceInput {
    pub name: String,
    pub index: String,
    pub kind: Option<String>,
    pub mode: Option<String>,
    pub path: String,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub format: Option<String>,
    pub compression: Option<String>,
    pub multiline_pattern: Option<String>,
    pub multiline_max_lines: Option<usize>,
    pub grok_pattern: Option<String>,
    pub interval_seconds: Option<i64>,
    pub source_tag: Option<String>,
    pub enabled: Option<bool>,
}

/// Payload for `PATCH /api/sources/:id`. Present fields are applied; nullable
/// fields use `Option<Option<_>>` so a JSON `null` clears them.
#[derive(Deserialize, Debug, Default)]
pub struct SourcePatch {
    pub name: Option<String>,
    /// Move the source to a different workspace. Caller must have access.
    pub env: Option<String>,
    pub index: Option<String>,
    pub mode: Option<String>,
    pub path: Option<String>,
    pub exclude: Option<Vec<String>>,
    pub format: Option<String>,
    pub compression: Option<String>,
    pub multiline_pattern: Option<Option<String>>,
    pub multiline_max_lines: Option<Option<usize>>,
    pub grok_pattern: Option<Option<String>>,
    pub interval_seconds: Option<i64>,
    pub source_tag: Option<Option<String>>,
    pub enabled: Option<bool>,
}

/// Per-source ingest progress — one entry per file path (the "fishbucket")
/// so restarts resume rather than re-ingest.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SourceCheckpoint {
    #[serde(default)]
    pub files: BTreeMap<String, FileMark>,
}

/// How much of one file we've ingested. `offset` = bytes consumed or last-seen
/// size; `mtime_ms` detects change for whole-file mode.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FileMark {
    pub offset: u64,
    pub mtime_ms: i64,
}
