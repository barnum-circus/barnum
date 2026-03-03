//! Filesystem utilities for atomic operations and file watching.

use crate::constants::SCRATCH_DIR;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use uuid::Uuid;

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

    fs::write(&temp_path, content)?;
    fs::rename(&temp_path, target)?;

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
        let dir = fs::canonicalize(dir)?;
        let path = dir.join(format!("{}.canary", Uuid::new_v4()));
        fs::write(&path, "0")?;
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
        fs::write(&self.path, self.writes.to_string())
    }
}

impl Drop for CanaryGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Internal state of the watcher.
struct WatcherState {
    rx: mpsc::Receiver<notify::Event>,
    /// Canary guards for directories still being verified.
    /// As directories are verified, their canaries are removed.
    remaining_canaries: Vec<CanaryGuard>,
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
    state: WatcherState,
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
        let (tx, rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            Config::default(),
        )
        .map_err(io::Error::other)?;

        watcher
            .watch(watch_dir, RecursiveMode::Recursive)
            .map_err(io::Error::other)?;

        let remaining_canaries = canary_dirs
            .iter()
            .map(|dir| CanaryGuard::new(dir))
            .collect::<io::Result<Vec<_>>>()?;

        Ok(Self {
            watcher,
            state: WatcherState {
                rx,
                remaining_canaries,
            },
        })
    }

    /// Wait for a specific file to appear.
    ///
    /// Handles canary verification alongside waiting. Canaries are removed
    /// as their directories are verified.
    ///
    /// Returns immediately if the file already exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the wait times out before the file appears.
    pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
        // Fast path: file already exists
        if target.exists() {
            return Ok(());
        }

        let WatcherState {
            rx,
            remaining_canaries,
        } = &mut self.state;

        let start = Instant::now();
        loop {
            // Check timeout
            if let Some(t) = timeout
                && start.elapsed() > t
            {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for {}", target.display()),
                ));
            }

            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    for path in &event.paths {
                        // Remove canary for verified directory
                        if let Some(parent) = path.parent() {
                            remaining_canaries.retain(|c| c.dir() != parent);
                        }

                        if path == target {
                            return Ok(());
                        }
                    }
                    if target.exists() {
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if target.exists() {
                        return Ok(());
                    }
                    // Retry only unverified canaries
                    for canary in remaining_canaries.iter_mut() {
                        canary.retry()?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "watcher disconnected",
                    ));
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
        self,
        timeout: Duration,
    ) -> io::Result<(RecommendedWatcher, mpsc::Receiver<notify::Event>)> {
        let WatcherState {
            rx,
            mut remaining_canaries,
        } = self.state;

        let start = Instant::now();
        while !remaining_canaries.is_empty() {
            if start.elapsed() > timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "verification timed out",
                ));
            }

            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    for path in &event.paths {
                        if let Some(parent) = path.parent() {
                            remaining_canaries.retain(|c| c.dir() != parent);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    for canary in &mut remaining_canaries {
                        canary.retry()?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "watcher disconnected",
                    ));
                }
            }
        }

        // Canaries verified and will be cleaned up when remaining_canaries drops
        Ok((self.watcher, rx))
    }
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
