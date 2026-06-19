// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `POST /api/ingest?env=&index=&source=` — accepts NDJSON, groups by day, and
//! writes each `(env, index, day)` partition; commits are async via the background
//! committer (`mark_dirty`). gzip bodies inflate transparently.

use std::borrow::Cow;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::catalog::{env_or_default, index_or_default, PartitionKey};
use crate::indexer::ingest;
use crate::indexer::parse::{self, Compression, Format, Multiline, ParseConfig};

use super::AppState;

pub(super) fn bad_request(msg: String) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg })))
}

/// Decode a request body to text, transparently inflating magic-byte-sniffed gzip.
/// zstd/other codecs go through the typed `/api/ingest/raw?compression=` path.
pub(super) fn body_text(bytes: &[u8]) -> Cow<'_, str> {
    match body_bytes(bytes) {
        Cow::Owned(v) => Cow::Owned(String::from_utf8_lossy(&v).into_owned()),
        Cow::Borrowed(b) => String::from_utf8_lossy(b),
    }
}

/// Like [`body_text`] but returns raw bytes — gzip transparently inflated,
/// otherwise borrowed as-is. For binary bodies (e.g. OTLP/protobuf).
pub(super) fn body_bytes(bytes: &[u8]) -> Cow<'_, [u8]> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;
        let mut out = Vec::new();
        if flate2::read::MultiGzDecoder::new(bytes)
            .read_to_end(&mut out)
            .is_ok()
        {
            return Cow::Owned(out);
        }
    }
    Cow::Borrowed(bytes)
}

/// Resolve + validate `(env, index)` for every ingest endpoint: apply defaults,
/// reject `_`-prefixed envs, and require the env to be registered in the control plane.
pub(super) async fn resolve_target(
    s: &AppState,
    env: Option<&str>,
    index: Option<&str>,
) -> Result<(String, String), (StatusCode, Json<Value>)> {
    let env = env_or_default(env).map_err(|e| bad_request(e.to_string()))?;
    let index = index_or_default(index, "default").map_err(|e| bad_request(e.to_string()))?;
    if env.starts_with('_') {
        return Err(bad_request(
            "env names starting with '_' are reserved for the system".to_string(),
        ));
    }
    match s.control.env_exists(&env).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(bad_request(format!(
                "unknown environment '{env}' — create it first via Admin → Environments"
            )));
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    }
    Ok((env, index))
}

/// Result of routing a batch into the block writer.
#[derive(Default)]
pub(super) struct IngestOutcome {
    pub ingested: usize,
    pub row_errors: usize,
    pub write_errors: Vec<String>,
    pub partitions: Vec<String>,
    /// Writer queue was full — caller answers 429; we stop submitting on first Full.
    pub throttled: bool,
    /// Events dropped because their index isn't in the token's allowlist.
    pub rejected: usize,
}

impl IngestOutcome {
    /// HTTP status: 429 when the queue pushed back, else 200.
    pub fn status(&self) -> StatusCode {
        if self.throttled {
            StatusCode::TOO_MANY_REQUESTS
        } else {
            StatusCode::OK
        }
    }

    pub fn errors(&self, parse_errors: usize) -> usize {
        parse_errors + self.row_errors + self.write_errors.len() + self.rejected
    }
}

/// Route each `(index, event)` to the bounded writer; `allowed` scopes writable indexes.
/// `block=true` waits for room (lossless, MUST be on a blocking thread); else flags `throttled`.
pub(super) fn submit_routed<'a, I>(
    env: &str,
    items: I,
    default_source: Option<&str>,
    allowed: Option<&std::collections::HashSet<String>>,
    block: bool,
) -> IngestOutcome
where
    I: IntoIterator<Item = (&'a str, &'a Value)>,
{
    use crate::engine::block::SubmitResult;
    let today = Utc::now().date_naive();
    let mut out = IngestOutcome::default();
    let mut touched: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (index, v) in items {
        // The index may be an `app-{{field}}` template — resolve per event.
        let index = ingest::resolve_index_template(index, v);
        if allowed.is_some_and(|a| !a.contains(&index)) {
            out.rejected += 1;
            continue;
        }
        let day = ingest::event_day(v).unwrap_or(today);
        let row = match ingest::json_to_row(v, default_source) {
            Ok(r) => r,
            Err(_) => {
                out.row_errors += 1;
                continue;
            }
        };
        let key = PartitionKey::new(env, &index, day);
        if block {
            // Lossless: waits for queue room. `false` only if the writer is gone.
            if crate::engine::block::submit_blocking(key.clone(), row) {
                out.ingested += 1;
                touched.insert(crate::catalog::partition_label(&key));
            } else {
                out.write_errors
                    .push("block ingest writer not running".to_string());
            }
        } else {
            match crate::engine::block::submit(key.clone(), row) {
                SubmitResult::Accepted => {
                    out.ingested += 1;
                    touched.insert(crate::catalog::partition_label(&key));
                }
                SubmitResult::Full => {
                    out.throttled = true;
                    break;
                }
                SubmitResult::NoWriter => out
                    .write_errors
                    .push("block ingest writer not running".to_string()),
            }
        }
    }
    out.partitions = touched.into_iter().collect();
    out
}

