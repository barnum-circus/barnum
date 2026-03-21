//! Filesystem utilities for atomic operations and file watching.

use crate::constants::{SCRATCH_DIR, STATUS_FILE, STATUS_STOP};
use crossbeam_channel::{self as channel, Receiver, RecvTimeoutError};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

// =============================================================================
// WaitError
// =============================================================================

/// Error type for wait operations.
#[derive(Debug)]
pub enum WaitError {
    /// Pool stop was requested (stop file written).
    Stopped,
    /// I/O error (timeout, disconnect, file error, etc).
    Io(io::Error),
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "pool stopped"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WaitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Stopped => None,
            Self::Io(e) => Some(e),
        }
    }
}

impl From<io::Error> for WaitError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<WaitError> for io::Error {
    fn from(e: WaitError) -> Self {
        match e {
            WaitError::Stopped => io::Error::new(io::ErrorKind::Interrupted, "pool stopped"),
            WaitError::Io(e) => e,
        }
    }
}

// =============================================================================
// Platform-specific event detection
// =============================================================================

/// Check if event kind indicates a file write is complete.
///
/// Platform-specific behavior:
/// - **Linux `inotify`**: Only `Close(Write)` guarantees data is flushed
/// - **macOS `FSEvents`**: `Create(File)` and `Modify(Data)` are accepted
///
/// Also handles atomic rename writes (write temp file, then rename).
#[cfg(target_os = "linux")]
pub const fn is_write_complete(kind: notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode, ModifyKind};
    matches!(
        kind,
        notify::EventKind::Access(AccessKind::Close(AccessMode::Write))
            | notify::EventKind::Modify(ModifyKind::Name(_))
    )
}

#[cfg(target_os = "macos")]
pub const fn is_write_complete(kind: notify::EventKind) -> bool {
    use notify::event::{CreateKind, ModifyKind};
    matches!(
        kind,
        notify::EventKind::Create(CreateKind::File)
            | notify::EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_))
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub const fn is_write_complete(kind: notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind};
    matches!(
        kind,
        notify::EventKind::Access(AccessKind::Close(AccessMode::Write))
            | notify::EventKind::Create(CreateKind::File)
            | notify::EventKind::Modify(ModifyKind::Data(_))
            | notify::EventKind::Modify(ModifyKind::Name(_))
    )
}

/// Write content to a file atomically.
///
/// Writes to a temp file in `<pool_root>/scratch/`, then renames to the target path.
/// This ensures the target file either doesn't exist or contains complete content -
/// never a partial write.
///
/// # Arguments
///
/// * `pool_root` - The pool root directory (must contain `scratch/` subdirectory)
/// * `target` - The final path where the file should appear
/// * `content` - The content to write
///
/// # Errors
///
/// Returns an error if the write or rename fails.
pub fn atomic_write(pool_root: &Path, target: &Path, content: &[u8]) -> io::Result<()> {
    let scratch_dir = pool_root.join(SCRATCH_DIR);
    let temp_path = scratch_dir.join(format!("{}.tmp", Uuid::new_v4()));

    fs::write(&temp_path, content).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E001] failed to write temp file {}: {e}",
                temp_path.display()
            ),
        )
    })?;
    fs::rename(&temp_path, target).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E002] failed to rename {} to {}: {e}",
                temp_path.display(),
                target.display()
            ),
        )
    })?;

    Ok(())
}

/// Write a string to a file atomically.
///
/// Convenience wrapper around [`atomic_write`] for string content.
///
/// # Errors
///
/// Returns an error if the write or rename fails.
pub fn atomic_write_str(pool_root: &Path, target: &Path, content: &str) -> io::Result<()> {
    atomic_write(pool_root, target, content.as_bytes())
}

// =============================================================================
// VerifiedWatcher - File watcher with lazy canary verification
// =============================================================================

/// Guard that cleans up the canary file when dropped.
struct CanaryGuard {
    path: PathBuf,
    dir: PathBuf,
    writes: u32,
}

