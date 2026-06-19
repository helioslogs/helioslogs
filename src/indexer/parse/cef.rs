// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! ArcSight CEF parser: seven `|`-delimited header fields plus a `key=`-split
//! extension. `Name` becomes `message`; non-CEF lines degrade to `{message}`.

use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

const HEADER_KEYS: [&str; 7] = [
    "cef_version",
    "device_vendor",
    "device_product",
    "device_version",
    "signature_id",
    "name",
    "severity",
];

pub fn parse_line(line: &str) -> Value {
    let Some(rest) = line.strip_prefix("CEF:") else {
        return json!({ "message": line });
    };
    let (header, extension) = split_header(rest);
    if header.len() < 7 {
        return json!({ "message": line });
    }
    let mut map = Map::new();
    for (k, v) in HEADER_KEYS.iter().zip(header.iter()) {
        // `name` doubles as the message line; keep severity/ids as fields.
        if *k == "name" {
            map.insert("message".into(), json!(v));
        } else {
            map.insert((*k).into(), json!(v));
        }
    }
    parse_extension(&extension, &mut map);
    // CEF `rt` (device receipt time) is the de-facto event timestamp — surface it
    // as `timestamp` so day-routing uses it (the raw `rt` field stays too).
    if let Some(rt) = map.get("rt").cloned() {
        map.entry("timestamp".to_string()).or_insert(rt);
    }
    Value::Object(map)
}

/// Split off the first 7 `|`-delimited header fields (honoring `\|`); the
/// remainder is the extension.
fn split_header(s: &str) -> (Vec<String>, String) {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                }
            }
            '|' if fields.len() < 6 => fields.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    // `cur` holds the 7th header field plus the extension; split once more.
    if let Some((seventh, ext)) = cur.split_once('|') {
        fields.push(seventh.to_string());
        (fields, ext.to_string())
    } else {
        fields.push(cur);
        (fields, String::new())
    }
}

/// CEF extension: `key=value key=value`, values may contain spaces. Slice each
/// value from just after its `=` up to the next ` key=` boundary.
fn parse_extension(ext: &str, map: &mut Map<String, Value>) {
    let re = key_re();
    let keys: Vec<regex::Captures> = re.captures_iter(ext).collect();
    for (i, cap) in keys.iter().enumerate() {
        let key = cap.get(1).unwrap().as_str();
        let val_start = cap.get(0).unwrap().end();
        let val_end = keys
            .get(i + 1)
            .map(|n| n.get(0).unwrap().start())
            .unwrap_or(ext.len());
        let val = ext[val_start..val_end].trim();
        // `=` is escaped as `\=` in CEF extension values.
        map.insert(key.to_string(), json!(val.replace("\\=", "=")));
    }
}

/// Matches a `key=` token, anchored to start-or-whitespace so `=` inside values
/// doesn't start a new key.
fn key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:^|\s)([A-Za-z][A-Za-z0-9_.]*)=").unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cef_header_and_extension() {
        let line = "CEF:0|Security|threatmanager|1.0|100|worm stopped|10|src=10.0.0.1 dst=2.1.2.2 spt=1232 msg=blocked at edge";
        let v = parse_line(line);
        assert_eq!(v["cef_version"], "0");
        assert_eq!(v["device_vendor"], "Security");
        assert_eq!(v["device_product"], "threatmanager");
        assert_eq!(v["signature_id"], "100");
        assert_eq!(v["message"], "worm stopped");
        assert_eq!(v["severity"], "10");
        assert_eq!(v["src"], "10.0.0.1");
        assert_eq!(v["dst"], "2.1.2.2");
        assert_eq!(v["spt"], "1232");
        assert_eq!(v["msg"], "blocked at edge"); // value with spaces preserved
    }

    #[test]
    fn handles_escaped_pipe_in_header() {
        let line = r"CEF:0|Vendor|Pro\|duct|1.0|1|name here|5|act=blocked";
        let v = parse_line(line);
        assert_eq!(v["device_product"], "Pro|duct");
        assert_eq!(v["act"], "blocked");
    }

    #[test]
    fn rt_extension_becomes_timestamp() {
        let v = parse_line("CEF:0|V|P|1|1|evt|5|src=1.2.3.4 rt=2026-06-01T05:00:00Z");
        assert_eq!(v["rt"], "2026-06-01T05:00:00Z");
        assert_eq!(v["timestamp"], "2026-06-01T05:00:00Z");
    }

    #[test]
    fn non_cef_is_message() {
        assert_eq!(parse_line("not a cef line")["message"], "not a cef line");
    }
}
