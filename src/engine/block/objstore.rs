// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Object-store abstraction (blocks + manifests as keyed objects) with two sync
//! backends: [`FsObjectStore`] (CAS via `hard_link`) and [`S3ObjectStore`] (CAS
//! via conditional `PutObject`, async SDK bridged to sync via a runtime handle).

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use uuid::Uuid;

/// Format an AWS SDK error with its full service-error detail (code + message)
/// instead of the terse top-level "service error".
pub(crate) fn s3_detail<E: std::error::Error>(e: &E) -> String {
    aws_smithy_types::error::display::DisplayErrorContext(e).to_string()
}

/// Minimal object operations the block store needs. `Send + Sync` so the store
/// can be shared across rayon workers and async tasks.
pub trait ObjectStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Vec<u8>>;
    /// Read a byte range, clamped to the object's end — for cheap footer reads. The
    /// default reads the whole object and slices; backends override for true ranged I/O.
    fn get_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let all = self.get(key)?;
        let start = (offset as usize).min(all.len());
        let end = start.saturating_add(len as usize).min(all.len());
        Ok(all[start..end].to_vec())
    }
    /// Atomic overwrite (readers never see a partial object).
    fn put(&self, key: &str, bytes: &[u8]) -> Result<()>;
    /// Compare-and-swap create: write only if `key` doesn't exist. `Ok(true)` =
    /// created, `Ok(false)` = already existed (lost the race).
    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool>;
    fn delete(&self, key: &str) -> Result<()>;
    /// Full keys (relative to store root) under `prefix`.
    fn list(&self, prefix: &str) -> Result<Vec<String>>;
    /// Object size in bytes, or `None` if absent.
    fn size(&self, key: &str) -> Result<Option<u64>>;
    /// Human-readable description for startup logging.
    fn describe(&self) -> String;
}

// filesystem backend

pub struct FsObjectStore {
    root: PathBuf,
}

impl FsObjectStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, key: &str) -> PathBuf {
        // Keys are '/'-separated; join component-wise so it's correct on any OS.
        let mut p = self.root.clone();
        for seg in key.split('/').filter(|s| !s.is_empty()) {
            p.push(seg);
        }
        p
    }
}

impl ObjectStore for FsObjectStore {
    fn get(&self, key: &str) -> Result<Vec<u8>> {
        let path = self.path(key);
        fs::read(&path).with_context(|| format!("reading {}", path.display()))
    }

    fn get_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let path = self.path(key);
        let mut f = fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        f.seek(SeekFrom::Start(offset))
            .with_context(|| format!("seeking {}", path.display()))?;
        let mut buf = Vec::with_capacity(len as usize);
        f.take(len)
            .read_to_end(&mut buf)
            .with_context(|| format!("range-reading {}", path.display()))?;
        Ok(buf)
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let tmp = path.with_extension(format!("tmp.{}", Uuid::new_v4()));
        fs::write(&tmp, bytes).with_context(|| format!("writing temp {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("renaming into {}", path.display()))?;
        Ok(())
    }

    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        // Write a complete temp file, then hard_link it onto the target. link()
        // is atomic and fails with AlreadyExists — both CAS and torn-read safety.
        let tmp = path.with_extension(format!("tmp.{}", Uuid::new_v4()));
        fs::write(&tmp, bytes).with_context(|| format!("writing temp {}", tmp.display()))?;
        let result = match fs::hard_link(&tmp, &path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e).with_context(|| format!("linking {}", path.display())),
        };
        let _ = fs::remove_file(&tmp);
        result
    }

    fn delete(&self, key: &str) -> Result<()> {
        match fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        // Recursive (matches S3's flat-listing semantics): every file object
        // under `prefix`, as a root-relative '/'-joined key.
        let base = self.path(prefix);
        let mut out = Vec::new();
        walk(&base, &self.root, &mut out)?;
        Ok(out)
    }

    fn size(&self, key: &str) -> Result<Option<u64>> {
        match fs::metadata(self.path(key)) {
            Ok(m) => Ok(Some(m.len())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn describe(&self) -> String {
        format!("filesystem {}", self.root.display())
    }
}

/// Recursively collect file keys (root-relative, '/'-joined) under `dir`.
fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("listing {}", dir.display())),
    };
    for e in entries.flatten() {
        let path = e.path();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            walk(&path, root, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            let key: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(key.join("/"));
        }
    }
    Ok(())
}

