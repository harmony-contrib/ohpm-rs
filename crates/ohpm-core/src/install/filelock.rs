//! The cross-process lock, mirroring `lib/tools/filelock/` — an
//! `oh-lock.lock` directory at the project root. Acquisition is `mkdir`-based
//! with a stale takeover after 2 s and a periodic mtime heartbeat; enabled
//! only by the `enable_cross_process_lock` config (default false).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{default::types, Config};
use crate::constants::LOCK_FILE_NAME;
use crate::error::{OhpmError, Result};

/// Stale threshold (ms) — a lock dir whose mtime is older is taken over.
const STALE_MS: u64 = 2_000;
/// Heartbeat interval (ms).
const HEARTBEAT_MS: u64 = 1_000;

/// A held cross-process lock; released on drop.
pub struct OhpmLock {
    path: PathBuf,
    _heartbeat: Option<Arc<tokio::task::JoinHandle<()>>>,
    stop: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl OhpmLock {
    /// Acquire the lock at `project_root/oh-lock.lock` (no-op when the config
    /// flag is off). Blocks while another process holds a fresh lock.
    pub async fn acquire(config: &Config, project_root: &Path) -> Result<Option<OhpmLock>> {
        if !config.get_bool(types::ENABLE_CROSS_PROCESS_LOCK) {
            return Ok(None);
        }
        let path = project_root.join(LOCK_FILE_NAME);
        loop {
            match std::fs::create_dir(&path) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Stale takeover: an mtime older than STALE_MS is dead.
                    let mtime = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    if mtime.map(|m| now.saturating_sub(m) > STALE_MS).unwrap_or(true) {
                        let _ = std::fs::remove_dir_all(&path);
                        continue;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(e) => return Err(OhpmError::file_lock_failed(&path, &e.to_string())),
            }
        }
        // Heartbeat: refresh the dir mtime periodically.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let heartbeat_path = path.clone();
        let stop_clone = stop.clone();
        let heartbeat = tokio::task::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(HEARTBEAT_MS)).await;
                if stop_clone.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                // The reference detects a compromised lock when our mtime is
                // overwritten by another process; refreshing to now is the
                // heartbeat, and a `utimes` failure means the lock is gone.
                let _ = filetime::set_file_mtime(
                    &heartbeat_path,
                    filetime::FileTime::now(),
                );
            }
        });
        Ok(Some(OhpmLock {
            path,
            _heartbeat: Some(Arc::new(heartbeat)),
            stop: Some(stop),
        }))
    }
}

impl Drop for OhpmLock {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
