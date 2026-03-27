//! Barnum event loop: receives workflow execution events and dispatches to appliers.
//!
//! The event loop receives [`Event`]s from a Tokio MPSC channel and calls
//! [`Applier::apply`] on each registered applier for every event. Appliers are
//! called in order, synchronously within the loop iteration.
//!
//! Two built-in appliers:
//! - [`NdjsonApplier`]: writes every event as a JSON line to a run log file.
//! - [`EngineApplier`]: drives the workflow AST evaluator (currently a stub).

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use barnum_ast::HandlerKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// =============================================================================
// Events
// =============================================================================

/// An event produced during workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Event {
    /// A handler invocation has started.
    TaskStarted(TaskStartedEvent),
    /// A handler invocation has completed (success or failure).
    TaskCompleted(TaskCompletedEvent),
}

/// A handler has started executing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStartedEvent {
    /// Unique ID for this invocation.
    pub task_id: String,
    /// Which handler is being invoked (TypeScript, Bash, etc.).
    pub handler: HandlerKind,
    /// The input value passed to the handler.
    pub value: Value,
}

/// A handler has finished executing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCompletedEvent {
    /// Unique ID for this invocation (matches the corresponding `TaskStarted`).
    pub task_id: String,
    /// The outcome of the invocation.
    pub result: TaskResult,
}

/// Outcome of a handler invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum TaskResult {
    /// Handler returned successfully.
    Success {
        /// The returned value.
        value: Value,
    },
    /// Handler failed.
    Failure {
        /// Error description.
        error: String,
    },
}

// =============================================================================
// Applier trait
// =============================================================================

/// Processes workflow execution events.
///
/// The event loop holds a `Vec<Box<dyn Applier>>` and calls [`Applier::apply`]
/// on each applier for every event received.
pub trait Applier: Send {
    /// Process a single event.
    fn apply(&mut self, event: &Event);
}

// =============================================================================
// NDJSON Applier
// =============================================================================

/// Writes every event as a JSON line to an NDJSON file.
///
/// Default file location: `/tmp/barnum/runs/{unix_timestamp}_{uuid}.ndjson`.
pub struct NdjsonApplier {
    file: File,
    path: PathBuf,
}

impl NdjsonApplier {
    /// Create a new NDJSON applier writing to `/tmp/barnum/runs/`.
    ///
    /// Creates the directory if it doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory can't be created or the file can't be opened.
    pub fn new() -> std::io::Result<Self> {
        Self::with_dir(Path::new("/tmp/barnum/runs"))
    }

    /// Create a new NDJSON applier writing to a custom directory.
    ///
    /// Creates the directory if it doesn't exist. The file is named
    /// `{unix_timestamp}_{uuid}.ndjson`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory can't be created or the file can't be opened.
    pub fn with_dir(runs_dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(runs_dir)?;

        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let id = Uuid::new_v4();
        let filename = format!("{timestamp}_{id}.ndjson");
        let path = runs_dir.join(filename);

        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        Ok(Self { file, path })
    }

    /// The path to the NDJSON file for this run.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Applier for NdjsonApplier {
    fn apply(&mut self, event: &Event) {
        // Serialization can't fail for our Event types (all fields are Serialize).
        // File write errors are silently dropped — losing a log line shouldn't
        // crash the workflow.
        if let Ok(json) = serde_json::to_string(event) {
            let _ = writeln!(self.file, "{json}");
        }
    }
}

// =============================================================================
// Engine Applier (stub)
// =============================================================================

/// The workflow execution engine. Receives events and drives the AST evaluator.
///
/// Currently a stub — will be wired to the evaluator that walks `barnum_ast::Action`
/// trees and dispatches handler calls.
pub struct EngineApplier;

impl Applier for EngineApplier {
    fn apply(&mut self, _event: &Event) {
        // Stub: will drive the AST evaluator based on task completions.
    }
}

// =============================================================================
// Event Loop
// =============================================================================

/// Run the event loop until the sender is dropped.
///
/// Receives events from the Tokio MPSC channel and dispatches each to every
/// applier in order.
pub async fn run_event_loop(
    mut receiver: tokio::sync::mpsc::Receiver<Event>,
    appliers: &mut [Box<dyn Applier>],
) {
    while let Some(event) = receiver.recv().await {
        for applier in appliers.iter_mut() {
            applier.apply(&event);
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use intern::string_key::Intern as _;

    #[test]
    fn ndjson_applier_writes_events() {
        let dir = tempfile::tempdir().ok();
        let Some(dir) = dir.as_ref() else {
            return;
        };

        let mut applier = NdjsonApplier::with_dir(dir.path());
        let Ok(applier) = applier.as_mut() else {
            return;
        };

        let event = Event::TaskStarted(TaskStartedEvent {
            task_id: "t1".to_owned(),
            handler: HandlerKind::TypeScript(barnum_ast::TypeScriptHandler {
                module: "/app/handlers/setup.ts".intern().into(),
                func: "default".intern().into(),
                step_config_schema: None,
                value_schema: None,
            }),
            value: serde_json::json!({"project": "my-app"}),
        });
        applier.apply(&event);

        let event = Event::TaskCompleted(TaskCompletedEvent {
            task_id: "t1".to_owned(),
            result: TaskResult::Success {
                value: serde_json::json!({"initialized": true}),
            },
        });
        applier.apply(&event);

        let contents = fs::read_to_string(applier.path());
        let Ok(contents) = contents else {
            return;
        };
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("TaskStarted"));
        assert!(lines[1].contains("TaskCompleted"));
    }

    #[tokio::test]
    async fn event_loop_dispatches_to_appliers() {
        let (tx, rx) = tokio::sync::mpsc::channel(16);

        let dir = tempfile::tempdir().ok();
        let Some(dir) = dir.as_ref() else {
            return;
        };

        let ndjson = NdjsonApplier::with_dir(dir.path());
        let Ok(ndjson) = ndjson else {
            return;
        };
        let ndjson_path = ndjson.path().to_owned();

        let mut appliers: Vec<Box<dyn Applier>> = vec![Box::new(ndjson), Box::new(EngineApplier)];

        let event = Event::TaskStarted(TaskStartedEvent {
            task_id: "t1".to_owned(),
            handler: HandlerKind::TypeScript(barnum_ast::TypeScriptHandler {
                module: "/app/handlers/setup.ts".intern().into(),
                func: "default".intern().into(),
                step_config_schema: None,
                value_schema: None,
            }),
            value: serde_json::json!({"project": "my-app"}),
        });
        tx.send(event).await.ok();
        drop(tx);

        run_event_loop(rx, &mut appliers).await;

        let contents = fs::read_to_string(&ndjson_path);
        let Ok(contents) = contents else {
            return;
        };
        assert_eq!(contents.lines().count(), 1);
    }
}
