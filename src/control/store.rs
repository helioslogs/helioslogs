// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Async object store for the JSON control plane with full version-CAS (write-iff-
//! unchanged on a stable key). Two backends: [`FsControlStore`] (content-hash
//! version, lock-serialized) and [`S3ControlStore`] (ETag, conditional `PutObject`).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::engine::block::{s3_client_and_loc, s3_detail};

/// Opaque per-object version (S3 ETag, or a content hash on the filesystem),
/// compared for equality only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(pub String);

/// Outcome of a conditional write: the new version, or a lost CAS race.
pub enum CasResult {
    /// Written; carries the new version (used by tests / future version chaining).
    Written(#[allow(dead_code)] Version),
    Conflict,
}

/// Minimal async store the control plane needs. `Send + Sync` for sharing across
/// axum handlers and background tasks.
#[async_trait]
pub trait ControlStore: Send + Sync {
    /// Current bytes + version, or `None` if absent.
    async fn get_versioned(&self, key: &str) -> Result<Option<(Vec<u8>, Version)>>;
    /// Write iff the current version equals `expected` (`None` = "must not exist").
    /// `Conflict` on precondition failure — the caller re-reads and retries.
    async fn put_if_version(
        &self,
        key: &str,
        bytes: &[u8],
        expected: Option<&Version>,
    ) -> Result<CasResult>;
    async fn delete(&self, key: &str) -> Result<()>;
    /// Full keys (store-relative) under `prefix`.
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;
    fn describe(&self) -> String;
}

/// Content hash → version token (filesystem backend).
fn hash_version(bytes: &[u8]) -> Version {
    Version(crate::crypto::digest::sha256_hex(bytes))
}

// ---- filesystem backend -----------------------------------------------------

pub struct FsControlStore {
    root: PathBuf,
    /// Serializes CAS writes in-process (single-host only, D2).
    write_lock: Mutex<()>,
}

impl FsControlStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            write_lock: Mutex::new(()),
        }
    }

    fn path(&self, key: &str) -> PathBuf {
        let mut p = self.root.clone();
        for seg in key.split('/').filter(|s| !s.is_empty()) {
            p.push(seg);
        }
        p
    }

    async fn current_version(&self, key: &str) -> Result<Option<Version>> {
        match tokio::fs::read(self.path(key)).await {
            Ok(b) => Ok(Some(hash_version(&b))),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {key}")),
        }
    }
}

#[async_trait]
impl ControlStore for FsControlStore {
    async fn get_versioned(&self, key: &str) -> Result<Option<(Vec<u8>, Version)>> {
        match tokio::fs::read(self.path(key)).await {
            Ok(bytes) => {
                let v = hash_version(&bytes);
                Ok(Some((bytes, v)))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {key}")),
        }
    }

    async fn put_if_version(
        &self,
        key: &str,
        bytes: &[u8],
        expected: Option<&Version>,
    ) -> Result<CasResult> {
        let _guard = self.write_lock.lock().await;
        // Re-read under the lock — this is the atomic check-and-set point.
        if self.current_version(key).await?.as_ref() != expected {
            return Ok(CasResult::Conflict);
        }
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let tmp = path.with_extension(format!("tmp.{}", Uuid::new_v4()));
        tokio::fs::write(&tmp, bytes)
            .await
            .with_context(|| format!("writing temp {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .with_context(|| format!("renaming into {}", path.display()))?;
        Ok(CasResult::Written(hash_version(bytes)))
    }

    async fn delete(&self, key: &str) -> Result<()> {
        match tokio::fs::remove_file(self.path(key)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("deleting {key}")),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let base = self.path(prefix);
        let mut out = Vec::new();
        let mut stack = vec![base];
        while let Some(dir) = stack.pop() {
            let mut rd = match tokio::fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(e) if e.kind() == ErrorKind::NotFound => continue,
                Err(e) => return Err(e).with_context(|| format!("listing {}", dir.display())),
            };
            while let Some(entry) = rd.next_entry().await? {
                let path = entry.path();
                if entry.file_type().await?.is_dir() {
                    stack.push(path);
                } else if let Ok(rel) = path.strip_prefix(&self.root) {
                    let key: Vec<String> = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect();
                    out.push(key.join("/"));
                }
            }
        }
        Ok(out)
    }

    fn describe(&self) -> String {
        format!("filesystem {}", self.root.display())
    }
}

// ---- S3 backend -------------------------------------------------------------

pub struct S3ControlStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    /// Key prefix within the bucket, normalized to end with `/`.
    prefix: String,
}

impl S3ControlStore {
    fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }
}

