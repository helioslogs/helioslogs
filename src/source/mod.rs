// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Background supervisor for ingestion sources (mirrors the monitor scheduler).
//! Only backend is a local-fs pull: glob a path and tail bytes appended since the
//! last run, tracked per-file in a checkpoint ("fishbucket") so restarts resume.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono::Utc;
use tracing::{error, info, warn};

use crate::control::sources::{FileMark, Source, SourceCheckpoint};
use crate::control::Control;
use crate::engine::block::ObjectStore;
use crate::indexer::ingest;
use crate::indexer::parse::{self, Compression, Format, Multiline, ParseConfig};

const TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Long-running supervisor task. Never returns; a failing tick is logged and the
/// loop continues so one bad source doesn't stall the rest.
pub async fn run_supervisor(control: Control) {
    info!(
        tick_interval_secs = TICK_INTERVAL.as_secs(),
        "source supervisor started"
    );
    let mut interval = tokio::time::interval(TICK_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if let Err(e) = tick(&control).await {
            warn!("source supervisor tick failed: {e:#}");
        }
    }
}

async fn tick(control: &Control) -> Result<()> {
    let now_ms = Utc::now().timestamp_millis();
    for (_owner, src) in control.source_list_all().await? {
        let supported_kind = src.kind == "fs" || src.kind == "s3";
        if !src.enabled || !supported_kind || src.mode != "pull" {
            continue;
        }
        if !is_due(&src, now_ms) {
            continue;
        }
        if !control.source_try_lease(&src.id).await? {
            continue;
        }
        let ctl = control.clone();
        let sid = src.id.clone();
        tokio::spawn(async move {
            let (status, error, ingested) = match run_source(&ctl, src).await {
                Ok(n) => ("ok", None, n),
                Err(e) => {
                    error!(source_id = sid, "source run failed: {e:#}");
                    ("error", Some(format!("{e:#}")), 0)
                }
            };
            if let Err(e) = ctl
                .source_finish_run(&sid, status, error.as_deref(), ingested)
                .await
            {
                error!(source_id = sid, "source finish_run failed: {e:#}");
            }
        });
    }
    Ok(())
}

fn is_due(src: &Source, now_ms: i64) -> bool {
    match src.last_run_at {
        None => true,
        Some(t) => (now_ms - t) >= src.interval_seconds * 1000,
    }
}

/// Load the checkpoint, scan/ingest on a blocking thread (fs/s3 reads + parse are
/// sync, CPU-bound), persist the advanced checkpoint. Returns rows ingested.
async fn run_source(control: &Control, src: Source) -> Result<u64> {
    let id = src.id.clone();
    let ckpt = control
        .source_checkpoint_get(&id)
        .await?
        .unwrap_or_default();
    // Captured into the blocking ingest so it can persist the checkpoint + live
    // progress after every file (block_on is safe on a blocking-pool thread).
    let handle = tokio::runtime::Handle::current();
    let (new_ckpt, ingested) = match src.kind.as_str() {
        "fs" => {
            let ctl = control.clone();
            let pid = id.clone();
            tokio::task::spawn_blocking(move || {
                let mut progress = make_progress(&handle, &ctl, &pid);
                pull_fs(&src, ckpt, &mut progress)
            })
            .await??
        }
        "s3" => {
            // Bucket-root store, rebuilt per run — fine at poll cadences. We
            // list by literal prefix and glob-match keys ourselves.
            let (bucket, _) = parse_s3_path(&src.path)?;
            let store = crate::engine::block::build_object_store(
                Some(&format!("s3://{bucket}/")),
                std::path::Path::new("."),
            )
            .await?;
            let ctl = control.clone();
            let pid = id.clone();
            tokio::task::spawn_blocking(move || {
                let mut progress = make_progress(&handle, &ctl, &pid);
                pull_s3(&src, store.as_ref(), ckpt, &mut progress)
            })
            .await??
        }
        other => anyhow::bail!("unsupported source kind: {other}"),
    };
    control.source_checkpoint_put(&id, &new_ckpt).await?;
    Ok(ingested)
}

