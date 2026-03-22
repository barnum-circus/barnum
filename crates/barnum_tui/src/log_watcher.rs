//! Tails the NDJSON state log via filesystem notifications.
//!
//! The [`LogWatcher`] uses `notify` to detect file changes and reads new lines
//! on demand via [`LogWatcher::poll`]. The notify callback only sends a `()`
//! signal through a channel — actual file I/O happens on the caller's thread,
//! avoiding `Fn` vs `FnMut` issues with the watcher callback.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::sync::mpsc;

use barnum_state::StateLogEntry;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// A single event produced by [`LogWatcher::poll`].
#[derive(Debug)]
pub enum LogEvent {
    /// A successfully parsed state log entry.
    Entry(StateLogEntry),
    /// A line that failed to parse.
    #[expect(dead_code, reason = "string captured for future error display in log panel")]
    Error(String),
}

/// Tails an NDJSON state log file, producing [`LogEvent`]s on each poll.
///
/// Create with [`LogWatcher::new`], then call [`poll`](LogWatcher::poll) at
/// your tick interval (~100ms) to drain any new entries.
pub struct LogWatcher {
    reader: BufReader<File>,
    notify_rx: mpsc::Receiver<()>,
    _watcher: RecommendedWatcher,
}

impl LogWatcher {
    /// Open the state log at `path` and begin watching for changes.
    ///
    /// When `replay` is `true`, existing content is readable from the first
    /// [`poll`](LogWatcher::poll) call. When `false`, the reader seeks to the
    /// end and only new lines are returned.
    pub fn new(path: &Path, replay: bool) -> anyhow::Result<Self> {
        let mut file = File::open(path)?;
        if !replay {
            file.seek(SeekFrom::End(0))?;
        }
        let reader = BufReader::new(file);

        let (tx, rx) = mpsc::channel();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                use notify::EventKind::{Create, Modify};
                if matches!(event.kind, Modify(_) | Create(_)) {
                    // Best-effort signal; a dropped receiver is fine.
                    let _ = tx.send(());
                }
            }
        })?;

        // Watch the parent directory so we catch file creation/truncation.
        let watch_dir = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("state log path has no parent directory"))?;
        watcher.watch(watch_dir, RecursiveMode::NonRecursive)?;

        Ok(Self {
            reader,
            notify_rx: rx,
            _watcher: watcher,
        })
    }

    /// Non-blocking drain of all pending log entries.
    ///
    /// Call this every tick. It drains the notification channel, then reads all
    /// complete lines currently available from the file.
    pub fn poll(&mut self) -> Vec<LogEvent> {
        // Drain all pending notifications (we only care that *something* changed).
        while self.notify_rx.try_recv().is_ok() {}

        let mut events = Vec::new();
        let mut line = String::new();

        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => break,   // EOF — no more data right now
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<StateLogEntry>(trimmed) {
                        Ok(entry) => events.push(LogEvent::Entry(entry)),
                        Err(e) => events.push(LogEvent::Error(format!(
                            "failed to parse state log line: {e}"
                        ))),
                    }
                }
                Err(e) => {
                    events.push(LogEvent::Error(format!("read error: {e}")));
                    break;
                }
            }
        }

        events
    }
}