/// Single-index convenience over [`submit_routed`].
pub(super) fn submit_events(
    env: &str,
    index: &str,
    events: &[Value],
    default_source: Option<&str>,
    allowed: Option<&std::collections::HashSet<String>>,
    block: bool,
) -> IngestOutcome {
    submit_routed(
        env,
        events.iter().map(|v| (index, v)),
        default_source,
        allowed,
        block,
    )
}

/// Authorized + scoped target for an ingest request: the env to write to, the
/// default index, and an optional index allowlist (from a push token).
pub(super) struct IngestAuthz {
    pub env: String,
    pub default_index: String,
    pub allowed_indexes: Option<std::collections::HashSet<String>>,
}

/// A push token from `Authorization`, plus whether the scheme is *strict*: Bearer
/// are explicit attempts (unknown → 401); Basic is lenient (unknown creds fall through).
struct IngestToken {
    token: String,
    strict: bool,
}

/// Pull a push token from `Authorization`: `Bearer`, HEC, or HTTP `Basic`
/// (for shippers like the ES bulk client). For Basic the token is whichever of user/pass is set.
fn extract_ingest_token(headers: &axum::http::HeaderMap) -> Option<IngestToken> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, value) = raw.split_once(' ')?;
    let value = value.trim();
    if scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("splunk") {
        (!value.is_empty()).then(|| IngestToken {
            token: value.to_string(),
            strict: true,
        })
    } else if scheme.eq_ignore_ascii_case("basic") {
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(value)
            .ok()?;
        let creds = String::from_utf8(decoded).ok()?;
        let (user, pass) = creds.split_once(':').unwrap_or((creds.as_str(), ""));
        let tok = if !user.trim().is_empty() { user } else { pass }.trim();
        (!tok.is_empty()).then(|| IngestToken {
            token: tok.to_string(),
            strict: false,
        })
    } else {
        None
    }
}

/// Read a non-empty, trimmed request header value.
fn header_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let v = headers.get(name)?.to_str().ok()?.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Authorize + scope an ingest/shim request. A valid token pins its env + index allowlist;
/// no token is open only when ingest-auth `require` is off; a present-but-unknown token is 401.
/// Which HTTP ingestion class a request belongs to — the unit the admin
/// enable/disable toggles operate on.
#[derive(Clone, Copy)]
pub(super) enum IngestKind {
    /// `/api/ingest` and `/api/ingest/raw`.
    Api,
    /// The compatibility shims (Elasticsearch, OTLP, Loki, HEC).
    Shim,
}

pub(super) async fn authorize_ingest(
    s: &AppState,
    headers: &axum::http::HeaderMap,
    query_env: Option<&str>,
    query_index: Option<&str>,
    kind: IngestKind,
) -> Result<IngestAuthz, (StatusCode, Json<Value>)> {
    let auth = s.control.ingest_auth().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;

    // Admins can disable a whole ingestion class (both default on).
    let (enabled, label) = match kind {
        IngestKind::Api => (auth.api_enabled, "HTTP API ingestion"),
        IngestKind::Shim => (auth.shims_enabled, "compatibility-shim ingestion"),
    };
    if !enabled {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("{label} is disabled by an administrator") })),
        ));
    }

    // env/index may also arrive as headers (X-Helios-Env / X-Helios-Index) for
    // shippers that can't add query params; the query param wins when both are set.
    let hdr_env = header_value(headers, "x-helios-env");
    let hdr_index = header_value(headers, "x-helios-index");
    let env = query_env.or(hdr_env.as_deref());
    let index = query_index.or(hdr_index.as_deref());

    if let Some(tok) = extract_ingest_token(headers) {
        let found = s.control.ingest_token_find(&tok.token).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        match found {
            Some(t) => {
                // A token pins its own env; a header/query env can't override the tenancy
                // boundary. The index default still applies (checked against the allowlist).
                let default_index =
                    index_or_default(index, "default").map_err(|e| bad_request(e.to_string()))?;
                let allowed = if t.indexes.is_empty() {
                    None
                } else {
                    Some(t.indexes.into_iter().collect())
                };
                return Ok(IngestAuthz {
                    env: t.env,
                    default_index,
                    allowed_indexes: allowed,
                });
            }
            // Strict schemes (Bearer): unknown token is a hard 401. A lenient
            // Basic cred that isn't a token falls through to the open path below.
            None if tok.strict => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "invalid ingest token" })),
                ));
            }
            None => {}
        }
    }

    if auth.require {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "an ingest token is required" })),
        ));
    }
    // Open path — validate env/index the same as a plain authenticated request.
    let (env, default_index) = resolve_target(s, env, index).await?;
    Ok(IngestAuthz {
        env,
        default_index,
        allowed_indexes: None,
    })
}

