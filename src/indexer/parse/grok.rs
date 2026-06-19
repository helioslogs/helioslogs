// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Grok engine: `%{SYNTAX:field}` expands recursively against a pattern dict
//! into a named-capture [`regex::Regex`] (captures → fields, presets by name).
//! RE2-based (linear time); unmatched lines degrade to `{ "message": line }`.

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

/// A compiled grok/regex pattern ready to apply per line.
pub struct Grok {
    re: Regex,
}

impl Grok {
    /// Compile a preset name (`nginx_access`, `log4j`, …), a raw `%{...}` grok
    /// string, or a plain named-capture regex.
    pub fn compile(pattern: &str) -> Result<Grok> {
        let source = preset(pattern.trim()).unwrap_or(pattern);
        let expanded = expand(source, 0)?;
        let re = Regex::new(&expanded)
            .with_context(|| format!("grok compiled to an invalid regex: {expanded}"))?;
        Ok(Grok { re })
    }

    /// Apply to one line: named captures → fields. No match (or no captures) →
    /// the raw line as `message`, so nothing is lost.
    pub fn apply(&self, line: &str) -> Value {
        let Some(caps) = self.re.captures(line) else {
            return json!({ "message": line });
        };
        let mut map = Map::new();
        for name in self.re.capture_names().flatten() {
            if let Some(m) = caps.name(name) {
                map.insert(name.to_string(), json!(m.as_str()));
            }
        }
        if map.is_empty() {
            json!({ "message": line })
        } else {
            Value::Object(map)
        }
    }
}

/// Recursively expand `%{NAME(:field)}` refs against [`base_pattern`]; `field`
/// becomes a named capture. A depth guard stops self-referential recursion.
fn expand(pattern: &str, depth: usize) -> Result<String> {
    if depth > 25 {
        bail!("grok pattern nested too deeply (cycle?)");
    }
    let token = token_re();
    let mut out = String::new();
    let mut last = 0;
    for cap in token.captures_iter(pattern) {
        let whole = cap.get(0).unwrap();
        out.push_str(&pattern[last..whole.start()]);
        let name = &cap[1];
        let field = cap.get(2).map(|m| m.as_str());
        let base = base_pattern(name).ok_or_else(|| anyhow!("unknown grok pattern %{{{name}}}"))?;
        let sub = expand(base, depth + 1)?;
        match field {
            Some(f) => {
                out.push_str("(?P<");
                out.push_str(f);
                out.push('>');
                out.push_str(&sub);
                out.push(')');
            }
            None => {
                out.push_str("(?:");
                out.push_str(&sub);
                out.push(')');
            }
        }
        last = whole.end();
    }
    out.push_str(&pattern[last..]);
    Ok(out)
}

/// Matches `%{NAME}`, `%{NAME:field}`, `%{NAME:field:type}` (type hint ignored).
fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"%\{([A-Z0-9_]+)(?::([a-zA-Z_][a-zA-Z0-9_]*))?(?::[a-z]+)?\}").unwrap()
    })
}

/// The curated dictionary — a practical subset of the Logstash grok patterns,
/// enough for web-server, app-framework, and container log lines.
fn base_pattern(name: &str) -> Option<&'static str> {
    Some(match name {
        "USERNAME" => r"[a-zA-Z0-9._-]+",
        "USER" => r"%{USERNAME}",
        "INT" => r"[+-]?[0-9]+",
        "BASE10NUM" => r"[+-]?(?:[0-9]+(?:\.[0-9]+)?|\.[0-9]+)",
        "NUMBER" => r"%{BASE10NUM}",
        "WORD" => r"\b\w+\b",
        "NOTSPACE" => r"\S+",
        "SPACE" => r"\s*",
        "DATA" => r".*?",
        "GREEDYDATA" => r".*",
        "QUOTEDSTRING" => r#"(?:"(?:\\.|[^\\"]+)*"|'(?:\\.|[^\\']+)*')"#,
        "QS" => r"%{QUOTEDSTRING}",
        "IPV4" => {
            r"(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)"
        }
        // Loose but linear IPv6 — adequate for log fields, no backtracking.
        "IPV6" => r"(?:[0-9A-Fa-f]{0,4}:){2,7}[0-9A-Fa-f]{0,4}",
        "IP" => r"(?:%{IPV6}|%{IPV4})",
        "HOSTNAME" => {
            r"\b(?:[0-9A-Za-z][0-9A-Za-z-]{0,62})(?:\.(?:[0-9A-Za-z][0-9A-Za-z-]{0,62}))*\b"
        }
        "IPORHOST" => r"(?:%{IP}|%{HOSTNAME})",
        "MONTHDAY" => r"(?:0[1-9]|[12][0-9]|3[01]|[1-9])",
        "MONTHNUM" => r"(?:0?[1-9]|1[0-2])",
        "MONTH" => {
            r"\b(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:tember)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)\b"
        }
        "YEAR" => r"(?:\d\d){1,2}",
        "HOUR" => r"(?:2[0123]|[01]?[0-9])",
        "MINUTE" => r"(?:[0-5][0-9])",
        "SECOND" => r"(?:(?:[0-5]?[0-9]|60)(?:[:.,][0-9]+)?)",
        "TIME" => r"%{HOUR}:%{MINUTE}(?::%{SECOND})?",
        "ISO8601_TIMEZONE" => r"(?:Z|[+-]%{HOUR}(?::?%{MINUTE})?)",
        "TIMESTAMP_ISO8601" => {
            r"%{YEAR}-%{MONTHNUM}-%{MONTHDAY}[T ]%{HOUR}:?%{MINUTE}(?::?%{SECOND})?%{ISO8601_TIMEZONE}?"
        }
        "HTTPDATE" => r"%{MONTHDAY}/%{MONTH}/%{YEAR}:%{TIME} %{INT}",
        "LOGLEVEL" => {
            r"(?:[Aa]lert|[Tt]race|[Dd]ebug|[Nn]otice|[Ii]nfo|[Ww]arn(?:ing)?|[Ee]rr(?:or)?|[Cc]rit(?:ical)?|[Ff]atal|[Ss]evere|[Ee]merg(?:ency)?|ALERT|TRACE|DEBUG|NOTICE|INFO|WARN(?:ING)?|ERR(?:OR)?|CRIT(?:ICAL)?|FATAL|SEVERE|EMERG(?:ENCY)?)"
        }
        _ => return None,
    })
}

