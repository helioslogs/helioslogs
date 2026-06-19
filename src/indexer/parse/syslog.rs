// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Syslog parsing (RFC 5424 + RFC 3164) into flat JSON: `timestamp`, `message`,
//! plus shredded host/appname/procid/facility/severity. Non-syslog → `{message}`.

use chrono::{Datelike, NaiveDateTime, TimeZone, Utc};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

fn re_5424() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // <PRI>VERSION TIMESTAMP HOST APP PROCID MSGID [SD or -] MSG
        Regex::new(
            r"(?x)
            ^<(?P<pri>\d{1,3})>(?P<ver>\d)\s+
            (?P<ts>\S+)\s+
            (?P<host>\S+)\s+
            (?P<app>\S+)\s+
            (?P<procid>\S+)\s+
            (?P<msgid>\S+)\s+
            (?P<rest>.*)$",
        )
        .unwrap()
    })
}

fn re_3164() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // <PRI>Mmm dd hh:mm:ss HOST TAG: MSG  (TAG optional)
        Regex::new(
            r"(?x)
            ^<(?P<pri>\d{1,3})>
            (?P<ts>[A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})\s+
            (?P<host>\S+)\s+
            (?:(?P<tag>[^:\[\s]+)(?:\[(?P<pid>\d+)\])?:\s*)?
            (?P<msg>.*)$",
        )
        .unwrap()
    })
}

/// Parse one syslog line. Always returns an object (never an error) — unparseable
/// input degrades to `{ "message": line }` rather than being lost.
pub fn parse_line(line: &str) -> Value {
    if let Some(v) = try_5424(line) {
        return v;
    }
    if let Some(v) = try_3164(line) {
        return v;
    }
    json!({ "message": line })
}

fn pri_parts(pri: u16) -> (u16, u16) {
    (pri / 8, pri % 8) // (facility, severity)
}

fn try_5424(line: &str) -> Option<Value> {
    let c = re_5424().captures(line)?;
    let pri: u16 = c.name("pri")?.as_str().parse().ok()?;
    let (facility, severity) = pri_parts(pri);
    let rest = c.name("rest")?.as_str();
    // Strip a leading structured-data blob (`[...]` or `-`) from the message.
    let msg = strip_structured_data(rest);
    let mut obj = serde_json::Map::new();
    obj.insert("timestamp".into(), json!(c.name("ts")?.as_str()));
    obj.insert("message".into(), json!(msg));
    obj.insert("priority".into(), json!(pri));
    obj.insert("facility".into(), json!(facility));
    obj.insert("severity".into(), json!(severity));
    insert_nondash(&mut obj, "host", c.name("host"));
    insert_nondash(&mut obj, "appname", c.name("app"));
    insert_nondash(&mut obj, "procid", c.name("procid"));
    insert_nondash(&mut obj, "msgid", c.name("msgid"));
    Some(Value::Object(obj))
}

fn try_3164(line: &str) -> Option<Value> {
    let c = re_3164().captures(line)?;
    let pri: u16 = c.name("pri")?.as_str().parse().ok()?;
    let (facility, severity) = pri_parts(pri);
    let mut obj = serde_json::Map::new();
    if let Some(ts) = c.name("ts").and_then(|m| bsd_ts_to_rfc3339(m.as_str())) {
        obj.insert("timestamp".into(), json!(ts));
    }
    obj.insert(
        "message".into(),
        json!(c.name("msg").map(|m| m.as_str()).unwrap_or("")),
    );
    obj.insert("priority".into(), json!(pri));
    obj.insert("facility".into(), json!(facility));
    obj.insert("severity".into(), json!(severity));
    insert_nondash(&mut obj, "host", c.name("host"));
    insert_nondash(&mut obj, "appname", c.name("tag"));
    insert_nondash(&mut obj, "procid", c.name("pid"));
    Some(Value::Object(obj))
}

fn insert_nondash(obj: &mut serde_json::Map<String, Value>, key: &str, m: Option<regex::Match>) {
    if let Some(v) = m.map(|m| m.as_str()) {
        if !v.is_empty() && v != "-" {
            obj.insert(key.into(), json!(v));
        }
    }
}

/// RFC5424 messages may start with `[sd-id ...]` structured data or `-` (none).
fn strip_structured_data(rest: &str) -> &str {
    let r = rest.trim_start();
    if let Some(stripped) = r.strip_prefix('-') {
        return stripped.trim_start();
    }
    if r.starts_with('[') {
        // Skip balanced `]...[` groups, then the following space.
        if let Some(end) = r.rfind(']') {
            return r[end + 1..].trim_start();
        }
    }
    r
}

/// BSD timestamps carry no year — assume the current UTC year. Returns RFC3339
/// so the universal-core timestamp parser consumes it unchanged.
fn bsd_ts_to_rfc3339(ts: &str) -> Option<String> {
    let year = Utc::now().year();
    // Collapse the double space in e.g. "Oct  1" so chrono's single-%e works.
    let normalized: String = {
        let mut parts = ts.split_whitespace();
        let mon = parts.next()?;
        let day = parts.next()?;
        let time = parts.next()?;
        format!("{mon} {day} {time}")
    };
    let with_year = format!("{normalized} {year}");
    let naive = NaiveDateTime::parse_from_str(&with_year, "%b %d %H:%M:%S %Y").ok()?;
    Some(Utc.from_utc_datetime(&naive).to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc5424() {
        let line = "<34>1 2003-10-11T22:14:15.003Z mymachine.example.com su - ID47 - msg here";
        let v = parse_line(line);
        assert_eq!(v["timestamp"], "2003-10-11T22:14:15.003Z");
        assert_eq!(v["host"], "mymachine.example.com");
        assert_eq!(v["appname"], "su");
        assert_eq!(v["message"], "msg here");
        assert_eq!(v["facility"], 4);
        assert_eq!(v["severity"], 2);
        assert!(v.get("procid").is_none()); // "-" dropped
    }

    #[test]
    fn parses_rfc5424_with_structured_data() {
        let line = r#"<165>1 2003-10-11T22:14:15.003Z host app 8710 ID [exampleSDID@32473 iut="3"] real message"#;
        let v = parse_line(line);
        assert_eq!(v["message"], "real message");
        assert_eq!(v["procid"], "8710");
    }

    #[test]
    fn parses_rfc3164_with_tag_and_pid() {
        let line = "<34>Oct 11 22:14:15 mymachine sshd[1234]: failed login";
        let v = parse_line(line);
        assert_eq!(v["host"], "mymachine");
        assert_eq!(v["appname"], "sshd");
        assert_eq!(v["procid"], "1234");
        assert_eq!(v["message"], "failed login");
        assert!(v["timestamp"].as_str().unwrap().contains("10-11T22:14:15"));
    }

    #[test]
    fn non_syslog_degrades_to_message() {
        let v = parse_line("just some text");
        assert_eq!(v["message"], "just some text");
        assert!(v.get("priority").is_none());
    }
}
