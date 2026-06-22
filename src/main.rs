// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Block-engine log indexer + searcher + HTTP server (partitioned).
//! Subcommands: `search`, `describe`, `serve`. Storage layout: see [`catalog`].

use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

// jemalloc everywhere, tuned to return freed memory to the OS proactively (short
// decay) so compaction/search spikes don't ratchet RSS into the container cap.
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Linux supports the background purge thread; pair it with short decay.
#[cfg(target_os = "linux")]
#[allow(non_upper_case_globals)]
#[export_name = "malloc_conf"]
pub static malloc_conf: &[u8] = b"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:1000\0";

// macOS has no jemalloc background thread; keep the short decay (purges on
// allocator activity) and drop background_thread, which would just be ignored.
#[cfg(not(target_os = "linux"))]
#[allow(non_upper_case_globals)]
#[export_name = "malloc_conf"]
pub static malloc_conf: &[u8] = b"dirty_decay_ms:1000,muzzy_decay_ms:1000\0";

mod agent;
mod auth;
mod catalog;
mod control;
mod crypto;
mod engine;
mod http;
mod indexer;
mod llm;
mod mcp;
mod memstats;
mod monitor;
mod notify;
mod outbound;
mod retention;
mod runtime_config;
mod saml;
mod sample_data;
mod schema;
mod search;
mod self_logs;
mod source;
mod syslog;

// Env-var reference appended to `--help` (and `serve --help`). These are honored
// in addition to the CLI flags; tuning knobs follow env > Admin setting > default.
const ENV_HELP: &str = "\
Environment variables (optional; override built-in defaults):

Storage engine (live-tunable from Admin \u{2192} General unless noted):
  HELIOS_BLOCK_COMPACT_SECS       Compactor interval, seconds [default: 30]
  HELIOS_BLOCK_TARGET_MB          Compaction target block size, MB [default: 64]
  HELIOS_BLOCK_MIN_COMPACT_MB     Min merge-group size before rewriting, MB [default: 5]
  HELIOS_BLOCK_MAX_SMALL_BLOCKS   Small-block count that waives the floor [default: 100]
  HELIOS_BLOCK_FLUSH_ROWS         Buffered-ingest row threshold to flush [default: 50000]
  HELIOS_BLOCK_FLUSH_SECS         Buffered-ingest time threshold to flush, seconds [default: 5]
  HELIOS_BLOCK_SYNC_SECS          Shared-store sync interval, seconds (--shared-store only) [default: 10]
  HELIOS_BLOCK_QUEUE_CAP          Ingest channel depth / backpressure bound [default: 100000]
  HELIOS_BLOCK_FLUSH_CONCURRENCY  Max concurrent block flushes [default: 2]
  HELIOS_BLOCK_COMPRESSION        Block codec; off/none/0/false = uncompressed [default: zstd]

Query:
  HELIOS_QUERY_THREADS            Query fan-out thread pool size; restart-only [default: 4]
  HELIOS_QUERY_CACHE_MB           Per-block match cache, MB; 0 disables [default: 1024]
  HELIOS_AGG_MAX_PARTITIONS       Partitions scanned exactly before agg sampling [default: 96]

Retention:
  HELIOS_RETENTION_DEFAULT_DAYS   Global retention, days; 0/unset = keep forever
  HELIOS_RETENTION_SWEEP_SECS     Retention sweep interval, seconds [default: 3600]

Control plane & secrets:
  HELIOS_CONTROL_ENCRYPTION       AES-256-GCM control-file encryption; 0/off/false/no disables [default: on]
  HELIOS_CONTROL_KEY_PATH         32-byte control-key file [default: ./secret-control.json]
  HELIOS_CONTROL_CACHE_TTL_SECS   Control read-cache TTL, seconds; 0 disables [default: 10]
  HELIOS_JWT_SECRET_PATH          JWT signing-secret file [default: ./secret-jwt.json]

Authentication:
  HELIOS_AUTH_TOKEN_TTL_HOURS     Session (JWT) lifetime, hours [default: 168]

First-run admin bootstrap (serve; a set password skips the setup screen):
  HELIOS_ADMIN_USER               Admin username [default: admin]
  HELIOS_ADMIN_PASSWORD           Admin password (enables non-interactive bootstrap)
  HELIOS_ADMIN_EMAIL              Admin email [default: <user>@localhost]
  HELIOS_ADMIN_RESET              Break-glass: reset the admin password on boot (truthy)

Demo mode (serve):
  HELIOS_DEMO_MODE                Read-only demo: reject all mutating APIs for every user (truthy)
  HELIOS_DEMO_LOGIN               Demo account login pre-filled on the sign-in page
  HELIOS_DEMO_PASSWORD            Demo account password pre-filled on the sign-in page

Logging:
  RUST_LOG                        Tracing filter [default: info,hyper=warn]";

#[derive(Parser)]
#[command(
    version,
    about = "Helios block-engine log indexer (partitioned)",
    flatten_help = true,
    after_help = "Run with --help to see the HELIOS_* / RUST_LOG environment variables the server honors.",
    after_long_help = ENV_HELP
)]
struct Cli {
    /// Root directory for partitioned indexes.
    #[arg(long, default_value = "./data", global = true)]
    data_dir: PathBuf,