#[derive(Deserialize, Default)]
pub(super) struct IngestParams {
    env: Option<String>,
    index: Option<String>,
    /// Default `source` for events lacking one in-body (often a shipper's file path).
    source: Option<String>,
}

pub(super) async fn ingest_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<IngestParams>,
    body: Bytes,
) -> impl IntoResponse {
    // Authorize + scope: a push token pins env (+ index allowlist); otherwise
    // open unless ingest-auth `require` is on. Env must be registered.
    let authz = match authorize_ingest(
        &s,
        &headers,
        p.env.as_deref(),
        p.index.as_deref(),
        IngestKind::Api,
    )
    .await
    {
        Ok(a) => a,
        Err(resp) => return resp,
    };

    // Parse + submit on a blocking thread (CPU-bound); `block = true` lets writer-queue
    // backpressure block this thread (lossless) instead of 429-ing a legitimate bulk load.
    let source = p.source;
    let (out, parse_errors) = tokio::task::spawn_blocking(move || {
        let text = body_text(&body);
        let mut events: Vec<Value> = Vec::new();
        let mut parse_errors = 0usize;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(v) => events.push(v),
                Err(_) => parse_errors += 1,
            }
        }
        let out = submit_events(
            &authz.env,
            &authz.default_index,
            &events,
            source.as_deref(),
            authz.allowed_indexes.as_ref(),
            true,
        );
        (out, parse_errors)
    })
    .await
    .unwrap_or_else(|_| (IngestOutcome::default(), 0));

    (
        out.status(),
        Json(json!({
            "ingested": out.ingested,
            "errors": out.errors(parse_errors),
            "partitions": out.partitions,
            "write_errors": out.write_errors,
            "throttled": out.throttled,
        })),
    )
}

/// `POST /api/ingest/raw` — accepts an arbitrary byte body (text/gzip/zstd) through the
/// [`crate::indexer::parse`] layer. The no-JSON escape hatch for `tail -F | curl`/appliances.
#[derive(Deserialize, Default)]
pub(super) struct IngestRawParams {
    env: Option<String>,
    index: Option<String>,
    source: Option<String>,
    /// `ndjson` | `json` | `text` | `syslog`; defaults to auto-detect.
    format: Option<String>,
    /// `gzip` | `zstd` | `none`; defaults to auto-sniff by magic bytes.
    compression: Option<String>,
    /// Regex that opens a new event; continuation lines fold into it.
    multiline_pattern: Option<String>,
    multiline_max_lines: Option<usize>,
    /// Grok / named-capture-regex pattern (or preset name) for `format=grok`.
    grok_pattern: Option<String>,
}

