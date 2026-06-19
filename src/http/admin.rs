// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `/api/admin/*` — indexes, partitions, index info, settings, merge, commit, gc.
//! `index-info`/`merge` accept an optional `?index=&day=` filter to scope a partition.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use chrono::NaiveDate;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::catalog::PartitionKey;
use crate::control::settings::{
    EnvIndexAllow, KEY_ALERT_WEBHOOK_ENABLED, KEY_ALERT_WEBHOOK_FORMAT, KEY_ALERT_WEBHOOK_URL,
    KEY_MCP_ALLOWED_INDEXES, KEY_MCP_ENABLED, KEY_MCP_ENABLED_TOOLS, KEY_RETENTION_DEFAULT_DAYS,
    KEY_THEME_DEFAULT_APPEARANCE, KEY_THEME_DEFAULT_PALETTE, THEME_DEFAULT_APPEARANCE,
    THEME_PALETTES,
};
use crate::engine::block::{configured_store, BlockStore};

use super::auth::Principal;
use super::AppState;

/// Onboarding helper: ingest a batch of synthetic logs into the caller's active env
/// so a fresh instance isn't an empty search. Admin-only; safe to click repeatedly
/// (each call just appends more). Returns how many events landed.
pub(super) async fn load_sample_handler(principal: Principal) -> impl IntoResponse {
    let env = match super::auth::resolve_request_env(None, &principal) {
        Ok(e) => e,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            )
        }
    };
    // Block-mode ingest waits for queue room, so it MUST run off the async runtime.
    let outcome = tokio::task::spawn_blocking(move || {
        let events = crate::sample_data::generate();
        super::ingest::submit_routed(
            &env,
            events.iter().map(|(idx, v)| (*idx, v)),
            None,
            None,
            true,
        )
    })
    .await;

    match outcome {
        Ok(out) => (
            StatusCode::OK,
            Json(json!({ "ingested": out.ingested, "partitions": out.partitions })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

/// Per-partition block stats: `(num_rows, num_blocks, total_bytes)`. Reads only
/// block footers, never the 64 MB bodies, so a whole-env rollup stays cheap.
fn block_stats(store: &BlockStore, key: &PartitionKey) -> (u64, usize, u64) {
    let manifest = store.load_manifest(key).unwrap_or_default();
    let mut rows = 0u64;
    let mut bytes = 0u64;
    for id in &manifest.blocks {
        bytes += store.block_size(key, id).unwrap_or(0);
        if let Ok(f) = store.read_footer(key, id) {
            rows += f.row_count as u64;
        }
    }
    (rows, manifest.blocks.len(), bytes)
}

/// Index names for the search-bar dropdown, scoped to the caller's active env.
/// `_*` indexes only exist under `_system`, so env scoping gives the right visibility.
pub(super) async fn indexes_handler(
    State(s): State<AppState>,
    principal: Principal,
) -> impl IntoResponse {
    let env = {
        let e = principal.active_env.trim();
        if e.is_empty() {
            crate::catalog::DEFAULT_ENV
        } else {
            e
        }
    };
    let store = configured_store(s.catalog.root());
    let mut set = std::collections::BTreeSet::new();
    for k in store.list_partitions().unwrap_or_default() {
        if k.env == env {
            set.insert(k.index);
        }
    }
    let indexes: Vec<String> = set.into_iter().collect();
    (StatusCode::OK, Json(json!({ "indexes": indexes })))
}

pub(super) async fn partitions_handler(State(s): State<AppState>) -> impl IntoResponse {
    let store = configured_store(s.catalog.root());
    let mut out: Vec<Value> = Vec::new();
    for k in store.list_partitions().unwrap_or_default() {
        let (docs, blocks, bytes) = block_stats(&store, &k);
        out.push(json!({
            "env": k.env,
            "index": k.index,
            "day": k.day_string(),
            "num_docs": docs,
            "num_segments": blocks,
            "byte_size": bytes,
            "file_count": blocks,
        }));
    }
    (StatusCode::OK, Json(json!({ "partitions": out })))
}

#[derive(Deserialize, Default)]
pub(super) struct InfoParams {
    env: Option<String>,
    index: Option<String>,
    day: Option<String>,
}

/// Distinct `(env, index)` identities (names only, from manifest listing — cheap).
/// Backs the allowlist UIs (user grants, MCP) that need index names per env, not sizes.
pub(super) async fn index_catalog_handler(State(s): State<AppState>) -> impl IntoResponse {
    let root = s.catalog.root().to_path_buf();
    let joined = tokio::task::spawn_blocking(move || {
        let store = configured_store(&root);
        let mut set = std::collections::BTreeSet::new();
        for k in store.list_partitions().unwrap_or_default() {
            set.insert((k.env, k.index));
        }
        set.into_iter()
            .map(|(env, index)| json!({ "env": env, "index": index }))
            .collect::<Vec<Value>>()
    })
    .await;
    match joined {
        Ok(indexes) => (StatusCode::OK, Json(json!({ "indexes": indexes }))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "index-catalog computation failed" })),
        ),
    }
}

