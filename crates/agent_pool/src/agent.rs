//! Agent-side event loop for waiting on tasks.
//!
//! This module provides `notify`-based waiting functions for agents. Instead of
//! polling the filesystem with `thread::sleep`, agents use these functions to
//! block on file events.

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::io;
use std::sync::mpsc;
use std::time::Duration;

use crate::{RESPONSE_FILE, TASK_FILE, Transport};

/// Check if an event kind indicates a file write is complete.
///
/// Platform-specific:
/// - Linux inotify: `Close(Write)` guarantees the file handle is closed
/// - macOS `FSEvents`: `Create(File)` or `Modify(Data)` - by the time we receive
///   these, the operation is complete
///
/// Also handles atomic rename writes (write temp file, then rename).
#[cfg(target_os = "linux")]
const fn is_file_write_event(kind: notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode, ModifyKind};
    matches!(
        kind,
        notify::EventKind::Access(AccessKind::Close(AccessMode::Write))
            | notify::EventKind::Modify(ModifyKind::Name(_))
    )
}

#[cfg(target_os = "macos")]
const fn is_file_write_event(kind: notify::EventKind) -> bool {
    use notify::event::{CreateKind, ModifyKind};
    matches!(
        kind,
        notify::EventKind::Create(CreateKind::File)
            | notify::EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_))
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const fn is_file_write_event(kind: notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind};
    matches!(
        kind,
        notify::EventKind::Access(AccessKind::Close(AccessMode::Write))
            | notify::EventKind::Create(CreateKind::File)
            | notify::EventKind::Modify(ModifyKind::Data(_))
            | notify::EventKind::Modify(ModifyKind::Name(_))
    )
}

/// Events the agent cares about.
#[derive(Debug)]
pub enum AgentEvent {
    /// A file changed in the agent directory.
    FileChanged,
    /// The watcher encountered an error.
    WatchError(notify::Error),
}

/// Create a watcher for a directory.
///
/// Watches the given directory with recursive mode. The caller should pass
/// the parent of the agent directory so the watcher can be set up before
/// the agent directory exists.
///
/// Returns the watcher (keep alive) and a receiver for events.
///
/// # Errors
///
/// Returns an error if the filesystem watcher cannot be created.
pub fn create_watcher(
    watch_dir: &std::path::Path,
) -> io::Result<(RecommendedWatcher, mpsc::Receiver<AgentEvent>)> {
    let (tx, rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| match res {
            Ok(event) => {
                tracing::trace!(?event, "watcher event");
                if is_file_write_event(event.kind) {
                    tracing::debug!(?event.kind, "watcher sending FileChanged");
                    let _ = tx.send(AgentEvent::FileChanged);
                }
            }
            Err(e) => {
                tracing::warn!(?e, "watcher error");
                let _ = tx.send(AgentEvent::WatchError(e));
            }
        },
        Config::default(),
    )
    .map_err(io::Error::other)?;

    watcher
        .watch(watch_dir, RecursiveMode::Recursive)
        .map_err(io::Error::other)?;

    Ok((watcher, rx))
}

/// Canary file used to verify watcher is receiving events.
const CANARY_FILE: &str = "canary";

/// Verify the watcher is actually receiving events by writing a canary file.
///
/// This is critical for avoiding race conditions: `watch()` returns immediately
/// but `FSEvents` may not be fully set up yet. Writing a canary and waiting for
/// the event confirms the watcher is live.
///
/// # Errors
///
/// Returns an error if the canary write fails or if no event is received
/// within the timeout.
pub fn verify_watcher_sync(
    watch_dir: &std::path::Path,
    events_rx: &mpsc::Receiver<AgentEvent>,
    timeout: Duration,
) -> io::Result<()> {
    use std::fs;
    use std::time::Instant;

    let canary_path = watch_dir.join(CANARY_FILE);
    let poll_interval = Duration::from_millis(100);
    let start = Instant::now();
    let mut retry_count = 0u32;

    // Write initial canary
    fs::write(&canary_path, "sync")?;

    loop {
        match events_rx.recv_timeout(poll_interval) {
            Ok(AgentEvent::FileChanged) => {
                // Got an event - watcher is working
                let _ = fs::remove_file(&canary_path);
                tracing::debug!("watcher sync verified via canary");
                return Ok(());
            }
            Ok(AgentEvent::WatchError(e)) => {
                let _ = fs::remove_file(&canary_path);
                return Err(io::Error::other(format!("watcher error during sync: {e}")));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if start.elapsed() > timeout {
                    let _ = fs::remove_file(&canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "watcher sync timed out - no events received",
                    ));
                }
                // Retry by rewriting canary with new value
                retry_count += 1;
                fs::write(&canary_path, retry_count.to_string())?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = fs::remove_file(&canary_path);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher channel disconnected during sync",
                ));
            }
        }
    }
}