impl CanaryGuard {
    fn new(dir: &Path) -> io::Result<Self> {
        // Canonicalize to match FSEvents paths (e.g., /tmp -> /private/tmp on macOS)
        let dir = fs::canonicalize(dir).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "[E047] failed to canonicalize canary dir {}: {e}",
                    dir.display()
                ),
            )
        })?;
        let path = dir.join(format!("{}.canary", Uuid::new_v4()));
        fs::write(&path, "0").map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("[E048] failed to write canary file {}: {e}", path.display()),
            )
        })?;
        Ok(Self {
            path,
            dir,
            writes: 0,
        })
    }

    fn dir(&self) -> &Path {
        &self.dir
    }

    fn retry(&mut self) -> io::Result<()> {
        self.writes += 1;
        fs::write(&self.path, self.writes.to_string()).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "[E049] failed to write canary file {} (retry {}): {e}",
                    self.path.display(),
                    self.writes
                ),
            )
        })
    }
}

impl Drop for CanaryGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// A file watcher with canary verification.
///
/// On Linux with inotify, recursive file watching has a race condition: when
/// a new subdirectory is created, there's a window where files can be written
/// before the watch is set up. This watcher uses canary files to verify that
/// watches are operational before proceeding.
///
/// Canaries are removed as their directories are verified. When all canaries
/// are gone, the watcher is fully verified.
pub struct VerifiedWatcher {
    watcher: RecommendedWatcher,
    rx: Receiver<notify::Event>,
    /// Canary guards for directories still being verified.
    /// As directories are verified, their canaries are removed.
    remaining_canaries: Vec<CanaryGuard>,
    /// Path to the stop file (`pool_root/status`). When this file contains
    /// `STATUS_STOP`, wait operations return `WaitError::Stopped`.
    stop_path: PathBuf,
}

impl VerifiedWatcher {
    /// Create a watcher and start canary verification (non-blocking).
    ///
    /// Writes canary files to each directory in `canary_dirs` to verify the
    /// watcher sees events from all of them. Verification happens lazily during
    /// [`wait_for`] or [`into_receiver`] calls.
    ///
    /// # Errors
    ///
    /// Returns an error if the watcher cannot be created or any canary file
    /// cannot be written.
    pub fn new(watch_dir: &Path, canary_dirs: &[PathBuf]) -> io::Result<Self> {
        let (tx, rx) = channel::unbounded();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            Config::default(),
        )
        .map_err(|e| {
            io::Error::other(format!("[E044] failed to create filesystem watcher: {e}"))
        })?;

        watcher
            .watch(watch_dir, RecursiveMode::Recursive)
            .map_err(|e| {
                io::Error::other(format!(
                    "[E045] failed to watch directory {}: {e}",
                    watch_dir.display()
                ))
            })?;

        let remaining_canaries = canary_dirs
            .iter()
            .map(|dir| CanaryGuard::new(dir))
            .collect::<io::Result<Vec<_>>>()?;

        // Assume watch_dir is pool root (true after SINGLE_WATCHER_AT_ENTRY_POINT)
        let stop_path = watch_dir.join(STATUS_FILE);