#[async_trait]
impl ControlStore for S3ControlStore {
    async fn get_versioned(&self, key: &str) -> Result<Option<(Vec<u8>, Version)>> {
        let full = self.full_key(key);
        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&full)
            .send()
            .await
        {
            Ok(resp) => {
                let etag = resp.e_tag().unwrap_or_default().to_string();
                let data = resp
                    .body
                    .collect()
                    .await
                    .with_context(|| format!("s3 read body {full}"))?
                    .into_bytes()
                    .to_vec();
                Ok(Some((data, Version(etag))))
            }
            Err(e) => {
                let status = e.raw_response().map(|r| r.status().as_u16());
                if status == Some(404) {
                    Ok(None)
                } else {
                    Err(anyhow!("s3 get {full}: {}", s3_detail(&e)))
                }
            }
        }
    }

    async fn put_if_version(
        &self,
        key: &str,
        bytes: &[u8],
        expected: Option<&Version>,
    ) -> Result<CasResult> {
        let full = self.full_key(key);
        let body = aws_sdk_s3::primitives::ByteStream::from(bytes.to_vec());
        let mut req = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&full)
            .body(body);
        // Conditional PUT: create-only when no prior version, else match the
        // exact ETag the caller read.
        req = match expected {
            None => req.if_none_match("*"),
            Some(v) => req.if_match(&v.0),
        };
        match req.send().await {
            Ok(resp) => {
                let etag = resp.e_tag().unwrap_or_default().to_string();
                Ok(CasResult::Written(Version(etag)))
            }
            Err(e) => {
                let status = e.raw_response().map(|r| r.status().as_u16());
                if status == Some(412) || status == Some(409) {
                    Ok(CasResult::Conflict)
                } else {
                    Err(anyhow!("s3 put-if-version {full}: {}", s3_detail(&e)))
                }
            }
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let full = self.full_key(key);
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(&full)
            .send()
            .await
            .map_err(|e| anyhow!("s3 delete {full}: {}", s3_detail(&e)))?;
        Ok(())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let full_prefix = self.full_key(prefix);
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&full_prefix);
            if let Some(t) = &token {
                req = req.continuation_token(t);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| anyhow!("s3 list {full_prefix}: {}", s3_detail(&e)))?;
            for obj in resp.contents() {
                if let Some(k) = obj.key() {
                    out.push(k.strip_prefix(&self.prefix).unwrap_or(k).to_string());
                }
            }
            if resp.is_truncated().unwrap_or(false) {
                token = resp.next_continuation_token().map(String::from);
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn describe(&self) -> String {
        format!("s3://{}/{}", self.bucket, self.prefix)
    }
}

// ---- construction -----------------------------------------------------------

/// Sub-path under the data/shared root that holds the control plane.
const CONTROL_SUBDIR: &str = "_control";

/// Build the control store from the `--shared-store` value. Rides the shared
/// store when configured (DR = "replicate the bucket"), else the local data dir.
pub async fn build_control_store(
    shared_store: Option<&str>,
    data_dir: &Path,
) -> Result<Arc<dyn ControlStore>> {
    let store: Arc<dyn ControlStore> = match shared_store {
        Some(s) if s.starts_with("s3://") => {
            let (client, bucket, mut prefix) = s3_client_and_loc(s).await?;
            prefix.push_str(CONTROL_SUBDIR);
            prefix.push('/');
            Arc::new(S3ControlStore {
                client,
                bucket,
                prefix,
            })
        }
        Some(path) => Arc::new(FsControlStore::new(Path::new(path).join(CONTROL_SUBDIR))),
        None => Arc::new(FsControlStore::new(data_dir.join(CONTROL_SUBDIR))),
    };
    tracing::info!(store = %store.describe(), "control: store backend");
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store(dir: &TempDir) -> FsControlStore {
        FsControlStore::new(dir.path())
    }

    #[tokio::test]
    async fn create_then_read_roundtrip() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir);
        assert!(s.get_versioned("users.json").await.unwrap().is_none());

        let r = s.put_if_version("users.json", b"v1", None).await.unwrap();
        let v1 = match r {
            CasResult::Written(v) => v,
            CasResult::Conflict => panic!("first create must not conflict"),
        };
        let (bytes, read_v) = s.get_versioned("users.json").await.unwrap().unwrap();
        assert_eq!(bytes, b"v1");
        assert_eq!(read_v, v1);
    }

    #[tokio::test]
    async fn create_only_conflicts_when_present() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir);
        assert!(matches!(
            s.put_if_version("k", b"a", None).await.unwrap(),
            CasResult::Written(_)
        ));
        // Second create (expected = None) must lose: the object now exists.
        assert!(matches!(
            s.put_if_version("k", b"b", None).await.unwrap(),
            CasResult::Conflict
        ));
        assert_eq!(s.get_versioned("k").await.unwrap().unwrap().0, b"a");
    }

    #[tokio::test]
    async fn update_requires_matching_version() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir);
        let v1 = match s.put_if_version("k", b"a", None).await.unwrap() {
            CasResult::Written(v) => v,
            _ => unreachable!(),
        };
        // Stale version loses.
        let stale = Version("deadbeef".into());
        assert!(matches!(
            s.put_if_version("k", b"b", Some(&stale)).await.unwrap(),
            CasResult::Conflict
        ));
        // Correct version wins and rotates the version.
        let v2 = match s.put_if_version("k", b"b", Some(&v1)).await.unwrap() {
            CasResult::Written(v) => v,
            _ => panic!("matching version must write"),
        };
        assert_ne!(v1, v2);
        assert_eq!(s.get_versioned("k").await.unwrap().unwrap().0, b"b");
        // The now-stale v1 can no longer write.
        assert!(matches!(
            s.put_if_version("k", b"c", Some(&v1)).await.unwrap(),
            CasResult::Conflict
        ));
    }

    #[tokio::test]
    async fn list_and_delete() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir);
        s.put_if_version("saved/u1/a.json", b"1", None)
            .await
            .unwrap();
        s.put_if_version("saved/u1/b.json", b"2", None)
            .await
            .unwrap();
        s.put_if_version("saved/u2/c.json", b"3", None)
            .await
            .unwrap();
        let mut u1 = s.list("saved/u1/").await.unwrap();
        u1.sort();
        assert_eq!(u1, vec!["saved/u1/a.json", "saved/u1/b.json"]);
        assert_eq!(s.list("saved/").await.unwrap().len(), 3);

        s.delete("saved/u1/a.json").await.unwrap();
        assert!(s.get_versioned("saved/u1/a.json").await.unwrap().is_none());
        s.delete("saved/u1/a.json").await.unwrap(); // idempotent
        assert_eq!(s.list("saved/u1/").await.unwrap(), vec!["saved/u1/b.json"]);
    }
}