pub(super) async fn index_info_handler(
    State(s): State<AppState>,
    Query(p): Query<InfoParams>,
) -> impl IntoResponse {
    // index+day → that partition's deep info; else a catalog-wide summary.
    // `env` defaults to `default` for single-partition lookups.
    if let (Some(index_name), Some(day_s)) = (p.index.as_deref(), p.day.as_deref()) {
        let env_name = match crate::catalog::env_or_default(p.env.as_deref()) {
            Ok(s) => s,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": e.to_string() })),
                );
            }
        };
        let day = match NaiveDate::parse_from_str(day_s, "%Y-%m-%d") {
            Ok(d) => d,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("bad day: {e}") })),
                );
            }
        };
        let key = PartitionKey::new(&env_name, index_name, day);
        let store = configured_store(s.catalog.root());
        if !store.partition_exists(&key) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no such partition" })),
            );
        }
        let dir = s.catalog.partition_path(&key);
        return (
            StatusCode::OK,
            Json(json!({
                "scope": "partition",
                "env": key.env,
                "index": key.index,
                "day": key.day_string(),
                "directory": dir.display().to_string(),
                "summary": partition_detail(&store, &key),
            })),
        );
    }

    // Catalog rollup, optionally scoped to `?env=`. One footer read per block, so run
    // off the async runtime and fan out with rayon; footer reads are cached by block id.
    let root = s.catalog.root().to_path_buf();
    let data_dir = root.display().to_string();
    let env_filter = p
        .env
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string);
    let joined = tokio::task::spawn_blocking(move || {
        let store = configured_store(&root);
        let mut keys = store.list_partitions().unwrap_or_default();
        if let Some(env) = &env_filter {
            keys.retain(|k| &k.env == env);
        }
        // Per-partition stats in parallel; collect preserves key order.
        let stats: Vec<(PartitionKey, (u64, usize, u64))> = keys
            .par_iter()
            .map(|k| (k.clone(), block_stats(&store, k)))
            .collect();

        let mut total_docs = 0u64;
        let mut total_blocks = 0usize;
        let mut total_bytes = 0u64;
        let mut parts: Vec<Value> = Vec::with_capacity(stats.len());
        for (k, (docs, blocks, bytes)) in stats {
            total_docs += docs;
            total_blocks += blocks;
            total_bytes += bytes;
            parts.push(json!({
                "env": k.env,
                "index": k.index,
                "day": k.day_string(),
                "num_docs": docs,
                "num_segments": blocks,
                "byte_size": bytes,
            }));
        }
        json!({
            "scope": "catalog",
            "data_dir": data_dir,
            "num_partitions": parts.len(),
            "num_docs": total_docs,
            "num_segments": total_blocks,
            "total_bytes": total_bytes,
            // Schema-on-read: only universal-core fields are structural; the rest is the
            // dynamic JSON column, sampled per query via /api/discover_fields.
            "schema": universal_core_schema(),
            "partitions": parts,
        })
    })
    .await;
    match joined {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "index-info computation failed" })),
        ),
    }
}

