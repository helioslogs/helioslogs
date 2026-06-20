// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Typed view over the `agent.*` settings keys, loaded fresh per request so
//! admin LLM-panel edits propagate without restart. API keys are read-redacted.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::control::Control;

/// Master on/off switch for all LLM/agent functionality (chat + AI monitors).
pub const KEY_AGENT_ENABLED: &str = "agent.enabled";
pub const KEY_PROVIDER: &str = "agent.provider";
/// Legacy single-model key; still read as a migration fallback for the active provider.
pub const KEY_MODEL: &str = "agent.model";
/// Per-provider model keys so a model survives switching providers.
pub const KEY_MODEL_OPENAI: &str = "agent.openai_model";
pub const KEY_MODEL_ANTHROPIC: &str = "agent.anthropic_model";
pub const KEY_MODEL_BEDROCK: &str = "agent.bedrock_model";
pub const KEY_OPENAI_ENDPOINT: &str = "agent.openai_endpoint";
pub const KEY_OPENAI_API_KEY: &str = "agent.openai_api_key";
pub const KEY_ANTHROPIC_ENDPOINT: &str = "agent.anthropic_endpoint";
pub const KEY_ANTHROPIC_API_KEY: &str = "agent.anthropic_api_key";
pub const KEY_BEDROCK_REGION: &str = "agent.bedrock_region";
pub const KEY_BEDROCK_AUTH_MODE: &str = "agent.bedrock_auth_mode";
pub const KEY_BEDROCK_ACCESS_KEY_ID: &str = "agent.bedrock_access_key_id";
pub const KEY_BEDROCK_SECRET_ACCESS_KEY: &str = "agent.bedrock_secret_access_key";
pub const KEY_BEDROCK_SESSION_TOKEN: &str = "agent.bedrock_session_token";
pub const KEY_BEDROCK_BEARER_TOKEN: &str = "agent.bedrock_bearer_token";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Openai,
    Anthropic,
    Bedrock,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai" => Some(Self::Openai),
            "anthropic" => Some(Self::Anthropic),
            "bedrock" => Some(Self::Bedrock),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BedrockAuthMode {
    /// Standard AWS credential chain (env vars, ~/.aws, IMDS, etc.).
    DefaultChain,
    /// `AWS_BEARER_TOKEN_BEDROCK` short-form auth.
    BearerToken,
}

impl BedrockAuthMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default_chain" | "default" => Some(Self::DefaultChain),
            "bearer_token" | "bearer" => Some(Self::BearerToken),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSettings {
    /// When false, chat and AI monitors are turned off system-wide.
    pub enabled: bool,
    pub provider: Provider,
    /// Resolved model for the active `provider` (one of the per-provider models below).
    pub model: String,
    pub openai_model: String,
    pub anthropic_model: String,
    pub bedrock_model: String,
    pub openai_endpoint: String,
    pub openai_api_key: Option<String>,
    pub anthropic_endpoint: String,
    pub anthropic_api_key: Option<String>,
    pub bedrock_region: String,
    pub bedrock_auth_mode: BedrockAuthMode,
    /// Admin AWS credentials; `None` falls back to the default chain.
    pub bedrock_access_key_id: Option<String>,
    pub bedrock_secret_access_key: Option<String>,
    pub bedrock_session_token: Option<String>,
    /// Admin bearer token; `Some` overrides `AWS_BEARER_TOKEN_BEDROCK` from env.
    pub bedrock_bearer_token: Option<String>,
}

impl Default for AgentSettings {
    /// Defaults match the legacy hardcoded `AGENT_CONFIG` from the
    /// removed frontend: local OpenAI-compatible server at :8080.
    fn default() -> Self {
        Self {
            // Off by default: a fresh instance has no LLM provider configured yet, so the
            // agent/MCP stay disabled until an admin sets one up and opts in. A stored
            // `agent.enabled` (from an explicit toggle) overrides this in `load()`.
            enabled: false,
            provider: Provider::Openai,
            model: "local".into(),
            openai_model: "local".into(),
            anthropic_model: "claude-sonnet-4-6".into(),
            bedrock_model: "anthropic.claude-sonnet-4-6".into(),
            openai_endpoint: "http://localhost:8080/v1".into(),
            openai_api_key: None,
            anthropic_endpoint: "https://api.anthropic.com/v1".into(),
            anthropic_api_key: None,
            bedrock_region: "us-east-1".into(),
            bedrock_auth_mode: BedrockAuthMode::DefaultChain,
            bedrock_access_key_id: None,
            bedrock_secret_access_key: None,
            bedrock_session_token: None,
            bedrock_bearer_token: None,
        }
    }
}

