//! Tails the NDJSON state log and yields parsed events.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::sync::mpsc;

use barnum_state::StateLogEntry;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind};

/// An event parsed from the state log.
#[derive(Debug)]
pub enum LogEvent {
    Entry(StateLogEntry),
    Error(String),
}

/// Watches a state log file and yields new entries as they're appended.
pub struct LogWatcher {
    reader: BufReader<File>,
    notify_rx: mpsc::Receiver<()>,
    _watcher: RecommendedWatcher,
}

impl LogWatcher {
    /// Create a new watcher.
    ///
    /// If `replay` is true, reads from the beginning of the file.
    /// If `replay` is false, seeks to the end and only tails new entries.
    pub fn new(path: &Path, replay: bool) -> anyhow::Result<Self> {
        let mut file = File::open(path)?;

        if !replay {
            file.seek(SeekFrom::End(0))?;
        }

        let reader = BufReader::new(file);

        // Create channel for notify signals
        let (notify_tx, notify_rx) = mpsc::channel();

        // Create the watcher - watch parent directory for reliability
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                // Only signal on modify or create events
                let dominated = matches!(
                    event.kind,
                    notify::EventKind::Modify(ModifyKind::Data(_))
                        | notify::EventKind::Create(_)
                        | notify::EventKind::Modify(ModifyKind::Any)
                );
                if dominated {
                    let _ = notify_tx.send(());
                }
            }
        })?;

        // Watch the parent directory - more reliable than watching file directly
        if let Some(parent) = path.parent() {
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
        } else {
            watcher.watch(path, RecursiveMode::NonRecursive)?;
        }

        Ok(Self {
            reader,
            notify_rx,
            _watcher: watcher,
        })
    }

    /// Drain all pending events (non-blocking). Call every tick (~100ms).
    pub fn poll(&mut self) -> Vec<LogEvent> {
        // Drain notify signals to clear the channel (don't care about count)
        while self.notify_rx.try_recv().is_ok() {}

        // Read all available lines
        let mut events = Vec::new();
        let mut line = String::new();

        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<StateLogEntry>(trimmed) {
                        Ok(entry) => events.push(LogEvent::Entry(entry)),
                        Err(e) => events.push(LogEvent::Error(format!(
                            "Failed to parse log entry: {e}"
                        ))),
                    }
                }
                Err(e) => {
                    events.push(LogEvent::Error(format!("Read error: {e}")));
                    break;
                }
            }
        }

        events
    }
}