/// Whole-line presets — pick by name instead of writing grok.
fn preset(name: &str) -> Option<&'static str> {
    Some(match name {
        "common_log" | "commonapachelog" | "apache_common" => {
            r#"%{IPORHOST:clientip} %{USER:ident} %{USER:auth} \[%{HTTPDATE:timestamp}\] "%{WORD:verb} %{NOTSPACE:request}(?: HTTP/%{NUMBER:httpversion})?" %{INT:response} (?:%{INT:bytes}|-)"#
        }
        "combined_log" | "combinedapachelog" | "apache_combined" | "nginx_access" => {
            r#"%{IPORHOST:clientip} %{USER:ident} %{USER:auth} \[%{HTTPDATE:timestamp}\] "%{WORD:verb} %{NOTSPACE:request}(?: HTTP/%{NUMBER:httpversion})?" %{INT:response} (?:%{INT:bytes}|-)(?: "%{DATA:referrer}" "%{DATA:agent}")?"#
        }
        // log4j/logback default PatternLayout: "2024-01-15 10:30:45,123 ERROR [main] com.acme.Svc - msg"
        "log4j" | "logback" => {
            r#"%{TIMESTAMP_ISO8601:timestamp}\s+%{LOGLEVEL:level}\s+\[%{DATA:thread}\]\s+%{NOTSPACE:logger}\s+-\s+%{GREEDYDATA:message}"#
        }
        // CRI/containerd: "2024-01-15T10:30:45.123Z stdout F the log line"
        "cri" | "containerd" => {
            r#"%{TIMESTAMP_ISO8601:timestamp} %{WORD:stream} %{WORD:logtag} %{GREEDYDATA:message}"#
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nginx_access_preset() {
        let g = Grok::compile("nginx_access").unwrap();
        let line = r#"192.168.1.10 - alice [10/Oct/2024:13:55:36 +0000] "GET /api/orders HTTP/1.1" 200 1234 "https://x.test" "curl/8.0""#;
        let v = g.apply(line);
        assert_eq!(v["clientip"], "192.168.1.10");
        assert_eq!(v["auth"], "alice");
        assert_eq!(v["verb"], "GET");
        assert_eq!(v["request"], "/api/orders");
        assert_eq!(v["response"], "200");
        assert_eq!(v["bytes"], "1234");
        assert_eq!(v["agent"], "curl/8.0");
    }

    #[test]
    fn log4j_preset_extracts_timestamp_and_level() {
        let g = Grok::compile("log4j").unwrap();
        let v = g.apply("2024-01-15 10:30:45,123 ERROR [main] com.acme.Svc - db timeout");
        assert_eq!(v["timestamp"], "2024-01-15 10:30:45,123");
        assert_eq!(v["level"], "ERROR");
        assert_eq!(v["thread"], "main");
        assert_eq!(v["logger"], "com.acme.Svc");
        assert_eq!(v["message"], "db timeout");
    }

    #[test]
    fn cri_preset() {
        let g = Grok::compile("cri").unwrap();
        let v = g.apply("2024-01-15T10:30:45.123Z stdout F hello from container");
        assert_eq!(v["stream"], "stdout");
        assert_eq!(v["message"], "hello from container");
    }

    #[test]
    fn custom_grok_pattern() {
        let g = Grok::compile(r"%{WORD:method} %{NUMBER:latency_ms}ms").unwrap();
        let v = g.apply("GET 42ms");
        assert_eq!(v["method"], "GET");
        assert_eq!(v["latency_ms"], "42");
    }

    #[test]
    fn raw_named_capture_regex() {
        let g = Grok::compile(r"(?P<level>\w+):(?P<code>\d+)").unwrap();
        let v = g.apply("error:500");
        assert_eq!(v["level"], "error");
        assert_eq!(v["code"], "500");
    }

    #[test]
    fn unmatched_line_is_lossless() {
        let g = Grok::compile("nginx_access").unwrap();
        let v = g.apply("totally unrelated line");
        assert_eq!(v["message"], "totally unrelated line");
    }

    #[test]
    fn unknown_pattern_name_errors() {
        assert!(Grok::compile("%{NOPE:x}").is_err());
    }
}
