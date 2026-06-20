// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Settings value types, the typed `McpSettings` view, parsing helpers, and
//! setting-key constants. The KV store + loaders live on [`crate::control::Control`].

use serde::{Deserialize, Serialize};

use crate::catalog::index_matches;

/// One row in the MCP allow-list: an env scope plus index glob patterns.
/// `env == "*"` matches any env; an `indexes` entry of `"*"` matches all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvIndexAllow {
    pub env: String,
    pub indexes: Vec<String>,
}

/// MCP server config. Loaded fresh on every tool call so admin edits propagate
/// without restarting the MCP child process.
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpSettings {
    pub enabled: bool,
    pub allowed: Vec<EnvIndexAllow>,
    pub enabled_tools: Vec<String>,
}

impl Default for McpSettings {
    fn default() -> Self {
        // Off by default — admins explicitly opt in via the UI.
        Self {
            enabled: false,
            allowed: vec![EnvIndexAllow {
                env: "*".into(),
                indexes: vec!["*".into()],
            }],
            enabled_tools: vec!["*".into()],
        }
    }
}

// Alert webhook delivery (notify.rs). URL may embed a secret — settings.json
// is encrypted at rest, and the admin API only ever reports `*_set` booleans.
pub const KEY_ALERT_WEBHOOK_ENABLED: &str = "alerts.webhook_enabled";
pub const KEY_ALERT_WEBHOOK_URL: &str = "alerts.webhook_url";
pub const KEY_ALERT_WEBHOOK_FORMAT: &str = "alerts.webhook_format";

/// Default retention in days for day-partitions; unset/0 = keep forever.
/// Per-env override lives on `EnvRow.retention_days`.
pub const KEY_RETENTION_DEFAULT_DAYS: &str = "retention.default_days";

// Instance-wide UI theme defaults; users override per-account (User.theme/palette).
pub const KEY_THEME_DEFAULT_APPEARANCE: &str = "theme.default_appearance";
pub const KEY_THEME_DEFAULT_PALETTE: &str = "theme.default_palette";

/// Color themes the frontend ships (frontend/src/index.css); shared validation
/// list for the instance default and per-user overrides.
pub const THEME_PALETTES: &[&str] = &[
    "helios", "slate", "emerald", "indigo", "homebrew", "dracula",
];
pub const THEME_DEFAULT_APPEARANCE: &str = "dark";
pub const THEME_DEFAULT_PALETTE: &str = "helios";

/// Maps a stored palette value to a shipping palette id; legacy/unknown ids
/// (e.g. pre-rename "github") fall back to the default.
pub fn palette_or_default(v: Option<String>) -> String {
    match v {
        Some(p) if THEME_PALETTES.contains(&p.as_str()) => p,
        _ => THEME_DEFAULT_PALETTE.to_string(),
    }
}

pub const KEY_MCP_ENABLED: &str = "mcp.enabled";
pub const KEY_MCP_ALLOWED_INDEXES: &str = "mcp.allowed_indexes";
pub const KEY_MCP_ENABLED_TOOLS: &str = "mcp.enabled_tools";

// SAML SP config (one trusted IdP); typed view is `crate::saml::SamlConfig`.
pub const KEY_SAML_ENABLED: &str = "saml.enabled";
pub const KEY_SAML_IDP_ENTITY_ID: &str = "saml.idp_entity_id";
pub const KEY_SAML_IDP_SSO_URL: &str = "saml.idp_sso_url";
pub const KEY_SAML_IDP_CERT: &str = "saml.idp_cert";
pub const KEY_SAML_SP_ENTITY_ID: &str = "saml.sp_entity_id";
pub const KEY_SAML_ACS_URL: &str = "saml.acs_url";
pub const KEY_SAML_EMAIL_ATTR: &str = "saml.email_attr";
pub const KEY_SAML_BUTTON_LABEL: &str = "saml.button_label";
// When true, non-admins are SSO-only; admins keep password login as break-glass.
pub const KEY_SAML_LOCAL_LOGIN_DISABLED: &str = "saml.local_login_disabled";

