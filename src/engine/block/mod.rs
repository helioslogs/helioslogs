// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! The **block** — Helios's immutable columnar storage unit (README.md, "Block
//! storage engine"): time-sorted rows, term indexes, and a section-mapping footer.
#![allow(dead_code, unused_imports)]

mod cache;
mod codec;
mod compact;
mod engine;
mod ingest;
mod objstore;
mod pending;
mod query;
mod reader;
mod store;
mod sync;
mod writer;

pub use cache::{configure_query_cache, resize_query_cache};
pub use compact::{run_compactor, AlwaysCompact, CompactionGate};
pub use engine::BlockEngine;
pub use ingest::{
    install_sender, queue_capacity, run_writer, submit, submit_blocking, BlockIngestEvent,
    SubmitResult,
};
pub use objstore::{build_object_store, ObjectStore};
pub(crate) use objstore::{s3_client_and_loc, s3_detail};
pub use pending::PendingStore;
pub use reader::{footer_field_stats, Block, BlockFieldStat, BlockFieldStats, FieldKind};
pub use store::{build_block_setup, BlockSetup, BlockStore, Manifest, SyncPair};
pub use sync::{run_puller, run_seeder, run_uploader};
pub use writer::BlockWriter;

use std::sync::{Arc, OnceLock};

/// The block store for this instance, installed at startup; shared via [`configured_store`].
static CONFIGURED_STORE: OnceLock<BlockStore> = OnceLock::new();

pub fn install_store(store: BlockStore) {
    let _ = CONFIGURED_STORE.set(store);
}

/// Pending-upload tracker, installed only with a shared store so every owned write records its block as pending; `None` local-only.
static CONFIGURED_PENDING: OnceLock<Arc<PendingStore>> = OnceLock::new();

pub fn install_pending(pending: Arc<PendingStore>) {
    let _ = CONFIGURED_PENDING.set(pending);
}

pub fn configured_pending() -> Option<Arc<PendingStore>> {
    CONFIGURED_PENDING.get().cloned()
}

/// The configured block store, or a filesystem store at `fallback_root` if none
/// was installed (tests, or block mode without `--shared-store`).
pub fn configured_store(fallback_root: &std::path::Path) -> BlockStore {
    CONFIGURED_STORE
        .get()
        .cloned()
        .unwrap_or_else(|| BlockStore::new(fallback_root))
}

use serde::{Deserialize, Serialize};

/// On-disk format version. v2 adds per-column `rows`/`cardinality` so the field catalog is footer-derivable; v1 lacks them (serde defaults to 0).
pub const FORMAT_VERSION: u16 = 2;

/// The lowest on-disk version this build can still read.
pub const MIN_READABLE_VERSION: u16 = 1;

/// Leading + trailing magic so a truncated/foreign object is rejected fast.
pub const MAGIC: &[u8; 4] = b"HBLK";

/// The shred type of a dynamic value. A logical path splits into one physical column
/// per `(path, type)` — `amount::f64` and `amount::str` coexist over disjoint row sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LogType {
    I64,
    F64,
    Bool,
    Str,
}

/// A typed dynamic value shredded to its physical type. The block format never sees
/// raw JSON except the stored `raw` text; the ingest adapter maps `serde_json::Value` → this.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
}

impl FieldValue {
    pub fn log_type(&self) -> LogType {
        match self {
            FieldValue::I64(_) => LogType::I64,
            FieldValue::F64(_) => LogType::F64,
            FieldValue::Bool(_) => LogType::Bool,
            FieldValue::Str(_) => LogType::Str,
        }
    }
}

/// One event as the block writer consumes it: `ts_millis` sort key, universal-core
/// display columns, and shredded dynamic `(path, value)` pairs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub ts_millis: i64,
    pub message: Option<String>,
    pub source: Option<String>,
    pub raw: Option<String>,
    pub fields: Vec<(String, FieldValue)>,
}

/// Per-section compression codec, recorded in the footer so `none`/`zstd` blocks
/// interoperate and flipping the flag never invalidates existing blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    None,
    Zstd,
}

/// Locates one serialized section within the block body: byte range, codec, and
/// the pre-compression length needed to size the decode buffer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionRef {
    pub offset: u64,
    pub stored_len: u32,
    pub raw_len: u32,
    pub codec: Codec,
}

/// A term index for one field: bloom (block-skip), sorted dict, and dict-aligned roaring
/// postings. `positions` is present only for tokenized fields, carrying offsets for phrase adjacency.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TermIndexDir {
    pub field: String,
    pub bloom: SectionRef,
    pub dict: SectionRef,
    pub postings: SectionRef,
    pub positions: Option<SectionRef>,
}

/// A numeric/bool value column for one dynamic `(path, type)`: present-bitmap + encoded
/// values with min/max for range pruning. Strings route through [`TermIndexDir`] instead.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValueColumnDir {
    pub path: String,
    pub ty: LogType,
    pub present: SectionRef,
    pub values: SectionRef,
    pub min: f64,
    pub max: f64,
    /// Rows carrying this typed value (= present cardinality); 0 on v1 blocks.
    #[serde(default)]
    pub rows: u32,
}

/// A string value column for one dynamic `(path, Str)`: the untokenized, original-case
/// value (what terms-agg groups on), distinct from the tokenized [`TermIndexDir`] for filtering.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrColumnDir {
    pub path: String,
    pub present: SectionRef,
    pub dict: SectionRef,
    pub ids: SectionRef,
    /// Rows carrying this string value (= present cardinality); 0 on v1 blocks.
    #[serde(default)]
    pub rows: u32,
    /// Distinct values in this block (= dictionary size). 0 on legacy v1 blocks.
    #[serde(default)]
    pub cardinality: u32,
}

/// The block footer: small, written last, read first. Tells a reader the row
/// count, time bounds (for block pruning), and every section's location.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Footer {
    pub format_version: u16,
    pub row_count: u32,
    pub min_ts: i64,
    pub max_ts: i64,
    pub timestamp: SectionRef,
    pub message: Option<SectionRef>,
    pub source: Option<SectionRef>,
    pub raw: Option<SectionRef>,
    pub term_indexes: Vec<TermIndexDir>,
    pub value_columns: Vec<ValueColumnDir>,
    pub str_columns: Vec<StrColumnDir>,
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod engine_tests;

#[cfg(test)]
mod bench;

#[cfg(test)]
mod bench_tests;
