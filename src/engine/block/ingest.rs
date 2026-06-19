// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Buffered block ingest: a background task batches rows per partition and
//! flushes on a row-count or time threshold (avoids one tiny block per call).
//! Buffered rows are in-memory, so a hard kill loses up to one flush window.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::catalog::PartitionKey;

use super::store::BlockStore;
use super::{BlockWriter, Codec, Row};

/// One row destined for a partition's buffer.
pub struct BlockIngestEvent {
    pub key: PartitionKey,
    pub row: Row,
}

/// Outcome of a non-blocking [`submit`]. `Full` is the backpressure signal — the HTTP
/// layer maps it to 429; file sources use [`submit_blocking`] and wait instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitResult {
    Accepted,
    Full,
    NoWriter,
}

/// Channel depth — bounded so a firehose becomes backpressure, not unbounded memory.
const DEFAULT_QUEUE_CAP: usize = 100_000;

pub fn queue_capacity() -> usize {
    std::env::var("HELIOS_BLOCK_QUEUE_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_QUEUE_CAP)
}

static SENDER: OnceLock<Sender<BlockIngestEvent>> = OnceLock::new();

/// Install the writer's channel sender. Called once at serve startup when the
/// block engine is active.
pub fn install_sender(tx: Sender<BlockIngestEvent>) {
    let _ = SENDER.set(tx);
}

/// Hand a row to the buffered writer without blocking. `Full` when the queue is
/// saturated (caller sheds load); `NoWriter` if the writer task isn't running.
pub fn submit(key: PartitionKey, row: Row) -> SubmitResult {
    match SENDER.get() {
        Some(tx) => match tx.try_send(BlockIngestEvent { key, row }) {
            Ok(()) => SubmitResult::Accepted,
            Err(TrySendError::Full(_)) => SubmitResult::Full,
            Err(TrySendError::Closed(_)) => SubmitResult::NoWriter,
        },
        None => SubmitResult::NoWriter,
    }
}

/// Blocking variant for sync producers (file sources) — waits for queue room instead
/// of dropping. MUST be called from a blocking thread, never an async task.
pub fn submit_blocking(key: PartitionKey, row: Row) -> bool {
    match SENDER.get() {
        Some(tx) => tx.blocking_send(BlockIngestEvent { key, row }).is_ok(),
        None => false,
    }
}

/// Max concurrent flushes — caps the buffered-memory ceiling (~`concurrency × flush_rows`),
/// since each in-flight flush builds a full block (term indexes included) in memory. Kept
/// low so a bulk-ingest burst doesn't stack several block builds at once.
const DEFAULT_FLUSH_CONCURRENCY: usize = 2;

fn flush_concurrency() -> usize {
    std::env::var("HELIOS_BLOCK_FLUSH_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_FLUSH_CONCURRENCY)
}

/// Drain `rx`, buffering rows per partition; flush on the row/time threshold (or a final
/// flush on channel close). Flushes run on background tasks so encoding never stalls intake.
pub async fn run_writer(store: BlockStore, codec: Codec, mut rx: Receiver<BlockIngestEvent>) {
    let slots = Arc::new(Semaphore::new(flush_concurrency()));
    let mut tasks: JoinSet<()> = JoinSet::new();
    let mut staging: HashMap<PartitionKey, Vec<Row>> = HashMap::new();
    let mut buffered = 0usize;
    let mut last_flush = Instant::now();

    loop {
        // Re-read the flush thresholds each pass so Admin → General edits apply live.
        let max_rows = crate::runtime_config::block_flush_rows();
        let interval = crate::runtime_config::block_flush_interval();
        // Fast path: drain everything available via timer-free `try_recv`. A per-event
        // `timeout` would register a timer each event and throttle intake below bulk-push rate.
        let mut drained = 0usize;
        while let Ok(ev) = rx.try_recv() {
            staging.entry(ev.key).or_default().push(ev.row);
            buffered += 1;
            drained += 1;
            if buffered >= max_rows {
                spawn_flush(&store, codec, &mut staging, &slots, &mut tasks).await;
                buffered = 0;
                last_flush = Instant::now();
            }
        }
        while tasks.try_join_next().is_some() {}

        if drained == 0 {
            // Idle: block for the next event, bounded by the flush interval so a
            // trickle still lands within `flush_secs`. Per-event timer cost is paid only here.
            let remaining = interval.saturating_sub(last_flush.elapsed());
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(ev)) => {
                    staging.entry(ev.key).or_default().push(ev.row);
                    buffered += 1;
                }
                Ok(None) => {
                    // Channel closed (shutdown): final flush, then exit.
                    spawn_flush(&store, codec, &mut staging, &slots, &mut tasks).await;
                    break;
                }
                Err(_) => {
                    if buffered > 0 {
                        spawn_flush(&store, codec, &mut staging, &slots, &mut tasks).await;
                        buffered = 0;
                    }
                    last_flush = Instant::now();
                }
            }
        } else if buffered > 0 && last_flush.elapsed() >= interval {
            // Under sustained load, still honor the time-based flush.
            spawn_flush(&store, codec, &mut staging, &slots, &mut tasks).await;
            buffered = 0;
            last_flush = Instant::now();
        }
    }
    // Drain in-flight flushes before exit so a clean shutdown loses nothing.
    while tasks.join_next().await.is_some() {}
}

/// Drain `staging` and flush on a background task, keeping the recv loop free. The
/// permit acquire only blocks when every flush slot is busy — the genuine-saturation case.
async fn spawn_flush(
    store: &BlockStore,
    codec: Codec,
    staging: &mut HashMap<PartitionKey, Vec<Row>>,
    slots: &Arc<Semaphore>,
    tasks: &mut JoinSet<()>,
) {
    if staging.is_empty() {
        return;
    }
    let batch: Vec<(PartitionKey, Vec<Row>)> = staging.drain().collect();
    let permit = slots
        .clone()
        .acquire_owned()
        .await
        .expect("flush semaphore never closed");
    let store = store.clone();
    tasks.spawn(async move {
        let _permit = permit; // held until the flush completes, freeing a slot
                              // Block encode + fs write can be tens of MB; keep it off the reactor.
        let _ = tokio::task::spawn_blocking(move || flush_batch(&store, codec, batch)).await;
    });
}

fn flush_batch(store: &BlockStore, codec: Codec, batch: Vec<(PartitionKey, Vec<Row>)>) {
    for (key, rows) in batch {
        if rows.is_empty() {
            continue;
        }
        let mut w = BlockWriter::new(codec);
        for r in rows {
            w.push(r);
        }
        match w.finish() {
            // `append_block` records the block as pending-upload (with a shared store)
            // before the manifest — same as the self-log and MCP ingest paths.
            Ok(bytes) => {
                if let Err(e) = store.append_block(&key, &bytes) {
                    eprintln!("block ingest: append {key:?}: {e}");
                }
            }
            Err(e) => eprintln!("block ingest: build {key:?}: {e}"),
        }
    }
}
