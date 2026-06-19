# HeliosLogs README

Welcome to HeliosLogs!

HeliosLogs is a modern log search tool with analytics & AI.

![HeliosLogs search and AI investigate UI](search-investigate-ui.png)

## Components

`src` - helioslogs server

`frontend` - search & admin ui

## Quickstart (Docker)

The fastest way to run HeliosLogs is the published image on Docker Hub:

```bash
docker pull helioslogs/helioslogs:latest

docker run -p 7300:7300 \
  -v helios-data:/app/data \
  -v helios-secret:/app/secret \
  helioslogs/helioslogs:latest

#  listening on http://localhost:7300
```

## Build from source

```bash
# build (embeds the committed frontend/dist into the binary)
cargo build --release

# start — the UI is served from the embedded bundle
./target/release/helioslogs --data-dir ./data serve --port 7300
#  listening on http://127.0.0.1:7300
```

## Documentation

Quickstart - https://docs.helioslogs.com/start/quickstart

Data Ingestion - https://docs.helioslogs.com/ingest/overview

Query Language - https://docs.helioslogs.com/search/query-language

AI/LLM Agent - https://docs.helioslogs.com/ai/agent-setup