/// Check if a task is ready to be processed (file-based only).
///
/// Returns `true` when `task.json` exists and `response.json` does not.
/// For socket-based transports, always returns `false` (task readiness is
/// determined by blocking on the socket).
#[must_use]
pub fn is_task_ready(transport: &Transport) -> bool {
    let Some(dir) = transport.path() else {
        // Socket transport - task readiness is handled by blocking read
        return false;
    };
    let task_file = dir.join(TASK_FILE);
    let response_file = dir.join(RESPONSE_FILE);
    task_file.exists() && !response_file.exists()
}

/// Wait for a task to be ready (file-based transports).
///
/// Blocks until `task.json` exists and `response.json` does not.
/// The condition `task.exists() && !response.exists()` handles all cases:
/// - After writing response: keeps waiting until daemon cleans up and assigns new task
/// - Fresh start: waits for first task assignment
///
/// For socket-based transports, just call `transport.read()` which blocks.
///
/// # Errors
///
/// Returns an error if the watcher encounters an error or the channel closes.
pub fn wait_for_task(
    transport: &Transport,
    events_rx: &mpsc::Receiver<AgentEvent>,
) -> io::Result<()> {
    if is_task_ready(transport) {
        return Ok(());
    }

    loop {
        match events_rx.recv() {
            Ok(AgentEvent::FileChanged) => {
                if is_task_ready(transport) {
                    return Ok(());
                }
            }
            Ok(AgentEvent::WatchError(e)) => {
                return Err(io::Error::other(e));
            }
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher channel closed",
                ));
            }
        }
    }
}

/// Wait for a task to be ready with a timeout.
///
/// Like `wait_for_task`, but returns `Ok(false)` on timeout instead of blocking
/// forever. This allows callers to periodically check a shutdown flag.
///
/// Returns:
/// - `Ok(true)` when a task is ready
/// - `Ok(false)` on timeout (no task ready yet)
/// - `Err(...)` on watcher error or channel close
///
/// # Errors
///
/// Returns an error if the watcher encounters an error or the channel closes.
pub fn wait_for_task_with_timeout(
    transport: &Transport,
    events_rx: &mpsc::Receiver<AgentEvent>,
    timeout: Duration,
) -> io::Result<bool> {
    if is_task_ready(transport) {
        return Ok(true);
    }

    loop {
        match events_rx.recv_timeout(timeout) {
            Ok(AgentEvent::FileChanged) => {
                if is_task_ready(transport) {
                    return Ok(true);
                }
            }
            Ok(AgentEvent::WatchError(e)) => {
                return Err(io::Error::other(e));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Ok(false);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher channel closed",
                ));
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".test-data")
            .join("agent")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn is_task_ready_no_files() {
        let dir = test_dir("no_files");
        let transport = Transport::Directory(dir);
        assert!(!is_task_ready(&transport));
    }

    #[test]
    fn is_task_ready_task_only() {
        let dir = test_dir("task_only");
        fs::write(dir.join(TASK_FILE), "{}").expect("write task");
        let transport = Transport::Directory(dir);
        assert!(is_task_ready(&transport));
    }

    #[test]
    fn is_task_ready_both_files() {
        let dir = test_dir("both_files");
        fs::write(dir.join(TASK_FILE), "{}").expect("write task");
        fs::write(dir.join(RESPONSE_FILE), "{}").expect("write response");
        let transport = Transport::Directory(dir);
        assert!(!is_task_ready(&transport));
    }

    #[test]
    fn is_task_ready_response_only() {
        let dir = test_dir("response_only");
        fs::write(dir.join(RESPONSE_FILE), "{}").expect("write response");
        let transport = Transport::Directory(dir);
        assert!(!is_task_ready(&transport));
    }

    #[test]
    fn create_watcher_works() {
        let dir = test_dir("watcher");
        let result = create_watcher(&dir);
        assert!(result.is_ok());
    }
}
