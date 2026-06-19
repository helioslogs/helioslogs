// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! [`BlockEngine`] — the sole [`PartitionEngine`], backed by the block store.
//! Reads route through [`super::query`]; writes append one immutable block.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::catalog::PartitionKey;
use crate::engine::{FieldBuckets, PartitionEngine, TimeRange};
use crate::indexer::ingest::json_to_row;
use crate::search::query::Node;
use crate::search::Hit;

use super::query as bq;
use super::store::BlockStore;
use super::{Block, BlockWriter, Codec};

pub struct BlockEngine {
    store: BlockStore,
    codec: Codec,
}

impl BlockEngine {
    /// Filesystem-rooted engine (tests + default local layout).
    pub fn new(root: impl Into<PathBuf>, codec: Codec) -> Self {
        Self {
            store: BlockStore::new(root),
            codec,
        }
    }

    /// Engine over an already-built store (e.g. the configured shared store).
    pub fn with_store(store: BlockStore, codec: Codec) -> Self {
        Self { store, codec }
    }

    /// Build one block from `events` (assumed to belong to `key`'s day) and append it.
    /// Returns `(ingested, parse_errors)`.
    pub fn ingest(
        &self,
        key: &PartitionKey,
        events: &[Value],
        default_source: Option<&str>,
    ) -> Result<(usize, usize)> {
        let mut writer = BlockWriter::new(self.codec);
        let mut parse_errors = 0usize;
        for ev in events {
            match json_to_row(ev, default_source) {
                Ok(row) => writer.push(row),
                Err(_) => parse_errors += 1,
            }
        }
        let ingested = writer.len();
        if ingested > 0 {
            let bytes = writer.finish()?;
            self.store.append_block(key, &bytes)?;
        }
        Ok((ingested, parse_errors))
    }
}

fn partition_label(key: &PartitionKey) -> String {
    format!("{}/{}", key.index, key.day_string())
}

fn to_millis(t: TimeRange) -> (Option<i64>, Option<i64>) {
    let ms = |d: DateTime<Utc>| d.timestamp_millis();
    (t.start.map(ms), t.end.map(ms))
}

impl PartitionEngine for BlockEngine {
    fn search(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        limit: usize,
    ) -> Result<(u64, Vec<(i64, Hit)>)> {
        let (start, end) = to_millis(time);
        let (count, mut hits) = bq::search(&self.store, key, filter, start, end, limit)?;
        let label = partition_label(key);
        for (_, h) in &mut hits {
            h.partition = label.clone();
        }
        Ok((count, hits))
    }

    fn histogram(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        interval: &str,
    ) -> Result<BTreeMap<i64, u64>> {
        let Some(interval_ms) = bq::parse_interval_ms(interval) else {
            return Ok(BTreeMap::new());
        };
        let (start, end) = to_millis(time);
        bq::histogram(&self.store, key, filter, start, end, interval_ms)
    }

    fn search_histogram(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        limit: usize,
        interval: &str,
    ) -> Result<(u64, Vec<(i64, Hit)>, BTreeMap<i64, u64>)> {
        let (start, end) = to_millis(time);
        let interval_ms = bq::parse_interval_ms(interval).unwrap_or(0);
        let (count, mut hits, buckets) =
            bq::search_histogram(&self.store, key, filter, start, end, limit, interval_ms)?;
        let label = partition_label(key);
        for (_, h) in &mut hits {
            h.partition = label.clone();
        }
        Ok((count, hits, buckets))
    }

    fn terms(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        fields: &[String],
        size_each: u32,
        want_count: bool,
    ) -> Result<(u64, FieldBuckets)> {
        let (start, end) = to_millis(time);
        bq::terms(
            &self.store,
            key,
            filter,
            start,
            end,
            fields,
            size_each as usize,
            want_count,
        )
    }

    fn partition_stats(&self, key: &PartitionKey) -> Result<Option<(u64, usize)>> {
        if !self.store.partition_exists(key) {
            return Ok(None);
        }
        // Stream block footers (row counts) without holding the whole partition.
        let ids = self.store.load_manifest(key)?.blocks;
        let mut docs = 0u64;
        let mut segments = 0usize;
        for id in &ids {
            if let Ok(bytes) = self.store.read_block(key, id) {
                docs += Block::open(bytes)?.row_count() as u64;
                segments += 1;
            }
        }
        Ok(Some((docs, segments)))
    }

    fn scan(
        &self,
        key: &PartitionKey,
        filter: Option<&Node>,
        time: TimeRange,
        limit: usize,
    ) -> Result<(u64, Vec<Hit>)> {
        let (start, end) = to_millis(time);
        let (count, mut hits) = bq::scan(&self.store, key, filter, start, end, limit)?;
        let label = partition_label(key);
        for h in &mut hits {
            h.partition = label.clone();
        }
        Ok((count, hits))
    }
}
