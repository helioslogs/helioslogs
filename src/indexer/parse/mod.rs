// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Pure format parsing layer: `bytes (+ config) → Vec<Value>` via the pipeline
//! `decode → frame → parse`. IO/clock-free (table-testable); unparseable input
//! degrades to `{ "message": line }` and parse failures are counted, not silent.

mod cef;
mod framer;
mod grok;
// `pub(crate)` so the syslog network listener (src/syslog/) can call `parse_line`
// per message; the file-source path still goes through `parse()` with Format::Syslog.
pub(crate) mod syslog;

use anyhow::{Context, Result};
use serde_json::{json, Value};

pub use framer::Multiline;
pub use grok::Grok;

/// Wire format of an incoming byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One JSON object per line.
    Ndjson,
    /// A single JSON value: an array of objects, or one object.
    Json,
    /// Line-oriented plain text; each line becomes `{ "message": line }`.
    Text,
    /// RFC 3164 / RFC 5424 syslog.
    Syslog,
    /// `key=value key="quoted value"` — one event per line.
    Logfmt,
    /// Delimited columns with a header row (comma or tab, sniffed).
    Csv,
    /// Field extraction via a grok / named-capture-regex pattern (in `ParseConfig.grok`).
    Grok,
    /// ArcSight CEF — security-appliance events.
    Cef,
    /// W3C extended log (`#Fields:` header) — CloudFront / IIS access logs.
    W3c,
    /// Sniff the bytes and pick one of the above.
    Auto,
}

impl Format {
    /// Parse a `format=` string (endpoint/connector config). Unknown → `Auto`.
    pub fn from_str_lenient(s: &str) -> Format {
        match s.trim().to_ascii_lowercase().as_str() {
            "ndjson" | "jsonl" => Format::Ndjson,
            "json" => Format::Json,
            "text" | "plain" | "raw" => Format::Text,
            "syslog" => Format::Syslog,
            "logfmt" => Format::Logfmt,
            "csv" | "tsv" => Format::Csv,
            "grok" | "regex" => Format::Grok,
            "cef" => Format::Cef,
            "w3c" | "cloudfront" => Format::W3c,
            _ => Format::Auto,
        }
    }
}

/// Transport compression. `Auto` sniffs magic bytes (gzip `1f 8b`, zstd `28 b5 2f fd`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Zstd,
    Auto,
}

impl Compression {
    pub fn from_str_lenient(s: &str) -> Compression {
        match s.trim().to_ascii_lowercase().as_str() {
            "gzip" | "gz" => Compression::Gzip,
            "zstd" | "zst" => Compression::Zstd,
            "none" | "" => Compression::None,
            _ => Compression::Auto,
        }
    }
}

/// What and how to parse. Built from endpoint query params or a connector's
/// source config.
pub struct ParseConfig {
    pub format: Format,
    pub compression: Compression,
    pub multiline: Option<Multiline>,
    /// Compiled pattern for `Format::Grok`; ignored by other formats.
    pub grok: Option<Grok>,
}

impl Default for ParseConfig {
    fn default() -> Self {
        Self {
            format: Format::Auto,
            compression: Compression::Auto,
            multiline: None,
            grok: None,
        }
    }
}

/// Events plus the count of records we couldn't turn into JSON (kept, not hidden,
/// so callers can surface a parse-error rate).
pub struct ParseOutput {
    pub events: Vec<Value>,
    pub parse_errors: usize,
}

/// Decode → frame → parse. The single entry point for the whole layer.
pub fn parse(bytes: &[u8], cfg: &ParseConfig) -> Result<ParseOutput> {
    let decoded = decode(bytes, cfg.compression)?;
    let text = String::from_utf8_lossy(&decoded);
    let format = match cfg.format {
        Format::Auto => detect(&text),
        f => f,
    };

    match format {
        Format::Json => Ok(parse_json_document(&text)),
        Format::Ndjson => Ok(parse_ndjson(&text)),
        Format::Text => Ok(parse_text(&text, cfg.multiline.as_ref())),
        Format::Syslog => Ok(parse_syslog(&text, cfg.multiline.as_ref())),
        Format::Logfmt => Ok(parse_logfmt(&text, cfg.multiline.as_ref())),
        Format::Csv => Ok(parse_csv(&text)),
        Format::Grok => Ok(parse_grok(&text, cfg.grok.as_ref(), cfg.multiline.as_ref())),
        Format::Cef => Ok(parse_cef(&text, cfg.multiline.as_ref())),
        Format::W3c => Ok(parse_w3c(&text)),
        // `detect` never returns Auto; this arm keeps the match total.
        Format::Auto => Ok(parse_text(&text, cfg.multiline.as_ref())),
    }
}