/// Progress reporter for the blocking pull: persists checkpoint + run counters
/// after each file, so the UI shows progress and a crash resumes mid-run.
fn make_progress<'a>(
    handle: &'a tokio::runtime::Handle,
    control: &'a Control,
    id: &'a str,
) -> impl FnMut(&SourceCheckpoint, u64, Option<&str>) -> bool + 'a {
    move |ckpt: &SourceCheckpoint, ingested: u64, current: Option<&str>| {
        handle.block_on(async {
            let _ = control.source_checkpoint_put(id, ckpt).await;
            // Returns whether the source is still enabled — `false` (or a
            // vanished/deleted source) tells the pull loop to stop now.
            control
                .source_progress_update(id, ingested, current)
                .await
                .unwrap_or(false)
        })
    }
}

/// Glob the source path, ingest each matching file's new bytes, return the
/// advanced checkpoint + row count. Pure w.r.t. the control plane (testable).
fn pull_fs(
    src: &Source,
    mut ckpt: SourceCheckpoint,
    progress: &mut dyn FnMut(&SourceCheckpoint, u64, Option<&str>) -> bool,
) -> Result<(SourceCheckpoint, u64)> {
    let excludes: Vec<glob::Pattern> = src
        .exclude
        .iter()
        .filter_map(|p| glob::Pattern::new(p).ok())
        .collect();

    let mut ingested = 0u64;
    let mut seen: HashSet<String> = HashSet::new();
    let mut stopped = false;

    let entries =
        glob::glob(&src.path).with_context(|| format!("invalid source path glob: {}", src.path))?;
    for entry in entries {
        let path = match entry {
            Ok(p) => p,
            Err(_) => continue, // unreadable dir entry — skip, keep going
        };
        if !path.is_file() {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        if excludes.iter().any(|pat| pat.matches(&path_str)) {
            continue;
        }
        seen.insert(path_str.clone());
        // Announce the file and checkpoint everything before it, so a crash
        // resumes here. `false` = source disabled/deleted: stop before touching it.
        if !progress(&ckpt, ingested, Some(&path_str)) {
            info!(
                source_id = src.id,
                "source disabled mid-run — stopping early"
            );
            stopped = true;
            break;
        }
        match ingest_file(src, &path, &mut ckpt) {
            Ok(n) => ingested += n,
            Err(e) => warn!(source_id = src.id, path = %path_str, "file ingest failed: {e:#}"),
        }
    }

    // Drop offsets for files that no longer match (bounds the fishbucket), but
    // only after a *complete* scan — an early stop hasn't reached every file yet.
    if !stopped {
        ckpt.files.retain(|p, _| seen.contains(p));
    }
    Ok((ckpt, ingested))
}

/// List the bucket under the glob's literal prefix and ingest each matching object
/// once (checkpoint grows with the bucket — pull is for small/archival buckets).
fn pull_s3(
    src: &Source,
    store: &dyn ObjectStore,
    mut ckpt: SourceCheckpoint,
    progress: &mut dyn FnMut(&SourceCheckpoint, u64, Option<&str>) -> bool,
) -> Result<(SourceCheckpoint, u64)> {
    let (_bucket, glob_part) = parse_s3_path(&src.path)?;
    let pattern =
        glob::Pattern::new(&glob_part).with_context(|| format!("invalid s3 glob: {glob_part}"))?;
    let excludes: Vec<glob::Pattern> = src
        .exclude
        .iter()
        .filter_map(|p| glob::Pattern::new(p).ok())
        .collect();
    let list_prefix = literal_prefix(&glob_part);

    let keys = store.list(&list_prefix)?;
    let mut ingested = 0u64;
    let mut seen: HashSet<String> = HashSet::new();
    let mut stopped = false;
    for key in keys {
        if !pattern.matches(&key) || excludes.iter().any(|p| p.matches(&key)) {
            continue;
        }
        seen.insert(key.clone());
        if ckpt.files.contains_key(&key) {
            continue; // already ingested
        }
        // Stop before fetching this object if the source was disabled/deleted.
        if !progress(&ckpt, ingested, Some(&key)) {
            info!(
                source_id = src.id,
                "source disabled mid-run — stopping early"
            );
            stopped = true;
            break;
        }
        let bytes = store.get(&key)?;
        let compression = effective_compression(src, &key);
        let n = parse_and_submit(src, &key, &bytes, compression)?;
        ingested += n;
        ckpt.files.insert(
            key,
            FileMark {
                offset: bytes.len() as u64,
                mtime_ms: 0,
            },
        );
    }
    if !stopped {
        ckpt.files.retain(|k, _| seen.contains(k));
    }
    Ok((ckpt, ingested))
}

/// Split `s3://bucket/glob...` into `(bucket, glob)`. The glob is the remainder
/// after the bucket, matched against object keys (e.g. `logs/**/*.gz`).
fn parse_s3_path(path: &str) -> Result<(String, String)> {
    let rest = path
        .strip_prefix("s3://")
        .ok_or_else(|| anyhow::anyhow!("s3 source path must start with s3:// (got {path})"))?;
    let mut it = rest.splitn(2, '/');
    let bucket = it
        .next()
        .filter(|b| !b.is_empty())
        .ok_or_else(|| anyhow::anyhow!("s3 source path missing bucket: {path}"))?;
    Ok((bucket.to_string(), it.next().unwrap_or("").to_string()))
}

/// The literal directory prefix of a glob (up to the first metachar, trimmed to
/// the last `/`) — what we hand to `list` so S3 only returns plausible keys.
fn literal_prefix(glob: &str) -> String {
    let end = glob.find(['*', '?', '[', '{']).unwrap_or(glob.len());
    match glob[..end].rfind('/') {
        Some(i) => glob[..=i].to_string(),
        None => String::new(),
    }
}

fn ingest_file(src: &Source, path: &Path, ckpt: &mut SourceCheckpoint) -> Result<u64> {
    let path_str = path.to_string_lossy().to_string();
    let meta = fs::metadata(path)?;
    let len = meta.len();
    let mtime_ms = mtime_ms(&meta);
    let compression = effective_compression(src, &path_str);
    let prev = ckpt.files.get(&path_str).cloned();

    if matches!(compression, Compression::None) {
        // --- tail mode: read only the new bytes, up to the last newline ---
        let prev_offset = prev.as_ref().map(|m| m.offset).unwrap_or(0);
        if len == prev_offset {
            return Ok(0); // unchanged
        }
        let start = if len < prev_offset { 0 } else { prev_offset };
        let buf = read_range(path, start, len)?;
        let consumed = last_newline_end(&buf);
        if consumed == 0 {
            // No complete line yet — keep the offset so we re-read it next tick.
            ckpt.files.insert(
                path_str,
                FileMark {
                    offset: start,
                    mtime_ms,
                },
            );
            return Ok(0);
        }
        let n = parse_and_submit(src, &path_str, &buf[..consumed], Compression::None)?;
        ckpt.files.insert(
            path_str,
            FileMark {
                offset: start + consumed as u64,
                mtime_ms,
            },
        );
        Ok(n)
    } else {
        // --- whole-file mode: re-ingest only when size or mtime changed ---
        if let Some(m) = &prev {
            if m.offset == len && m.mtime_ms == mtime_ms {
                return Ok(0);
            }
        }
        let buf = fs::read(path)?;
        let n = parse_and_submit(src, &path_str, &buf, compression)?;
        ckpt.files.insert(
            path_str,
            FileMark {
                offset: len,
                mtime_ms,
            },
        );
        Ok(n)
    }
}

/// Parse `bytes` per config and submit each event to the block writer, returning
/// the row count. `name` (file/key) becomes the per-event `source` absent a `source_tag`.
fn parse_and_submit(
    src: &Source,
    name: &str,
    bytes: &[u8],
    compression: Compression,
) -> Result<u64> {
    let multiline = match &src.multiline_pattern {
        Some(p) if !p.is_empty() => {
            Some(Multiline::new(p, src.multiline_max_lines.unwrap_or(500))?)
        }
        _ => None,
    };
    let grok = match &src.grok_pattern {
        Some(p) if !p.is_empty() => Some(crate::indexer::parse::Grok::compile(p)?),
        _ => None,
    };
    let cfg = ParseConfig {
        format: Format::from_str_lenient(&src.format),
        compression,
        multiline,
        grok,
    };
    let out = parse::parse(bytes, &cfg)?;
    // Default the per-event source to the file/object being ingested, unless the
    // source pins an explicit `source_tag` (which then wins for every file).
    let default_source = src
        .source_tag
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(name);
    let today = Utc::now().date_naive();
    let mut n = 0u64;
    for v in &out.events {
        let day = ingest::event_day(v).unwrap_or(today);
        if let Ok(row) = ingest::json_to_row(v, Some(default_source)) {
            // `index` may be an `app-{{field}}` template — resolve per event.
            let index = ingest::resolve_index_template(&src.index, v);
            let key = crate::catalog::PartitionKey::new(&src.env, &index, day);
            // Blocking send: the file is the durable buffer, so waiting for queue
            // room is lossless backpressure. No-op in tests (no writer installed).
            crate::engine::block::submit_blocking(key, row);
            n += 1;
        }
    }
    Ok(n)
}

fn effective_compression(src: &Source, path_str: &str) -> Compression {
    match Compression::from_str_lenient(&src.compression) {
        Compression::Auto => {
            let p = path_str.to_ascii_lowercase();
            if p.ends_with(".gz") {
                Compression::Gzip
            } else if p.ends_with(".zst") || p.ends_with(".zstd") {
                Compression::Zstd
            } else {
                Compression::None
            }
        }
        c => c,
    }
}

fn read_range(path: &Path, start: u64, len: u64) -> Result<Vec<u8>> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(len.saturating_sub(start) as usize);
    f.take(len - start).read_to_end(&mut buf)?;
    Ok(buf)
}

