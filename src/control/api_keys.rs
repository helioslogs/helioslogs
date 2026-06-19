// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! REST API keys (wire shapes; storage lives on [`crate::control::Control`]).
//! A key is a long-lived `Authorization: Bearer` secret that authenticates REST
//! calls (and the MCP server) through the same path as a JWT, minting a
//! synthesized `Principal`. Each key carries a multi-select [`ApiKeyScopes`]
//! grant. The secret is shown once at creation; listings mask it.

use serde::{Deserialize, Serialize};

/// All API keys (single document `api_keys.json`) — one cached read on the hot path.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ApiKeyStore {
    #[serde(default)]
    pub keys: Vec<ApiKey>,
}

/// What a key may reach. Multi-select — a key can hold any combination. `admin`
/// is a superset of `api` (admin REST access implies standard non-admin access).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ApiKeyScopes {
    /// Standard non-admin REST API (`/api/*`).
    #[serde(default)]
    pub api: bool,
    /// Admin REST API (`/api/admin/*`); also grants `api`.
    #[serde(default)]
    pub admin: bool,
    /// MCP server (`/mcp` tool calls).
    #[serde(default)]
    pub mcp: bool,
}

impl ApiKeyScopes {
    /// True if the key may call the standard REST surface (admin implies it).
    pub fn allows_api(&self) -> bool {
        self.api || self.admin
    }

    /// True if at least one scope is granted (a scopeless key is rejected at create).
    pub fn any(&self) -> bool {
        self.api || self.admin || self.mcp
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    /// Free-text purpose/notes (audit only).
    #[serde(default)]
    pub description: String,
    /// The secret bearer value (stored in the encrypted control plane).
    pub token: String,
    /// What this key is allowed to reach.
    #[serde(default)]
    pub scopes: ApiKeyScopes,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// User id of the admin who minted it (audit only).
    #[serde(default)]
    pub created_by: String,
    pub created_at: String,
    pub last_used_at: Option<i64>,
    /// Optional expiry (epoch ms); None = never expires.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

fn default_true() -> bool {
    true
}

/// Listing view — the secret is masked (shown in full only once, at creation).
#[derive(Serialize, Clone, Debug)]
pub struct ApiKeyView {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Last 4 chars, e.g. `…a1b2` — enough to disambiguate, useless to replay.
    pub token_hint: String,
    pub scopes: ApiKeyScopes,
    pub enabled: bool,
    pub created_at: String,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
}

impl ApiKey {
    pub fn view(&self) -> ApiKeyView {
        let n = self.token.len();
        let hint = if n >= 4 {
            format!("…{}", &self.token[n - 4..])
        } else {
            "…".to_string()
        };
        ApiKeyView {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            token_hint: hint,
            scopes: self.scopes.clone(),
            enabled: self.enabled,
            created_at: self.created_at.clone(),
            last_used_at: self.last_used_at,
            expires_at: self.expires_at,
        }
    }

    /// True if the key carries an expiry that has already passed (epoch ms).
    pub fn is_expired(&self, now_ms: i64) -> bool {
        self.expires_at.map(|e| now_ms >= e).unwrap_or(false)
    }
}

/// Payload for `POST /api/admin/api-keys`.
#[derive(Deserialize, Debug, Default)]
pub struct ApiKeyInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub scopes: ApiKeyScopes,
    /// Optional lifetime in days; None / 0 / negative = never expires.
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(secret: &str) -> ApiKey {
        ApiKey {
            token: secret.into(),
            ..Default::default()
        }
    }

    #[test]
    fn view_masks_secret_to_last_four() {
        let v = key("hlk_supersecret-a1b2").view();
        assert_eq!(v.token_hint, "…a1b2");
        assert!(!v.token_hint.contains("supersecret"));
    }

    #[test]
    fn view_short_secret_fully_masked() {
        assert_eq!(key("abc").view().token_hint, "…");
        assert_eq!(key("").view().token_hint, "…");
        assert_eq!(key("wxyz").view().token_hint, "…wxyz");
    }

    #[test]
    fn admin_scope_implies_api_access() {
        let admin = ApiKeyScopes {
            admin: true,
            ..Default::default()
        };
        assert!(admin.allows_api());
        let mcp_only = ApiKeyScopes {
            mcp: true,
            ..Default::default()
        };
        assert!(!mcp_only.allows_api());
        assert!(mcp_only.any());
        assert!(!ApiKeyScopes::default().any());
    }

    #[test]
    fn expiry_is_inclusive_and_optional() {
        let mut k = key("hlk_x");
        assert!(!k.is_expired(10_000)); // no expiry set
        k.expires_at = Some(10_000);
        assert!(!k.is_expired(9_999));
        assert!(k.is_expired(10_000));
        assert!(k.is_expired(10_001));
    }
}
