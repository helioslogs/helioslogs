// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Environment catalog types. CRUD lives on [`crate::control::Control`]
//! (`backend.rs`); this module keeps the [`EnvRow`] wire shape.

use serde::{Deserialize, Serialize};

/// Wire shape for `GET /api/envs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvRow {
    pub name: String,
    pub is_system: bool,
    pub created_at: String,
    /// Days to keep this env's day-partitions; None = use the global
    /// `retention.default_days` setting (which itself may be "keep forever").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<i64>,
}
