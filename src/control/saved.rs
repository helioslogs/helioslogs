// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Saved-search wire shapes (storage lives on [`crate::control::Control`]).
//! `index` aliases `source` so pre-rename payloads stay readable.

use serde::{Deserialize, Serialize};

/// One saved query + its surrounding UI state — enough to reconstruct the
/// exact search view the user was looking at when they hit "save".
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SavedSearch {
    pub id: String,
    pub name: String,
    pub q: String,
    #[serde(alias = "source")]
    pub index: Option<String>,
    pub range: String,
    pub start: Option<String>,
    pub end: Option<String>,
    #[serde(default)]
    pub follow: bool,
    /// `true` = visible to all users and editable by anyone. `false`
    /// (default) = owner-only.
    #[serde(default)]
    pub public: bool,
    /// Env this saved search belongs to. Stamped from the caller's active env
    /// at create time; lists filter by env.
    #[serde(default = "default_env_string")]
    pub env: String,
    pub created_at: String,
    pub updated_at: String,
    /// Owner display label, set only in the admin "view all" listing. Never
    /// persisted (skipped when `None`, which it always is on the stored doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

fn default_env_string() -> String {
    crate::catalog::DEFAULT_ENV.to_string()
}

/// Payload for `POST /api/searches`. Server fills in id + timestamps.
#[derive(Deserialize, Debug, Default)]
pub struct SavedSearchInput {
    pub name: String,
    pub q: String,
    #[serde(alias = "source")]
    pub index: Option<String>,
    pub range: String,
    pub start: Option<String>,
    pub end: Option<String>,
    #[serde(default)]
    pub follow: bool,
    /// Defaults to public when the client omits it (new searches are shared).
    #[serde(default = "default_true")]
    pub public: bool,
}

fn default_true() -> bool {
    true
}

/// Payload for `PATCH /api/searches/:id`. All fields optional; `Option<Option<_>>`
/// lets a JSON `null` explicitly clear nullable fields.
#[derive(Deserialize, Debug, Default)]
pub struct SavedSearchPatch {
    pub name: Option<String>,
    pub q: Option<String>,
    #[serde(alias = "source")]
    pub index: Option<Option<String>>,
    pub range: Option<String>,
    pub start: Option<Option<String>>,
    pub end: Option<Option<String>>,
    pub follow: Option<bool>,
    pub public: Option<bool>,
}