// S3 backend

/// S3-backed object store. The sync trait bridges the async SDK by spawning each future
/// onto the runtime and blocking on a channel — safe from any thread, unlike `block_on`.
pub struct S3ObjectStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    /// Key prefix within the bucket, normalized to end with `/` (or empty).
    prefix: String,
    handle: tokio::runtime::Handle,
}

impl S3ObjectStore {
    fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    /// Drive an S3 future to completion from sync code without `block_on`.
    fn run<F, T>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.handle.spawn(async move {
            let _ = tx.send(fut.await);
        });
        rx.recv()
            .expect("s3: runtime task dropped before completing")
    }
}

impl ObjectStore for S3ObjectStore {
    fn get(&self, key: &str) -> Result<Vec<u8>> {
        let (client, bucket, full) = (self.client.clone(), self.bucket.clone(), self.full_key(key));
        self.run(async move {
            let resp = client
                .get_object()
                .bucket(&bucket)
                .key(&full)
                .send()
                .await
                .map_err(|e| anyhow!("s3 get {full}: {}", s3_detail(&e)))?;
            let data = resp
                .body
                .collect()
                .await
                .with_context(|| format!("s3 read body {full}"))?;
            Ok(data.into_bytes().to_vec())
        })
    }

    fn get_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let (client, bucket, full) = (self.client.clone(), self.bucket.clone(), self.full_key(key));
        let range = format!("bytes={}-{}", offset, offset + len - 1);
        self.run(async move {
            let resp = client
                .get_object()
                .bucket(&bucket)
                .key(&full)
                .range(range)
                .send()
                .await
                .map_err(|e| anyhow!("s3 get-range {full}: {}", s3_detail(&e)))?;
            let data = resp
                .body
                .collect()
                .await
                .with_context(|| format!("s3 read body {full}"))?;
            Ok(data.into_bytes().to_vec())
        })
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let (client, bucket, full) = (self.client.clone(), self.bucket.clone(), self.full_key(key));
        let body = aws_sdk_s3::primitives::ByteStream::from(bytes.to_vec());
        self.run(async move {
            client
                .put_object()
                .bucket(&bucket)
                .key(&full)
                .body(body)
                .send()
                .await
                .map_err(|e| anyhow!("s3 put {full}: {}", s3_detail(&e)))?;
            Ok(())
        })
    }

    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
        let (client, bucket, full) = (self.client.clone(), self.bucket.clone(), self.full_key(key));
        let body = aws_sdk_s3::primitives::ByteStream::from(bytes.to_vec());
        self.run(async move {
            let res = client
                .put_object()
                .bucket(&bucket)
                .key(&full)
                .if_none_match("*") // conditional create — fails 412 if present
                .body(body)
                .send()
                .await;
            match res {
                Ok(_) => Ok(true),
                Err(e) => {
                    let status = e.raw_response().map(|r| r.status().as_u16());
                    if status == Some(412) || status == Some(409) {
                        Ok(false) // already exists — lost the CAS race
                    } else {
                        Err(anyhow!("s3 put-if-absent {full}: {}", s3_detail(&e)))
                    }
                }
            }
        })
    }

    fn delete(&self, key: &str) -> Result<()> {
        let (client, bucket, full) = (self.client.clone(), self.bucket.clone(), self.full_key(key));
        self.run(async move {
            client
                .delete_object()
                .bucket(&bucket)
                .key(&full)
                .send()
                .await
                .map_err(|e| anyhow!("s3 delete {full}: {}", s3_detail(&e)))?;
            Ok(())
        })
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let (client, bucket) = (self.client.clone(), self.bucket.clone());
        let full_prefix = self.full_key(prefix);
        let strip = self.prefix.clone();
        self.run(async move {
            let mut out = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut req = client
                    .list_objects_v2()
                    .bucket(&bucket)
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
                        // Strip the store prefix to return store-relative keys.
                        out.push(k.strip_prefix(&strip).unwrap_or(k).to_string());
                    }
                }
                if resp.is_truncated().unwrap_or(false) {
                    token = resp.next_continuation_token().map(String::from);
                } else {
                    break;
                }
            }
            Ok(out)
        })
    }

    fn size(&self, key: &str) -> Result<Option<u64>> {
        let (client, bucket, full) = (self.client.clone(), self.bucket.clone(), self.full_key(key));
        self.run(async move {
            match client.head_object().bucket(&bucket).key(&full).send().await {
                Ok(resp) => Ok(resp.content_length().map(|l| l as u64)),
                Err(e) => {
                    let status = e.raw_response().map(|r| r.status().as_u16());
                    if status == Some(404) {
                        Ok(None)
                    } else {
                        Err(anyhow!("s3 head {full}: {}", s3_detail(&e)))
                    }
                }
            }
        })
    }

    fn describe(&self) -> String {
        format!("s3://{}/{}", self.bucket, self.prefix)
    }
}

