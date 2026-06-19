// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Scoped push tokens (wire shapes; storage lives on [`crate::control::Control`]).
//! A token pins a shipper to one `env` + optional index allowlist; when `require`
//! is off and no token is presented, ingest stays open (the local-box default).

use serde::{Deserialize, Serialize};

/// The full ingest-auth config: the require switch + every token.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IngestAuth {
    /// When true, ingest/shim requests without a valid token are rejected (401).
    #[serde(default)]
    pub require: bool,
    /// HTTP ingestion endpoints, on by default. Admins can disable a whole class:
    /// `api` = `/api/ingest` (+ `/raw`); `shims` = the ES/OTLP/Loki/HEC shims.
    #[serde(default = "default_true")]
    pub api_enabled: bool,
    #[serde(default = "default_true")]
    pub shims_enabled: bool,
    #[serde(default)]
    pub tokens: Vec<PushToken>,
}

impl Default for IngestAuth {
    fn default() -> Self {
        // Open + both HTTP ingestion classes on — the local-box default.
        Self {
            require: false,
            api_enabled: true,
            shims_enabled: true,
            tokens: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PushToken {
    pub id: String,
    pub name: String,
    /// The secret bearer value (stored in the encrypted control plane).
    pub token: String,
    /// Workspace this token may write to.
    pub env: String,
    /// Allowed indexes; empty = any index within `env`.
    #[serde(default)]
    pub indexes: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub last_used_at: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

fn default_true() -> bool {
    true
}

/// Listing view — the secret is masked (shown in full only once, at creation).
#[derive(Serialize, Clone, Debug)]
pub struct PushTokenView {
    pub id: String,
    pub name: String,
    /// Last 4 chars, e.g. `…a1b2` — enough to disambiguate, useless to replay.
    pub token_hint: String,
    pub env: String,
    pub indexes: Vec<String>,
    pub enabled: bool,
    pub last_used_at: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

impl PushToken {
    pub fn view(&self) -> PushTokenView {
        let n = self.token.len();
        let hint = if n >= 4 {
            format!("…{}", &self.token[n - 4..])
        } else {
            "…".to_string()
        };
        PushTokenView {
            id: self.id.clone(),
            name: self.name.clone(),
            token_hint: hint,
            env: self.env.clone(),
            indexes: self.indexes.clone(),
            enabled: self.enabled,
            last_used_at: self.last_used_at,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

/// Payload for `POST /api/admin/ingest-tokens`.
#[derive(Deserialize, Debug, Default)]
pub struct PushTokenInput {
    pub name: String,
    pub env: String,
    #[serde(default)]
    pub indexes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(secret: &str) -> PushToken {
        PushToken {
            token: secret.into(),
            ..Default::default()
        }
    }

    #[test]
    fn view_masks_secret_to_last_four() {
        let v = token("supersecret-a1b2").view();
        assert_eq!(v.token_hint, "…a1b2");
        // The full secret never appears in the view.
        assert!(!v.token_hint.contains("supersecret"));
    }

    #[test]
    fn view_short_secret_fully_masked() {
        assert_eq!(token("abc").view().token_hint, "…");
        assert_eq!(token("").view().token_hint, "…");
        assert_eq!(token("wxyz").view().token_hint, "…wxyz");
    }

    #[test]
    fn view_copies_metadata() {
        let mut t = token("0123456789");
        t.id = "tok_1".into();
        t.name = "shipper".into();
        t.env = "prod".into();
        t.enabled = true;
        let v = t.view();
        assert_eq!(v.id, "tok_1");
        assert_eq!(v.name, "shipper");
        assert_eq!(v.env, "prod");
        assert!(v.enabled);
    }
}
