// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! HTTP entry point. Owns [`AppState`] and the axum router; defers every
//! handler to its domain submodule (search / ingest / admin / saved).

use anyhow::{Context, Result};
use axum::extract::DefaultBodyLimit;
use axum::response::IntoResponse;
use axum::routing::{get, patch, post, put};
use axum::Router;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::catalog::Catalog;
use crate::control::saved::SavedSearch;
use crate::control::Control;
use crate::schema::{build_schema, Fields};

mod admin;
mod agent;
mod api_keys;
mod auth;
mod dashboards;
mod embedded;
mod envs;
mod ingest;
mod ingest_tokens;
mod mcp_http;
mod monitors;
mod otlp_proto;
mod saml;
mod saved;
mod search;
mod shims;
mod sources;
mod syslog_admin;
mod time;
mod users;

/// Single-writer gate backed by a named control-plane lease — the multi-node
/// path for the compactor and the retention sweeper. Each pass tries to
/// acquire/renew the lease; only the holder runs.
struct LeaseGate {
    control: Control,
    node_id: String,
    ttl: std::time::Duration,
    /// Lease name — "compactor" or "retention".
    lease: &'static str,
}

#[async_trait::async_trait]
impl crate::engine::block::CompactionGate for LeaseGate {
    async fn should_compact(&self) -> bool {
        match self
            .control
            .acquire_named_lease(self.lease, &self.node_id, self.ttl)
            .await
        {
            Ok(held) => held,
            // Can't reach the control store to confirm leadership — sit this
            // pass out rather than risk every node running.
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), lease = self.lease, "lease acquire failed");
                false
            }
        }
    }
}

/// Shared state available to every handler. Cheap to clone — the only
/// non-trivially-cloneable members are inside `Arc`s.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) catalog: Catalog,
    pub(crate) fields: Fields,
    /// Control plane (identity, RBAC, saved searches, monitors, settings, conversations).
    pub(crate) control: Control,
    /// JWT signing secret (HS256), shared across nodes and validated locally. See [`crate::auth`].
    pub(crate) jwt_secret: Arc<Vec<u8>>,
    /// The `--shared-store` target, if replication is enabled. Read-only, for the admin view.
    pub(crate) shared_store: Option<String>,
    /// Stores + control handle for the retention sweeper; `/api/admin/gc` runs it inline.
    pub(crate) retention: Arc<crate::retention::RetentionCtx>,
    /// `--syslog-port` CLI override (UDP + TCP), if set — shadows the control-plane
    /// port so multiple instances can share a control plane but bind distinct ports.
    pub(crate) syslog_port: Option<u16>,
    /// Read-only demo lockdown (`--demo`) + optional pre-fill creds for the login page.
    pub(crate) demo: DemoConfig,
}

/// Demo-mode config (`--demo` / `HELIOS_DEMO_*`). When `enabled`, mutating APIs and
/// agent/MCP write-tools are rejected for the single account named by `login` — every
/// other user is unaffected. The login/password are advertised on the public login page
/// so a visitor can click straight into the demo account.
#[derive(Clone, Default)]
pub struct DemoConfig {
    pub enabled: bool,
    pub login: Option<String>,
    pub password: Option<String>,
}

/// HTTPS listener config (`--ssl-port` / `--tls-cert` / `--tls-key`). When `ssl_port`
/// is set the cert + key are required, and HTTPS is served alongside the plaintext
/// `--port` (set `--port 0` for HTTPS-only).
#[derive(Clone, Default)]
pub struct TlsArgs {
    pub ssl_port: Option<u16>,
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
}

impl TlsArgs {
    pub fn new(ssl_port: Option<u16>, cert: Option<PathBuf>, key: Option<PathBuf>) -> Self {
        Self {
            ssl_port,
            cert,
            key,
        }
    }
}

impl DemoConfig {
    pub fn new(enabled: bool, login: Option<String>, password: Option<String>) -> Self {
        let clean = |s: Option<String>| s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Self {
            enabled,
            login: clean(login),
            password: clean(password),
        }
    }

    /// True when demo mode is on AND `userid`/`email` identify the restricted demo
    /// account (matched case-insensitively, since logins resolve either way).
    pub fn restricts(&self, userid: &str, email: &str) -> bool {
        self.enabled
            && self.login.as_deref().is_some_and(|l| {
                l.eq_ignore_ascii_case(userid.trim()) || l.eq_ignore_ascii_case(email.trim())
            })
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn serve(
    data_dir: PathBuf,
    host: String,
    port: u16,
    frontend_dir: Option<PathBuf>,
    shared_store: Option<String>,
    verbose: bool,
    syslog_port: Option<u16>,
    tls: TlsArgs,
    demo: DemoConfig,
) -> Result<()> {
    // Install the aws-lc-rs rustls provider before any HTTPS client (LLM, S3)
    // is built — our reqwest clients are compiled `-no-provider` and use it.
    // The HTTPS listener (below) shares this same process-wide provider.
    crate::crypto::tls::install_default_provider();

    // Resolve the optional HTTPS listener up-front so a bad cert/key fails fast,
    // before any heavy startup. `--ssl-port` requires both cert and key.
    let tls_server: Option<(u16, Arc<rustls::ServerConfig>)> = match tls.ssl_port {
        Some(ssl_port) => {
            let cert = tls
                .cert
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("--ssl-port requires --tls-cert"))?;
            let key = tls
                .key
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("--ssl-port requires --tls-key"))?;
            Some((ssl_port, crate::crypto::tls::load_server_config(cert, key)?))
        }
        None => None,
    };
    if port == 0 && tls_server.is_none() {
        anyhow::bail!("nothing to serve: --port is 0 and --ssl-port is unset");
    }

    // Control plane: encrypted JSON files on the shared store or local data dir. Encryption
    // is on unless HELIOS_CONTROL_ENCRYPTION is falsey; the key lives in its own file.
    let control_store =
        crate::control::store::build_control_store(shared_store.as_deref(), &data_dir).await?;
    let encryption_enabled = std::env::var("HELIOS_CONTROL_ENCRYPTION")
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "no"
            )
        })
        .unwrap_or(true);
    let crypto = Arc::new(crate::control::crypto::Crypto::new(encryption_enabled)?);
    let control = Control::new(control_store, crypto);

    // Seed live-tunable knobs (env > control setting > default) before sizing the
    // query pool/cache or spawning the background loops, then keep them fresh so
    // Admin → General edits apply without a restart.
    crate::runtime_config::init(&control).await;
    crate::engine::configure_query_pool();
    crate::engine::block::configure_query_cache(crate::engine::query_cache_mb());
    tokio::spawn(crate::runtime_config::run_refresher(control.clone()));

    // Per-process identity for best-effort compactor-lease election (only
    // meaningful with a shared control plane; harmless otherwise).
    let node_id = uuid::Uuid::new_v4().to_string();
    crate::control::ensure_admin(&control).await?;
    crate::control::ensure_reserved_envs(&control).await?;
    import_legacy_saved_searches(&data_dir, &control).await?;
    // Load (or first-run generate) the JWT signing secret. Lives in its own file
    // (HELIOS_JWT_SECRET_PATH or ./secret-jwt.json), not the data-dir cache.
    let jwt_secret = Arc::new(crate::auth::load_or_create_secret()?);
    let catalog = Catalog::open(data_dir)?;
    let fields = build_schema();

    // Surface the active storage engine; logged after the self-logs channel is wired
    // (so it lands in `_helioslogs`) and on the console banner.
    let engine_summary = crate::engine::engine_startup_summary();

    // Block engine setup before self-logs so the self-log writer shares the store. The
    // engine is local-first; with `--shared-store` it falls back to shared on read miss.
    let setup =
        crate::engine::block::build_block_setup(catalog.root(), shared_store.as_deref()).await?;
    let block_store_desc = Some(setup.desc.clone());
    crate::engine::block::install_store(setup.engine.clone());

    // Tracks blocks this node ingested but hasn't confirmed in shared — the ownership
    // signal the uploader/puller use. Only meaningful with a shared store.
    let pending = std::sync::Arc::new(crate::engine::block::PendingStore::new(catalog.root()));
    if setup.sync.is_some() {
        crate::engine::block::install_pending(pending.clone());
    }

    // Buffered writer — ingest lands on the local engine store. The channel is bounded
    // so a firehose becomes backpressure (429 / blocked tail), not unbounded memory.
    let (block_tx, block_rx) = tokio::sync::mpsc::channel(crate::engine::block::queue_capacity());
    crate::engine::block::install_sender(block_tx);
    tokio::spawn(crate::engine::block::run_writer(
        setup.engine.clone(),
        crate::engine::block_codec(),
        block_rx,
    ));

    // Plain local + optional shared stores for the retention sweeper (the
    // engine store is read-through with a shared store, so not used here).
    let (retention_local, retention_shared) = match &setup.sync {
        Some(pair) => (pair.local.clone(), Some(pair.shared.clone())),
        None => (setup.engine.clone(), None),
    };

    match setup.sync {
        Some(pair) => {
            // Replicate local ⇄ shared. Compaction runs shared-side on the
            // opt-in compactor node; the puller propagates its results.
            tokio::spawn(crate::engine::block::run_uploader(
                pair.local.clone(),
                pair.shared.clone(),
                pending.clone(),
            ));
            tokio::spawn(crate::engine::block::run_puller(
                pair.local.clone(),
                pair.shared.clone(),
                pending.clone(),
            ));
            // Bootstrap: push local partitions absent from shared (on startup and
            // periodically), so a fresh shared store populates from existing local data.
            tokio::spawn(crate::engine::block::run_seeder(
                pair.local.clone(),
                pair.shared.clone(),
            ));
            // Compaction runs shared-side, gated by a best-effort single-writer lease so
            // exactly one node compacts without per-node config (overlap is safe anyway).
            let gate = Arc::new(LeaseGate {
                control: control.clone(),
                node_id: node_id.clone(),
                ttl: crate::runtime_config::compactor_lease_ttl(),
                lease: "compactor",
            });
            tokio::spawn(crate::engine::block::run_compactor(
                pair.shared,
                crate::engine::block_codec(),
                gate,
            ));
        }
        None => {
            // Single node, no shared store: nothing to coordinate, always compact
            // the local engine store.
            tokio::spawn(crate::engine::block::run_compactor(
                setup.engine,
                crate::engine::block_codec(),
                Arc::new(crate::engine::block::AlwaysCompact),
            ));
        }
    }

    // Wire up the self-logs channel so tracing events during router
    // construction already land in `_helioslogs` (block-backed).
    let (self_log_tx, self_log_rx) = tokio::sync::mpsc::unbounded_channel();
    crate::self_logs::install_sender(self_log_tx);
    let node_info = crate::self_logs::NodeInfo::new(node_id.clone(), port);
    tokio::spawn(crate::self_logs::run_writer(
        catalog.clone(),
        node_info,
        self_log_rx,
    ));
    crate::memstats::spawn_purger();
    if verbose {
        crate::memstats::spawn_logger();
    }
    tracing::info!(engine = %engine_summary, "storage: read engine selected");
    if let Some(desc) = &block_store_desc {
        tracing::info!(store = %desc, "storage: block store backend");
        if shared_store.is_some() {
            tracing::info!(
                node_id = %node_id,
                "storage: block compactor elected per-node via shared control-plane lease"
            );
        } else {
            tracing::info!("storage: block compactor running (single node)");
        }
    }

    // Retention sweeper: hourly, leader-elected with a shared store (same
    // lease pattern as the compactor); a lone node always leads.
    let retention_ctx = Arc::new(crate::retention::RetentionCtx {
        catalog: catalog.clone(),
        control: control.clone(),
        local: retention_local,
        shared: retention_shared,
        pending: pending.clone(),
    });
    let retention_gate: Arc<dyn crate::engine::block::CompactionGate> = if shared_store.is_some() {
        Arc::new(LeaseGate {
            control: control.clone(),
            node_id: node_id.clone(),
            ttl: crate::runtime_config::retention_sweeper_lease_ttl(),
            lease: "retention",
        })
    } else {
        Arc::new(crate::engine::block::AlwaysCompact)
    };
    tokio::spawn(crate::retention::run_sweeper(
        retention_ctx.clone(),
        retention_gate,
    ));

    if demo.enabled {
        match &demo.login {
            Some(login) => tracing::warn!(
                login = %login,
                "demo mode ON — mutating APIs + agent write-tools are read-only for this account; other users unaffected"
            ),
            None => tracing::warn!(
                "demo mode ON but HELIOS_DEMO_LOGIN is unset — no account is restricted; set --demo-login"
            ),
        }
    }
    let state = AppState {
        catalog,
        fields,
        control,
        jwt_secret,
        shared_store: shared_store.clone(),
        retention: retention_ctx,
        syslog_port,
        demo,
    };

    // Monitor scheduler — every 10s, runs any monitor whose `interval_seconds` elapsed;
    // alerts land via the `raise_alert` tool.
    tokio::spawn(crate::monitor::run_scheduler(
        state.catalog.clone(),
        state.fields,
        state.control.clone(),
    ));

    // Source supervisor — polls enabled ingestion sources (currently local-fs
    // `pull`) on their interval and ingests new bytes via the block writer.
    tokio::spawn(crate::source::run_supervisor(state.control.clone()));

    // Syslog supervisor — binds UDP/TCP listeners per the control-plane config and
    // ingests parsed RFC 5424/3164 messages via the block writer. `--syslog-port`
    // overrides the control-plane port (both UDP + TCP) for this node.
    tokio::spawn(crate::syslog::run_supervisor(
        state.control.clone(),
        state.syslog_port,
    ));

    let app = build_router(state, frontend_dir);
    bind_and_serve(
        app,
        &host,
        port,
        tls_server,
        &engine_summary,
        block_store_desc.as_deref(),
    )
    .await
}