/// The fixed universal-core fields shared by every partition (schema-on-read).
/// Drives the "Shared schema" card in the admin UI.
fn universal_core_schema() -> Value {
    json!([
        { "name": "timestamp", "type": "datetime" },
        { "name": "message", "type": "text" },
        { "name": "raw", "type": "text" },
        { "name": "source", "type": "keyword" },
        { "name": "source_raw", "type": "keyword" },
    ])
}

/// Per-block detail for one partition (block engine).
fn partition_detail(store: &BlockStore, key: &PartitionKey) -> Value {
    let manifest = store.load_manifest(key).unwrap_or_default();
    let mut total_docs = 0u64;
    let mut total_bytes = 0u64;
    let blocks: Vec<Value> = manifest
        .blocks
        .iter()
        .map(|id| {
            let byte_size = store.block_size(key, id).unwrap_or(0);
            let num_rows = store.read_footer(key, id).map(|f| f.row_count).unwrap_or(0);
            total_bytes += byte_size;
            total_docs += num_rows as u64;
            json!({ "id": id, "num_rows": num_rows, "byte_size": byte_size })
        })
        .collect();
    json!({
        "num_docs": total_docs,
        "num_segments": blocks.len(),
        "total_bytes": total_bytes,
        "blocks": blocks,
    })
}