/// Magic-byte + first-line sniffing. Conservative: anything ambiguous falls back
/// to `Text`, which is lossless, rather than risking a misparse.
pub fn detect(text: &str) -> Format {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if first.starts_with('[') {
        return Format::Json;
    }
    if first.starts_with('{') {
        return Format::Ndjson;
    }
    if is_syslog_prefix(first) {
        return Format::Syslog;
    }
    if looks_like_logfmt(first) {
        return Format::Logfmt;
    }
    // CSV is never auto-detected — commas are too common in prose to sniff
    // safely; it's opt-in via `format=csv`.
    Format::Text
}

/// A line is logfmt when it's mostly `key=value` tokens (≥2 pairs that cover the
/// bulk of the line). Keeps prose with an incidental `a=b` out.
fn looks_like_logfmt(line: &str) -> bool {
    let toks = logfmt_tokens(line);
    if toks.len() < 2 {
        return false;
    }
    let kv = toks.iter().filter(|t| is_logfmt_pair(t)).count();
    kv >= 2 && kv * 2 >= toks.len()
}

fn is_logfmt_pair(tok: &str) -> bool {
    match tok.split_once('=') {
        Some((k, _)) => {
            !k.is_empty()
                && k.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        }
        None => false,
    }
}

/// `<PRI>` with a 1-3 digit priority is the syslog tell for both RFCs.
fn is_syslog_prefix(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('<') else {
        return false;
    };
    let Some(close) = rest.find('>') else {
        return false;
    };
    let digits = &rest[..close];
    !digits.is_empty() && digits.len() <= 3 && digits.bytes().all(|b| b.is_ascii_digit())
}

// --- compression -------------------------------------------------------------

fn decode(bytes: &[u8], compression: Compression) -> Result<Vec<u8>> {
    let effective = match compression {
        Compression::Auto => sniff_compression(bytes),
        c => c,
    };
    match effective {
        Compression::None => Ok(bytes.to_vec()),
        Compression::Gzip => {
            use flate2::read::MultiGzDecoder;
            use std::io::Read;
            let mut out = Vec::new();
            MultiGzDecoder::new(bytes)
                .read_to_end(&mut out)
                .context("gzip decode failed")?;
            Ok(out)
        }
        Compression::Zstd => zstd::decode_all(bytes).context("zstd decode failed"),
        Compression::Auto => unreachable!("resolved above"),
    }
}

fn sniff_compression(bytes: &[u8]) -> Compression {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        Compression::Gzip
    } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Compression::Zstd
    } else {
        Compression::None
    }
}

// --- parsers -----------------------------------------------------------------

fn parse_ndjson(text: &str) -> ParseOutput {
    let mut events = Vec::new();
    let mut parse_errors = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) if v.is_object() => events.push(v),
            _ => parse_errors += 1,
        }
    }
    ParseOutput {
        events,
        parse_errors,
    }
}

fn parse_json_document(text: &str) -> ParseOutput {
    match serde_json::from_str::<Value>(text.trim()) {
        Ok(Value::Array(items)) => {
            let mut events = Vec::new();
            let mut parse_errors = 0;
            for it in items {
                if it.is_object() {
                    events.push(it);
                } else {
                    parse_errors += 1;
                }
            }
            ParseOutput {
                events,
                parse_errors,
            }
        }
        Ok(v @ Value::Object(_)) => ParseOutput {
            events: vec![v],
            parse_errors: 0,
        },
        // Not JSON, or a bare scalar — one failed document.
        _ => ParseOutput {
            events: Vec::new(),
            parse_errors: 1,
        },
    }
}

fn parse_text(text: &str, multiline: Option<&Multiline>) -> ParseOutput {
    let events = framer::frame(text, multiline)
        .into_iter()
        .map(|line| json!({ "message": line }))
        .collect();
    ParseOutput {
        events,
        parse_errors: 0,
    }
}