// Raw syslog network listener (src/syslog/). No auth: admins opt in, pin the listen
// address, and messages route to admin-configured partitions.
pub const KEY_SYSLOG_ENABLED: &str = "syslog.enabled";
pub const KEY_SYSLOG_BIND: &str = "syslog.bind";
pub const KEY_SYSLOG_UDP_PORT: &str = "syslog.udp_port";
pub const KEY_SYSLOG_TCP_PORT: &str = "syslog.tcp_port";
pub const KEY_SYSLOG_DEFAULT_ENV: &str = "syslog.default_env";
pub const KEY_SYSLOG_DEFAULT_INDEX: &str = "syslog.default_index";
pub const KEY_SYSLOG_ROUTES: &str = "syslog.routes";

pub const SYSLOG_DEFAULT_BIND: &str = "0.0.0.0";
pub const SYSLOG_DEFAULT_PORT: u16 = 5514;
pub const SYSLOG_DEFAULT_ENV: &str = "default";
pub const SYSLOG_DEFAULT_INDEX: &str = "syslog";
// Fields a route may match on, and the supported operators (used for admin validation).
pub const SYSLOG_ROUTE_FIELDS: &[&str] = &[
    "host",
    "appname",
    "facility",
    "severity",
    "message",
    "source_ip",
];
pub const SYSLOG_ROUTE_OPS: &[&str] = &["equals", "contains", "regex"];

impl McpSettings {
    /// True when the allowlist is open-ended — empty list, or any rule matches
    /// `(env="*", indexes contains "*")`.
    pub fn indexes_unrestricted(&self) -> bool {
        if self.allowed.is_empty() {
            return true;
        }
        self.allowed
            .iter()
            .any(|r| r.env == "*" && r.indexes.iter().any(|p| p == "*"))
    }

    /// True iff a `(env, index)` partition is permitted by the allowlist.
    pub fn allows(&self, env: &str, index: &str) -> bool {
        if self.indexes_unrestricted() {
            return true;
        }
        for r in &self.allowed {
            let env_match = r.env == "*" || r.env.eq_ignore_ascii_case(env);
            if !env_match {
                continue;
            }
            if r.indexes.iter().any(|p| p == "*") {
                return true;
            }
            if index_matches(&r.indexes, index) {
                return true;
            }
        }
        false
    }

    /// Distinct envs the allowlist *explicitly* names, or `None` when
    /// unrestricted / any rule uses `env="*"` (all envs visible).
    pub fn allowed_envs(&self) -> Option<Vec<String>> {
        if self.indexes_unrestricted() {
            return None;
        }
        if self.allowed.iter().any(|r| r.env == "*") {
            return None;
        }
        let mut s: std::collections::HashSet<String> = std::collections::HashSet::new();
        for r in &self.allowed {
            s.insert(r.env.clone());
        }
        let mut v: Vec<String> = s.into_iter().collect();
        v.sort();
        Some(v)
    }

    pub fn tools_unrestricted(&self) -> bool {
        self.enabled_tools.is_empty() || self.enabled_tools.iter().any(|s| s == "*")
    }

    pub fn is_tool_enabled(&self, tool_name: &str) -> bool {
        self.tools_unrestricted() || self.enabled_tools.iter().any(|t| t == tool_name)
    }
}

/// Parse `mcp.allowed_indexes` into the env-aware shape. New format is a JSON
/// array of `EnvIndexAllow`; legacy CSV of globs is promoted to one `env:"*"` rule.
pub(crate) fn parse_allowed(s: &str) -> Vec<EnvIndexAllow> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return vec![EnvIndexAllow {
            env: "*".into(),
            indexes: vec!["*".into()],
        }];
    }
    if trimmed.starts_with('[') {
        if let Ok(parsed) = serde_json::from_str::<Vec<EnvIndexAllow>>(trimmed) {
            let cleaned: Vec<EnvIndexAllow> = parsed
                .into_iter()
                .filter(|r| !r.env.trim().is_empty() && !r.indexes.is_empty())
                .collect();
            if cleaned.is_empty() {
                return vec![EnvIndexAllow {
                    env: "*".into(),
                    indexes: vec!["*".into()],
                }];
            }
            return cleaned;
        }
    }
    let indexes = parse_csv(trimmed, "*");
    vec![EnvIndexAllow {
        env: "*".into(),
        indexes,
    }]
}