pub(super) async fn get_settings_handler(State(s): State<AppState>) -> impl IntoResponse {
    match build_settings_response(&s).await {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(Deserialize, Default)]
pub(super) struct SettingsPatch {
    // MCP settings (see McpSettings). Access is gated by API keys (Admin → API keys),
    // not a per-server token.
    mcp_enabled: Option<bool>,
    /// New env-aware shape, persisted as JSON; wire shape mirrors [`EnvIndexAllow`].
    mcp_allowed: Option<Vec<EnvIndexAllow>>,
    /// Legacy CSV shape (pre-env); promoted to a single global rule (`env: "*"`).
    mcp_allowed_indexes: Option<Vec<String>>,
    mcp_enabled_tools: Option<Vec<String>>,
    // Alert webhook delivery; `alert_webhook_url: Some("")` clears the URL.
    alert_webhook_enabled: Option<bool>,
    alert_webhook_url: Option<String>,
    alert_webhook_format: Option<String>,
    // Retention; `Some(0)` or `Some(null)`-as-0 clears (keep forever).
    retention_default_days: Option<i64>,
    // Instance theme defaults; users override per-account.
    theme_default_appearance: Option<String>,
    theme_default_palette: Option<String>,
}

pub(super) async fn post_settings_handler(
    State(s): State<AppState>,
    Json(p): Json<SettingsPatch>,
) -> impl IntoResponse {
    // --- MCP settings persist to the control DB ---
    if let Some(b) = p.mcp_enabled {
        if let Err(e) = s.control.set_setting(KEY_MCP_ENABLED, &b.to_string()).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save mcp_enabled: {e}") })),
            );
        }
    }
    if let Some(allowed) = p.mcp_allowed {
        // Drop rules with blank env or empty index lists at save-time so the
        // DB row is clean (the loader would also drop them).
        let cleaned: Vec<EnvIndexAllow> = allowed
            .into_iter()
            .filter(|r| !r.env.trim().is_empty() && !r.indexes.is_empty())
            .map(|r| EnvIndexAllow {
                env: r.env.trim().to_string(),
                indexes: r
                    .indexes
                    .iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            })
            .filter(|r| !r.indexes.is_empty())
            .collect();
        let json_str = serde_json::to_string(&cleaned).unwrap_or_else(|_| "[]".into());
        if let Err(e) = s
            .control
            .set_setting(KEY_MCP_ALLOWED_INDEXES, &json_str)
            .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save mcp_allowed: {e}") })),
            );
        }
    } else if let Some(list) = p.mcp_allowed_indexes {
        // Legacy callers: promote a flat CSV list to a single global rule.
        let csv = sanitize_csv(&list);
        if let Err(e) = s.control.set_setting(KEY_MCP_ALLOWED_INDEXES, &csv).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save mcp_allowed_indexes: {e}") })),
            );
        }
    }
    if let Some(list) = p.mcp_enabled_tools {
        let csv = sanitize_csv(&list);
        if let Err(e) = s.control.set_setting(KEY_MCP_ENABLED_TOOLS, &csv).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save mcp_enabled_tools: {e}") })),
            );
        }
    }
    if let Some(b) = p.alert_webhook_enabled {
        if let Err(e) = s
            .control
            .set_setting(KEY_ALERT_WEBHOOK_ENABLED, &b.to_string())
            .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save alert_webhook_enabled: {e}") })),
            );
        }
    }
    if let Some(url) = p.alert_webhook_url {
        let trimmed = url.trim();
        let res = if trimmed.is_empty() {
            s.control.unset_setting(KEY_ALERT_WEBHOOK_URL).await
        } else if let Err(e) = crate::outbound::validate_outbound_url(trimmed) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("alert_webhook_url: {e}") })),
            );
        } else {
            s.control.set_setting(KEY_ALERT_WEBHOOK_URL, trimmed).await
        };
        if let Err(e) = res {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save alert_webhook_url: {e}") })),
            );
        }
    }
    if let Some(f) = p.alert_webhook_format {
        if let Err(e) = s
            .control
            .set_setting(KEY_ALERT_WEBHOOK_FORMAT, f.trim())
            .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save alert_webhook_format: {e}") })),
            );
        }
    }
    if let Some(d) = p.retention_default_days {
        let res = if d <= 0 {
            s.control.unset_setting(KEY_RETENTION_DEFAULT_DAYS).await
        } else {
            s.control
                .set_setting(KEY_RETENTION_DEFAULT_DAYS, &d.to_string())
                .await
        };
        if let Err(e) = res {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save retention_default_days: {e}") })),
            );
        }
    }
    if let Some(a) = p.theme_default_appearance {
        let a = a.trim();
        if a != "light" && a != "dark" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "theme_default_appearance must be 'light' or 'dark'" })),
            );
        }
        if let Err(e) = s.control.set_setting(KEY_THEME_DEFAULT_APPEARANCE, a).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save theme_default_appearance: {e}") })),
            );
        }
    }
    if let Some(pal) = p.theme_default_palette {
        let pal = pal.trim();
        if !THEME_PALETTES.contains(&pal) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!(
                    "theme_default_palette must be one of: {}",
                    THEME_PALETTES.join(", ")
                ) })),
            );
        }
        if let Err(e) = s.control.set_setting(KEY_THEME_DEFAULT_PALETTE, pal).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("save theme_default_palette: {e}") })),
            );
        }
    }
    match build_settings_response(&s).await {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

async fn build_settings_response(s: &AppState) -> anyhow::Result<Value> {
    let mcp = s.control.mcp_settings().await?;
    let webhook_enabled = s
        .control
        .get_setting(KEY_ALERT_WEBHOOK_ENABLED)
        .await?
        .map(|v| v == "true")
        .unwrap_or(false);
    let webhook_url = s.control.get_setting(KEY_ALERT_WEBHOOK_URL).await?;
    let webhook_format = s
        .control
        .get_setting(KEY_ALERT_WEBHOOK_FORMAT)
        .await?
        .unwrap_or_else(|| "generic".into());
    // Configured value drives the editable input; the effective value folds in the
    // env override (which, when set, wins and locks the field in the UI).
    let retention_setting = s.control.get_setting(KEY_RETENTION_DEFAULT_DAYS).await?;
    let retention_days: i64 = retention_setting
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let retention_env_overridden = crate::retention::is_default_days_env_overridden();
    let retention_effective: i64 =
        crate::retention::effective_default_days(retention_setting).unwrap_or(0);
    let theme_appearance = s
        .control
        .get_setting(KEY_THEME_DEFAULT_APPEARANCE)
        .await?
        .unwrap_or_else(|| THEME_DEFAULT_APPEARANCE.into());
    let theme_palette = crate::control::settings::palette_or_default(
        s.control.get_setting(KEY_THEME_DEFAULT_PALETTE).await?,
    );
    Ok(json!({
        "mcp_enabled": mcp.enabled,
        "mcp_allowed": mcp.allowed,
        "mcp_enabled_tools": mcp.enabled_tools,
        // The URL may embed a secret — report presence only.
        "alert_webhook_enabled": webhook_enabled,
        "alert_webhook_url_set": webhook_url.is_some_and(|u| !u.trim().is_empty()),
        "alert_webhook_format": webhook_format,
        "retention_default_days": retention_days,
        "retention_default_days_effective": retention_effective,
        "retention_default_days_env": std::env::var(crate::retention::RETENTION_DEFAULT_DAYS_ENV).ok(),
        "retention_default_days_env_overridden": retention_env_overridden,
        "theme_default_appearance": theme_appearance,
        "theme_default_palette": theme_palette,
    }))
}

#[derive(Deserialize, Default)]
pub(super) struct TestWebhookRequest {
    /// Test this URL/format directly when given; else the saved settings target.
    url: Option<String>,
    format: Option<String>,
}

/// Send a synthetic alert to the webhook target so admins can verify wiring.
pub(super) async fn test_webhook_handler(
    State(s): State<AppState>,
    Json(req): Json<TestWebhookRequest>,
) -> impl IntoResponse {
    let url = match req.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => u.to_string(),
        None => match s.control.get_setting(KEY_ALERT_WEBHOOK_URL).await {
            Ok(Some(u)) if !u.trim().is_empty() => u,
            Ok(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "ok": false, "error": "no webhook url configured" })),
                )
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "ok": false, "error": e.to_string() })),
                )
            }
        },
    };
    let format = match req.format {
        Some(f) => crate::notify::WebhookFormat::parse(&f),
        None => crate::notify::WebhookFormat::parse(
            &s.control
                .get_setting(KEY_ALERT_WEBHOOK_FORMAT)
                .await
                .ok()
                .flatten()
                .unwrap_or_default(),
        ),
    };
    let payload = crate::notify::build_payload(format, &crate::notify::test_alert());
    match crate::notify::send_webhook(&url, &payload).await {
        Ok(status) if status.is_success() => (
            StatusCode::OK,
            Json(json!({ "ok": true, "status": status.as_u16() })),
        ),
        Ok(status) => (
            StatusCode::OK,
            Json(json!({ "ok": false, "status": status.as_u16(), "error": "non-2xx response" })),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

/// One `HELIOS_*` configuration knob as shown on the admin page.
#[derive(Serialize)]
struct ConfigEntry {
    name: &'static str,
    category: &'static str,
    /// Effective value the server is running with (secrets reported as a status string).
    value: String,
    /// `true` when an explicit env override is set (vs. the built-in default).
    overridden: bool,
    description: &'static str,
}

/// Read-only snapshot of storage locations + `HELIOS_*` config (all fixed at startup);
/// secret-file paths report only a status, never the value.
pub(super) async fn runtime_config_handler(State(s): State<AppState>) -> impl IntoResponse {
    Json(json!({ "entries": runtime_config_entries(&s) }))
}

fn runtime_config_entries(s: &AppState) -> Vec<ConfigEntry> {
    fn present(name: &str) -> bool {
        std::env::var(name)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    }
    // Effective value of a knob with a documented default, plus whether it was
    // explicitly overridden.
    fn or_default(name: &'static str, default: &str) -> (String, bool) {
        match std::env::var(name) {
            Ok(v) if !v.trim().is_empty() => (v.trim().to_string(), true),
            _ => (default.to_string(), false),
        }
    }
    let val = |name: &'static str, category, default: &str, description| {
        let (value, overridden) = or_default(name, default);
        ConfigEntry {
            name,
            category,
            value,
            overridden,
            description,
        }
    };

    let codec = match crate::engine::block_codec() {
        crate::engine::block::Codec::Zstd => "zstd",
        crate::engine::block::Codec::None => "off",
    };
    let encryption_disabled = std::env::var("HELIOS_CONTROL_ENCRYPTION")
        .map(|v| {
            matches!(
                v.trim().to_lowercase().as_str(),
                "0" | "off" | "false" | "no"
            )
        })
        .unwrap_or(false);
    let secret_status = |name: &str, default_file: &str| match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => format!("file: {}", v.trim()),
        _ => format!("auto-generated (./{default_file})"),
    };

    let data_dir = s.catalog.root().display().to_string();
    let (shared_value, shared_set) = match &s.shared_store {
        Some(uri) if !uri.trim().is_empty() => (uri.trim().to_string(), true),
        _ => ("(none — single-node, local only)".to_string(), false),
    };

    vec![
        // ---- Storage location (CLI args) ----
        ConfigEntry {
            name: "--data-dir",
            category: "Storage location",
            value: data_dir,
            overridden: true,
            description: "Local data directory: block partitions, the control plane (when no shared store), and generated key files.",
        },
        ConfigEntry {
            name: "--shared-store",
            category: "Storage location",
            value: shared_value,
            overridden: shared_set,
            description: "Shared object store (S3 URL or path) for multi-node replication and DR. The engine always reads/writes local first and syncs in the background.",
        },
        // ---- Storage engine (block engine) ----
        ConfigEntry {
            name: "HELIOS_BLOCK_COMPRESSION",
            category: "Storage engine",
            value: codec.to_string(),
            overridden: present("HELIOS_BLOCK_COMPRESSION"),
            description: "Per-section block compression codec. 'off' writes uncompressed blocks (benchmarking only).",
        },
        // Storage-engine, retention, and query tunables that used to live here are
        // now editable under Admin → General → Server tunables (env still wins).
        // ---- Control plane ----
        ConfigEntry {
            name: "HELIOS_CONTROL_ENCRYPTION",
            category: "Control plane",
            value: if encryption_disabled { "off" } else { "on" }.to_string(),
            overridden: present("HELIOS_CONTROL_ENCRYPTION"),
            description: "AES-256-GCM encryption of the JSON control files at rest. On by default.",
        },
        ConfigEntry {
            name: "HELIOS_CONTROL_KEY_PATH",
            category: "Control plane",
            value: secret_status("HELIOS_CONTROL_KEY_PATH", "secret-control.json"),
            overridden: present("HELIOS_CONTROL_KEY_PATH"),
            description: "Path to the 32-byte control-plane key file. Point at a persistent, shared/identical path on every node for multi-node; auto-generated as ./secret-control.json otherwise. Never the data-dir (a per-node cache).",
        },
        val("HELIOS_CONTROL_CACHE_TTL_SECS", "Control plane", "10",
            "TTL for the hot single-document read cache (users/envs/settings). 0 disables it."),
        // ---- Security ----
        ConfigEntry {
            name: "crypto provider",
            category: "Security",
            value: if crate::crypto::fips_active() {
                "AWS-LC FIPS 140-3 (active)".to_string()
            } else {
                "AWS-LC (standard, non-FIPS build)".to_string()
            },
            overridden: false,
            description: "Cryptographic backend. All AEAD/hash/MAC/random ops route through the crypto seam; the FIPS-validated module is selected by building with the `fips` feature.",
        },
        // ---- Authentication ----
        ConfigEntry {
            name: "HELIOS_JWT_SECRET_PATH",
            category: "Authentication",
            value: secret_status("HELIOS_JWT_SECRET_PATH", "secret-jwt.json"),
            overridden: present("HELIOS_JWT_SECRET_PATH"),
            description: "Path to the JWT signing-secret file. Point at a persistent, identical path on every node for multi-node; auto-generated as ./secret-jwt.json otherwise. Never the data-dir (a per-node cache).",
        },
    ]
}