// local-first backend

/// Local-primary store with a shared fallback for reads. Writes/CAS go to `local` only;
/// a read miss fetches from `shared` and caches it locally (blocks are immutable).
pub struct LocalFirstObjectStore {
    local: Arc<dyn ObjectStore>,
    shared: Arc<dyn ObjectStore>,
}

impl LocalFirstObjectStore {
    pub fn new(local: Arc<dyn ObjectStore>, shared: Arc<dyn ObjectStore>) -> Self {
        Self { local, shared }
    }
}

impl ObjectStore for LocalFirstObjectStore {
    fn get(&self, key: &str) -> Result<Vec<u8>> {
        match self.local.get(key) {
            Ok(bytes) => Ok(bytes),
            Err(_) => {
                // Miss → fetch from shared and cache locally (immutable object).
                let bytes = self.shared.get(key)?;
                let _ = self.local.put(key, &bytes);
                // Block objects only — manifest fetches go through here too but
                // are frequent and uninteresting.
                if key.contains("/blocks/") {
                    tracing::info!(
                        object = %key,
                        bytes = bytes.len() as u64,
                        "block sync: downloaded block from shared store"
                    );
                }
                Ok(bytes)
            }
        }
    }

    fn get_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        // Ranged reads (footer probes) never warrant fetching a whole block:
        // try local, then read the same range straight from shared.
        match self.local.get_range(key, offset, len) {
            Ok(b) => Ok(b),
            Err(_) => self.shared.get_range(key, offset, len),
        }
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.local.put(key, bytes)
    }

    fn put_if_absent(&self, key: &str, bytes: &[u8]) -> Result<bool> {
        self.local.put_if_absent(key, bytes)
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.local.delete(key)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.local.list(prefix)
    }

    fn size(&self, key: &str) -> Result<Option<u64>> {
        // Local ONLY — `size` drives local-authoritative manifest existence checks.
        // A shared fallback would do an S3 HEAD per manifest op and couple writes to S3.
        self.local.size(key)
    }

    fn describe(&self) -> String {
        format!(
            "local-first ({} ⇄ {})",
            self.local.describe(),
            self.shared.describe()
        )
    }
}

// construction

/// Build an S3 client + `(bucket, normalized_prefix)` for `url`. Shared with the control
/// plane so the credential/region setup (notably the no-IMDS region chain) lives in one place.
pub(crate) async fn s3_client_and_loc(url: &str) -> Result<(aws_sdk_s3::Client, String, String)> {
    let (bucket, prefix) = parse_s3_url(url)?;
    // Region: env → profile `region`. Deliberately NO IMDS — off-EC2 it just hangs
    // on a connect timeout to 169.254.169.254. Credentials still use the full default chain.
    let region_provider = aws_config::meta::region::RegionProviderChain::first_try(
        aws_config::environment::region::EnvironmentVariableRegionProvider::new(),
    )
    .or_else(aws_config::profile::region::ProfileFileRegionProvider::default());
    let conf = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .http_client(crate::crypto::tls::aws_http_client())
        .region(region_provider)
        .load()
        .await;
    if conf.region().is_none() {
        bail!(
            "no AWS region for the s3 shared store — set AWS_REGION (e.g. \
             `AWS_REGION=us-east-1`) or add a `region` line to your AWS profile. \
             (SSO profiles often have `sso_region` but no plain `region`.)"
        );
    }
    Ok((aws_sdk_s3::Client::new(&conf), bucket, prefix))
}

