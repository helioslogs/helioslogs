// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! jemalloc memory introspection for `--verbose`: logs `allocated` (live bytes)
//! vs `resident`/`retained` (pages the allocator holds) every few seconds, so
//! genuine working-set growth is distinguishable from allocator retention.
//!
//! Also hosts [`spawn_purger`]: on platforms without a jemalloc background thread
//! (notably macOS), a small task periodically runs decay-based purging so freed
//! pages are returned to the OS after a spike, mirroring Linux's `background_thread`.

use std::time::Duration;

use tikv_jemalloc_ctl::{epoch, stats};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(3);

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Spawn a periodic jemalloc sampler (opt-in via `--verbose`). Logs every tick;
/// returns silently if the ctl handles can't be resolved.
pub fn spawn_logger() {
    tokio::spawn(async move {
        let Ok(e) = epoch::mib() else { return };
        let (Ok(allocated), Ok(active), Ok(resident), Ok(retained), Ok(mapped)) = (
            stats::allocated::mib(),
            stats::active::mib(),
            stats::resident::mib(),
            stats::retained::mib(),
            stats::mapped::mib(),
        ) else {
            return;
        };
        loop {
            // Advancing the epoch refreshes the cached stats below.
            let _ = e.advance();
            tracing::info!(
                allocated_mb = mb(allocated.read().unwrap_or(0)),
                active_mb = mb(active.read().unwrap_or(0)),
                resident_mb = mb(resident.read().unwrap_or(0)),
                retained_mb = mb(retained.read().unwrap_or(0)),
                mapped_mb = mb(mapped.read().unwrap_or(0)),
                "jemalloc"
            );
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
    });
}

/// On Linux, jemalloc's `background_thread` returns idle pages to the OS on the
/// decay schedule — nothing to do here.
#[cfg(target_os = "linux")]
pub fn spawn_purger() {}

/// Elsewhere (notably macOS) there is no jemalloc background thread, so freed pages
/// only decay when the allocator is next active — after an ingest/compaction spike
/// the process goes idle and RSS stays at the high-water. This task forces a full
/// `arena.all.purge` every second, returning dirty/muzzy pages to the OS. (Decay-
/// based purging — respecting `dirty_decay_ms` — does not actually reclaim on macOS,
/// so we use the unconditional purge.) Purge is a cheap no-op when nothing is dirty.
#[cfg(not(target_os = "linux"))]
pub fn spawn_purger() {
    // MALLCTL_ARENAS_ALL — the sentinel arena index meaning "every arena".
    const ARENAS_ALL: usize = 4096;
    tokio::spawn(async move {
        let name = format!("arena.{ARENAS_ALL}.purge\0");
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            // Best-effort void command (no in/out args); ignore the return code.
            unsafe {
                tikv_jemalloc_sys::mallctl(
                    name.as_ptr() as *const std::ffi::c_char,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                );
            }
        }
    });
}