/// Live-tunable knobs (env > control setting > default) for `GET /api/admin/tunables`.
/// These mirror the read-only `HELIOS_*` knobs but are editable; the env override,
/// when present, wins and locks the field in the UI.
pub(super) async fn get_tunables_handler(State(s): State<AppState>) -> impl IntoResponse {
    (StatusCode::OK, Json(tunables_response(&s).await))
}

async fn tunables_response(s: &AppState) -> Value {
    let entries: Vec<Value> = crate::runtime_config::snapshot(&s.control)
        .await
        .iter()
        .map(|v| {
            let d = v.def;
            json!({
                "id": d.slug,
                "env": d.env,
                "category": d.category,
                "label": d.label,
                "unit": d.unit,
                "default": d.default,
                "effective": v.effective,
                "configured": v.configured,
                "env_override": v.env_override,
                "live": d.live,
                "description": d.description,
            })
        })
        .collect();
    json!({ "entries": entries })
}

#[derive(Deserialize)]
pub(super) struct TunablePatch {
    id: String,
    /// `null` (or a value below the knob's minimum) clears it — back to env/default.
    value: Option<i64>,
}

pub(super) async fn post_tunable_handler(
    State(s): State<AppState>,
    Json(p): Json<TunablePatch>,
) -> impl IntoResponse {
    let Some(def) = crate::runtime_config::knob_by_slug(&p.id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("unknown tunable: {}", p.id) })),
        );
    };
    let res = match p.value {
        Some(v) if v >= def.min_value as i64 => {
            s.control.set_setting(def.key, &v.to_string()).await
        }
        _ => s.control.unset_setting(def.key).await,
    };
    if let Err(e) = res {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("save {}: {e}", p.id) })),
        );
    }
    // Apply now so live knobs don't wait for the next refresher tick.
    crate::runtime_config::refresh(&s.control).await;
    (StatusCode::OK, Json(tunables_response(&s).await))
}