pub(super) async fn ingest_raw_handler(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<IngestRawParams>,
    body: Bytes,
) -> impl IntoResponse {
    let authz = match authorize_ingest(
        &s,
        &headers,
        p.env.as_deref(),
        p.index.as_deref(),
        IngestKind::Api,
    )
    .await
    {
        Ok(a) => a,
        Err(resp) => return resp,
    };

    let multiline = match p.multiline_pattern.as_deref() {
        Some(pat) if !pat.is_empty() => {
            match Multiline::new(pat, p.multiline_max_lines.unwrap_or(500)) {
                Ok(m) => Some(m),
                Err(e) => return bad_request(e.to_string()),
            }
        }
        _ => None,
    };
    let grok = match p.grok_pattern.as_deref() {
        Some(pat) if !pat.is_empty() => match crate::indexer::parse::Grok::compile(pat) {
            Ok(g) => Some(g),
            Err(e) => return bad_request(format!("invalid grok pattern: {e}")),
        },
        _ => None,
    };
    let cfg = ParseConfig {
        format: p
            .format
            .as_deref()
            .map(Format::from_str_lenient)
            .unwrap_or(Format::Auto),
        compression: p
            .compression
            .as_deref()
            .map(Compression::from_str_lenient)
            .unwrap_or(Compression::Auto),
        multiline,
        grok,
    };

    // Parse + submit on a blocking thread (CPU-bound parse; lossless `block`
    // backpressure). The compiled `cfg` (grok/multiline) moves in.
    let source = p.source;
    let result = tokio::task::spawn_blocking(move || {
        let parsed = parse::parse(&body, &cfg).map_err(|e| e.to_string())?;
        let out = submit_events(
            &authz.env,
            &authz.default_index,
            &parsed.events,
            source.as_deref(),
            authz.allowed_indexes.as_ref(),
            true,
        );
        Ok::<_, String>((out, parsed.parse_errors))
    })
    .await
    .unwrap_or_else(|_| Err("ingest task panicked".to_string()));

    match result {
        Ok((out, parse_errors)) => (
            out.status(),
            Json(json!({
                "ingested": out.ingested,
                "errors": out.errors(parse_errors),
                "partitions": out.partitions,
                "write_errors": out.write_errors,
                "throttled": out.throttled,
            })),
        ),
        Err(e) => bad_request(format!("parse failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gzip(s: &str) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(s.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn body_text_inflates_gzip_ndjson() {
        let ndjson = "{\"a\":1}\n{\"b\":2}\n";
        let compressed = gzip(ndjson);
        let decoded = body_text(&compressed);
        assert_eq!(decoded, ndjson);
        // Lines parse back to the original events.
        let n = decoded.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(n, 2);
    }

    #[test]
    fn body_text_passes_plain_utf8_through() {
        let plain = "{\"a\":1}\n";
        assert_eq!(body_text(plain.as_bytes()), plain);
    }

    #[test]
    fn body_text_falls_back_on_truncated_gzip() {
        // Starts with the gzip magic but the stream is garbage — don't panic,
        // fall back to lossy UTF-8 so the row parser just reports parse errors.
        let mut bad = gzip("{\"a\":1}\n");
        bad.truncate(4);
        let _ = body_text(&bad); // must not panic
    }

    fn auth_header(value: &str) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        h.insert(axum::http::header::AUTHORIZATION, value.parse().unwrap());
        h
    }

    fn basic(user: &str, pass: &str) -> String {
        use base64::Engine as _;
        let b = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        format!("Basic {b}")
    }

    #[test]
    fn token_from_bearer_and_hec_is_strict() {
        for scheme in ["Bearer", "bearer", "Splunk", "splunk"] {
            let t = extract_ingest_token(&auth_header(&format!("{scheme} tok123"))).unwrap();
            assert_eq!(t.token, "tok123");
            assert!(t.strict, "{scheme} should be a strict token attempt");
        }
    }

    #[test]
    fn token_from_basic_is_lenient_and_prefers_username() {
        // Token in the username (Stripe-style `tok:`), password ignored.
        let t = extract_ingest_token(&auth_header(&basic("tok123", "ignored"))).unwrap();
        assert_eq!(t.token, "tok123");
        assert!(!t.strict, "Basic should be lenient (fall through on miss)");

        // Empty username → token taken from the password.
        let t = extract_ingest_token(&auth_header(&basic("", "tok456"))).unwrap();
        assert_eq!(t.token, "tok456");
        assert!(!t.strict);
    }

    #[test]
    fn no_token_for_empty_or_unknown_scheme() {
        assert!(extract_ingest_token(&auth_header("Bearer ")).is_none());
        assert!(extract_ingest_token(&auth_header("Negotiate abc")).is_none());
        assert!(extract_ingest_token(&auth_header(&basic("", ""))).is_none());
        assert!(extract_ingest_token(&axum::http::HeaderMap::new()).is_none());
    }

    #[test]
    fn header_value_trims_and_rejects_empty() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-helios-env", "  prod  ".parse().unwrap());
        h.insert("x-helios-index", "".parse().unwrap());
        assert_eq!(header_value(&h, "x-helios-env"), Some("prod".to_string()));
        assert_eq!(header_value(&h, "x-helios-index"), None);
        assert_eq!(header_value(&h, "x-helios-missing"), None);
    }
}