/// Build the full axum router (routes, auth middleware, SPA fallback, CORS, logging).
/// Split from [`serve`] so integration tests can `oneshot` it without a socket.
pub(crate) fn build_router(state: AppState, frontend_dir: Option<PathBuf>) -> Router {
    // `expose_headers` lets the cross-origin dev frontend read the sliding-renewal token
    // from `X-Helios-Token-Refresh`; `allow_headers` already permits `Authorization`.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers(Any);

    let app = Router::new()
        // ---- health (unauthenticated; restart.sh / load balancers) ----
        .route(
            "/api/health",
            get(|| async { axum::Json(serde_json::json!({ "ok": true })) }),
        )
        // ---- MCP over HTTP ----
        // Top-level (NOT `/api/*`) so the JWT layer skips it; MCP has its own
        // bearer-token auth in the handler. See src/http/mcp_http.rs.
        .route("/mcp", post(mcp_http::handler))
        // ---- search surface ----
        .route("/api/stats", get(search::stats_handler))
        .route("/api/search", get(search::search_handler))
        .route(
            "/api/search_partitions",
            get(search::search_partitions_handler),
        )
        .route("/api/histogram", get(search::histogram_handler))
        .route(
            "/api/search_histogram",
            get(search::search_histogram_handler),
        )
        .route("/api/aggregate", get(search::aggregate_handler))
        .route("/api/discover_fields", get(search::discover_fields_handler))
        // Index name list — general read (search-bar dropdown etc.), not an
        // admin-only concern, so it lives outside the /api/admin namespace.
        .route("/api/indexes", get(admin::indexes_handler))
        // ---- admin (read-only — both modes get these) ----
        .route("/api/admin/partitions", get(admin::partitions_handler))
        .route("/api/admin/index-info", get(admin::index_info_handler))
        .route(
            "/api/admin/index-catalog",
            get(admin::index_catalog_handler),
        )
        .route(
            "/api/admin/settings",
            get(admin::get_settings_handler).post(admin::post_settings_handler),
        )
        .route(
            "/api/admin/runtime-config",
            get(admin::runtime_config_handler),
        )
        .route(
            "/api/admin/tunables",
            get(admin::get_tunables_handler).post(admin::post_tunable_handler),
        )
        .route(
            "/api/admin/alerts/test-webhook",
            post(admin::test_webhook_handler),
        );

    // ---- ingest + admin maintenance ----
    // Override axum's 2 MB body cap — a single NDJSON batch can run tens of MB; 256 MB headroom.
    let app = app
        .route(
            "/api/ingest",
            post(ingest::ingest_handler).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route(
            "/api/ingest/raw",
            post(ingest::ingest_raw_handler).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        // ---- push-protocol shims
        .route("/services/collector", post(shims::hec_handler))
        .route("/services/collector/event", post(shims::hec_handler))
        .route("/services/collector/health", get(shims::hec_health_handler))
        .route(
            "/_bulk",
            post(shims::es_bulk_handler).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route(
            "/api/es/_bulk",
            post(shims::es_bulk_handler).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route(
            "/v1/logs",
            post(shims::otlp_logs_handler).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route(
            "/api/otlp/v1/logs",
            post(shims::otlp_logs_handler).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route(
            "/loki/api/v1/push",
            post(shims::loki_push_handler).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        // ---- scoped push tokens (admin) ----
        .route(
            "/api/admin/ingest-tokens",
            get(ingest_tokens::list_handler).post(ingest_tokens::create_handler),
        )
        .route(
            "/api/admin/ingest-tokens/:id",
            patch(ingest_tokens::patch_handler).delete(ingest_tokens::delete_handler),
        )
        .route(
            "/api/admin/ingest-auth",
            put(ingest_tokens::set_require_handler),
        )
        // ---- REST API keys (admin) ----
        .route(
            "/api/admin/api-keys",
            get(api_keys::list_handler).post(api_keys::create_handler),
        )
        .route(
            "/api/admin/api-keys/:id",
            patch(api_keys::patch_handler).delete(api_keys::delete_handler),
        )
        .route("/api/admin/merge", post(admin::merge_handler))
        .route("/api/admin/commit", post(admin::force_commit_handler))
        .route("/api/admin/gc", post(admin::gc_handler));

    let app = app
        // ---- saved searches ----
        .route(
            "/api/searches",
            get(saved::list_searches_handler).post(saved::create_search_handler),
        )
        .route(
            "/api/searches/:id",
            patch(saved::update_search_handler).delete(saved::delete_search_handler),
        )
        // ---- dashboards ----
        .route(
            "/api/dashboards",
            get(dashboards::list_dashboards_handler).post(dashboards::create_dashboard_handler),
        )
        .route(
            "/api/dashboards/:id",
            get(dashboards::get_dashboard_handler)
                .patch(dashboards::update_dashboard_handler)
                .delete(dashboards::delete_dashboard_handler),
        )
        // ---- ingestion sources ----
        .route(
            "/api/sources",
            get(sources::list_sources_handler).post(sources::create_source_handler),
        )
        .route("/api/sources/browse", get(sources::browse_handler))
        .route(
            "/api/sources/:id",
            get(sources::get_source_handler)
                .patch(sources::update_source_handler)
                .delete(sources::delete_source_handler),
        )
        .route("/api/sources/:id/run", post(sources::run_source_handler))
        .route(
            "/api/sources/:id/reset",
            post(sources::reset_source_handler),
        )
        // ---- monitors + alerts ----
        .route(
            "/api/monitors",
            get(monitors::list_monitors_handler).post(monitors::create_monitor_handler),
        )
        .route(
            "/api/monitors/:id",
            get(monitors::get_monitor_handler)
                .patch(monitors::update_monitor_handler)
                .delete(monitors::delete_monitor_handler),
        )
        .route("/api/monitors/:id/run", post(monitors::run_monitor_handler))
        .route(
            "/api/monitors/:id/run_live",
            post(monitors::run_monitor_live_handler),
        )
        .route(
            "/api/monitors/:id/alerts",
            get(monitors::list_monitor_alerts_handler),
        )
        .route("/api/alerts", get(monitors::list_alerts_handler))
        .route(
            "/api/alerts/unacked-count",
            get(monitors::alerts_unacked_count_handler),
        )
        .route(
            "/api/alerts/dismiss-all",
            post(monitors::dismiss_all_alerts_handler),
        )
        .route("/api/alerts/:id", patch(monitors::patch_alert_handler))
        // ---- auth (login + first-run setup are the public endpoints) ----
        .route("/api/auth/login", post(auth::login_handler))
        .route("/api/auth/setup_status", get(auth::setup_status_handler))
        .route("/api/auth/setup", post(auth::setup_handler))
        .route("/api/auth/logout", post(auth::logout_handler))
        .route("/api/auth/me", get(auth::me_handler))
        .route("/api/auth/password", post(auth::change_password_handler))
        .route(
            "/api/account/preferences",
            post(auth::update_preferences_handler),
        )
        // ---- SAML SP (public: IdP + browser reach these without a token) ----
        .route("/api/auth/saml/status", get(saml::status_handler))
        .route("/api/auth/saml/metadata", get(saml::metadata_handler))
        .route("/api/auth/saml/login", get(saml::login_handler))
        .route("/api/auth/saml/acs", post(saml::acs_handler))
        .route(
            "/api/admin/saml",
            get(saml::get_config_handler).post(saml::post_config_handler),
        )
        // ---- syslog listener config (admin) ----
        .route(
            "/api/admin/syslog",
            get(syslog_admin::get_config_handler).post(syslog_admin::post_config_handler),
        )
        // ---- envs (read = any authed user, write = admin) ----
        .route("/api/envs", get(envs::list_envs_handler))
        .route("/api/admin/envs", post(envs::create_env_handler))
        // Picker order + login default. Kept off the `/envs/:name` subtree so a
        // user env named "reorder"/"default" can't shadow these static paths.
        .route("/api/admin/env-order", post(envs::reorder_envs_handler))
        .route(
            "/api/admin/env-default",
            put(envs::set_default_env_handler).delete(envs::clear_default_env_handler),
        )
        .route(
            "/api/admin/envs/:name",
            patch(envs::patch_env_handler).delete(envs::delete_env_handler),
        )
        .route(
            "/api/admin/users/:id/allowed",
            get(envs::list_user_allowed_handler).put(envs::set_user_allowed_handler),
        )
        // ---- admin: onboarding sample data ----
        .route("/api/admin/load_sample", post(admin::load_sample_handler))
        // ---- admin: user management ----
        .route(
            "/api/admin/users",
            get(users::list_handler).post(users::create_handler),
        )
        .route(
            "/api/admin/users/:id",
            patch(users::update_handler).delete(users::delete_handler),
        )
        .route(
            "/api/admin/users/:id/password",
            post(users::regenerate_password_handler),
        )
        // ---- agent: conversations + streaming chat ----
        .route("/api/agent/status", get(agent::agent_status_handler))
        .route(
            "/api/agent/conversations",
            get(agent::list_conversations_handler).post(agent::create_conversation_handler),
        )
        .route(
            "/api/agent/conversations/:id",
            get(agent::get_conversation_handler)
                .patch(agent::rename_conversation_handler)
                .delete(agent::delete_conversation_handler),
        )
        .route(
            "/api/agent/conversations/:id/messages",
            post(agent::send_message_handler),
        )
        // ---- admin: agent configuration (LLM provider + future agent knobs) ----
        .route(
            "/api/admin/agent",
            get(agent::get_llm_settings_handler).put(agent::put_llm_settings_handler),
        )
        .route("/api/admin/agent/test", post(agent::test_llm_handler))
        // Auth middleware is inside CORS so OPTIONS preflights bypass it.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::auth_layer,
        ));

    // SPA static fallback (after the auth layer so the bundle loads pre-session). Uses
    // `.fallback(handler)` not `.fallback_service(ServeDir)`, which would force the index to 404.
    let app = if let Some(dir) = frontend_dir {
        let index_path = Arc::new(dir.join("index.html"));
        let spa_index = move || {
            let index_path = index_path.clone();
            async move {
                match tokio::fs::read(&*index_path).await {
                    Ok(bytes) => (
                        axum::http::StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        bytes,
                    )
                        .into_response(),
                    Err(e) => {
                        tracing::error!(
                            path = %index_path.display(),
                            "spa fallback: could not read index.html: {e}"
                        );
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }
            }
        };
        app.nest_service("/assets", ServeDir::new(dir.join("assets")))
            .route_service("/favicon.svg", ServeFile::new(dir.join("favicon.svg")))
            .fallback(spa_index)
    } else {
        // No on-disk SPA dir: serve the frontend/dist bundle embedded at compile time.
        app.fallback(|uri: axum::http::Uri| async move { embedded::serve(uri.path()) })
    };

    app.layer(cors)
        // Outermost layer — runs after all other middleware so it measures
        // total request time. Skips /api/health (load-balancer noise).
        .layer(axum::middleware::from_fn(
            crate::self_logs::http_log_middleware,
        ))
        .with_state(state)
}

/// Bind the listener and serve `app`, printing the banner. Socket-bound tail of [`serve`],
/// kept out of [`build_router`] so the router builds without a port (tests).
async fn bind_and_serve(
    app: Router,
    host: &str,
    port: u16,
    tls: Option<(u16, Arc<rustls::ServerConfig>)>,
    engine_summary: &str,
    block_store_desc: Option<&str>,
) -> Result<()> {
    let ip: std::net::IpAddr = host
        .parse()
        .with_context(|| format!("invalid --host bind address: {host}"))?;
    if port != 0 {
        println!("helioslogs serving on http://{}", SocketAddr::new(ip, port));
    }
    if let Some((ssl_port, _)) = &tls {
        println!(
            "helioslogs serving on https://{}",
            SocketAddr::new(ip, *ssl_port)
        );
    }
    println!("  engine: {engine_summary}");
    if let Some(desc) = block_store_desc {
        println!("  block store: {desc}");
    }
    println!("  GET  /api/stats");
    println!("  GET  /api/search?q=...&index=...&start=...&end=...&limit=50");
    println!("  GET  /api/histogram?q=...&index=...&start=...&end=...&interval=1m");
    println!("  GET  /api/aggregate?q=...&index=...&start=...&end=...&fields=...&size=10");
    println!("  GET  /api/discover_fields?q=...&index=...&start=...&end=...&top=30");
    println!("  POST /api/ingest?index=...    (body: NDJSON, one event per line)");
    println!("  GET  /api/indexes             /api/admin/partitions");
    println!("  GET  /api/admin/index-info?index=...&day=YYYY-MM-DD   (filter optional)");
    println!("  GET  /api/admin/index-catalog (distinct env/index names — no stats)");
    println!("  GET  /api/admin/settings      POST same path to update");
    println!("  POST /api/admin/merge?index=...&day=YYYY-MM-DD   (filter optional)");
    println!("  POST /api/admin/commit        /api/admin/gc");
    println!("  GET  /api/searches            POST same path to create");
    println!("  PATCH /api/searches/:id       DELETE same path");
    println!("  GET  /api/dashboards          POST same path to create");
    println!("  GET/PATCH/DELETE /api/dashboards/:id");
    println!("  POST /api/auth/login          /api/auth/logout");
    println!("  GET  /api/auth/me             POST /api/auth/password");
    println!("  GET  /api/admin/users         POST same path to create");
    println!("  DELETE /api/admin/users/:id   POST /api/admin/users/:id/password");
    println!("  POST /mcp                     (Model Context Protocol — JSON-RPC over HTTP)");

    // Serve HTTP (plaintext) and/or HTTPS (TLS) concurrently. A disabled listener
    // parks on `pending` so the other drives the process; `serve` guarantees at
    // least one is active. The first to exit/error ends the process.
    let http_app = app.clone();
    let http = async move {
        if port == 0 {
            return std::future::pending::<std::io::Result<()>>().await;
        }
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(ip, port)).await?;
        axum::serve(listener, http_app).await
    };
    let https = async move {
        match tls {
            Some((ssl_port, config)) => {
                let cfg = axum_server::tls_rustls::RustlsConfig::from_config(config);
                axum_server::bind_rustls(SocketAddr::new(ip, ssl_port), cfg)
                    .serve(app.into_make_service())
                    .await
            }
            None => std::future::pending::<std::io::Result<()>>().await,
        }
    };
    tokio::select! {
        r = http => r?,
        r = https => r?,
    }
    Ok(())
}

/// One-shot import of the pre-v5 `saved_searches.json`: attribute to the bootstrap admin
/// and rename aside so it doesn't repeat. Parse failures are logged and left in place.
async fn import_legacy_saved_searches(data_dir: &std::path::Path, control: &Control) -> Result<()> {
    let path = data_dir.join("saved_searches.json");
    if !path.exists() {
        return Ok(());
    }
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("could not read legacy {}: {e}", path.display());
            return Ok(());
        }
    };
    let items: Vec<SavedSearch> = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "legacy saved_searches.json present but unparseable ({e}); leaving in place"
            );
            return Ok(());
        }
    };
    // Attribute to the earliest admin (`list_users` is oldest-first = bootstrap account);
    // skip rather than guess if no admin exists.
    let admin_id: Option<String> = control
        .list_users()
        .await?
        .into_iter()
        .find(|u| u.is_admin)
        .map(|u| u.id);
    let Some(admin_id) = admin_id else {
        tracing::warn!("legacy saved_searches.json present but no admin user to attribute to");
        return Ok(());
    };

    let n = control.saved_import_bulk(&admin_id, &items).await?;
    let moved = path.with_extension("json.migrated");
    if let Err(e) = std::fs::rename(&path, &moved) {
        tracing::warn!(
            "imported {n} saved searches but could not rename {}: {e}",
            path.display()
        );
    } else {
        tracing::info!(
            "imported {n} saved searches from legacy file → control DB (renamed to {})",
            moved.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod integration_tests {
    //! End-to-end HTTP tests that drive the real router (`build_router`) via
    //! `tower::ServiceExt::oneshot` — no socket, no background schedulers.
    //! These cover the seams unit tests can't reach: the JWT auth middleware,
    //! RBAC enforcement applied through a live request, validation/error-code
    //! mapping, and an ingest→search round-trip over real blocks.
    //!
    //! Data isolation: the block engine's `CONFIGURED_STORE` global is never
    //! installed here, so every read falls back to a filesystem store rooted at
    //! the test's own tempdir. Tests seed data by writing blocks directly to
    //! that root (bypassing the process-global async ingest writer), keeping
    //! every test independent even under parallel execution.

    use super::*;
    use crate::catalog::PartitionKey;
    use crate::control::crypto::Crypto;
    use crate::control::settings::EnvIndexAllow;
    use crate::control::store::FsControlStore;
    use crate::control::users::User;
    use crate::control::Control;
    use crate::engine::block::BlockEngine;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use chrono::NaiveDate;
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tower::ServiceExt;

    struct Ctx {
        _dir: TempDir,
        control: Control,
        catalog: Catalog,
        secret: Arc<Vec<u8>>,
        app: Router,
    }

    async fn ctx() -> Ctx {
        ctx_with_demo(DemoConfig::default()).await
    }

    async fn ctx_with_demo(demo: DemoConfig) -> Ctx {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(FsControlStore::new(dir.path().join("control")));
        let control = Control::new(store, Arc::new(Crypto::Disabled));
        // Register the reserved envs (`default`, `_system`) so ingest's
        // env-existence check passes, mirroring `serve`'s startup.
        crate::control::ensure_reserved_envs(&control)
            .await
            .unwrap();
        let catalog = Catalog::open(dir.path().join("data")).unwrap();
        let secret = Arc::new(b"integration-test-secret".to_vec());
        let local = crate::engine::block::BlockStore::new(dir.path().join("data"));
        let state = AppState {
            catalog: catalog.clone(),
            fields: build_schema(),
            control: control.clone(),
            jwt_secret: secret.clone(),
            shared_store: None,
            retention: Arc::new(crate::retention::RetentionCtx {
                catalog: catalog.clone(),
                control: control.clone(),
                local,
                shared: None,
                pending: Arc::new(crate::engine::block::PendingStore::new(
                    dir.path().join("data"),
                )),
            }),
            syslog_port: None,
            demo,
        };
        let app = build_router(state, None);
        Ctx {
            _dir: dir,
            control,
            catalog,
            secret,
            app,
        }
    }

    async fn user(c: &Control, userid: &str, admin: bool) -> User {
        c.create_user(
            userid,
            &format!("{userid}@x.com"),
            userid,
            "pw12345678",
            admin,
        )
        .await
        .unwrap()
    }

    fn token(u: &User, secret: &[u8]) -> String {
        crate::auth::jwt::mint(&u.id, u.credentials_version, secret).unwrap()
    }

    fn get(path: &str, tok: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().method("GET").uri(path);
        if let Some(t) = tok {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        b.body(Body::empty()).unwrap()
    }

    fn post_json(path: &str, tok: Option<&str>, body: Value) -> Request<Body> {
        let mut b = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(t) = tok {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        b.body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, body)
    }

    /// Seed a partition with real blocks at the catalog root, readable by the
    /// HTTP search path via the fallback store.
    fn seed(c: &Catalog, env: &str, index: &str, events: &[&str]) {
        let day = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let evs: Vec<Value> = events
            .iter()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        let engine = BlockEngine::new(c.root(), crate::engine::block_codec());
        engine
            .ingest(&PartitionKey::new(env, index, day), &evs, None)
            .unwrap();
    }

    // ---- auth middleware ----------------------------------------------------

    #[tokio::test]
    async fn health_is_public() {
        let c = ctx().await;
        let (status, body) = send(&c.app, get("/api/health", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn protected_route_requires_token() {
        let c = ctx().await;
        let (status, _) = send(&c.app, get("/api/search?q=*", None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn malformed_token_rejected() {
        let c = ctx().await;
        let (status, _) = send(&c.app, get("/api/search?q=*", Some("not-a-jwt"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn valid_token_allows_search() {
        let c = ctx().await;
        let admin = user(&c.control, "admin", true).await;
        let tok = token(&admin, &c.secret);
        let (status, body) = send(&c.app, get("/api/search?q=*", Some(&tok))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("total").is_some(), "search response shape: {body}");
    }

    #[tokio::test]
    async fn admin_route_gated_by_role() {
        let c = ctx().await;
        let admin = user(&c.control, "admin", true).await;
        let plain = user(&c.control, "bob", false).await;
        let (s_admin, _) = send(
            &c.app,
            get("/api/admin/users", Some(&token(&admin, &c.secret))),
        )
        .await;
        assert_eq!(s_admin, StatusCode::OK);
        let (s_plain, _) = send(
            &c.app,
            get("/api/admin/users", Some(&token(&plain, &c.secret))),
        )
        .await;
        assert_eq!(s_plain, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn token_revoked_after_credentials_bump() {
        let c = ctx().await;
        let u = user(&c.control, "carol", false).await;
        let tok = token(&u, &c.secret);
        // Works before revocation.
        let (before, _) = send(&c.app, get("/api/auth/me", Some(&tok))).await;
        assert_eq!(before, StatusCode::OK);
        // Logout/password-change bumps the credentials_version → old token dies.
        c.control.bump_credentials_version(&u.id).await.unwrap();
        let (after, _) = send(&c.app, get("/api/auth/me", Some(&tok))).await;
        assert_eq!(after, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_flow_issues_usable_token() {
        let c = ctx().await;
        user(&c.control, "dave", false).await;
        // Wrong password → 401.
        let (bad, _) = send(
            &c.app,
            post_json(
                "/api/auth/login",
                None,
                json!({"login": "dave", "password": "nope"}),
            ),
        )
        .await;
        assert_eq!(bad, StatusCode::UNAUTHORIZED);
        // Correct password → token that authenticates /me.
        let (ok, body) = send(
            &c.app,
            post_json(
                "/api/auth/login",
                None,
                json!({"login": "dave", "password": "pw12345678"}),
            ),
        )
        .await;
        assert_eq!(ok, StatusCode::OK);
        let tok = body["token"].as_str().unwrap();
        let (me, who) = send(&c.app, get("/api/auth/me", Some(tok))).await;
        assert_eq!(me, StatusCode::OK);
        assert_eq!(who["user"]["userid"], "dave");
    }

    // ---- RBAC through a live request ----------------------------------------

    #[tokio::test]
    async fn rbac_env_gate() {
        let c = ctx().await;
        let admin = user(&c.control, "admin", true).await;
        let plain = user(&c.control, "erin", false).await;
        c.control
            .set_user_allowed(
                &plain.id,
                &[EnvIndexAllow {
                    env: "prod".into(),
                    indexes: vec!["*".into()],
                }],
            )
            .await
            .unwrap();
        let ptok = token(&plain, &c.secret);

        // Allowed env → 200; un-granted env → 403; system env → 403.
        let (allowed, _) = send(&c.app, get("/api/search?q=*&env=prod", Some(&ptok))).await;
        assert_eq!(allowed, StatusCode::OK);
        let (denied, _) = send(&c.app, get("/api/search?q=*&env=dev", Some(&ptok))).await;
        assert_eq!(denied, StatusCode::FORBIDDEN);
        let (system, _) = send(&c.app, get("/api/search?q=*&env=_system", Some(&ptok))).await;
        assert_eq!(system, StatusCode::FORBIDDEN);

        // Admin bypasses the system-env gate.
        let (admin_sys, _) = send(
            &c.app,
            get(
                "/api/search?q=*&env=_system",
                Some(&token(&admin, &c.secret)),
            ),
        )
        .await;
        assert_eq!(admin_sys, StatusCode::OK);
    }

    #[tokio::test]
    async fn rbac_filters_partitions_by_index_allowlist() {
        let c = ctx().await;
        seed(
            &c.catalog,
            "default",
            "web",
            &[
                r#"{"timestamp":"2026-01-01T00:00:00Z","message":"web a","status":200}"#,
                r#"{"timestamp":"2026-01-01T00:01:00Z","message":"web b","status":500}"#,
            ],
        );
        seed(
            &c.catalog,
            "default",
            "secret",
            &[r#"{"timestamp":"2026-01-01T00:00:00Z","message":"secret a"}"#],
        );

        // Admin sees both indexes (3 events).
        let admin = user(&c.control, "admin", true).await;
        let (s, body) = send(
            &c.app,
            get(
                "/api/search?q=*&env=default",
                Some(&token(&admin, &c.secret)),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["total"], 3);

        // Non-admin allowed only `web` → 2 events, none from `secret`.
        let plain = user(&c.control, "frank", false).await;
        c.control
            .set_user_allowed(
                &plain.id,
                &[EnvIndexAllow {
                    env: "default".into(),
                    indexes: vec!["web".into()],
                }],
            )
            .await
            .unwrap();
        let (s2, body2) = send(
            &c.app,
            get(
                "/api/search?q=*&env=default",
                Some(&token(&plain, &c.secret)),
            ),
        )
        .await;
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(body2["total"], 2);
        let hits = body2["hits"].as_array().unwrap();
        assert!(
            hits.iter()
                .all(|h| h["partition"].as_str().unwrap().starts_with("web/")),
            "restricted user must only see `web` partitions: {body2}"
        );
    }

    // ---- ingest → search round-trip (real blocks) ---------------------------

    #[tokio::test]
    async fn search_reads_seeded_events() {
        let c = ctx().await;
        seed(
            &c.catalog,
            "default",
            "web",
            &[
                r#"{"timestamp":"2026-01-01T00:00:00Z","message":"order placed","status":200}"#,
                r#"{"timestamp":"2026-01-01T00:05:00Z","message":"payment failed","status":500}"#,
            ],
        );
        let admin = user(&c.control, "admin", true).await;
        let tok = token(&admin, &c.secret);
        // Field query routes through parse → engine and matches one event.
        let (s, body) = send(
            &c.app,
            get("/api/search?q=status:500&env=default&index=web", Some(&tok)),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["total"], 1);
        assert_eq!(body["hits"][0]["message"], "payment failed");
    }

    // ---- validation / error mapping -----------------------------------------

    #[tokio::test]
    async fn bad_env_is_400() {
        let c = ctx().await;
        let tok = token(&user(&c.control, "admin", true).await, &c.secret);
        // `bad/name` fails env-name validation → 400, not 500.
        let (status, _) = send(&c.app, get("/api/search?q=*&env=bad%2Fname", Some(&tok))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn malformed_query_is_400() {
        let c = ctx().await;
        let tok = token(&user(&c.control, "admin", true).await, &c.secret);
        // Unbalanced paren → parser error → 400 (not a 500).
        let (status, _) = send(&c.app, get("/api/search?q=%28a&env=default", Some(&tok))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // ---- MCP over HTTP ------------------------------------------------------

    #[tokio::test]
    async fn mcp_disabled_lists_no_tools() {
        let c = ctx().await;
        // `/mcp` is mounted outside `/api/*`, so the JWT layer skips it; MCP is
        // disabled by default, so the catalog is empty.
        let (status, body) = send(
            &c.app,
            post_json(
                "/mcp",
                None,
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["tools"], json!([]));
    }

    // Extra request helpers for the full endpoint sweep below.

    fn req(method: &str, path: &str, tok: Option<&str>, body: Option<Value>) -> Request<Body> {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(t) = tok {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let body = match body {
            Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
            None => Body::empty(),
        };
        b.body(body).unwrap()
    }

    /// POST a raw (non-JSON) body — for NDJSON / shim payloads.
    fn raw_post(path: &str, content_type: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn admin(c: &Ctx) -> String {
        token(&user(&c.control, "admin", true).await, &c.secret)
    }

    // Search surface (read engine; isolated empty tempdir → empty results)

    #[tokio::test]
    async fn search_surface_endpoints_ok() {
        let c = ctx().await;
        let t = admin(&c).await;
        let cases: &[(&str, &str)] = &[
            ("/api/stats", "num_docs"),
            ("/api/search?q=*", "total"),
            ("/api/search_partitions?q=*", "partitions"),
            ("/api/histogram?q=*", "buckets"),
            ("/api/search_histogram?q=*", "buckets"),
            ("/api/aggregate?q=*&fields=status", "aggs"),
            ("/api/discover_fields?q=*", "fields"),
            ("/api/indexes", "indexes"),
        ];
        for (path, key) in cases {
            let (status, body) = send(&c.app, get(path, Some(&t))).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.get(*key).is_some(), "{path} missing `{key}`: {body}");
        }
    }

    #[tokio::test]
    async fn pipe_query_returns_table() {
        let c = ctx().await;
        seed(
            &c.catalog,
            "default",
            "web",
            &[
                r#"{"timestamp":"2026-01-01T00:00:00Z","message":"a","service":"api"}"#,
                r#"{"timestamp":"2026-01-01T00:01:00Z","message":"b","service":"api"}"#,
                r#"{"timestamp":"2026-01-01T00:02:00Z","message":"c","service":"web"}"#,
            ],
        );
        let t = admin(&c).await;
        let (status, body) = send(
            &c.app,
            get(
                "/api/search?q=*%20%7C%20stats%20count%20by%20service&env=default",
                Some(&t),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Pipe queries return a `table` instead of hits.
        assert!(body.get("table").is_some(), "expected table: {body}");
        let cols = body["table"]["columns"].as_array().unwrap();
        assert!(cols.iter().any(|c| c == "service"));
    }

    #[tokio::test]
    async fn timechart_query_returns_time_table() {
        let c = ctx().await;
        seed(
            &c.catalog,
            "default",
            "web",
            &[
                r#"{"timestamp":"2026-01-01T00:00:10Z","message":"a","service":"api"}"#,
                r#"{"timestamp":"2026-01-01T00:00:20Z","message":"b","service":"api"}"#,
                r#"{"timestamp":"2026-01-01T00:02:30Z","message":"c","service":"web"}"#,
            ],
        );
        let t = admin(&c).await;
        // `* | timechart span=1m count by service`
        let q = "%2A%20%7C%20timechart%20span%3D1m%20count%20by%20service";
        let (status, body) = send(
            &c.app,
            get(&format!("/api/search?q={q}&env=default"), Some(&t)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let table = body.get("table").expect("expected table");
        assert_eq!(table["columns"][0], "_time", "{table}");
        assert_eq!(table["columns"][1], "service");
        let rows = table["rows"].as_array().unwrap();
        // Two api events in minute 0, one web event in minute 2.
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0][0], "2026-01-01T00:00:00Z");
        assert_eq!(rows[0][2], 2);
        assert_eq!(rows[1][0], "2026-01-01T00:02:00Z");
        // Rows arrive time-ascending.
        assert!(rows[0][0].as_str().unwrap() < rows[1][0].as_str().unwrap());
    }

    #[tokio::test]
    async fn pipe_where_rename_fields_compose() {
        let c = ctx().await;
        seed(
            &c.catalog,
            "default",
            "web",
            &[
                r#"{"timestamp":"2026-01-01T00:00:00Z","message":"a","service":"api"}"#,
                r#"{"timestamp":"2026-01-01T00:01:00Z","message":"b","service":"api"}"#,
                r#"{"timestamp":"2026-01-01T00:02:00Z","message":"c","service":"web"}"#,
            ],
        );
        let t = admin(&c).await;
        // `* | stats count by service | where count > 1 | rename count as n | fields service, n`
        let q = "%2A%20%7C%20stats%20count%20by%20service%20%7C%20where%20count%20%3E%201%20%7C%20rename%20count%20as%20n%20%7C%20fields%20service%2C%20n";
        let (status, body) = send(
            &c.app,
            get(&format!("/api/search?q={q}&env=default"), Some(&t)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let table = body.get("table").expect("expected table");
        assert_eq!(
            table["columns"],
            serde_json::json!(["service", "n"]),
            "{table}"
        );
        let rows = table["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "only api has count > 1: {rows:?}");
        assert_eq!(rows[0][0], "api");
        assert_eq!(rows[0][1], 2);
    }

    #[tokio::test]
    async fn pipe_earliest_latest_rollup() {
        let c = ctx().await;
        seed(
            &c.catalog,
            "default",
            "traces",
            &[
                r#"{"timestamp":"2026-01-01T00:00:00Z","message":"start","trace_id":"t1","usage.total_tokens":100}"#,
                r#"{"timestamp":"2026-01-01T00:00:05Z","message":"end","trace_id":"t1","usage.total_tokens":250}"#,
                r#"{"timestamp":"2026-01-01T00:01:00Z","message":"only","trace_id":"t2"}"#,
            ],
        );
        let t = admin(&c).await;
        // `* | stats count, earliest(timestamp), latest(timestamp) by trace_id`
        let q = "%2A%20%7C%20stats%20count%2C%20earliest%28timestamp%29%2C%20latest%28timestamp%29%20by%20trace_id";
        let (status, body) = send(
            &c.app,
            get(&format!("/api/search?q={q}&env=default"), Some(&t)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let table = body.get("table").expect("expected table");
        let rows = table["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "t1");
        assert_eq!(rows[0][1], 2);
        assert_eq!(rows[0][2], "2026-01-01T00:00:00.000Z");
        assert_eq!(rows[0][3], "2026-01-01T00:00:05.000Z");

        // Trace-style rollup: group on the trace field, drop the "—"
        // missing-value group via `where` (dynamic-field wildcards like
        // `trace_id:*` don't exist), sum of a token field (Null where absent),
        // newest-trace-first ordering.
        let q2 = urlencoding_for_test(
            "* | stats count, earliest(timestamp), latest(timestamp), \
             sum(usage.total_tokens) by trace_id | where trace_id != \"—\" \
             | sort -latest(timestamp) | head 50",
        );
        let (s2, body2) = send(
            &c.app,
            get(&format!("/api/search?q={q2}&env=default"), Some(&t)),
        )
        .await;
        assert_eq!(s2, StatusCode::OK);
        let table2 = body2.get("table").expect("expected table: {body2}");
        let rows2 = table2["rows"].as_array().unwrap();
        assert_eq!(rows2.len(), 2);
        // t2 is newer (later latest) so it sorts first; its token sum is null.
        assert_eq!(rows2[0][0], "t2");
        assert!(rows2[0][4].is_null());
        assert_eq!(rows2[1][0], "t1");
        assert_eq!(rows2[1][4], 350.0);
    }

    fn urlencoding_for_test(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'_' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn pipe_stage_before_stats_is_400() {
        let c = ctx().await;
        let t = admin(&c).await;
        // `* | head 5 | stats count` — stages before stats are rejected.
        let q = "%2A%20%7C%20head%205%20%7C%20stats%20count";
        let (status, _) = send(
            &c.app,
            get(&format!("/api/search?q={q}&env=default"), Some(&t)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // Admin read + maintenance

    #[tokio::test]
    async fn admin_read_endpoints_ok() {
        let c = ctx().await;
        let t = admin(&c).await;
        for (path, key) in [
            ("/api/admin/partitions", "partitions"),
            ("/api/admin/index-catalog", "indexes"),
            ("/api/admin/index-info", "scope"),
            ("/api/admin/settings", "mcp_enabled"),
            ("/api/admin/runtime-config", "entries"),
        ] {
            let (status, body) = send(&c.app, get(path, Some(&t))).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.get(key).is_some(), "{path} missing `{key}`: {body}");
        }
    }

    #[tokio::test]
    async fn admin_endpoints_forbidden_for_non_admin() {
        let c = ctx().await;
        let plain = token(&user(&c.control, "bob", false).await, &c.secret);
        for path in [
            "/api/admin/partitions",
            "/api/admin/settings",
            "/api/admin/users",
        ] {
            let (status, _) = send(&c.app, get(path, Some(&plain))).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        }
    }

    #[tokio::test]
    async fn admin_settings_roundtrip() {
        let c = ctx().await;
        let t = admin(&c).await;
        let (s, body) = send(
            &c.app,
            post_json(
                "/api/admin/settings",
                Some(&t),
                json!({ "mcp_enabled": true }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["mcp_enabled"], true);
        // The per-server MCP token is gone — MCP access is gated by API keys.
        assert!(body.get("mcp_auth_token_set").is_none());
    }

    #[tokio::test]
    async fn admin_maintenance_noops() {
        let c = ctx().await;
        let t = admin(&c).await;
        for path in ["/api/admin/merge", "/api/admin/commit", "/api/admin/gc"] {
            let (status, body) = send(&c.app, req("POST", path, Some(&t), None)).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.get("message").is_some(), "{path}: {body}");
        }
    }

    // Saved searches (CRUD + ownership)

    #[tokio::test]
    async fn saved_search_crud_and_ownership() {
        let c = ctx().await;
        let a = token(&user(&c.control, "alice", false).await, &c.secret);
        let b = token(&user(&c.control, "bob", false).await, &c.secret);

        let (s, created) = send(
            &c.app,
            post_json(
                "/api/searches",
                Some(&a),
                json!({"name": "errors", "q": "status:500", "range": "-1h", "public": false}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let id = created["id"].as_str().unwrap().to_string();

        // Owner sees it; another user's own-list is empty.
        let (_, a_list) = send(&c.app, get("/api/searches", Some(&a))).await;
        assert_eq!(a_list["searches"].as_array().unwrap().len(), 1);
        let (_, b_list) = send(&c.app, get("/api/searches", Some(&b))).await;
        assert_eq!(b_list["searches"].as_array().unwrap().len(), 0);

        // Non-owner can't patch or delete → 404.
        let (s_patch, _) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/searches/{id}"),
                Some(&b),
                Some(json!({"name": "x"})),
            ),
        )
        .await;
        assert_eq!(s_patch, StatusCode::NOT_FOUND);

        // Owner updates + deletes.
        let (s_upd, _) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/searches/{id}"),
                Some(&a),
                Some(json!({"name": "renamed"})),
            ),
        )
        .await;
        assert_eq!(s_upd, StatusCode::OK);
        let (s_del, del) = send(
            &c.app,
            req("DELETE", &format!("/api/searches/{id}"), Some(&a), None),
        )
        .await;
        assert_eq!(s_del, StatusCode::OK);
        assert_eq!(del["deleted"], id);
    }

    // Dashboards (CRUD)

    #[tokio::test]
    async fn dashboard_crud() {
        let c = ctx().await;
        let t = token(&user(&c.control, "alice", false).await, &c.secret);
        let (s, created) = send(
            &c.app,
            post_json(
                "/api/dashboards",
                Some(&t),
                json!({"name": "ops", "spec": {"widgets": []}}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let id = created["id"].as_str().unwrap().to_string();

        let (s_get, got) = send(&c.app, get(&format!("/api/dashboards/{id}"), Some(&t))).await;
        assert_eq!(s_get, StatusCode::OK);
        assert_eq!(got["name"], "ops");

        let (s_list, list) = send(&c.app, get("/api/dashboards", Some(&t))).await;
        assert_eq!(s_list, StatusCode::OK);
        assert_eq!(list["dashboards"].as_array().unwrap().len(), 1);

        let (s_patch, _) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/dashboards/{id}"),
                Some(&t),
                Some(json!({"name": "ops2"})),
            ),
        )
        .await;
        assert_eq!(s_patch, StatusCode::OK);

        let (s_del, _) = send(
            &c.app,
            req("DELETE", &format!("/api/dashboards/{id}"), Some(&t), None),
        )
        .await;
        assert_eq!(s_del, StatusCode::OK);

        // Unknown id → 404.
        let (s_404, _) = send(&c.app, get("/api/dashboards/dash_nope", Some(&t))).await;
        assert_eq!(s_404, StatusCode::NOT_FOUND);
    }

    // Monitors + alerts

    #[tokio::test]
    async fn env_retention_patch_and_gc_drop_expired_partition() {
        let c = ctx().await;
        let t = admin(&c).await;
        // Old + fresh partitions in the default env (`seed` pins day 2026-01-01,
        // which is past retention relative to the mocked clock-free sweep; the
        // fresh partition is written explicitly under today's date).
        seed(
            &c.catalog,
            "default",
            "web",
            &[r#"{"timestamp":"2020-01-01T00:00:00Z","message":"ancient"}"#],
        );
        let now = chrono::Utc::now();
        let engine = BlockEngine::new(c.catalog.root(), crate::engine::block_codec());
        engine
            .ingest(
                &PartitionKey::new("default", "web", now.date_naive()),
                &[serde_json::json!({"timestamp": now.to_rfc3339(), "message": "fresh"})],
                None,
            )
            .unwrap();

        // Set a 7-day env override and confirm it echoes + lists.
        let (s, body) = send(
            &c.app,
            req(
                "PATCH",
                "/api/admin/envs/default",
                Some(&t),
                Some(json!({"retention_days": 7})),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["env"]["retention_days"], 7);

        // GC = run retention sweep now.
        let (s2, gc) = send(&c.app, req("POST", "/api/admin/gc", Some(&t), None)).await;
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(gc["partitions_dropped"], 1, "{gc}");

        // The ancient event is gone; the fresh one remains.
        let (s3, found) = send(
            &c.app,
            get(
                "/api/search?q=*&env=default&start=2019-01-01T00:00:00Z",
                Some(&t),
            ),
        )
        .await;
        assert_eq!(s3, StatusCode::OK);
        assert_eq!(found["total"], 1, "{found}");

        // Clearing the override (null) echoes no retention_days.
        let (s4, cleared) = send(
            &c.app,
            req(
                "PATCH",
                "/api/admin/envs/default",
                Some(&t),
                Some(json!({"retention_days": null})),
            ),
        )
        .await;
        assert_eq!(s4, StatusCode::OK);
        assert!(cleared["env"]
            .get("retention_days")
            .is_none_or(|v| v.is_null()));
    }

    #[tokio::test]
    async fn alert_webhook_settings_roundtrip() {
        let c = ctx().await;
        let t = admin(&c).await;
        let (s, body) = send(
            &c.app,
            post_json(
                "/api/admin/settings",
                Some(&t),
                json!({
                    "alert_webhook_enabled": true,
                    "alert_webhook_url": "https://example.com/hook",
                    "alert_webhook_format": "slack",
                    "retention_default_days": 30
                }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["alert_webhook_enabled"], true);
        assert_eq!(body["alert_webhook_url_set"], true);
        assert_eq!(body["alert_webhook_format"], "slack");
        assert_eq!(body["retention_default_days"], 30);

        // Empty URL clears; 0 days clears retention; the URL is never echoed.
        let (s2, body2) = send(
            &c.app,
            post_json(
                "/api/admin/settings",
                Some(&t),
                json!({ "alert_webhook_url": "", "retention_default_days": 0 }),
            ),
        )
        .await;
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(body2["alert_webhook_url_set"], false);
        assert_eq!(body2["retention_default_days"], 0);
        assert!(body2.get("alert_webhook_url").is_none());

        // Bad scheme → 400.
        let (s3, _) = send(
            &c.app,
            post_json(
                "/api/admin/settings",
                Some(&t),
                json!({ "alert_webhook_url": "file:///etc/passwd" }),
            ),
        )
        .await;
        assert_eq!(s3, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn monitor_notify_override_roundtrip() {
        let c = ctx().await;
        let t = token(&user(&c.control, "alice", false).await, &c.secret);
        let (s, created) = send(
            &c.app,
            post_json(
                "/api/monitors?env=default",
                Some(&t),
                json!({
                    "name": "hooked",
                    "prompt": "watch errors",
                    "notify": {"webhook_url": "https://example.com/mon", "format": "slack"}
                }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(created["notify"]["webhook_url"], "https://example.com/mon");
        let id = created["id"].as_str().unwrap().to_string();

        // Empty webhook_url clears the override.
        let (s2, patched) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/monitors/{id}"),
                Some(&t),
                Some(json!({"notify": {"webhook_url": ""}})),
            ),
        )
        .await;
        assert_eq!(s2, StatusCode::OK);
        assert!(patched.get("notify").is_none() || patched["notify"].is_null());
    }

    #[tokio::test]
    async fn monitor_crud_and_run_trigger() {
        let c = ctx().await;
        let t = token(&user(&c.control, "alice", false).await, &c.secret);

        // AI monitor.
        let (s_ai, ai) = send(
            &c.app,
            post_json(
                "/api/monitors?env=default",
                Some(&t),
                json!({"name": "ai watch", "prompt": "watch errors"}),
            ),
        )
        .await;
        assert_eq!(s_ai, StatusCode::OK);
        let ai_id = ai["id"].as_str().unwrap().to_string();

        // Threshold monitor.
        let (s_th, _) = send(
            &c.app,
            post_json(
                "/api/monitors?env=default",
                Some(&t),
                json!({
                    "name": "th watch",
                    "kind": "threshold",
                    "threshold": {"query": "status:500", "comparison": "gt", "threshold": 5, "window_seconds": 300}
                }),
            ),
        )
        .await;
        assert_eq!(s_th, StatusCode::OK);

        let (_, list) = send(&c.app, get("/api/monitors", Some(&t))).await;
        assert_eq!(list["monitors"].as_array().unwrap().len(), 2);

        let (s_get, _) = send(&c.app, get(&format!("/api/monitors/{ai_id}"), Some(&t))).await;
        assert_eq!(s_get, StatusCode::OK);

        let (s_patch, _) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/monitors/{ai_id}"),
                Some(&t),
                Some(json!({"enabled": false})),
            ),
        )
        .await;
        assert_eq!(s_patch, StatusCode::OK);

        // Run-now is an async trigger (no LLM call inline).
        let (s_run, run) = send(
            &c.app,
            req(
                "POST",
                &format!("/api/monitors/{ai_id}/run"),
                Some(&t),
                None,
            ),
        )
        .await;
        assert_eq!(s_run, StatusCode::OK);
        assert_eq!(run["ok"], true);

        let (s_ma, ma) = send(
            &c.app,
            get(&format!("/api/monitors/{ai_id}/alerts"), Some(&t)),
        )
        .await;
        assert_eq!(s_ma, StatusCode::OK);
        assert_eq!(ma["alerts"].as_array().unwrap().len(), 0);

        let (s_del, _) = send(
            &c.app,
            req("DELETE", &format!("/api/monitors/{ai_id}"), Some(&t), None),
        )
        .await;
        assert_eq!(s_del, StatusCode::OK);
    }

    #[tokio::test]
    async fn alert_listing_and_patch_validation() {
        let c = ctx().await;
        let t = admin(&c).await;
        let (s1, l) = send(&c.app, get("/api/alerts", Some(&t))).await;
        assert_eq!(s1, StatusCode::OK);
        assert_eq!(l["alerts"].as_array().unwrap().len(), 0);

        let (s2, cnt) = send(&c.app, get("/api/alerts/unacked-count", Some(&t))).await;
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(cnt["unacked"], 0);

        let (s3, d) = send(
            &c.app,
            req("POST", "/api/alerts/dismiss-all", Some(&t), None),
        )
        .await;
        assert_eq!(s3, StatusCode::OK);
        assert_eq!(d["dismissed"], 0);

        // PATCH with no ack/dismiss flag → 400; unknown id with a flag → 404.
        let (s_bad, _) = send(
            &c.app,
            req("PATCH", "/api/alerts/alt_x", Some(&t), Some(json!({}))),
        )
        .await;
        assert_eq!(s_bad, StatusCode::BAD_REQUEST);
        let (s_404, _) = send(
            &c.app,
            req(
                "PATCH",
                "/api/alerts/alt_x",
                Some(&t),
                Some(json!({"acknowledged": true})),
            ),
        )
        .await;
        assert_eq!(s_404, StatusCode::NOT_FOUND);
    }

    // Auth: logout, password, preferences

    #[tokio::test]
    async fn logout_revokes_token() {
        let c = ctx().await;
        let u = user(&c.control, "carol", false).await;
        let tok = token(&u, &c.secret);
        let (s, _) = send(&c.app, req("POST", "/api/auth/logout", Some(&tok), None)).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (after, _) = send(&c.app, get("/api/auth/me", Some(&tok))).await;
        assert_eq!(after, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn change_password_paths() {
        let c = ctx().await;
        let u = user(&c.control, "dave", false).await;
        let tok = token(&u, &c.secret);
        // Too-short new password → 400.
        let (s_short, _) = send(
            &c.app,
            post_json(
                "/api/auth/password",
                Some(&tok),
                json!({"current_password": "pw12345678", "new_password": "short"}),
            ),
        )
        .await;
        assert_eq!(s_short, StatusCode::BAD_REQUEST);
        // Wrong current → 401.
        let (s_wrong, _) = send(
            &c.app,
            post_json(
                "/api/auth/password",
                Some(&tok),
                json!({"current_password": "nope", "new_password": "newpass12"}),
            ),
        )
        .await;
        assert_eq!(s_wrong, StatusCode::UNAUTHORIZED);
        // Correct → 200 + a fresh token.
        let (s_ok, body) = send(
            &c.app,
            post_json(
                "/api/auth/password",
                Some(&tok),
                json!({"current_password": "pw12345678", "new_password": "newpass12"}),
            ),
        )
        .await;
        assert_eq!(s_ok, StatusCode::OK);
        assert!(body["token"].as_str().is_some());
    }

    #[tokio::test]
    async fn preferences_update() {
        let c = ctx().await;
        let tok = token(&user(&c.control, "erin", false).await, &c.secret);
        let (s, body) = send(
            &c.app,
            post_json(
                "/api/account/preferences",
                Some(&tok),
                json!({"timezone": "UTC"}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["user"]["timezone"], "UTC");

        // Theme + palette persist and survive /me; unknown palette is a 400.
        let (s2, body2) = send(
            &c.app,
            post_json(
                "/api/account/preferences",
                Some(&tok),
                json!({"theme": "light", "palette": "dracula"}),
            ),
        )
        .await;
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(body2["user"]["palette"], "dracula");
        let (s_me, me) = send(&c.app, get("/api/auth/me", Some(&tok))).await;
        assert_eq!(s_me, StatusCode::OK);
        assert_eq!(me["user"]["theme"], "light");
        assert_eq!(me["user"]["palette"], "dracula");
        let (s_bad, _) = send(
            &c.app,
            post_json(
                "/api/account/preferences",
                Some(&tok),
                json!({"palette": "nope"}),
            ),
        )
        .await;
        assert_eq!(s_bad, StatusCode::BAD_REQUEST);
        // Empty string clears back to "follow the instance default".
        let (s_clear, cleared) = send(
            &c.app,
            post_json(
                "/api/account/preferences",
                Some(&tok),
                json!({"palette": ""}),
            ),
        )
        .await;
        assert_eq!(s_clear, StatusCode::OK);
        assert!(cleared["user"]["palette"].is_null());
    }

    // Envs + per-user allowlist

    #[tokio::test]
    async fn envs_list_create_delete() {
        let c = ctx().await;
        let t = admin(&c).await;
        let (s_list, list) = send(&c.app, get("/api/envs", Some(&t))).await;
        assert_eq!(s_list, StatusCode::OK);
        let names: Vec<&str> = list["envs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert!(names.contains(&"default"));

        let (s_create, _) = send(
            &c.app,
            post_json("/api/admin/envs", Some(&t), json!({"name": "staging"})),
        )
        .await;
        assert_eq!(s_create, StatusCode::OK);

        // Reserved env can't be deleted.
        let (s_reserved, _) = send(
            &c.app,
            req("DELETE", "/api/admin/envs/default", Some(&t), None),
        )
        .await;
        assert_eq!(s_reserved, StatusCode::BAD_REQUEST);

        let (s_del, del) = send(
            &c.app,
            req("DELETE", "/api/admin/envs/staging", Some(&t), None),
        )
        .await;
        assert_eq!(s_del, StatusCode::OK);
        assert_eq!(del["ok"], true);
    }

    #[tokio::test]
    async fn env_reorder_and_login_default() {
        let c = ctx().await;
        let t = admin(&c).await;

        // Two user envs alongside the reserved `default`.
        for n in ["alpha", "beta"] {
            let (s, _) = send(
                &c.app,
                post_json("/api/admin/envs", Some(&t), json!({ "name": n })),
            )
            .await;
            assert_eq!(s, StatusCode::OK);
        }

        // Reorder: the list echoes the requested order.
        let (s_reorder, _) = send(
            &c.app,
            post_json(
                "/api/admin/env-order",
                Some(&t),
                json!({ "names": ["beta", "default", "alpha"] }),
            ),
        )
        .await;
        assert_eq!(s_reorder, StatusCode::OK);
        let (_, list) = send(&c.app, get("/api/envs", Some(&t))).await;
        let names: Vec<&str> = list["envs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert_eq!(names, vec!["beta", "default", "alpha"]);

        // Set the login default; it rides along on the env list...
        let (s_def, def) = send(
            &c.app,
            req(
                "PUT",
                "/api/admin/env-default",
                Some(&t),
                Some(json!({ "name": "beta" })),
            ),
        )
        .await;
        assert_eq!(s_def, StatusCode::OK);
        assert_eq!(def["default_env"], "beta");
        let (_, list2) = send(&c.app, get("/api/envs", Some(&t))).await;
        assert_eq!(list2["default_env"], "beta");

        // ...and a fresh login lands on it.
        user(&c.control, "dave", false).await;
        let creds = json!({ "login": "dave", "password": "pw12345678" });
        let (s_login, login) =
            send(&c.app, post_json("/api/auth/login", None, creds.clone())).await;
        assert_eq!(s_login, StatusCode::OK);
        assert_eq!(login["user"]["active_env"], "beta");

        // A system env or an unknown env can't be the default.
        for bad in ["_system", "ghost"] {
            let (s, _) = send(
                &c.app,
                req(
                    "PUT",
                    "/api/admin/env-default",
                    Some(&t),
                    Some(json!({ "name": bad })),
                ),
            )
            .await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "default should reject {bad}");
        }

        // Clearing falls back to `default` for new logins.
        let (s_clear, _) = send(
            &c.app,
            req("DELETE", "/api/admin/env-default", Some(&t), None),
        )
        .await;
        assert_eq!(s_clear, StatusCode::OK);
        let (_, list3) = send(&c.app, get("/api/envs", Some(&t))).await;
        assert!(list3["default_env"].is_null());
        let (_, login2) = send(&c.app, post_json("/api/auth/login", None, creds)).await;
        assert_eq!(login2["user"]["active_env"], "default");
    }

    #[tokio::test]
    async fn demo_mode_restricts_only_the_demo_account() {
        let demo = DemoConfig::new(true, Some("demo".into()), Some("demo-pw".into()));
        let c = ctx_with_demo(demo).await;
        // The restricted account (matches HELIOS_DEMO_LOGIN) and an unrelated user.
        let demo_user = user(&c.control, "demo", false).await;
        let demo_tok = token(&demo_user, &c.secret);
        let admin_tok = admin(&c).await;

        // Demo account: reads work...
        let (s_read, _) = send(&c.app, get("/api/envs", Some(&demo_tok))).await;
        assert_eq!(s_read, StatusCode::OK);

        // ...but a (non-admin) mutating API is rejected with the demo marker.
        let (s_w, body) = send(
            &c.app,
            req("POST", "/api/monitors", Some(&demo_tok), Some(json!({}))),
        )
        .await;
        assert_eq!(s_w, StatusCode::FORBIDDEN, "demo account write blocked");
        assert_eq!(body["demo_mode"], true);

        // Agent chat + logout stay open even for the demo account.
        let (s_agent, _) = send(
            &c.app,
            req(
                "POST",
                "/api/agent/conversations/nope/messages",
                Some(&demo_tok),
                Some(json!({ "content": "hi" })),
            ),
        )
        .await;
        assert_ne!(
            s_agent,
            StatusCode::FORBIDDEN,
            "agent chat must not be gated"
        );
        let (s_logout, _) = send(
            &c.app,
            req("POST", "/api/auth/logout", Some(&demo_tok), None),
        )
        .await;
        assert_ne!(s_logout, StatusCode::FORBIDDEN, "logout must not be gated");

        // Dismissing/acking alerts stays open even for the demo account.
        let (s_dismiss, _) = send(
            &c.app,
            req("POST", "/api/alerts/dismiss-all", Some(&demo_tok), None),
        )
        .await;
        assert_ne!(
            s_dismiss,
            StatusCode::FORBIDDEN,
            "alert dismiss-all must not be gated"
        );
        let (s_ack, _) = send(
            &c.app,
            req(
                "PATCH",
                "/api/alerts/alt_x",
                Some(&demo_tok),
                Some(json!({ "acknowledged": true })),
            ),
        )
        .await;
        assert_ne!(
            s_ack,
            StatusCode::FORBIDDEN,
            "alert ack/patch must not be gated"
        );

        // A DIFFERENT user writes freely even with demo mode on.
        let (s_other, other) = send(
            &c.app,
            post_json(
                "/api/admin/envs",
                Some(&admin_tok),
                json!({ "name": "staging" }),
            ),
        )
        .await;
        assert_eq!(s_other, StatusCode::OK, "non-demo user unaffected: {other}");
        // ...and even a non-admin non-demo user isn't demo-gated (an empty body
        // just fails validation in the handler, past the gate — no demo marker).
        let alice = user(&c.control, "alice", false).await;
        let (_, alice_body) = send(
            &c.app,
            req(
                "POST",
                "/api/monitors",
                Some(&token(&alice, &c.secret)),
                Some(json!({})),
            ),
        )
        .await;
        assert_ne!(
            alice_body["demo_mode"], true,
            "alice is not demo-restricted"
        );

        // setup_status advertises demo mode + the pre-fill creds (public).
        let (s_setup, setup) = send(&c.app, get("/api/auth/setup_status", None)).await;
        assert_eq!(s_setup, StatusCode::OK);
        assert_eq!(setup["demo_mode"], true);
        assert_eq!(setup["demo_login"], "demo");
        assert_eq!(setup["demo_password"], "demo-pw");
    }

    #[tokio::test]
    async fn non_demo_setup_status_hides_demo_creds() {
        let c = ctx().await;
        let (s, setup) = send(&c.app, get("/api/auth/setup_status", None)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(setup["demo_mode"], false);
        assert!(setup["demo_login"].is_null());
        assert!(setup["demo_password"].is_null());
    }

    #[tokio::test]
    async fn user_allowlist_get_and_set() {
        let c = ctx().await;
        let t = admin(&c).await;
        let target = user(&c.control, "frank", false).await;
        let (s_get, got) = send(
            &c.app,
            get(&format!("/api/admin/users/{}/allowed", target.id), Some(&t)),
        )
        .await;
        assert_eq!(s_get, StatusCode::OK);
        assert_eq!(got["allowed"].as_array().unwrap().len(), 0);

        let (s_put, put) = send(
            &c.app,
            req(
                "PUT",
                &format!("/api/admin/users/{}/allowed", target.id),
                Some(&t),
                Some(json!({"allowed": [{"env": "default", "indexes": ["web"]}]})),
            ),
        )
        .await;
        assert_eq!(s_put, StatusCode::OK);
        assert_eq!(put["allowed"][0]["env"], "default");
    }

    // User management (admin)

    #[tokio::test]
    async fn user_management_crud() {
        let c = ctx().await;
        let t = admin(&c).await;
        let (s_create, created) = send(
            &c.app,
            post_json(
                "/api/admin/users",
                Some(&t),
                json!({"userid": "grace", "email": "grace@x.com", "display_name": "Grace"}),
            ),
        )
        .await;
        assert_eq!(s_create, StatusCode::OK);
        let id = created["user"]["id"].as_str().unwrap().to_string();
        assert!(
            created["password"].as_str().is_some(),
            "create returns a password"
        );

        let (s_list, list) = send(&c.app, get("/api/admin/users", Some(&t))).await;
        assert_eq!(s_list, StatusCode::OK);
        assert!(list["users"].as_array().unwrap().len() >= 2);

        let (s_patch, patched) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/admin/users/{id}"),
                Some(&t),
                Some(json!({"display_name": "Grace H"})),
            ),
        )
        .await;
        assert_eq!(s_patch, StatusCode::OK);
        assert_eq!(patched["user"]["display_name"], "Grace H");

        let (s_pw, pw) = send(
            &c.app,
            req(
                "POST",
                &format!("/api/admin/users/{id}/password"),
                Some(&t),
                None,
            ),
        )
        .await;
        assert_eq!(s_pw, StatusCode::OK);
        assert!(pw["password"].as_str().is_some());

        let (s_del, del) = send(
            &c.app,
            req("DELETE", &format!("/api/admin/users/{id}"), Some(&t), None),
        )
        .await;
        assert_eq!(s_del, StatusCode::OK);
        assert_eq!(del["deleted"], id);
    }

    // Agent: conversations CRUD + LLM-config + message validation

    #[tokio::test]
    async fn conversation_crud_and_message_validation() {
        let c = ctx().await;
        let tok = token(&user(&c.control, "alice", false).await, &c.secret);

        let (s_c, conv) = send(
            &c.app,
            post_json(
                "/api/agent/conversations",
                Some(&tok),
                json!({"title": "debugging"}),
            ),
        )
        .await;
        assert_eq!(s_c, StatusCode::OK);
        let id = conv["conversation"]["id"].as_str().unwrap().to_string();

        let (s_l, list) = send(&c.app, get("/api/agent/conversations", Some(&tok))).await;
        assert_eq!(s_l, StatusCode::OK);
        assert_eq!(list["conversations"].as_array().unwrap().len(), 1);

        let (s_g, got) = send(
            &c.app,
            get(&format!("/api/agent/conversations/{id}"), Some(&tok)),
        )
        .await;
        assert_eq!(s_g, StatusCode::OK);
        assert_eq!(got["id"], id);

        let (s_r, _) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/agent/conversations/{id}"),
                Some(&tok),
                Some(json!({"title": "renamed"})),
            ),
        )
        .await;
        assert_eq!(s_r, StatusCode::OK);

        // Empty message → 400; unknown conversation → 404. (Happy path needs an
        // LLM provider, out of scope for an offline harness.)
        let (s_empty, _) = send(
            &c.app,
            post_json(
                &format!("/api/agent/conversations/{id}/messages"),
                Some(&tok),
                json!({"content": "   "}),
            ),
        )
        .await;
        assert_eq!(s_empty, StatusCode::BAD_REQUEST);
        let (s_unknown, _) = send(
            &c.app,
            post_json(
                "/api/agent/conversations/conv_nope/messages",
                Some(&tok),
                json!({"content": "hi"}),
            ),
        )
        .await;
        assert_eq!(s_unknown, StatusCode::NOT_FOUND);

        let (s_d, _) = send(
            &c.app,
            req(
                "DELETE",
                &format!("/api/agent/conversations/{id}"),
                Some(&tok),
                None,
            ),
        )
        .await;
        assert_eq!(s_d, StatusCode::OK);
    }

    #[tokio::test]
    async fn agent_llm_settings_redacts_keys() {
        let c = ctx().await;
        let t = admin(&c).await;
        let (s_get, body) = send(&c.app, get("/api/admin/agent", Some(&t))).await;
        assert_eq!(s_get, StatusCode::OK);
        // Keys are reported as `*_set` booleans, never echoed.
        assert!(body.get("openai_api_key_set").is_some());
        assert!(body.get("openai_api_key").is_none());

        // Per-provider model: openai is the default active provider, so setting
        // openai_model resolves the active `model`. Also flips the enabled flag.
        let (s_put, after) = send(
            &c.app,
            req(
                "PUT",
                "/api/admin/agent",
                Some(&t),
                Some(json!({"openai_model": "claude-x", "enabled": false})),
            ),
        )
        .await;
        assert_eq!(s_put, StatusCode::OK);
        assert_eq!(after["openai_model"], "claude-x");
        assert_eq!(after["model"], "claude-x");
        assert_eq!(after["enabled"], false);
    }

    // Ingest tokens (admin)

    #[tokio::test]
    async fn ingest_token_admin_crud() {
        let c = ctx().await;
        let t = admin(&c).await;
        let (s_list, list) = send(&c.app, get("/api/admin/ingest-tokens", Some(&t))).await;
        assert_eq!(s_list, StatusCode::OK);
        assert_eq!(list["require"], false);

        let (s_create, created) = send(
            &c.app,
            post_json(
                "/api/admin/ingest-tokens",
                Some(&t),
                json!({"name": "shipper", "env": "default"}),
            ),
        )
        .await;
        assert_eq!(s_create, StatusCode::OK);
        let id = created["id"].as_str().unwrap().to_string();
        assert!(created["token"].as_str().unwrap().starts_with("hli_"));

        let (s_req, set) = send(
            &c.app,
            req(
                "PUT",
                "/api/admin/ingest-auth",
                Some(&t),
                Some(json!({"require": true})),
            ),
        )
        .await;
        assert_eq!(s_req, StatusCode::OK);
        assert_eq!(set["require"], true);

        let (s_patch, _) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/admin/ingest-tokens/{id}"),
                Some(&t),
                Some(json!({"enabled": false})),
            ),
        )
        .await;
        assert_eq!(s_patch, StatusCode::OK);

        let (s_del, del) = send(
            &c.app,
            req(
                "DELETE",
                &format!("/api/admin/ingest-tokens/{id}"),
                Some(&t),
                None,
            ),
        )
        .await;
        assert_eq!(s_del, StatusCode::OK);
        assert_eq!(del["deleted"], id);
    }

    #[tokio::test]
    async fn api_key_crud_scope_and_auth() {
        let c = ctx().await;
        let t = admin(&c).await;

        // Mint an admin-scoped key.
        let (s_create, created) = send(
            &c.app,
            post_json(
                "/api/admin/api-keys",
                Some(&t),
                json!({"name": "ci-admin", "scopes": {"admin": true}}),
            ),
        )
        .await;
        assert_eq!(s_create, StatusCode::OK);
        let admin_key = created["token"].as_str().unwrap().to_string();
        let admin_id = created["id"].as_str().unwrap().to_string();
        assert!(admin_key.starts_with("hlk_"));

        // The admin key authenticates and clears the admin gate.
        let (s_users, _) = send(&c.app, get("/api/admin/users", Some(&admin_key))).await;
        assert_eq!(s_users, StatusCode::OK);
        // Admin scope implies standard API access too.
        let (s_admin_me, _) = send(&c.app, get("/api/auth/me", Some(&admin_key))).await;
        assert_eq!(s_admin_me, StatusCode::OK);

        // Listings mask the secret — the full `hlk_…` value is never echoed back.
        let (s_list, list) = send(&c.app, get("/api/admin/api-keys", Some(&t))).await;
        assert_eq!(s_list, StatusCode::OK);
        let row = &list["keys"].as_array().unwrap()[0];
        assert_eq!(row["scopes"]["admin"], true);
        assert!(row["token_hint"].as_str().unwrap().starts_with('…'));
        assert!(
            !list.to_string().contains("hlk_"),
            "secret leaked in listing"
        );

        // A standard (non-admin) key.
        let (_, std_created) = send(
            &c.app,
            post_json(
                "/api/admin/api-keys",
                Some(&t),
                json!({"name": "ci-std", "scopes": {"api": true}}),
            ),
        )
        .await;
        let std_key = std_created["token"].as_str().unwrap().to_string();
        let std_id = std_created["id"].as_str().unwrap().to_string();

        // It authenticates on a normal endpoint as a non-admin principal…
        let (s_me, me) = send(&c.app, get("/api/auth/me", Some(&std_key))).await;
        assert_eq!(s_me, StatusCode::OK);
        assert_eq!(me["user"]["is_admin"], false);
        assert_eq!(me["user"]["display_name"], "API key: ci-std");
        // …but the admin gate rejects it.
        let (s_forbidden, _) = send(&c.app, get("/api/admin/users", Some(&std_key))).await;
        assert_eq!(s_forbidden, StatusCode::FORBIDDEN);

        // An MCP-only key can't drive the REST surface at all.
        let (_, mcp_created) = send(
            &c.app,
            post_json(
                "/api/admin/api-keys",
                Some(&t),
                json!({"name": "ci-mcp", "scopes": {"mcp": true}}),
            ),
        )
        .await;
        let mcp_key = mcp_created["token"].as_str().unwrap().to_string();
        assert_eq!(mcp_created["scopes"]["mcp"], true);
        let (s_mcp_rest, _) = send(&c.app, get("/api/auth/me", Some(&mcp_key))).await;
        assert_eq!(s_mcp_rest, StatusCode::FORBIDDEN);

        // A scopeless key is rejected at creation.
        let (s_noscope, _) = send(
            &c.app,
            post_json(
                "/api/admin/api-keys",
                Some(&t),
                json!({"name": "ci-none", "scopes": {}}),
            ),
        )
        .await;
        assert_eq!(s_noscope, StatusCode::BAD_REQUEST);

        // Disabling a key stops it authenticating.
        let (s_patch, _) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/admin/api-keys/{admin_id}"),
                Some(&t),
                Some(json!({"enabled": false})),
            ),
        )
        .await;
        assert_eq!(s_patch, StatusCode::OK);
        let (s_disabled, _) = send(&c.app, get("/api/admin/users", Some(&admin_key))).await;
        assert_eq!(s_disabled, StatusCode::UNAUTHORIZED);

        // An unknown `hlk_` token is unauthorized (not a 500).
        let (s_unknown, _) = send(&c.app, get("/api/auth/me", Some("hlk_deadbeef"))).await;
        assert_eq!(s_unknown, StatusCode::UNAUTHORIZED);

        let (s_del, del) = send(
            &c.app,
            req(
                "DELETE",
                &format!("/api/admin/api-keys/{std_id}"),
                Some(&t),
                None,
            ),
        )
        .await;
        assert_eq!(s_del, StatusCode::OK);
        assert_eq!(del["deleted"], std_id);
    }

    // Sources (admin browse + CRUD)

    #[tokio::test]
    async fn source_crud_and_browse() {
        let c = ctx().await;
        let t = admin(&c).await;

        // Browse is admin-only and lists real directories.
        let (s_browse, browse) = send(&c.app, get("/api/sources/browse?path=/tmp", Some(&t))).await;
        assert_eq!(s_browse, StatusCode::OK);
        assert!(browse.get("dirs").is_some(), "{browse}");

        let (s_create, created) = send(
            &c.app,
            post_json(
                "/api/sources?env=default",
                Some(&t),
                json!({"name": "syslog", "index": "web", "path": "/tmp/*.log"}),
            ),
        )
        .await;
        assert_eq!(s_create, StatusCode::OK);
        let id = created["id"].as_str().unwrap().to_string();

        let (s_list, list) = send(&c.app, get("/api/sources", Some(&t))).await;
        assert_eq!(s_list, StatusCode::OK);
        assert_eq!(list["sources"].as_array().unwrap().len(), 1);

        let (s_get, got) = send(&c.app, get(&format!("/api/sources/{id}"), Some(&t))).await;
        assert_eq!(s_get, StatusCode::OK);
        assert!(got.get("source").is_some() && got.get("checkpoint").is_some());

        let (s_patch, _) = send(
            &c.app,
            req(
                "PATCH",
                &format!("/api/sources/{id}"),
                Some(&t),
                Some(json!({"enabled": false})),
            ),
        )
        .await;
        assert_eq!(s_patch, StatusCode::OK);

        // Run-now queues the source for the supervisor (idle in tests).
        let (s_run, run) = send(
            &c.app,
            req("POST", &format!("/api/sources/{id}/run"), Some(&t), None),
        )
        .await;
        assert_eq!(s_run, StatusCode::OK);
        assert_eq!(run["queued"], id);

        let (s_reset, _) = send(
            &c.app,
            req("POST", &format!("/api/sources/{id}/reset"), Some(&t), None),
        )
        .await;
        assert_eq!(s_reset, StatusCode::OK);

        let (s_del, del) = send(
            &c.app,
            req("DELETE", &format!("/api/sources/{id}"), Some(&t), None),
        )
        .await;
        assert_eq!(s_del, StatusCode::OK);
        assert_eq!(del["deleted"], id);
    }

    // Ingest endpoints + shims (block writer absent → 200 with 0 ingested)

    #[tokio::test]
    async fn ingest_handler_validation_and_open_path() {
        let c = ctx().await;
        // Open path (no auth). Writer isn't installed in tests, so the row is
        // counted as a write error but the request still succeeds.
        let (s, body) = send(
            &c.app,
            raw_post(
                "/api/ingest?env=default&index=web",
                "application/x-ndjson",
                "{\"message\":\"hello\"}\n",
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["ingested"], 0);
        assert!(body["write_errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap().contains("writer not running")));

        // The raw endpoint takes the same shape with an explicit format.
        let (s_raw, raw) = send(
            &c.app,
            raw_post(
                "/api/ingest/raw?env=default&index=web&format=ndjson",
                "application/x-ndjson",
                "{\"message\":\"hello\"}\n",
            ),
        )
        .await;
        assert_eq!(s_raw, StatusCode::OK);
        assert!(raw.get("ingested").is_some(), "{raw}");

        // System env is rejected (400).
        let (s_sys, _) = send(
            &c.app,
            raw_post(
                "/api/ingest?env=_system&index=web",
                "application/x-ndjson",
                "{}\n",
            ),
        )
        .await;
        assert_eq!(s_sys, StatusCode::BAD_REQUEST);

        // Unknown env is rejected (400).
        let (s_unknown, _) = send(
            &c.app,
            raw_post(
                "/api/ingest?env=ghost&index=web",
                "application/x-ndjson",
                "{}\n",
            ),
        )
        .await;
        assert_eq!(s_unknown, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn shim_endpoints_accept_native_formats() {
        let c = ctx().await;

        // HEC health (always healthy).
        let (s_h, h) = send(&c.app, get("/services/collector/health", None)).await;
        assert_eq!(s_h, StatusCode::OK);
        assert_eq!(h["code"], 17);

        // HEC event.
        let (s_hec, hec) = send(
            &c.app,
            raw_post(
                "/services/collector?env=default&index=web",
                "application/json",
                "{\"event\": {\"message\": \"hi\"}}",
            ),
        )
        .await;
        assert_eq!(s_hec, StatusCode::OK);
        assert!(hec.get("code").is_some());

        // Elasticsearch _bulk (action + doc lines).
        let (s_es, es) = send(
            &c.app,
            raw_post(
                "/api/es/_bulk?env=default&index=web",
                "application/x-ndjson",
                "{\"index\":{}}\n{\"message\":\"x\"}\n",
            ),
        )
        .await;
        assert_eq!(s_es, StatusCode::OK);
        assert!(es.get("items").is_some());

        // OTLP logs (JSON), empty payload accepted.
        let (s_otlp, _) = send(
            &c.app,
            raw_post(
                "/api/otlp/v1/logs?env=default",
                "application/json",
                "{\"resourceLogs\":[]}",
            ),
        )
        .await;
        assert_eq!(s_otlp, StatusCode::OK);

        // Loki push returns 204 No Content.
        let (s_loki, _) = send(
            &c.app,
            raw_post(
                "/loki/api/v1/push?env=default&index=web",
                "application/json",
                "{\"streams\":[]}",
            ),
        )
        .await;
        assert_eq!(s_loki, StatusCode::NO_CONTENT);
    }

    // SAML SP endpoints

    #[tokio::test]
    async fn saml_public_endpoints() {
        let c = ctx().await;
        // Status is public and reports disabled by default.
        let (s_status, status) = send(&c.app, get("/api/auth/saml/status", None)).await;
        assert_eq!(s_status, StatusCode::OK);
        assert_eq!(status["enabled"], false);

        // Metadata 404s when SP entity / ACS aren't configured.
        let (s_meta, _) = send(&c.app, get("/api/auth/saml/metadata", None)).await;
        assert_eq!(s_meta, StatusCode::NOT_FOUND);

        // Login redirects (to the error page when unconfigured).
        let resp = c
            .app
            .clone()
            .oneshot(get("/api/auth/saml/login", None))
            .await
            .unwrap();
        assert!(resp.status().is_redirection(), "got {}", resp.status());

        // ACS with a garbage assertion redirects to the error page, not 500.
        let acs = raw_post(
            "/api/auth/saml/acs",
            "application/x-www-form-urlencoded",
            "SAMLResponse=not-base64",
        );
        let resp = c.app.clone().oneshot(acs).await.unwrap();
        assert!(resp.status().is_redirection(), "got {}", resp.status());
    }

    #[tokio::test]
    async fn saml_admin_config_roundtrip() {
        let c = ctx().await;
        let t = admin(&c).await;
        let (s_get, body) = send(&c.app, get("/api/admin/saml", Some(&t))).await;
        assert_eq!(s_get, StatusCode::OK);
        // Cert is never echoed — only a `cert_set` flag.
        assert_eq!(body["cert_set"], false);
        assert!(body.get("idp_cert").is_none());

        let (s_post, after) = send(
            &c.app,
            post_json(
                "/api/admin/saml",
                Some(&t),
                json!({"button_label": "Corp SSO"}),
            ),
        )
        .await;
        assert_eq!(s_post, StatusCode::OK);
        assert_eq!(after["button_label"], "Corp SSO");
    }

    // MCP JSON-RPC envelope

    #[tokio::test]
    async fn mcp_initialize_and_ping() {
        let c = ctx().await;
        let (s_init, init) = send(
            &c.app,
            post_json(
                "/mcp",
                None,
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
            ),
        )
        .await;
        assert_eq!(s_init, StatusCode::OK);
        assert!(init["result"]["protocolVersion"].as_str().is_some());
        assert_eq!(init["result"]["serverInfo"]["name"], "helioslogs");

        let (s_ping, ping) = send(
            &c.app,
            post_json(
                "/mcp",
                None,
                json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
            ),
        )
        .await;
        assert_eq!(s_ping, StatusCode::OK);
        assert_eq!(ping["result"], json!({}));
    }

    #[tokio::test]
    async fn mcp_unknown_method_and_disabled_call() {
        let c = ctx().await;
        // Unknown method → JSON-RPC method-not-found (-32601), HTTP 200.
        let (s_unknown, unknown) = send(
            &c.app,
            post_json(
                "/mcp",
                None,
                json!({"jsonrpc": "2.0", "id": 1, "method": "no.such.method"}),
            ),
        )
        .await;
        assert_eq!(s_unknown, StatusCode::OK);
        assert_eq!(unknown["error"]["code"], -32601);

        // tools/call while MCP is disabled → tool-level error in the result.
        let (s_call, call) = send(
            &c.app,
            post_json(
                "/mcp",
                None,
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                       "params": {"name": "list_indexes", "arguments": {}}}),
            ),
        )
        .await;
        assert_eq!(s_call, StatusCode::OK);
        assert_eq!(call["result"]["isError"], true);
    }

    #[tokio::test]
    async fn mcp_open_until_first_mcp_key_then_requires_one() {
        let c = ctx().await;
        let t = admin(&c).await;

        // Enable MCP.
        let (s_en, _) = send(
            &c.app,
            post_json(
                "/api/admin/settings",
                Some(&t),
                json!({"mcp_enabled": true}),
            ),
        )
        .await;
        assert_eq!(s_en, StatusCode::OK);

        let call_req = || {
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": "list_indexes", "arguments": {}}})
        };

        // No MCP-scoped key yet → anonymous call is allowed.
        let (_, anon) = send(&c.app, post_json("/mcp", None, call_req())).await;
        assert_eq!(anon["result"]["isError"], false, "{anon}");

        // Mint an MCP-scoped key.
        let (_, key) = send(
            &c.app,
            post_json(
                "/api/admin/api-keys",
                Some(&t),
                json!({"name": "agent", "scopes": {"mcp": true}}),
            ),
        )
        .await;
        let mcp_key = key["token"].as_str().unwrap().to_string();

        // Now an anonymous call is rejected…
        let (_, anon2) = send(&c.app, post_json("/mcp", None, call_req())).await;
        assert_eq!(anon2["result"]["isError"], true, "{anon2}");

        // …but the MCP-scoped key works.
        let (_, ok) = send(&c.app, post_json("/mcp", Some(&mcp_key), call_req())).await;
        assert_eq!(ok["result"]["isError"], false, "{ok}");

        // An admin key with no MCP scope can't reach MCP.
        let (_, akey) = send(
            &c.app,
            post_json(
                "/api/admin/api-keys",
                Some(&t),
                json!({"name": "adm", "scopes": {"admin": true}}),
            ),
        )
        .await;
        let admin_key = akey["token"].as_str().unwrap().to_string();
        let (_, denied) = send(&c.app, post_json("/mcp", Some(&admin_key), call_req())).await;
        assert_eq!(denied["result"]["isError"], true, "{denied}");
    }

    #[tokio::test]
    async fn first_run_setup_creates_admin_then_self_closes() {
        let c = ctx().await;

        // No users yet → setup screen.
        let (s, body) = send(&c.app, get("/api/auth/setup_status", None)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["needs_setup"], true);

        // Claim the instance: first user is created as an admin and logged in.
        let (s, body) = send(
            &c.app,
            post_json(
                "/api/auth/setup",
                None,
                json!({"userid": "founder", "password": "supersecret"}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert!(body["token"].as_str().is_some_and(|t| !t.is_empty()));
        assert_eq!(body["user"]["is_admin"], true);
        assert_eq!(body["user"]["userid"], "founder");

        // Instance is now claimed: status flips and setup self-closes (409).
        let (_, body) = send(&c.app, get("/api/auth/setup_status", None)).await;
        assert_eq!(body["needs_setup"], false);

        let (s, _) = send(
            &c.app,
            post_json(
                "/api/auth/setup",
                None,
                json!({"userid": "intruder", "password": "supersecret"}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn setup_rejects_short_password() {
        let c = ctx().await;
        let (s, _) = send(
            &c.app,
            post_json(
                "/api/auth/setup",
                None,
                json!({"userid": "founder", "password": "short"}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // A rejected attempt must not have claimed the instance.
        let (_, body) = send(&c.app, get("/api/auth/setup_status", None)).await;
        assert_eq!(body["needs_setup"], true);
    }
}