/// Split a comma-joined value, trim each entry, drop empties; substitute
/// `["<fallback>"]` when the result is empty.
pub(crate) fn parse_csv(s: &str, fallback: &str) -> Vec<String> {
    let out: Vec<String> = s
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if out.is_empty() {
        vec![fallback.to_string()]
    } else {
        out
    }
}

/// One syslog routing rule: when the parsed `field` satisfies `op`/`value`, the
/// message is sent to `env`/`index` (each falling back to the configured default).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyslogRoute {
    pub field: String,
    pub op: String,
    pub value: String,
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub index: Option<String>,
}

impl SyslogRoute {
    /// A rule is usable only with a known field, a known op, and a non-empty value.
    pub fn is_valid(&self) -> bool {
        !self.value.is_empty()
            && SYSLOG_ROUTE_FIELDS.contains(&self.field.as_str())
            && SYSLOG_ROUTE_OPS.contains(&self.op.as_str())
    }
}

/// Typed view of the syslog listener config. Loaded fresh so admin edits propagate
/// without a restart; the supervisor compiles it into a router and (re)binds sockets.
#[derive(Debug, Clone, Serialize)]
pub struct SyslogSettings {
    pub enabled: bool,
    pub bind: String,
    pub udp_port: u16,
    pub tcp_port: u16,
    pub default_env: String,
    pub default_index: String,
    pub routes: Vec<SyslogRoute>,
}

impl Default for SyslogSettings {
    fn default() -> Self {
        // Off by default — admins explicitly opt in via the UI.
        Self {
            enabled: false,
            bind: SYSLOG_DEFAULT_BIND.to_string(),
            udp_port: SYSLOG_DEFAULT_PORT,
            tcp_port: SYSLOG_DEFAULT_PORT,
            default_env: SYSLOG_DEFAULT_ENV.to_string(),
            default_index: SYSLOG_DEFAULT_INDEX.to_string(),
            routes: Vec::new(),
        }
    }
}