/// Trim, drop blanks, dedup-preserve-order, join with `,` for allowlist
/// serialization. Defensive — keeps the persisted row clean for inspection.
fn sanitize_csv(items: &[String]) -> String {
    let mut seen = std::collections::HashSet::new();
    items
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .collect::<Vec<_>>()
        .join(",")
}

// The block engine compacts automatically and commits on flush, so these former
// storage-maintenance endpoints are no-ops kept for API/UI compatibility.

pub(super) async fn merge_handler(State(_s): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "message": "compaction runs automatically in the block engine \
                        (size-based; a single compactor self-elects across nodes)",
        })),
    )
}

pub(super) async fn force_commit_handler(State(_s): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({ "message": "block engine commits on flush — nothing to do" })),
    )
}

/// Run a retention sweep now. Forced-leader: an explicit admin click should
/// always clean up (overlap with the background sweeper is CAS-safe).
pub(super) async fn gc_handler(State(s): State<AppState>) -> impl IntoResponse {
    let t0 = std::time::Instant::now();
    match crate::retention::sweep_once(&s.retention, true).await {
        Ok(r) => (
            StatusCode::OK,
            Json(json!({
                "partitions_dropped": r.partitions_dropped,
                "blocks_deleted": r.blocks_deleted,
                "deleted_files": r.blocks_deleted,
                "took_ms": t0.elapsed().as_millis() as u64,
                "message": if r.partitions_dropped == 0 {
                    "no partitions past retention (block files themselves are reclaimed by compaction)".to_string()
                } else {
                    format!("dropped {} expired partition(s)", r.partitions_dropped)
                },
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}