impl AgentSettings {
    pub async fn load(c: &Control) -> Result<Self> {
        let mut s = Self::default();
        if let Some(v) = c.get_setting(KEY_AGENT_ENABLED).await? {
            if let Ok(b) = v.trim().parse::<bool>() {
                s.enabled = b;
            }
        }
        if let Some(v) = c.get_setting(KEY_PROVIDER).await? {
            if let Some(p) = Provider::parse(&v) {
                s.provider = p;
            }
        }
        // Per-provider models, with the legacy single `agent.model` migrated into
        // whichever provider is active (so existing configs keep their model).
        let legacy = c
            .get_setting(KEY_MODEL)
            .await?
            .map(|v| v.trim().to_string())
            .filter(|t| !t.is_empty());
        for (key, slot, is_active) in [
            (
                KEY_MODEL_OPENAI,
                &mut s.openai_model,
                s.provider == Provider::Openai,
            ),
            (
                KEY_MODEL_ANTHROPIC,
                &mut s.anthropic_model,
                s.provider == Provider::Anthropic,
            ),
            (
                KEY_MODEL_BEDROCK,
                &mut s.bedrock_model,
                s.provider == Provider::Bedrock,
            ),
        ] {
            if let Some(v) = c.get_setting(key).await? {
                let t = v.trim();
                if !t.is_empty() {
                    *slot = t.to_string();
                    continue;
                }
            }
            if is_active {
                if let Some(m) = &legacy {
                    *slot = m.clone();
                }
            }
        }
        s.model = s.active_model().to_string();
        if let Some(v) = c.get_setting(KEY_OPENAI_ENDPOINT).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.openai_endpoint = t.to_string();
            }
        }
        if let Some(v) = c.get_setting(KEY_OPENAI_API_KEY).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.openai_api_key = Some(t.to_string());
            }
        }
        if let Some(v) = c.get_setting(KEY_ANTHROPIC_ENDPOINT).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.anthropic_endpoint = t.to_string();
            }
        }
        if let Some(v) = c.get_setting(KEY_ANTHROPIC_API_KEY).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.anthropic_api_key = Some(t.to_string());
            }
        }
        if let Some(v) = c.get_setting(KEY_BEDROCK_REGION).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.bedrock_region = t.to_string();
            }
        }
        if let Some(v) = c.get_setting(KEY_BEDROCK_AUTH_MODE).await? {
            if let Some(m) = BedrockAuthMode::parse(&v) {
                s.bedrock_auth_mode = m;
            }
        }
        if let Some(v) = c.get_setting(KEY_BEDROCK_ACCESS_KEY_ID).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.bedrock_access_key_id = Some(t.to_string());
            }
        }
        if let Some(v) = c.get_setting(KEY_BEDROCK_SECRET_ACCESS_KEY).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.bedrock_secret_access_key = Some(t.to_string());
            }
        }
        if let Some(v) = c.get_setting(KEY_BEDROCK_SESSION_TOKEN).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.bedrock_session_token = Some(t.to_string());
            }
        }
        if let Some(v) = c.get_setting(KEY_BEDROCK_BEARER_TOKEN).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.bedrock_bearer_token = Some(t.to_string());
            }
        }
        Ok(s)
    }

    /// The model string for the currently-selected provider.
    pub fn active_model(&self) -> &str {
        match self.provider {
            Provider::Openai => &self.openai_model,
            Provider::Anthropic => &self.anthropic_model,
            Provider::Bedrock => &self.bedrock_model,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_parse_case_insensitive_and_trimmed() {
        assert_eq!(Provider::parse("openai"), Some(Provider::Openai));
        assert_eq!(Provider::parse("  Anthropic "), Some(Provider::Anthropic));
        assert_eq!(Provider::parse("BEDROCK"), Some(Provider::Bedrock));
        assert_eq!(Provider::parse("gemini"), None);
    }

    #[test]
    fn bedrock_auth_mode_parse_aliases() {
        assert_eq!(
            BedrockAuthMode::parse("default_chain"),
            Some(BedrockAuthMode::DefaultChain)
        );
        assert_eq!(
            BedrockAuthMode::parse("default"),
            Some(BedrockAuthMode::DefaultChain)
        );
        assert_eq!(
            BedrockAuthMode::parse("bearer_token"),
            Some(BedrockAuthMode::BearerToken)
        );
        assert_eq!(
            BedrockAuthMode::parse("BEARER"),
            Some(BedrockAuthMode::BearerToken)
        );
        assert_eq!(BedrockAuthMode::parse("nope"), None);
    }

    #[test]
    fn provider_serde_lowercase() {
        assert_eq!(
            serde_json::to_value(Provider::Anthropic).unwrap(),
            serde_json::json!("anthropic")
        );
    }

    #[test]
    fn defaults_point_at_local_openai() {
        let d = AgentSettings::default();
        // Fresh instances ship with the agent disabled until an admin configures a provider.
        assert!(!d.enabled);
        assert_eq!(d.provider, Provider::Openai);
        assert_eq!(d.openai_endpoint, "http://localhost:8080/v1");
        assert_eq!(d.bedrock_auth_mode, BedrockAuthMode::DefaultChain);
        assert!(d.openai_api_key.is_none());
        assert_eq!(d.openai_model, "local");
        assert_eq!(d.anthropic_model, "claude-sonnet-4-6");
        assert_eq!(d.bedrock_model, "anthropic.claude-sonnet-4-6");
    }

    #[test]
    fn active_model_tracks_provider() {
        let mut s = AgentSettings {
            openai_model: "gpt-x".into(),
            anthropic_model: "claude-y".into(),
            bedrock_model: "anthropic.claude-z".into(),
            ..Default::default()
        };

        s.provider = Provider::Openai;
        assert_eq!(s.active_model(), "gpt-x");
        s.provider = Provider::Anthropic;
        assert_eq!(s.active_model(), "claude-y");
        s.provider = Provider::Bedrock;
        assert_eq!(s.active_model(), "anthropic.claude-z");
    }
}