/// Parse `s3://bucket/prefix...` into `(bucket, normalized_prefix)`. The prefix
/// is normalized to end with `/` (or be empty).
fn parse_s3_url(url: &str) -> Result<(String, String)> {
    let rest = url
        .strip_prefix("s3://")
        .ok_or_else(|| anyhow!("not an s3 url: {url}"))?;
    let mut parts = rest.splitn(2, '/');
    let bucket = parts
        .next()
        .filter(|b| !b.is_empty())
        .ok_or_else(|| anyhow!("s3 url missing bucket: {url}"))?;
    let prefix = parts.next().unwrap_or("");
    let prefix = if prefix.is_empty() || prefix.ends_with('/') {
        prefix.to_string()
    } else {
        format!("{prefix}/")
    };
    Ok((bucket.to_string(), prefix))
}

/// Build the object store from a `--shared-store` value (`None` = local dir). `s3://...`
/// selects S3, else a filesystem path. Async because the S3 client loads AWS config.
pub async fn build_object_store(
    shared_store: Option<&str>,
    local_default: &Path,
) -> Result<Arc<dyn ObjectStore>> {
    match shared_store {
        Some(s) if s.starts_with("s3://") => {
            let (client, bucket, prefix) = s3_client_and_loc(s).await?;
            let handle = tokio::runtime::Handle::current();
            Ok(Arc::new(S3ObjectStore {
                client,
                bucket,
                prefix,
                handle,
            }))
        }
        Some(path) => Ok(Arc::new(FsObjectStore::new(PathBuf::from(path)))),
        None => Ok(Arc::new(FsObjectStore::new(local_default))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fs_put_get_size_delete_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = FsObjectStore::new(dir.path());
        assert_eq!(store.size("a/b.txt").unwrap(), None);
        store.put("a/b.txt", b"hello").unwrap();
        assert_eq!(store.get("a/b.txt").unwrap(), b"hello");
        assert_eq!(store.size("a/b.txt").unwrap(), Some(5));
        store.delete("a/b.txt").unwrap();
        assert_eq!(store.size("a/b.txt").unwrap(), None);
        store.delete("a/b.txt").unwrap(); // idempotent
    }

    #[test]
    fn fs_put_if_absent_is_cas() {
        let dir = TempDir::new().unwrap();
        let store = FsObjectStore::new(dir.path());
        assert!(store.put_if_absent("m/1.json", b"first").unwrap());
        assert!(!store.put_if_absent("m/1.json", b"second").unwrap());
        assert_eq!(store.get("m/1.json").unwrap(), b"first"); // first write wins
    }

    #[test]
    fn fs_list_returns_keys_under_prefix() {
        let dir = TempDir::new().unwrap();
        let store = FsObjectStore::new(dir.path());
        store.put("p/manifest/1.json", b"x").unwrap();
        store.put("p/manifest/2.json", b"y").unwrap();
        let mut keys = store.list("p/manifest/").unwrap();
        keys.sort();
        assert_eq!(keys, vec!["p/manifest/1.json", "p/manifest/2.json"]);
        assert!(store.list("p/empty/").unwrap().is_empty());
    }

    #[test]
    fn parse_s3() {
        assert_eq!(
            parse_s3_url("s3://bucket/helios-data/").unwrap(),
            ("bucket".into(), "helios-data/".into())
        );
        assert_eq!(
            parse_s3_url("s3://bucket/a/b").unwrap(),
            ("bucket".into(), "a/b/".into())
        );
        assert_eq!(
            parse_s3_url("s3://bucket").unwrap(),
            ("bucket".into(), "".into())
        );
        assert!(parse_s3_url("/local/path").is_err());
    }
}