fn parse_syslog(text: &str, multiline: Option<&Multiline>) -> ParseOutput {
    let events = framer::frame(text, multiline)
        .into_iter()
        .map(|line| syslog::parse_line(&line))
        .collect();
    ParseOutput {
        events,
        parse_errors: 0,
    }
}

/// Apply a grok pattern per framed line. With no pattern configured, degrade to
/// plain text (each line → `message`) rather than dropping anything.
fn parse_grok(text: &str, grok: Option<&Grok>, multiline: Option<&Multiline>) -> ParseOutput {
    let lines = framer::frame(text, multiline);
    let events = match grok {
        Some(g) => lines.into_iter().map(|line| g.apply(&line)).collect(),
        None => lines
            .into_iter()
            .map(|line| json!({ "message": line }))
            .collect(),
    };
    ParseOutput {
        events,
        parse_errors: 0,
    }
}

fn parse_cef(text: &str, multiline: Option<&Multiline>) -> ParseOutput {
    let events = framer::frame(text, multiline)
        .into_iter()
        .map(|line| cef::parse_line(&line))
        .collect();
    ParseOutput {
        events,
        parse_errors: 0,
    }
}

/// W3C extended log: `#Fields:` names the columns (sniffed delimiter); split
/// CloudFront `date`/`time` become a `timestamp`. Comments and `-` cells skip.
fn parse_w3c(text: &str) -> ParseOutput {
    let mut header: Option<Vec<String>> = None;
    let mut events = Vec::new();
    let mut parse_errors = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(fields) = line.strip_prefix("#Fields:") {
            header = Some(fields.split_whitespace().map(str::to_string).collect());
            continue;
        }
        if line.starts_with('#') {
            continue; // #Version, #Date, …
        }
        let Some(keys) = &header else {
            continue; // data before the header — nothing to name it with
        };
        let row: Vec<&str> = if line.contains('\t') {
            line.split('\t').collect()
        } else {
            line.split(' ').collect()
        };
        let mut map = serde_json::Map::new();
        for (i, key) in keys.iter().enumerate() {
            if let Some(val) = row.get(i) {
                if !val.is_empty() && *val != "-" {
                    map.insert(key.clone(), json!(val));
                }
            }
        }
        // CloudFront carries date + time as separate columns.
        if let (Some(d), Some(t)) = (str_field(&map, "date"), str_field(&map, "time")) {
            map.entry("timestamp".to_string())
                .or_insert_with(|| json!(format!("{d} {t}")));
        }
        if map.is_empty() {
            parse_errors += 1;
        } else {
            events.push(Value::Object(map));
        }
    }
    ParseOutput {
        events,
        parse_errors,
    }
}

fn str_field(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

// ---- logfmt -----------------------------------------------------------------

fn parse_logfmt(text: &str, multiline: Option<&Multiline>) -> ParseOutput {
    let events = framer::frame(text, multiline)
        .into_iter()
        .map(|line| {
            let obj = parse_logfmt_line(&line);
            // Lossless fallback: a line with no `k=v` becomes a message.
            if obj.is_empty() {
                json!({ "message": line })
            } else {
                Value::Object(obj)
            }
        })
        .collect();
    ParseOutput {
        events,
        parse_errors: 0,
    }
}

fn parse_logfmt_line(line: &str) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    for tok in logfmt_tokens(line) {
        if let Some((k, v)) = tok.split_once('=') {
            if !k.is_empty() {
                map.insert(k.to_string(), json!(logfmt_unquote(v)));
            }
        }
    }
    map
}

/// Whitespace-split, but keep runs inside double quotes together.
fn logfmt_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Strip surrounding quotes and unescape `\"` / `\\`.
fn logfmt_unquote(v: &str) -> String {
    let bytes = v.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        v[1..v.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        v.to_string()
    }
}

// ---- csv / tsv --------------------------------------------------------------

/// Header row + delimited rows (RFC4180-ish quoting). Delimiter sniffed from the
/// header (tab vs comma); parsed whole-text since the header is shared.
fn parse_csv(text: &str) -> ParseOutput {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return ParseOutput {
            events: Vec::new(),
            parse_errors: 0,
        };
    };
    let delim = if header_line.matches('\t').count() > header_line.matches(',').count() {
        '\t'
    } else {
        ','
    };
    let header = csv_split(header_line, delim);

    let mut events = Vec::new();
    let mut parse_errors = 0;
    for line in lines {
        let row = csv_split(line, delim);
        let mut map = serde_json::Map::new();
        for (i, key) in header.iter().enumerate() {
            if key.is_empty() {
                continue;
            }
            if let Some(val) = row.get(i) {
                if !val.is_empty() {
                    map.insert(key.clone(), json!(val));
                }
            }
        }
        if map.is_empty() {
            parse_errors += 1;
        } else {
            events.push(Value::Object(map));
        }
    }
    ParseOutput {
        events,
        parse_errors,
    }
}

