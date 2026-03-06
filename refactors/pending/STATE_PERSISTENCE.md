# State Persistence and Resume

**Status:** Not started

**Depends on:** ROOT_FLAG_REFACTOR (complete), INLINED_CONFIG (complete)

## Motivation

Long-running GSD jobs can be interrupted (crash, Ctrl+C, OOM). State persistence enables resuming from where you left off.

## Core Idea

A run creates a single NDJSON log file. The first entry is always the config, followed by task events. On resume, replay the log to reconstruct pending tasks.

**Two types of logs (don't confuse them):**
- **Debug logs** (`--log-file`): Tracing output for debugging. Human-readable, not structured.
- **State logs** (this feature): Machine-readable NDJSON for persistence/resume.

## Log File Location

```
<root>/runs/<pool>/<run-id>.ndjson
```

Example: `/tmp/agent_pool/runs/mypool/a3f2c1.ndjson`

## State Log Format

Newline-delimited JSON. First entry MUST be `Config`, which MUST NOT appear again. Uses `#[serde(tag = "kind")]` with UpperCamel variants.

```json
{"kind":"Config","config":{...}}
{"kind":"TaskSubmitted","task_id":1,"step":"Analyze","value":{...},"origin_id":null}
{"kind":"TaskSubmitted","task_id":2,"step":"Analyze","value":{...},"origin_id":null}
{"kind":"TaskCompleted","task_id":1,"new_task_ids":[3]}
{"kind":"TaskSubmitted","task_id":3,"step":"Process","value":{...},"origin_id":1}
{"kind":"TaskRequeued","task_id":2,"reason":"timeout","retry_count":1}
{"kind":"TaskDropped","task_id":2,"reason":"max retries exceeded"}
```

## Data Structures

```rust
// crates/gsd_config/src/state_log.rs

use serde::{Deserialize, Serialize};
use crate::resolved::Config;

/// A state log entry. First entry must be Config; Config must not appear again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StateLogEntry {
    /// The resolved config. Must be first entry, must appear exactly once.
    Config { config: Config },
    /// Task submitted to the queue.
    TaskSubmitted {
        task_id: u64,
        step: String,
        value: serde_json::Value,
        /// ID of the task that spawned this one (None for initial tasks).
        origin_id: Option<u64>,
    },
    /// Task completed successfully.
    TaskCompleted {
        task_id: u64,
        /// IDs of newly spawned tasks.
        new_task_ids: Vec<u64>,
    },
    /// Task requeued for retry.
    TaskRequeued {
        task_id: u64,
        reason: String,
        retry_count: u32,
    },
    /// Task dropped (no more retries).
    TaskDropped {
        task_id: u64,
        reason: String,
    },
    /// Task skipped (e.g., step not found).
    TaskSkipped {
        task_id: u64,
        reason: String,
    },
}
```

## Writer/Reader with Validation

```rust
/// Writes state log entries with validation.
pub struct StateLogWriter<W> {
    writer: W,
    config_written: bool,
}

impl<W: Write> StateLogWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer, config_written: false }
    }

    pub fn write(&mut self, entry: &StateLogEntry) -> io::Result<()> {
        match entry {
            StateLogEntry::Config { .. } => {
                if self.config_written {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Config entry already written",
                    ));
                }
                self.config_written = true;
            }
            _ => {
                if !self.config_written {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "First entry must be Config",
                    ));
                }
            }
        }
        serde_json::to_writer(&mut self.writer, entry)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }
}

/// Reads state log entries with validation.
pub struct StateLogReader<R> {
    reader: Lines<BufReader<R>>,
    config_read: bool,
}

impl<R: Read> StateLogReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader).lines(),
            config_read: false,
        }
    }
}

impl<R: Read> Iterator for StateLogReader<R> {
    type Item = io::Result<StateLogEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        let line = self.reader.next()?;
        let line = match line {
            Ok(l) => l,
            Err(e) => return Some(Err(e)),
        };

        let entry: StateLogEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(e) => return Some(Err(io::Error::new(io::ErrorKind::InvalidData, e))),
        };

        // Validate config constraints
        match &entry {
            StateLogEntry::Config { .. } => {
                if self.config_read {
                    return Some(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Config entry appeared more than once",
                    )));
                }
                self.config_read = true;
            }
            _ => {
                if !self.config_read {
                    return Some(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "First entry must be Config",
                    )));
                }
            }
        }

        Some(Ok(entry))
    }
}
```

## Reconstructing State on Resume

```rust
fn reconstruct_pending(entries: &[StateLogEntry]) -> HashMap<u64, PendingTask> {
    let mut pending: HashMap<u64, PendingTask> = HashMap::new();

    for entry in entries {
        match entry {
            StateLogEntry::Config { .. } => {}
            StateLogEntry::TaskSubmitted { task_id, step, value, origin_id } => {
                pending.insert(*task_id, PendingTask {
                    step: step.clone(),
                    value: value.clone(),
                    origin_id: *origin_id,
                });
            }
            StateLogEntry::TaskCompleted { task_id, .. }
            | StateLogEntry::TaskDropped { task_id, .. }
            | StateLogEntry::TaskSkipped { task_id, .. } => {
                pending.remove(task_id);
            }
            StateLogEntry::TaskRequeued { .. } => {
                // Task stays pending
            }
        }
    }

    pending
}
```

## Implementation Phases

### Phase 1: State Log Infrastructure

**Changes:**
- Add `state_log.rs` with `StateLogEntry`, `StateLogWriter`, `StateLogReader`
- Runtime validation that Config is first and only once

### Phase 2: CLI Integration

**Changes:**
- Add `--state-log <path>` CLI flag (creates log file at path)
- On startup with `--state-log`:
  - Create log file
  - Write Config entry first
  - Print: `State log: <path>. Resume with: gsd run --resume-from <path>`
- Log file is NOT deleted on completion (for debugging/auditing)

### Phase 3: Task Logging

**Changes:**
- After task submission, write `TaskSubmitted` entry
- After task completes/fails, write appropriate entry
- Flush after each write for durability

### Phase 4: Resume from Log

**Changes:**
- Add `--resume-from <path>` flag (mutually exclusive with config file + initial state)
- Parse log file, extract config from first entry
- Reconstruct pending tasks
- Continue from where we left off

## CLI

```bash
# Normal run (no persistence)
gsd run config.jsonc --pool mypool --initial-state '[{"kind": "Start", "value": {}}]'

# Run with state logging for resume capability
gsd run config.jsonc --pool mypool --initial-state '[...]' --state-log /tmp/myrun.ndjson
# Prints: State log: /tmp/myrun.ndjson. Resume with: gsd run --resume-from /tmp/myrun.ndjson

# Resume from state log (config is in the log, no config.jsonc needed)
gsd run --resume-from /tmp/myrun.ndjson
```

## What We Don't Track (v1)

- **In-flight tasks**: On resume, tasks being processed are lost. May cause duplicate work.
- **Finally hook state**: On resume, finally hooks won't fire correctly if mid-fan-out.

## Future Work

### Visualization

Add to todos.md:
- Visualize state logs (TUI or web UI for viewing run history/events)

### List Runs

```bash
gsd runs list --dir /tmp/agent_pool/runs
# Shows:
# mypool/a3f2c1.ndjson (3 pending, 5 completed, 2 failed)
# mypool/b7d4e2.ndjson (0 pending, 12 completed, 0 failed) [complete]
```