        Ok(Self {
            watcher,
            rx,
            remaining_canaries,
            stop_path,
        })
    }

    /// Wait for a specific file to appear (no timeout).
    ///
    /// Returns immediately if the file already exists.
    ///
    /// # Errors
    ///
    /// Returns `WaitError::Stopped` if the pool stop was requested.
    /// Returns `WaitError::Io` if the watcher disconnects.
    pub fn wait_for_file(&mut self, target: &Path) -> Result<(), WaitError> {
        self.wait_for_file_impl(target, None)
    }

    /// Wait for a specific file to appear with a timeout.
    ///
    /// Returns immediately if the file already exists.
    ///
    /// # Errors
    ///
    /// Returns `WaitError::Stopped` if the pool stop was requested.
    /// Returns `WaitError::Io` if the wait times out or the watcher disconnects.
    pub fn wait_for_file_with_timeout(
        &mut self,
        target: &Path,
        timeout: Duration,
    ) -> Result<(), WaitError> {
        self.wait_for_file_impl(target, Some(timeout))
    }

    /// Internal implementation for waiting on a file.
    ///
    /// Handles canary verification alongside waiting. Canaries are removed
    /// as their directories are verified. Also monitors the stop file and
    /// returns `WaitError::Stopped` if stop is requested.
    fn wait_for_file_impl(
        &mut self,
        target: &Path,
        timeout: Option<Duration>,
    ) -> Result<(), WaitError> {
        // Fast path: file already exists
        if target.exists() {
            return Ok(());
        }

        // Check if stop was already requested
        if is_stop_requested(&self.stop_path) {
            return Err(WaitError::Stopped);
        }

        let deadline = timeout.map(|t| Instant::now() + t);

        loop {
            let wait_time = match deadline {
                Some(d) => {
                    let remaining = d.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(WaitError::Io(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("[E046] timed out waiting for {}", target.display()),
                        )));
                    }
                    remaining.min(Duration::from_millis(100))
                }
                None => Duration::from_millis(100),
            };

            match self.rx.recv_timeout(wait_time) {
                Ok(event) => {
                    for path in &event.paths {
                        // Remove canary for verified directory
                        if let Some(parent) = path.parent() {
                            self.remaining_canaries.retain(|c| c.dir() != parent);
                        }

                        // Check for stop file
                        if path == &self.stop_path && is_stop_requested(&self.stop_path) {
                            return Err(WaitError::Stopped);
                        }

                        if path == target {
                            return Ok(());
                        }
                    }
                    if target.exists() {
                        return Ok(());
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if target.exists() {
                        return Ok(());
                    }
                    // Retry only unverified canaries
                    for canary in &mut self.remaining_canaries {
                        canary.retry()?;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // The FSEvents stream may have died, but the target file could
                    // already exist. Check before giving up.
                    if target.exists() {
                        return Ok(());
                    }
                    return Err(WaitError::Io(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        format!(
                            "[E003] watcher disconnected while waiting for {}",
                            target.display()
                        ),
                    )));
                }
            }
        }
    }

    /// Consume the watcher and return the raw receiver after verification.
    ///
    /// Blocks until all canary directories are verified, then returns both the
    /// watcher handle and the event receiver. The caller must keep the watcher
    /// in scope - dropping it stops the filesystem watch.
    ///
    /// # Errors
    ///
    /// Returns an error if verification times out.
    pub fn into_receiver(
        mut self,
        timeout: Duration,
    ) -> io::Result<(RecommendedWatcher, Receiver<notify::Event>)> {
        let start = Instant::now();
        while !self.remaining_canaries.is_empty() {
            if start.elapsed() > timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("[E047] watcher canary verification timed out after {timeout:?}"),
                ));
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    for path in &event.paths {
                        if let Some(parent) = path.parent() {
                            self.remaining_canaries.retain(|c| c.dir() != parent);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    for canary in &mut self.remaining_canaries {
                        canary.retry()?;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "[E004] watcher disconnected during canary verification",
                    ));
                }
            }
        }

        // Canaries verified and will be cleaned up when remaining_canaries drops
        Ok((self.watcher, self.rx))
    }
}

/// Check if the stop file contains the stop signal.
fn is_stop_requested(stop_path: &Path) -> bool {
    fs::read_to_string(stop_path)
        .map(|s| s.trim().starts_with(STATUS_STOP))
        .unwrap_or(false)
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_creates_file() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create scratch directory
        fs::create_dir(root.join(SCRATCH_DIR)).unwrap();

        let target = root.join("test.txt");
        atomic_write_str(root, &target, "hello world").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "hello world");
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::create_dir(root.join(SCRATCH_DIR)).unwrap();

        let target = root.join("test.txt");
        fs::write(&target, "old content").unwrap();

        atomic_write_str(root, &target, "new content").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "new content");
    }

    #[test]
    fn temp_file_cleaned_up() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let scratch = root.join(SCRATCH_DIR);
        fs::create_dir(&scratch).unwrap();

        let target = root.join("test.txt");
        atomic_write_str(root, &target, "content").unwrap();

        // Scratch directory should be empty (temp file renamed away)
        let entries: Vec<_> = fs::read_dir(&scratch).unwrap().collect();
        assert!(entries.is_empty());
    }
}