    /// Verbose diagnostics (e.g. periodic jemalloc memory stats during `serve`).
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Search across all partitions (or a specific index).
    Search {
        /// Query string in the Helios query language.
        query: String,
        /// Restrict to a single index; omit to search every index.
        #[arg(long)]
        index: Option<String>,
        /// Max rows to print.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Describe the catalog: list all partitions with doc + segment counts.
    Describe,
    /// Start the HTTP server (MCP exposed at `POST /mcp`).
    #[command(after_long_help = ENV_HELP)]
    Serve {
        /// HTTP listen port.
        #[arg(long, default_value_t = 7300)]
        port: u16,
        /// Bind address. Defaults to loopback; set `0.0.0.0` to listen on all interfaces (e.g. in Docker).
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Shared store for blocks + manifests: an FS/NFS path or `s3://bucket/prefix`. Omit for single-node local-only.
        #[arg(long)]
        shared_store: Option<String>,
        /// Optional override for built-in frontend SPA directory to serve at /.
        #[arg(long)]
        frontend_dir: Option<PathBuf>,
        /// Override the syslog listener port from the control plane (UDP + TCP) — handy
        /// for running several instances on one host. `0` disables the listener.
        #[arg(long, env = "HELIOS_SYSLOG_PORT")]
        syslog_port: Option<u16>,
        /// Read-only demo mode: reject every mutating API (saved searches, monitors,
        /// dashboards, users, settings, admin actions) for ALL users. Ingest and agent
        /// chat stay open; agent/MCP write-tools are disabled too.
        #[arg(long, env = "HELIOS_DEMO_MODE")]
        demo: bool,
        /// Demo login to pre-fill on the sign-in page (the account must already exist).
        #[arg(long, env = "HELIOS_DEMO_LOGIN")]
        demo_login: Option<String>,
        /// Demo password to pre-fill alongside `--demo-login` (advertised to the public
        /// pre-login page — only use a throwaway demo account).
        #[arg(long, env = "HELIOS_DEMO_PASSWORD")]
        demo_password: Option<String>,
    },
}

fn main() -> Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,hyper=warn".into());

    // `SelfLogsLayer` no-ops until `serve` installs the sender. fmt writer is
    // pinned to stderr: `mcp` uses stdout for the JSON-RPC wire.
    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .compact()
                .with_writer(std::io::stderr),
        )
        .with(self_logs::SelfLogsLayer)
        .init();

    let cli = Cli::parse();

    // One-shot, idempotent storage upgrade: promote pre-env partitions into the
    // env-aware layout (user→`default`, `_*` self-logs→`_system`) before catalog open.
    std::fs::create_dir_all(&cli.data_dir).ok();
    match catalog::migrate_to_env_layout(&cli.data_dir) {
        Ok(moved) if !moved.is_empty() => {
            for (index, env) in &moved {
                tracing::info!(
                    data_dir = %cli.data_dir.display(),
                    %index,
                    %env,
                    "storage: migrated index into env-aware layout"
                );
            }
        }
        Ok(_) => {}
        Err(e) => {
            // Surface but don't abort — the user can re-run after fixing
            // perms / dest collisions.
            tracing::error!("storage: env-layout migration failed: {e}");
        }
    }

    // One-shot legacy migration: move an old ./index/ dir into
    // `data/default/default/<today>/` so previous demo data survives.
    let legacy = PathBuf::from("./index");
    let catalog_for_migration = catalog::Catalog::open(cli.data_dir.clone())?;
    if let Some(moved) = catalog::migrate_legacy_index(&legacy, &catalog_for_migration, "default")?
    {
        // Emitted before `install_sender` runs, so this only lands in
        // stderr (helioslogs.log) — _helioslogs doesn't exist yet either way.
        tracing::info!(
            data_dir = %cli.data_dir.display(),
            env = %moved.env,
            index = %moved.index,
            day = %moved.day_string(),
            "migrated legacy ./index/ into new partition layout"
        );
    }

    match cli.cmd {
        Cmd::Search {
            query,
            index,
            limit,
        } => {
            let catalog = catalog::Catalog::open(cli.data_dir)?;
            let fields = schema::build_schema();
            search::cli_print_search(&catalog, &fields, &query, index.as_deref(), limit)
        }
        Cmd::Describe => {
            let store = engine::block::BlockStore::new(&cli.data_dir);
            let partitions = store.list_partitions()?;
            println!("data dir: {}", cli.data_dir.display());
            println!("today (UTC): {}", Utc::now().date_naive());
            println!("partitions: {}", partitions.len());
            for k in &partitions {
                let blocks = store.open_blocks(k)?;
                let docs: u64 = blocks.iter().map(|b| b.row_count() as u64).sum();
                println!(
                    "  {}/{}/{}  docs={:>6}  blocks={}",
                    k.env,
                    k.index,
                    k.day_string(),
                    docs,
                    blocks.len()
                );
            }
            Ok(())
        }
        Cmd::Serve {
            port,
            host,
            shared_store,
            frontend_dir,
            syslog_port,
            demo,
            demo_login,
            demo_password,
        } => {
            if let Some(dir) = &frontend_dir {
                if !dir.join("index.html").is_file() {
                    anyhow::bail!(
                        "--frontend-dir {} does not contain index.html",
                        dir.display()
                    );
                }
            }
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(http::serve(
                cli.data_dir,
                host,
                port,
                frontend_dir,
                shared_store,
                cli.verbose,
                syslog_port,
                http::DemoConfig::new(demo, demo_login, demo_password),
            ))
        }
    }
}