fn last_newline_end(buf: &[u8]) -> usize {
    match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => 0,
    }
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;

    /// In-memory object store for s3-pull tests — no AWS, no network.
    struct MemStore {
        objs: HashMap<String, Vec<u8>>,
    }

    impl ObjectStore for MemStore {
        fn get(&self, key: &str) -> Result<Vec<u8>> {
            self.objs
                .get(key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no object {key}"))
        }
        fn put(&self, _key: &str, _bytes: &[u8]) -> Result<()> {
            unimplemented!()
        }
        fn put_if_absent(&self, _key: &str, _bytes: &[u8]) -> Result<bool> {
            unimplemented!()
        }
        fn delete(&self, _key: &str) -> Result<()> {
            unimplemented!()
        }
        fn list(&self, prefix: &str) -> Result<Vec<String>> {
            Ok(self
                .objs
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect())
        }
        fn size(&self, key: &str) -> Result<Option<u64>> {
            Ok(self.objs.get(key).map(|b| b.len() as u64))
        }
        fn describe(&self) -> String {
            "mem".into()
        }
    }

    fn fs_source(glob: &str) -> Source {
        Source {
            id: "src_test".into(),
            name: "t".into(),
            env: "default".into(),
            index: "app".into(),
            kind: "fs".into(),
            mode: "pull".into(),
            path: glob.into(),
            format: "text".into(),
            compression: "auto".into(),
            ..Default::default()
        }
    }

    fn write_file(path: &Path, contents: &str) {
        let mut f = File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn first_pull_reads_all_complete_lines() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("a.log"), "l1\nl2\nl3\n");
        let src = fs_source(&format!("{}/**/*.log", dir.path().display()));
        let (ckpt, n) = pull_fs(&src, SourceCheckpoint::default(), &mut |_, _, _| true).unwrap();
        assert_eq!(n, 3);
        assert_eq!(ckpt.files.len(), 1);
    }

    #[test]
    fn disable_mid_run_stops_and_keeps_unreached_offsets() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("a.log"), "l1\nl2\n");
        write_file(&dir.path().join("b.log"), "l1\nl2\n");
        write_file(&dir.path().join("c.log"), "l1\nl2\n");
        let src = fs_source(&format!("{}/**/*.log", dir.path().display()));
        // Pre-seed an offset for a file that won't be visited this run. A full
        // scan would prune it; an early stop must preserve it.
        let mut ckpt = SourceCheckpoint::default();
        ckpt.files.insert(
            "/rotated/away.log".into(),
            FileMark {
                offset: 42,
                mtime_ms: 1,
            },
        );
        // Return `false` on the second announce — i.e. "disabled" after the
        // first file — so the loop stops before touching the second.
        let mut calls = 0;
        let (out, n) = pull_fs(&src, ckpt, &mut |_, _, _| {
            calls += 1;
            calls < 2
        })
        .unwrap();
        assert_eq!(n, 2, "only the first file should ingest before stopping");
        assert!(
            out.files.contains_key("/rotated/away.log"),
            "early stop must not prune unreached checkpoint entries"
        );
    }

    #[test]
    fn second_pull_tails_only_appended_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.log");
        write_file(&file, "l1\nl2\n");
        let src = fs_source(&format!("{}/**/*.log", dir.path().display()));

        let (ckpt1, n1) = pull_fs(&src, SourceCheckpoint::default(), &mut |_, _, _| true).unwrap();
        assert_eq!(n1, 2);

        // append
        let mut f = File::options().append(true).open(&file).unwrap();
        f.write_all(b"l3\nl4\n").unwrap();
        drop(f);

        let (ckpt2, n2) = pull_fs(&src, ckpt1, &mut |_, _, _| true).unwrap();
        assert_eq!(n2, 2);

        // no change
        let (_ckpt3, n3) = pull_fs(&src, ckpt2, &mut |_, _, _| true).unwrap();
        assert_eq!(n3, 0);
    }

    #[test]
    fn partial_trailing_line_waits_for_newline() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.log");
        write_file(&file, "complete\npartial-no-newline");
        let src = fs_source(&format!("{}/**/*.log", dir.path().display()));

        let (ckpt1, n1) = pull_fs(&src, SourceCheckpoint::default(), &mut |_, _, _| true).unwrap();
        assert_eq!(n1, 1); // only the complete line

        // finish the partial line
        let mut f = File::options().append(true).open(&file).unwrap();
        f.write_all(b" now-complete\n").unwrap();
        drop(f);

        let (_ckpt2, n2) = pull_fs(&src, ckpt1, &mut |_, _, _| true).unwrap();
        assert_eq!(n2, 1); // the previously-partial line, now whole
    }

    #[test]
    fn truncation_rereads_from_zero() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.log");
        write_file(&file, "a\nb\nc\n");
        let src = fs_source(&format!("{}/**/*.log", dir.path().display()));
        let (ckpt1, _) = pull_fs(&src, SourceCheckpoint::default(), &mut |_, _, _| true).unwrap();

        // rotate-in-place: smaller file
        write_file(&file, "x\n");
        let (_ckpt2, n2) = pull_fs(&src, ckpt1, &mut |_, _, _| true).unwrap();
        assert_eq!(n2, 1);
    }

    #[test]
    fn exclude_pattern_skips_files() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("keep.log"), "k\n");
        write_file(&dir.path().join("skip.log"), "s1\ns2\n");
        let mut src = fs_source(&format!("{}/**/*.log", dir.path().display()));
        src.exclude = vec![format!("{}/**/skip.log", dir.path().display())];
        let (_ckpt, n) = pull_fs(&src, SourceCheckpoint::default(), &mut |_, _, _| true).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn gzip_file_whole_then_unchanged() {
        use flate2::write::GzEncoder;
        use flate2::Compression as GzLevel;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.log.gz");
        let mut enc = GzEncoder::new(Vec::new(), GzLevel::default());
        enc.write_all(b"one\ntwo\n").unwrap();
        let gz = enc.finish().unwrap();
        fs::write(&file, &gz).unwrap();

        let src = fs_source(&format!("{}/**/*.gz", dir.path().display()));
        let (ckpt1, n1) = pull_fs(&src, SourceCheckpoint::default(), &mut |_, _, _| true).unwrap();
        assert_eq!(n1, 2);
        // unchanged file → no re-ingest
        let (_ckpt2, n2) = pull_fs(&src, ckpt1, &mut |_, _, _| true).unwrap();
        assert_eq!(n2, 0);
    }

    fn s3_source(path: &str) -> Source {
        Source {
            id: "src_s3".into(),
            env: "default".into(),
            index: "cdn".into(),
            kind: "s3".into(),
            mode: "pull".into(),
            path: path.into(),
            format: "text".into(),
            compression: "auto".into(),
            ..Default::default()
        }
    }

    #[test]
    fn parse_and_prefix_helpers() {
        let (b, g) = parse_s3_path("s3://bucket/logs/app/**/*.gz").unwrap();
        assert_eq!(b, "bucket");
        assert_eq!(g, "logs/app/**/*.gz");
        assert_eq!(literal_prefix("logs/app/**/*.gz"), "logs/app/");
        assert_eq!(literal_prefix("*.gz"), "");
        assert!(parse_s3_path("/not/s3").is_err());
    }

    #[test]
    fn s3_pull_ingests_matching_objects_once() {
        let mut objs = HashMap::new();
        objs.insert("logs/app/2026/01/a.log".to_string(), b"l1\nl2\n".to_vec());
        objs.insert("logs/app/2026/01/b.log".to_string(), b"l3\n".to_vec());
        objs.insert("logs/other/skip.txt".to_string(), b"nope\n".to_vec());
        let store = MemStore { objs };
        let src = s3_source("s3://bucket/logs/app/**/*.log");

        let (ckpt1, n1) = pull_s3(&src, &store, SourceCheckpoint::default(), &mut |_, _, _| {
            true
        })
        .unwrap();
        assert_eq!(n1, 3); // a.log(2) + b.log(1); skip.txt excluded by glob
                           // immutable objects → second run ingests nothing
        let (_ckpt2, n2) = pull_s3(&src, &store, ckpt1, &mut |_, _, _| true).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn s3_pull_honors_exclude_and_gzip() {
        use flate2::write::GzEncoder;
        use flate2::Compression as GzLevel;
        let mut enc = GzEncoder::new(Vec::new(), GzLevel::default());
        enc.write_all(b"x\ny\n").unwrap();
        let gz = enc.finish().unwrap();

        let mut objs = HashMap::new();
        objs.insert("logs/keep.log.gz".to_string(), gz);
        objs.insert("logs/drop.log.gz".to_string(), b"raw\n".to_vec());
        let store = MemStore { objs };
        let mut src = s3_source("s3://bucket/logs/**/*.gz");
        src.exclude = vec!["**/drop.log.gz".to_string()];

        let (_ckpt, n) = pull_s3(&src, &store, SourceCheckpoint::default(), &mut |_, _, _| {
            true
        })
        .unwrap();
        assert_eq!(n, 2); // keep.log.gz gunzipped → 2 lines; drop excluded
    }
}