/// Parse the stored `syslog.routes` JSON array, dropping malformed/invalid rules.
pub(crate) fn parse_syslog_routes(s: &str) -> Vec<SyslogRoute> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Vec<SyslogRoute>>(trimmed)
        .unwrap_or_default()
        .into_iter()
        .filter(SyslogRoute::is_valid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(env: &str, indexes: &[&str]) -> EnvIndexAllow {
        EnvIndexAllow {
            env: env.into(),
            indexes: indexes.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn settings(allowed: Vec<EnvIndexAllow>) -> McpSettings {
        McpSettings {
            enabled: true,
            allowed,
            enabled_tools: vec!["*".into()],
        }
    }

    #[test]
    fn unrestricted_when_empty_or_star() {
        assert!(settings(vec![]).indexes_unrestricted());
        assert!(settings(vec![allow("*", &["*"])]).indexes_unrestricted());
        // Scoped env or scoped index is restricted.
        assert!(!settings(vec![allow("prod", &["*"])]).indexes_unrestricted());
        assert!(!settings(vec![allow("*", &["orders-*"])]).indexes_unrestricted());
    }

    #[test]
    fn allows_open_allowlist_permits_everything() {
        let s = settings(vec![]);
        assert!(s.allows("prod", "anything"));
    }

    #[test]
    fn allows_env_and_index_scoping() {
        let s = settings(vec![allow("prod", &["orders-*", "payments"])]);
        assert!(s.allows("prod", "orders-eu")); // glob
        assert!(s.allows("PROD", "payments")); // env case-insensitive, exact index
        assert!(!s.allows("prod", "billing")); // index not allowed
        assert!(!s.allows("dev", "orders-eu")); // env not allowed
    }

    #[test]
    fn allows_star_index_within_env() {
        let s = settings(vec![allow("prod", &["*"])]);
        assert!(s.allows("prod", "whatever"));
        assert!(!s.allows("dev", "whatever"));
    }

    #[test]
    fn allowed_envs_listing() {
        // Unrestricted / any star-env rule -> None (all envs).
        assert_eq!(settings(vec![]).allowed_envs(), None);
        assert_eq!(settings(vec![allow("*", &["orders"])]).allowed_envs(), None);
        // Distinct scoped envs are returned sorted + deduped.
        let s = settings(vec![
            allow("prod", &["a"]),
            allow("dev", &["b"]),
            allow("prod", &["c"]),
        ]);
        assert_eq!(
            s.allowed_envs(),
            Some(vec!["dev".to_string(), "prod".to_string()])
        );
    }

    #[test]
    fn tool_allowlist() {
        let mut s = settings(vec![]);
        assert!(s.tools_unrestricted());
        assert!(s.is_tool_enabled("query_logs"));
        s.enabled_tools = vec!["query_logs".into()];
        assert!(!s.tools_unrestricted());
        assert!(s.is_tool_enabled("query_logs"));
        assert!(!s.is_tool_enabled("ingest_events"));
    }

    #[test]
    fn parse_allowed_empty_is_open() {
        let r = parse_allowed("");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].env, "*");
        assert_eq!(r[0].indexes, vec!["*".to_string()]);
    }

    #[test]
    fn parse_allowed_legacy_csv() {
        let r = parse_allowed("orders, payments");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].env, "*");
        assert_eq!(
            r[0].indexes,
            vec!["orders".to_string(), "payments".to_string()]
        );
    }

    #[test]
    fn parse_allowed_json_array() {
        let r = parse_allowed(r#"[{"env":"prod","indexes":["orders-*"]}]"#);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].env, "prod");
        assert_eq!(r[0].indexes, vec!["orders-*".to_string()]);
    }

    #[test]
    fn parse_allowed_json_filters_blank_rules_then_falls_back() {
        // All rules invalid (blank env / empty indexes) -> open fallback.
        let r = parse_allowed(r#"[{"env":"","indexes":["x"]},{"env":"prod","indexes":[]}]"#);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].env, "*");
    }

    #[test]
    fn parse_csv_trims_and_drops_empties() {
        assert_eq!(
            parse_csv("a, ,b ,", "*"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(parse_csv("   ", "fallback"), vec!["fallback".to_string()]);
    }

    #[test]
    fn syslog_route_validity() {
        let ok = SyslogRoute {
            field: "appname".into(),
            op: "equals".into(),
            value: "sshd".into(),
            env: None,
            index: Some("ssh".into()),
        };
        assert!(ok.is_valid());
        // Unknown field, unknown op, or empty value -> invalid.
        assert!(!SyslogRoute {
            field: "nope".into(),
            ..ok.clone()
        }
        .is_valid());
        assert!(!SyslogRoute {
            op: "startswith".into(),
            ..ok.clone()
        }
        .is_valid());
        assert!(!SyslogRoute {
            value: "".into(),
            ..ok.clone()
        }
        .is_valid());
    }

    #[test]
    fn parse_syslog_routes_filters_and_handles_garbage() {
        assert!(parse_syslog_routes("").is_empty());
        assert!(parse_syslog_routes("not json").is_empty());
        let routes = parse_syslog_routes(
            r#"[
                {"field":"appname","op":"equals","value":"sshd","index":"ssh"},
                {"field":"bogus","op":"equals","value":"x"}
            ]"#,
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].field, "appname");
        assert_eq!(routes[0].index.as_deref(), Some("ssh"));
    }
}
