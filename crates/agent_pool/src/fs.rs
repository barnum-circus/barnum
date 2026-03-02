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
    writes: u32,
}

impl CanaryGuard {
    fn new(path: PathBuf) -> io::Result<Self> {
        fs::write(&path, "0")?;
        Ok(Self { path, writes: 0 })
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
enum WatcherState {
    /// Watcher is operational. Has receiver and optional canary guard.
    /// - `canary: Some(_)` = unverified, still waiting for first event
    /// - `canary: None` = verified, canary was cleaned up
    Connected {
        rx: mpsc::Receiver<PathBuf>,
        canary: Option<CanaryGuard>,
    },
    /// Channel disconnected; watcher is broken.
    Disconnected,
}

/// A file watcher with lazy canary verification.
///
/// # Key Assumption
///
/// Once we observe any filesystem event, the watcher is fully operational.
/// Filesystem watchers (`FSEvents` on macOS, `inotify` on Linux) don't "partially work".
/// The only failure mode is during initial setup - there's a brief window after
/// `watch()` returns where events might not be delivered yet. Once we receive ANY
/// event, we can trust that:
/// - The watcher is fully registered with the kernel
/// - All subsequent filesystem operations in the watched directory will generate events
/// - We won't miss events due to setup races
pub struct VerifiedWatcher {
    _watcher: RecommendedWatcher,
    state: WatcherState,
}

#[allow(clippy::panic)] // Panics are intentional for invalid state transitions
impl VerifiedWatcher {
    /// Create a watcher and start canary verification (non-blocking).
    ///
    /// Writes the canary file but returns immediately. Verification happens
    /// lazily during [`wait_for`] or [`ensure_verified`] calls.
    ///
    /// # Errors
    ///
    /// Returns an error if the watcher cannot be created or the canary file
    /// cannot be written.
    pub fn new(watch_dir: &Path, canary_path: PathBuf) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
            },
            Config::default(),
        )
        .map_err(io::Error::other)?;

        watcher
            .watch(watch_dir, RecursiveMode::Recursive)
            .map_err(io::Error::other)?;

        let canary = CanaryGuard::new(canary_path)?;

        Ok(Self {
            _watcher: watcher,
            state: WatcherState::Connected {
                rx,
                canary: Some(canary),
            },
        })
    }

    /// Wait for a specific file to appear.
    ///
    /// If not yet verified, handles canary verification alongside waiting.
    /// Returns immediately if the file already exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the wait times out before the file appears.
    ///
    /// # Panics
    ///
    /// Panics if called when watcher is disconnected.
    pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
        // Fast path: file already exists
        if target.exists() {
            return Ok(());
        }

        let WatcherState::Connected { rx, canary } = &mut self.state else {
            panic!("wait_for called on disconnected watcher");
        };

        let start = Instant::now();
        loop {
            // Check timeout
            if let Some(t) = timeout
                && start.elapsed() > t
            {
                if target.exists() {
                    return Ok(());
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for {}", target.display()),
                ));
            }

            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(path) => {
                    // Any event proves watcher works
                    if canary.is_some() {
                        *canary = None;
                    }

                    if path == target {
                        return Ok(());
                    }
                    if target.exists() {
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if target.exists() {
                        return Ok(());
                    }
                    if let Some(c) = canary {
                        c.retry()?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.state = WatcherState::Disconnected;
                    panic!("watcher disconnected unexpectedly");
                }
            }
        }
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