/// Split one delimited line, honoring `"`-quoted fields and `""` escapes.
fn csv_split(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            c if c == delim && !in_quotes => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(format: Format) -> ParseConfig {
        ParseConfig {
            format,
            ..Default::default()
        }
    }

    #[test]
    fn ndjson_counts_bad_lines() {
        let out = parse(b"{\"a\":1}\nnot json\n{\"b\":2}", &cfg(Format::Ndjson)).unwrap();
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.parse_errors, 1);
    }

    #[test]
    fn json_array_explodes_to_events() {
        let out = parse(b"[{\"a\":1},{\"a\":2}]", &cfg(Format::Json)).unwrap();
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.parse_errors, 0);
    }

    #[test]
    fn json_single_object() {
        let out = parse(b"{\"a\":1}", &cfg(Format::Json)).unwrap();
        assert_eq!(out.events.len(), 1);
    }

    #[test]
    fn text_wraps_each_line_as_message() {
        let out = parse(b"line one\nline two", &cfg(Format::Text)).unwrap();
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.events[0]["message"], "line one");
    }

    #[test]
    fn text_multiline_via_config() {
        let c = ParseConfig {
            format: Format::Text,
            multiline: Some(Multiline::new(r"^\d{4}", 100).unwrap()),
            ..Default::default()
        };
        let out = parse(b"2026 a\n  cont\n2026 b", &c).unwrap();
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.events[0]["message"], "2026 a\n  cont");
    }

    #[test]
    fn syslog_format_parses_pri() {
        let out = parse(
            b"<34>1 2003-10-11T22:14:15Z host su - - hi",
            &cfg(Format::Syslog),
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0]["severity"], 2);
    }

    #[test]
    fn logfmt_parses_quoted_and_bare() {
        let out = parse(
            b"level=error msg=\"db timeout occurred\" code=500 svc=checkout",
            &cfg(Format::Logfmt),
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        let e = &out.events[0];
        assert_eq!(e["level"], "error");
        assert_eq!(e["msg"], "db timeout occurred"); // quotes stripped
        assert_eq!(e["code"], "500");
        assert_eq!(e["svc"], "checkout");
    }

    #[test]
    fn logfmt_no_pairs_falls_back_to_message() {
        let out = parse(b"just a plain sentence here", &cfg(Format::Logfmt)).unwrap();
        assert_eq!(out.events[0]["message"], "just a plain sentence here");
    }

    #[test]
    fn logrus_text_formatter_is_logfmt() {
        // sirupsen/logrus TextFormatter output is logfmt; auto-detected.
        let line = br#"time="2021-01-01T00:00:00Z" level=info msg="user signed in" user_id=42"#;
        assert_eq!(detect(std::str::from_utf8(line).unwrap()), Format::Logfmt);
        let out = parse(line, &cfg(Format::Auto)).unwrap();
        let e = &out.events[0];
        assert_eq!(e["level"], "info");
        assert_eq!(e["msg"], "user signed in");
        assert_eq!(e["user_id"], "42");
        assert_eq!(e["time"], "2021-01-01T00:00:00Z");
    }

    #[test]
    fn cef_format_via_parse() {
        let out = parse(
            b"CEF:0|Vendor|Prod|1.0|42|port scan|7|src=10.0.0.1 dpt=22",
            &cfg(Format::Cef),
        )
        .unwrap();
        assert_eq!(out.events[0]["device_vendor"], "Vendor");
        assert_eq!(out.events[0]["message"], "port scan");
        assert_eq!(out.events[0]["src"], "10.0.0.1");
        assert_eq!(out.events[0]["dpt"], "22");
    }

    #[test]
    fn w3c_cloudfront_with_fields_header() {
        let body = b"#Version: 1.0\n#Fields: date time x-edge-location sc-status cs-method cs-uri-stem\n2024-01-15\t10:30:45\tIAD\t200\tGET\t/index.html\n2024-01-15\t10:30:46\tIAD\t404\tGET\t/missing";
        let out = parse(body, &cfg(Format::W3c)).unwrap();
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.events[0]["x-edge-location"], "IAD");
        assert_eq!(out.events[0]["sc-status"], "200");
        assert_eq!(out.events[0]["cs-uri-stem"], "/index.html");
        // date + time synthesized into a parseable timestamp.
        assert_eq!(out.events[0]["timestamp"], "2024-01-15 10:30:45");
        assert_eq!(out.events[1]["sc-status"], "404");
    }

    #[test]
    fn grok_format_extracts_fields() {
        let c = ParseConfig {
            format: Format::Grok,
            grok: Some(Grok::compile("log4j").unwrap()),
            ..Default::default()
        };
        let out = parse(
            b"2024-01-15 10:30:45,123 ERROR [main] com.acme.Svc - db down",
            &c,
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0]["level"], "ERROR");
        assert_eq!(out.events[0]["message"], "db down");
    }

    #[test]
    fn log4j_pattern_layout_lands_as_message() {
        // Unstructured log4j/logback PatternLayout: no JSON, no k=v — preserved
        // losslessly as `message` (field extraction awaits the grok parser).
        let line = b"2024-01-15 10:30:45,123 ERROR [main] com.acme.Svc - db timeout";
        assert_eq!(detect(std::str::from_utf8(line).unwrap()), Format::Text);
        let out = parse(line, &cfg(Format::Auto)).unwrap();
        assert_eq!(
            out.events[0]["message"],
            "2024-01-15 10:30:45,123 ERROR [main] com.acme.Svc - db timeout"
        );
    }

    #[test]
    fn csv_uses_header_and_sniffs_tab() {
        let out = parse(
            b"name,level,code\ncheckout,error,500\npayments,info,200",
            &cfg(Format::Csv),
        )
        .unwrap();
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.events[0]["name"], "checkout");
        assert_eq!(out.events[0]["level"], "error");
        assert_eq!(out.events[1]["code"], "200");

        let tsv = parse(b"a\tb\n1\t2", &cfg(Format::Csv)).unwrap();
        assert_eq!(tsv.events[0]["a"], "1");
        assert_eq!(tsv.events[0]["b"], "2");
    }

    #[test]
    fn csv_handles_quoted_commas() {
        let out = parse(
            b"msg,svc\n\"hello, world\",api\n\"she said \"\"hi\"\"\",web",
            &cfg(Format::Csv),
        )
        .unwrap();
        assert_eq!(out.events[0]["msg"], "hello, world");
        assert_eq!(out.events[1]["msg"], "she said \"hi\"");
    }

    #[test]
    fn detect_picks_format() {
        assert_eq!(detect("{\"a\":1}"), Format::Ndjson);
        assert_eq!(detect("[{\"a\":1}]"), Format::Json);
        assert_eq!(detect("<34>1 2003 host"), Format::Syslog);
        assert_eq!(detect("level=info msg=hi ts=123"), Format::Logfmt);
        assert_eq!(detect("plain log line"), Format::Text);
        assert_eq!(detect("a sentence with one=pair only"), Format::Text);
        assert_eq!(detect(""), Format::Text);
    }

    #[test]
    fn auto_format_routes_through_detect() {
        let out = parse(b"hello world", &cfg(Format::Auto)).unwrap();
        assert_eq!(out.events[0]["message"], "hello world");
    }

    #[test]
    fn gzip_roundtrip() {
        use flate2::write::GzEncoder;
        use flate2::Compression as GzLevel;
        use std::io::Write;
        let mut enc = GzEncoder::new(Vec::new(), GzLevel::default());
        enc.write_all(b"{\"a\":1}\n{\"a\":2}").unwrap();
        let gz = enc.finish().unwrap();
        // Default compression is Auto, so the gzip magic bytes are sniffed.
        let out = parse(&gz, &cfg(Format::Ndjson)).unwrap();
        assert_eq!(out.events.len(), 2);
    }

    #[test]
    fn zstd_auto_sniffed() {
        let raw = b"plain text line";
        let zst = zstd::encode_all(&raw[..], 0).unwrap();
        let out = parse(&zst, &cfg(Format::Text)).unwrap();
        assert_eq!(out.events[0]["message"], "plain text line");
    }
}
