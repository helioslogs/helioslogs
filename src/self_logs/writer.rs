// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Background task draining [`SelfLogEvent`]s and batch-writing them into the
//! `_system` self-log partitions (flush on MAX_BATCH or FLUSH_INTERVAL). Errors
//! are `eprintln!`'d (not `tracing::error!`, which would re-enter our own layer).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::catalog::{Catalog, PartitionKey, SYSTEM_ENV};
use crate::engine::block::{configured_store, BlockWriter};
use crate::engine::block_codec;
use crate::indexer::ingest::json_to_row;

use super::{NodeInfo, SelfLogEvent};

/// Max events to buffer before a forced flush — bounds p95 flush size under burst.
const MAX_BATCH: usize = 500;

/// Time-based flush. Two seconds keeps live logs visible in the UI
/// without us spinning the writer when the system is idle.
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);

pub async fn run_writer(
    catalog: Catalog,
    node: NodeInfo,
    mut rx: mpsc::UnboundedReceiver<SelfLogEvent>,
) {
    // Stage raw event JSON per `(env, index, day)` partition (the day dimension
    // catches events straddling midnight); convert to blocks at flush time.
    let mut staging: HashMap<PartitionKey, Vec<Value>> = HashMap::new();
    let mut buffered = 0usize;
    let mut last_flush = Instant::now();
    // Precomputed once — stamped onto every doc so each self-log line carries
    // its originating node (`node_id` / `node_host` / `node_port`).
    let node_fields = node.fields();

    loop {
        let timeout = FLUSH_INTERVAL.saturating_sub(last_flush.elapsed());
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(mut ev)) => {
                if let Value::Object(map) = &mut ev.doc {
                    for (k, v) in &node_fields {
                        map.entry(*k).or_insert_with(|| v.clone());
                    }
                }
                let day = Utc::now().date_naive();
                let key = PartitionKey::new(SYSTEM_ENV, ev.index, day);
                staging.entry(key).or_default().push(ev.doc);
                buffered += 1;
                if buffered >= MAX_BATCH {
                    flush(&catalog, &mut staging).await;
                    buffered = 0;
                    last_flush = Instant::now();
                }
            }
            Ok(None) => {
                // Channel closed (shutdown): final flush, then exit.
                flush(&catalog, &mut staging).await;
                break;
            }
            Err(_) => {
                // Timeout fired — periodic flush even when below MAX_BATCH.
                if buffered > 0 {
                    flush(&catalog, &mut staging).await;
                    buffered = 0;
                }
                last_flush = Instant::now();
            }
        }
    }
}

async fn flush(catalog: &Catalog, staging: &mut HashMap<PartitionKey, Vec<Value>>) {
    if staging.is_empty() {
        return;
    }
    let batch: Vec<(PartitionKey, Vec<Value>)> = staging.drain().collect();

    // One immutable block per partition per flush (already rate-capped by the
    // batch/interval). Encode + write off the reactor.
    let store = configured_store(catalog.root());
    let codec = block_codec();
    let _ = tokio::task::spawn_blocking(move || {
        for (key, events) in batch {
            let mut w = BlockWriter::new(codec);
            for ev in &events {
                if let Ok(row) = json_to_row(ev, None) {
                    w.push(row);
                }
            }
            if w.is_empty() {
                continue;
            }
            match w.finish() {
                Ok(bytes) => {
                    if let Err(e) = store.append_block(&key, &bytes) {
                        eprintln!("self_logs block: append {key:?}: {e}");
                    }
                }
                Err(e) => eprintln!("self_logs block: build {key:?}: {e}"),
            }
        }
    })
    .await;
}
