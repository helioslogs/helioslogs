# Helios Logs README

Welcome to HeliosLogs!

HeliosLogs is a modern log search tool with analytics & AI.

## Components

`src` - helioslogs server

`frontend` - search & admin ui

## Development Build

```bash
# rebuild frontend
cd frontend && npm install && npm run build && cd ..

#rebuild backend
cargo build --release

# start backend with frontend 
./target/release/helios --data-dir ./data serve --port 7300 --frontend-dir ./frontend/dist
#  listening on http://127.0.0.1:7300

# start frontend in dev mode
cd frontend && npm run dev
```

## Documentation

Quickstart - https://docs.helioslogs.com/start/quickstart

Data Ingestion - https://docs.helioslogs.com/ingest/overview

Query Language - https://docs.helioslogs.com/search/query-language

AI/LLM Agent - https://docs.helioslogs.com/ai/agent-setup
