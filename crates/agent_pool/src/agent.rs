//! Agent-side event loop for waiting on tasks.
//!
//! This module provides `notify`-based waiting functions for agents. Instead of
//! polling the filesystem with `thread::sleep`, agents use these functions to
//! block on file events.

use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::io;
use std::sync::mpsc;
use std::time::Duration;

use crate::{RESPONSE_FILE, TASK_FILE, Transport};

/// Events the agent cares about.
#[derive(Debug)]
pub enum AgentEvent {
    /// A file changed in the agent directory.
    FileChanged,
    /// The watcher encountered an error.
    WatchError(notify::Error),
}

/// Create a watcher for a directory-based transport.
///
/// Watches the PARENT directory with recursive mode so that the agent directory
/// doesn't need to exist yet. This allows setting up the watcher before creating
/// the agent directory, avoiding race conditions where we miss events.
///
/// Returns the watcher (keep alive) and a receiver for events.
/// For socket-based transports, returns `None` (sockets are already event-driven).
///
/// # Errors
///
/// Returns an error if the filesystem watcher cannot be created or the parent
/// directory doesn't exist.
pub fn create_watcher(
    transport: &Transport,
) -> io::Result<Option<(RecommendedWatcher, mpsc::Receiver<AgentEvent>)>> {
    let Some(dir) = transport.path() else {
        // Socket transport - no filesystem watcher needed
        return Ok(None);
    };

    // Watch parent directory so we can set up watcher before agent dir exists
    let parent = dir.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "agent directory has no parent")
    })?;

    let (tx, rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| match res {
            Ok(event) => {
                tracing::trace!(?event, "watcher event");
                // Signal on events that indicate file content is available:
                // - Close(Write) on Linux inotify
                // - Create(File) or Modify(Data) on macOS FSEvents
                let dominated_event = matches!(
                    event.kind,
                    notify::EventKind::Access(AccessKind::Close(AccessMode::Write))
                        | notify::EventKind::Create(CreateKind::File)
                        | notify::EventKind::Modify(ModifyKind::Data(_))
                );
                if dominated_event {
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

    // Recursive mode to see events in subdirectories (agent directories)
    watcher
        .watch(parent, RecursiveMode::Recursive)
        .map_err(io::Error::other)?;

    Ok(Some((watcher, rx)))
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
#[expect(clippy::expect_used, clippy::unwrap_used)]
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
    fn create_watcher_directory_transport() {
        let dir = test_dir("watcher");
        let transport = Transport::Directory(dir);
        let result = create_watcher(&transport);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }
}
