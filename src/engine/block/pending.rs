// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Per-node persistent pending-upload set: block ids this node ingested but
//! hasn't confirmed in shared. The ownership signal the sync tasks need to tell
//! "owed to shared" from "compacted away". One JSON per partition; lock + rename.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::catalog::PartitionKey;

#[derive(Default, Serialize, Deserialize)]
struct PendingFile {
    blocks: Vec<String>,
}

/// Persistent owned/pending-upload tracking, rooted at the local data dir. One per
/// node, shared via `Arc` by the ingest writer and sync tasks so they share the lock.
pub struct PendingStore {
    root: PathBuf,
    lock: Mutex<()>,
}

impl PendingStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lock: Mutex::new(()),
        }
    }

    fn path(&self, key: &PartitionKey) -> PathBuf {
        self.root
            .join(&key.env)
            .join(&key.index)
            .join(key.day_string())
            .join("pending.json")
    }

    fn read(path: &Path) -> Vec<String> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<PendingFile>(&bytes)
                .map(|p| p.blocks)
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    fn write(path: &Path, ids: &[String]) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_file_name("pending.json.tmp");
        let body = serde_json::to_vec_pretty(&PendingFile {
            blocks: ids.to_vec(),
        })
        .unwrap_or_default();
        fs::write(&tmp, &body)?;
        fs::rename(&tmp, path)
    }

    /// Mark a freshly-ingested block as owed to shared, before it lands in the local
    /// manifest — so the puller never mistakes a manifest entry for compacted-away data.
    pub fn add(&self, key: &PartitionKey, id: &str) {
        let _g = self.lock.lock().unwrap();
        let path = self.path(key);
        let mut ids = Self::read(&path);
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
            if let Err(e) = Self::write(&path, &ids) {
                eprintln!("pending: add {id}: {e}");
            }
        }
    }

    /// The ids this node still owes the shared store for a partition.
    pub fn load(&self, key: &PartitionKey) -> Vec<String> {
        let _g = self.lock.lock().unwrap();
        Self::read(&self.path(key))
    }

    /// Drop ids that are no longer pending (landed in shared, or unrecoverable).
    pub fn remove(&self, key: &PartitionKey, done: &[String]) {
        if done.is_empty() {
            return;
        }
        let _g = self.lock.lock().unwrap();
        let path = self.path(key);
        let drop: BTreeSet<&str> = done.iter().map(String::as_str).collect();
        let mut ids = Self::read(&path);
        let before = ids.len();
        ids.retain(|x| !drop.contains(x.as_str()));
        if ids.len() == before {
            return;
        }
        if ids.is_empty() {
            let _ = fs::remove_file(&path); // tidy — no empty pending files left behind
        } else if let Err(e) = Self::write(&path, &ids) {
            eprintln!("pending: remove: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use tempfile::TempDir;

    fn key() -> PartitionKey {
        PartitionKey::new(
            "default",
            "orders",
            NaiveDate::from_ymd_opt(2026, 5, 30).unwrap(),
        )
    }

    #[test]
    fn add_load_remove_roundtrip() {
        let dir = TempDir::new().unwrap();
        let p = PendingStore::new(dir.path());
        let k = key();
        assert!(p.load(&k).is_empty());
        p.add(&k, "a");
        p.add(&k, "b");
        p.add(&k, "a"); // idempotent
        assert_eq!(p.load(&k), vec!["a".to_string(), "b".to_string()]);
        p.remove(&k, &["a".to_string()]);
        assert_eq!(p.load(&k), vec!["b".to_string()]);
        p.remove(&k, &["b".to_string()]);
        assert!(p.load(&k).is_empty());
    }

    #[test]
    fn survives_reopen() {
        let dir = TempDir::new().unwrap();
        let k = key();
        PendingStore::new(dir.path()).add(&k, "persisted");
        // A fresh instance (simulating a restart) sees the persisted id.
        assert_eq!(
            PendingStore::new(dir.path()).load(&k),
            vec!["persisted".to_string()]
        );
    }
}
