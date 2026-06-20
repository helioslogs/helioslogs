// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! OTLP/protobuf logs decoding for the `/v1/logs` shim: hand-written prost types
//! for the logs subset (no `protoc`/build.rs). Records flatten into the same
//! `(index, event)` shape as the JSON path so both encodings ingest identically.

use base64::Engine as _;
use prost::Message;
use serde_json::{json, Map, Value};

#[derive(Clone, PartialEq, Message)]
pub struct LogsData {
    #[prost(message, repeated, tag = "1")]
    pub resource_logs: Vec<ResourceLogs>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ResourceLogs {
    #[prost(message, optional, tag = "1")]
    pub resource: Option<Resource>,
    #[prost(message, repeated, tag = "2")]
    pub scope_logs: Vec<ScopeLogs>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Resource {
    #[prost(message, repeated, tag = "1")]
    pub attributes: Vec<KeyValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ScopeLogs {
    #[prost(message, repeated, tag = "2")]
    pub log_records: Vec<LogRecord>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LogRecord {
    #[prost(fixed64, tag = "1")]
    pub time_unix_nano: u64,
    #[prost(string, tag = "3")]
    pub severity_text: String,
    #[prost(message, optional, tag = "5")]
    pub body: Option<AnyValue>,
    #[prost(message, repeated, tag = "6")]
    pub attributes: Vec<KeyValue>,
    #[prost(bytes = "vec", tag = "9")]
    pub trace_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "10")]
    pub span_id: Vec<u8>,
    #[prost(fixed64, tag = "11")]
    pub observed_time_unix_nano: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct KeyValue {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(message, optional, tag = "2")]
    pub value: Option<AnyValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AnyValue {
    #[prost(oneof = "any_value::Value", tags = "1, 2, 3, 4, 5, 6, 7")]
    pub value: Option<any_value::Value>,
}

pub mod any_value {
    #[derive(Clone, PartialEq, prost::Oneof)]
    #[allow(clippy::enum_variant_names)] // variant names mirror the OTLP AnyValue oneof
    pub enum Value {
        #[prost(string, tag = "1")]
        StringValue(String),
        #[prost(bool, tag = "2")]
        BoolValue(bool),
        #[prost(int64, tag = "3")]
        IntValue(i64),
        #[prost(double, tag = "4")]
        DoubleValue(f64),
        #[prost(message, tag = "5")]
        ArrayValue(super::ArrayValue),
        #[prost(message, tag = "6")]
        KvlistValue(super::KeyValueList),
        #[prost(bytes = "vec", tag = "7")]
        BytesValue(Vec<u8>),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct ArrayValue {
    #[prost(message, repeated, tag = "1")]
    pub values: Vec<AnyValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct KeyValueList {
    #[prost(message, repeated, tag = "1")]
    pub values: Vec<KeyValue>,
}

/// Decode a binary `ExportLogsServiceRequest`/`LogsData` body into `(index, event)` pairs
/// (resource + record attrs merged, `body` → `message`, `time_unix_nano` → `timestamp`).
pub fn parse_otlp_logs_proto(
    bytes: &[u8],
    default_index: &str,
) -> Result<Vec<(String, Value)>, String> {
    let data = LogsData::decode(bytes).map_err(|e| format!("OTLP/protobuf decode failed: {e}"))?;
    let mut out = Vec::new();
    for rl in &data.resource_logs {
        let mut base = Map::new();
        if let Some(res) = &rl.resource {
            flatten_attrs(&res.attributes, &mut base);
        }
        for sl in &rl.scope_logs {
            for rec in &sl.log_records {
                let mut ev = base.clone();
                flatten_attrs(&rec.attributes, &mut ev);
                if let Some(body) = &rec.body {
                    ev.insert("message".into(), any_value_to_json(body));
                }
                let ts = if rec.time_unix_nano != 0 {
                    rec.time_unix_nano
                } else {
                    rec.observed_time_unix_nano
                };
                if ts != 0 {
                    ev.insert("timestamp".into(), json!(ts as i64));
                }
                if !rec.severity_text.is_empty() {
                    ev.entry("severity".to_string())
                        .or_insert_with(|| json!(rec.severity_text));
                }
                if !rec.trace_id.is_empty() {
                    ev.entry("traceId".to_string())
                        .or_insert_with(|| json!(hex::encode(&rec.trace_id)));
                }
                if !rec.span_id.is_empty() {
                    ev.entry("spanId".to_string())
                        .or_insert_with(|| json!(hex::encode(&rec.span_id)));
                }
                out.push((default_index.to_string(), Value::Object(ev)));
            }
        }
    }
    Ok(out)
}

fn flatten_attrs(attrs: &[KeyValue], into: &mut Map<String, Value>) {
    for a in attrs {
        if let Some(v) = &a.value {
            into.insert(a.key.clone(), any_value_to_json(v));
        }
    }
}

/// OTLP `AnyValue` → JSON scalar; arrays/kvlists nest structurally, bytes are
/// base64 (matching the OTLP/JSON wire form).
fn any_value_to_json(v: &AnyValue) -> Value {
    match &v.value {
        Some(any_value::Value::StringValue(s)) => json!(s),
        Some(any_value::Value::BoolValue(b)) => json!(b),
        Some(any_value::Value::IntValue(i)) => json!(i),
        Some(any_value::Value::DoubleValue(d)) => json!(d),
        Some(any_value::Value::BytesValue(b)) => {
            json!(base64::engine::general_purpose::STANDARD.encode(b))
        }
        Some(any_value::Value::ArrayValue(a)) => {
            Value::Array(a.values.iter().map(any_value_to_json).collect())
        }
        Some(any_value::Value::KvlistValue(kv)) => {
            let mut m = Map::new();
            for x in &kv.values {
                if let Some(val) = &x.value {
                    m.insert(x.key.clone(), any_value_to_json(val));
                }
            }
            Value::Object(m)
        }
        None => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a protobuf body via the same prost types, then assert the mapping.
    fn str_val(s: &str) -> AnyValue {
        AnyValue {
            value: Some(any_value::Value::StringValue(s.to_string())),
        }
    }

    #[test]
    fn decodes_and_flattens_logs() {
        let body = LogsData {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".into(),
                        value: Some(str_val("checkout")),
                    }],
                }),
                scope_logs: vec![ScopeLogs {
                    log_records: vec![LogRecord {
                        time_unix_nano: 1_780_000_000_000_000_000,
                        severity_text: "ERROR".into(),
                        body: Some(str_val("boom")),
                        attributes: vec![KeyValue {
                            key: "http.status".into(),
                            value: Some(AnyValue {
                                value: Some(any_value::Value::IntValue(500)),
                            }),
                        }],
                        trace_id: vec![0xab, 0xcd],
                        span_id: vec![],
                        observed_time_unix_nano: 0,
                    }],
                }],
            }],
        };
        let encoded = body.encode_to_vec();
        let items = parse_otlp_logs_proto(&encoded, "otlp").unwrap();
        assert_eq!(items.len(), 1);
        let (idx, ev) = &items[0];
        assert_eq!(idx, "otlp");
        assert_eq!(ev["service.name"], "checkout"); // resource attr
        assert_eq!(ev["http.status"], json!(500)); // record attr (int)
        assert_eq!(ev["message"], "boom"); // body
        assert_eq!(ev["severity"], "ERROR");
        assert_eq!(ev["timestamp"], json!(1_780_000_000_000_000_000i64));
        assert_eq!(ev["traceId"], "abcd"); // hex-encoded bytes
    }

    #[test]
    fn falls_back_to_observed_time() {
        let body = LogsData {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs {
                    log_records: vec![LogRecord {
                        time_unix_nano: 0,
                        severity_text: String::new(),
                        body: Some(str_val("x")),
                        attributes: vec![],
                        trace_id: vec![],
                        span_id: vec![],
                        observed_time_unix_nano: 42,
                    }],
                }],
            }],
        };
        let items = parse_otlp_logs_proto(&body.encode_to_vec(), "otlp").unwrap();
        assert_eq!(items[0].1["timestamp"], json!(42i64));
    }

    #[test]
    fn garbage_errors_cleanly() {
        // Random bytes that aren't a valid protobuf message.
        assert!(parse_otlp_logs_proto(&[0xff, 0xff, 0xff, 0xff], "x").is_err());
    }
}
