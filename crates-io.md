# HeliosLogs

[![crates.io](https://img.shields.io/crates/v/helioslogs.svg)](https://crates.io/crates/helioslogs)
[![downloads](https://img.shields.io/crates/d/helioslogs.svg)](https://crates.io/crates/helioslogs)
[![license](https://img.shields.io/crates/l/helioslogs.svg)](https://github.com/helioslogs/helioslogs/blob/main/LICENSE.md)
![MSRV](https://img.shields.io/badge/rustc-1.91.1+-blue.svg)

**A fast, modern log search engine with analytics & AI — one self-contained binary.**

HeliosLogs ingests newline-delimited JSON (or syslog), makes every field instantly
queryable via **schema-on-read**, and ships a pipelined query language, dashboards,
alerting, an AI investigation agent, and a built-in MCP server. The web UI is embedded in
the binary — there is nothing else to deploy.

> **This is an application crate, not a library.** Install it with `cargo install` and run
> the `helioslogs` server. It exposes no stable public Rust API; don't add it as a
> dependency.

## Install

```bash
cargo install helioslogs
```

This builds a single self-contained binary with the frontend embedded (release builds bake
in `frontend/dist` via [`rust-embed`](https://crates.io/crates/rust-embed)). Then:

```bash
helioslogs serve --port 7300 --data-dir ./data
# → open http://localhost:7300  (first visitor creates the admin account)
```

### Common run options

Durable shared store — blocks, manifests, and the control plane live on a shared
filesystem/NFS path or in S3; `--data-dir` becomes a rebuildable per-node cache:

```bash
# Filesystem / NFS path
helioslogs serve --port 7300 --data-dir ./cache \
  --shared-store /mnt/helios-shared

# S3 (region is resolved from env/profile only — AWS_REGION is required)
AWS_REGION=us-east-1 helioslogs serve --port 7300 --data-dir ./cache \
  --shared-store s3://my-bucket/helios
```

Syslog listener — enable it under **Admin → Data Ingestion → Syslog**, then override its
port (UDP + TCP, via `--syslog-port` / `HELIOS_SYSLOG_PORT`; `0` disables it):

```bash
helioslogs serve --port 7300 --data-dir ./data \
  --syslog-port 5514
```

Prefer a container or a native OS package? See the
[Quickstart](https://docs.helioslogs.com/start/quickstart) for the Docker image
(`helioslogs/helioslogs`) and the `brew`/`apt`/`dnf`/`apk` installers.

## Build prerequisites

The crypto stack ([`aws-lc-rs`](https://crates.io/crates/aws-lc-rs)) and `zstd` compile
from C, so a build host needs:

- A C/C++ toolchain, **CMake**, **Clang/libclang**, and **Perl**
- For the `fips` feature, additionally **Go** (builds the AWS-LC FIPS 140-3 module)

`cargo install` / `cargo build --release` will fail fast on a toolchain older than the
MSRV (**Rust 1.91.1**, floored by the aws-sdk/smithy crates).

## Cargo features

| Feature | Default | Effect |
|---|:---:|---|
| `fips` | off | Build all crypto (incl. TLS) against the AWS-LC **FIPS 140-3** validated module. |

```bash
cargo install helioslogs --features fips
```

## Build from source

```bash
git clone https://github.com/helioslogs/helioslogs
cd helioslogs
cargo build --release      # embeds the committed frontend/dist
./target/release/helioslogs serve --port 7300 --data-dir ./data
```

## 60-second tour

Send events — no schema to declare first:

```bash
curl -X POST 'http://localhost:7300/api/ingest?env=default&index=adhoc' \
  --data-binary @- <<'JSON'
{"timestamp":"2026-06-14T18:00:00Z","level":"INFO","service":"web","message":"hello"}
{"timestamp":"2026-06-14T18:00:01Z","level":"ERROR","service":"web","message":"boom","status":504}
JSON
# {"ingested":2,"errors":0}
```

Then on the **Search** page, `level`, `service`, and `status` are all queryable
immediately:

```
level:ERROR | stats count by service
```

## What's inside

- **Ingest anything** — NDJSON over HTTP, Syslog, and drop-in compatibility APIs.
- **A query language for logs** — full-text + fields, with pipeline operators (`| stats`, `| top`, …).
- **Dashboards & monitoring** — widgets, threshold and AI monitors, alerts to webhooks/Slack.
- **An AI agent that investigates** — describe an incident in plain English; it searches, aggregates, and charts to explain it.
- **Built-in MCP server** — the same toolset over one HTTP endpoint for Claude Desktop, Claude Code, Cursor, or any MCP client.
- **Enterprise-ready** — RBAC, SAML SSO, scoped API keys, an encrypted control plane, an optional S3/NFS shared store with DR, and the FIPS build.

A custom columnar block engine (immutable `.hb` blocks behind a CAS manifest) does the
storage; there is no external database.

## Links

- **Docs** — <https://docs.helioslogs.com>
- **Query language** — <https://docs.helioslogs.com/search/query-language>
- **Ingestion** — <https://docs.helioslogs.com/ingest/overview>
- **AI / LLM agent** — <https://docs.helioslogs.com/ai/agent-setup>
- **Website** — <https://www.helioslogs.com>
- **Source** — <https://github.com/helioslogs/helioslogs>

## License

`AGPL-3.0-only OR LicenseRef-Appbird-Commercial` — open source under the AGPL, or a
commercial license from Appbird LLC. © 2026 Appbird LLC.
