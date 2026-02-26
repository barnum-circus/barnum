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
    enum Event {
        Canary,
        Status,
    }

    let root = root.as_ref();

    if !root.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("pool directory does not exist: {}", root.display()),
        ));
    }

    let status_file = root.join(STATUS_FILE);
    let canary_path = root.join(CANARY_FILE);

    let (tx, rx) = mpsc::channel();
    let status_check = status_file.clone();
    let canary_check = canary_path.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                for path in &event.paths {
                    if path == &canary_check {
                        let _ = tx.send(Event::Canary);
                    } else if path == &status_check {
                        let _ = tx.send(Event::Status);
                    }
                }
            }
        },
        Config::default(),
    )
    .map_err(io::Error::other)?;

    watcher
        .watch(root, RecursiveMode::NonRecursive)
        .map_err(io::Error::other)?;

    // Verify watcher is live via canary file
    fs::write(&canary_path, "sync")?;
    let start = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Canary) => {
                let _ = fs::remove_file(&canary_path);
                break;
            }
            Ok(Event::Status) => {
                let _ = fs::remove_file(&canary_path);
                return Ok(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if start.elapsed() > timeout {
                    let _ = fs::remove_file(&canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "watcher sync timed out",
                    ));
                }
                fs::write(&canary_path, start.elapsed().as_millis().to_string())?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = fs::remove_file(&canary_path);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher disconnected",
                ));
            }
        }
    }

    if status_file.exists() {
        return Ok(());
    }

    // Wait for status file
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        match rx.recv_timeout(remaining) {
            Ok(Event::Status) => return Ok(()),
            Ok(Event::Canary) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if status_file.exists() {
                    return Ok(());
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "pool not ready within {timeout:?} (status file: {})",
                        status_file.display()
                    ),
                ));
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
