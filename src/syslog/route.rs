// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Compiled routing table for syslog messages. Built once from `SyslogSettings`
//! (regexes compiled up front), then consulted per message to pick `(env, index)`.

use regex::Regex;
use serde_json::Value;
use tracing::warn;

use crate::control::settings::SyslogSettings;

/// The parsed syslog fields the router matches against (all stringified so a single
/// operator set covers text and numeric fields like facility/severity).
#[derive(Debug, Default)]
pub struct SyslogFields {
    pub host: Option<String>,
    pub appname: Option<String>,
    pub facility: Option<String>,
    pub severity: Option<String>,
    pub message: Option<String>,
    pub source_ip: String,
}

impl SyslogFields {
    /// Build from the parser's flat JSON `Value` plus the sender's IP.
    pub fn from_parsed(v: &Value, source_ip: &str) -> Self {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| v.get(k).and_then(Value::as_i64).map(|x| x.to_string());
        Self {
            host: s("host"),
            appname: s("appname"),
            facility: n("facility"),
            severity: n("severity"),
            message: s("message"),
            source_ip: source_ip.to_string(),
        }
    }

    fn get(&self, field: &str) -> Option<&str> {
        match field {
            "host" => self.host.as_deref(),
            "appname" => self.appname.as_deref(),
            "facility" => self.facility.as_deref(),
            "severity" => self.severity.as_deref(),
            "message" => self.message.as_deref(),
            "source_ip" => Some(self.source_ip.as_str()),
            _ => None,
        }
    }
}

enum Matcher {
    Equals(String),
    Contains(String), // pre-lowercased
    Regex(Regex),
}

struct CompiledRoute {
    field: String,
    matcher: Matcher,
    env: String,
    index: String,
}

impl CompiledRoute {
    fn matches(&self, fields: &SyslogFields) -> bool {
        let Some(actual) = fields.get(&self.field) else {
            return false;
        };
        match &self.matcher {
            Matcher::Equals(v) => actual.eq_ignore_ascii_case(v),
            Matcher::Contains(v) => actual.to_ascii_lowercase().contains(v.as_str()),
            Matcher::Regex(re) => re.is_match(actual),
        }
    }
}

/// Compiled routing table: a default `(env, index)` plus ordered rules.
pub struct SyslogRouter {
    default_env: String,
    default_index: String,
    routes: Vec<CompiledRoute>,
}

impl SyslogRouter {
    /// Compile settings into a router. Rules with an uncompilable regex are dropped
    /// (logged) so one bad rule can't break ingestion.
    pub fn build(s: &SyslogSettings) -> Self {
        let pick = |o: &Option<String>, default: &str| {
            o.as_deref()
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .unwrap_or(default)
                .to_string()
        };
        let mut routes = Vec::new();
        for r in &s.routes {
            let matcher = match r.op.as_str() {
                "equals" => Matcher::Equals(r.value.clone()),
                "contains" => Matcher::Contains(r.value.to_ascii_lowercase()),
                "regex" => match Regex::new(&r.value) {
                    Ok(re) => Matcher::Regex(re),
                    Err(e) => {
                        warn!(rule = %r.value, "syslog: invalid route regex, skipping: {e}");
                        continue;
                    }
                },
                _ => continue,
            };
            routes.push(CompiledRoute {
                field: r.field.clone(),
                matcher,
                env: pick(&r.env, &s.default_env),
                index: pick(&r.index, &s.default_index),
            });
        }
        Self {
            default_env: s.default_env.clone(),
            default_index: s.default_index.clone(),
            routes,
        }
    }

    /// First matching rule wins; otherwise the configured default `(env, index)`.
    pub fn route_for(&self, fields: &SyslogFields) -> (&str, &str) {
        for r in &self.routes {
            if r.matches(fields) {
                return (&r.env, &r.index);
            }
        }
        (&self.default_env, &self.default_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::settings::SyslogRoute;

    fn route(field: &str, op: &str, value: &str, index: &str) -> SyslogRoute {
        SyslogRoute {
            field: field.into(),
            op: op.into(),
            value: value.into(),
            env: None,
            index: Some(index.into()),
        }
    }

    fn settings(routes: Vec<SyslogRoute>) -> SyslogSettings {
        SyslogSettings {
            default_env: "default".into(),
            default_index: "syslog".into(),
            routes,
            ..Default::default()
        }
    }

    fn fields(appname: &str, severity: i64, msg: &str) -> SyslogFields {
        SyslogFields {
            appname: Some(appname.into()),
            severity: Some(severity.to_string()),
            message: Some(msg.into()),
            source_ip: "10.0.0.1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn falls_back_to_default_when_no_rules() {
        let r = SyslogRouter::build(&settings(vec![]));
        assert_eq!(r.route_for(&fields("sshd", 6, "hi")), ("default", "syslog"));
    }

    #[test]
    fn first_matching_rule_wins() {
        let r = SyslogRouter::build(&settings(vec![
            route("appname", "equals", "sshd", "ssh"),
            route("appname", "equals", "sshd", "never"),
        ]));
        assert_eq!(r.route_for(&fields("sshd", 6, "hi")), ("default", "ssh"));
    }

    #[test]
    fn equals_is_case_insensitive_contains_and_regex() {
        let r = SyslogRouter::build(&settings(vec![
            route("appname", "equals", "SSHD", "ssh"),
            route("message", "contains", "Panic", "crash"),
            route("severity", "regex", r"^[0-3]$", "urgent"),
        ]));
        assert_eq!(r.route_for(&fields("sshd", 6, "ok")).1, "ssh");
        assert_eq!(
            r.route_for(&fields("nginx", 6, "kernel panic now")).1,
            "crash"
        );
        assert_eq!(r.route_for(&fields("nginx", 2, "ok")).1, "urgent");
        assert_eq!(r.route_for(&fields("nginx", 6, "ok")).1, "syslog");
    }

    #[test]
    fn route_inherits_default_env_when_unset() {
        let mut rt = route("appname", "equals", "sshd", "ssh");
        rt.env = None;
        let r = SyslogRouter::build(&settings(vec![rt]));
        assert_eq!(r.route_for(&fields("sshd", 6, "hi")), ("default", "ssh"));
    }

    #[test]
    fn invalid_regex_rule_is_dropped() {
        let r = SyslogRouter::build(&settings(vec![route("message", "regex", "(unclosed", "x")]));
        // Bad rule dropped -> default used, no panic.
        assert_eq!(r.route_for(&fields("sshd", 6, "anything")).1, "syslog");
    }

    #[test]
    fn from_parsed_extracts_numeric_and_string_fields() {
        let v = crate::indexer::parse::syslog::parse_line(
            "<34>Oct 11 22:14:15 myhost sshd[123]: failed login",
        );
        let f = SyslogFields::from_parsed(&v, "192.0.2.5");
        assert_eq!(f.appname.as_deref(), Some("sshd"));
        assert_eq!(f.host.as_deref(), Some("myhost"));
        assert_eq!(f.severity.as_deref(), Some("2")); // 34 % 8
        assert_eq!(f.source_ip, "192.0.2.5");
    }
}
