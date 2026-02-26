//! Client operations for interacting with the agent pool daemon.

mod payload;
mod stop;
mod submit;
mod submit_file;

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::constants::STATUS_FILE;

pub use payload::Payload;
pub use stop::stop;
pub use submit::submit;
pub use submit_file::{cleanup_submission, submit_file};

/// Default timeout for waiting for the pool to become ready.
pub const DEFAULT_POOL_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Canary file for verifying watcher is working.
const CANARY_FILE: &str = "client_canary";

/// Wait for the agent pool daemon to become ready.
///
/// Uses a filesystem watcher to efficiently wait for the status file.
/// Verifies the watcher is working via a canary file before waiting.
///
/// # Errors
///
/// Returns an error if the timeout is exceeded before the pool becomes ready.
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    let root = root.as_ref();
    let status_file = root.join(STATUS_FILE);

    // Fast path: already ready
    if status_file.exists() {
        return Ok(());
    }

    // Set up watcher
    let (tx, rx) = mpsc::channel();
    let status_path = status_file.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res
                && event.paths.iter().any(|p| p == &status_path)
            {
                let _ = tx.send(());
            }
        },
        Config::default(),
    )
    .map_err(io::Error::other)?;

    watcher
        .watch(root, RecursiveMode::NonRecursive)
        .map_err(io::Error::other)?;

    // Verify watcher is working via canary
    let canary_path = root.join(CANARY_FILE);
    let (canary_tx, canary_rx) = mpsc::channel();
    let canary_check = canary_path.clone();
    let mut canary_watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res
                && event.paths.iter().any(|p| p == &canary_check)
            {
                let _ = canary_tx.send(());
            }
        },
        Config::default(),
    )
    .map_err(io::Error::other)?;

    canary_watcher
        .watch(root, RecursiveMode::NonRecursive)
        .map_err(io::Error::other)?;

    // Write canary and wait for event
    fs::write(&canary_path, "sync")?;
    let start = Instant::now();
    loop {
        match canary_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(()) => {
                let _ = fs::remove_file(&canary_path);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if start.elapsed() > timeout {
                    let _ = fs::remove_file(&canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "watcher sync timed out",
                    ));
                }
                // Retry with new content
                fs::write(&canary_path, start.elapsed().as_millis().to_string())?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = fs::remove_file(&canary_path);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "canary watcher disconnected",
                ));
            }
        }
    }
    drop(canary_watcher);

    // Check again - status file might exist now
    if status_file.exists() {
        return Ok(());
    }

    // Wait for status file event
    let remaining = timeout.saturating_sub(start.elapsed());
    match rx.recv_timeout(remaining) {
        Ok(()) => Ok(()),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Final check - might have missed the event
            if status_file.exists() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "agent pool did not become ready within {:?} (status file: {})",
                        timeout,
                        status_file.display()
                    ),
                ))
            }
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "watcher channel disconnected",
        )),
    }
}
