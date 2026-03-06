# State Persistence and Resume

**Status:** Not started

**Depends on:** ROOT_FLAG_REFACTOR (complete), INLINED_CONFIG (complete)

## Motivation

Long-running GSD jobs can be interrupted (crash, Ctrl+C, OOM). State persistence enables resuming from where you left off.

## Core Idea

A run creates a single NDJSON log file. First entry is config, then task events. On resume, replay log to reconstruct pending tasks.

**Two types of logs (don't confuse them):**
- **Debug logs** (`--log-file`): Tracing output for debugging
- **State logs** (this feature): Machine-readable NDJSON for persistence/resume

## State Log Format

Newline-delimited JSON. First entry MUST be `Config` (exactly once). Uses `#[serde(tag = "kind")]`.

```json
{"kind":"Config","config":{...}}
{"kind":"TaskSubmitted","task_id":1,"step":"Analyze","value":{...},"origin_id":null,"retries":0}
{"kind":"TaskSubmitted","task_id":2,"step":"Analyze","value":{...},"origin_id":null,"retries":0}
{"kind":"TaskCompleted","task_id":1,"outcome":{"kind":"Success","value":{"new_task_ids":[3]}}}
{"kind":"TaskSubmitted","task_id":3,"step":"Process","value":{...},"origin_id":1,"retries":0}
{"kind":"TaskCompleted","task_id":2,"outcome":{"kind":"Failed","value":{"kind":"Timeout"}}}
{"kind":"TaskCompleted","task_id":2,"outcome":{"kind":"Failed","value":{"kind":"InvalidResponse","message":"parse error"}}}
{"kind":"TaskCompleted","task_id":2,"outcome":{"kind":"Success","value":{"new_task_ids":[]}}}
```

## Data Structures

```rust
use serde::{Deserialize, Serialize};
use crate::resolved::Config;

#[derive(Debug, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LogTaskId(pub u64);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StateLogConfig {
    pub config: Config,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskSubmitted {
    pub task_id: LogTaskId,
    pub step: String,
    pub value: serde_json::Value,
    pub origin_id: Option<LogTaskId>,
    pub retries: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskCompleted {
    pub task_id: LogTaskId,
    pub outcome: TaskOutcome,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum TaskOutcome {
    Success(TaskSuccess),
    Failed(FailureReason),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskSuccess {
    pub new_task_ids: Vec<LogTaskId>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum FailureReason {
    Timeout,
    AgentLost,
    InvalidResponse { message: String },
}
```

## Writing/Reading

```rust
fn write_entry(file: &mut File, entry: &StateLogEntry) -> io::Result<()> {
    serde_json::to_writer(&mut *file, entry)?;
    file.write_all(b"\n")?;
    file.flush()
}

fn read_entries(file: File) -> impl Iterator<Item = io::Result<StateLogEntry>> {
    BufReader::new(file).lines().map(|line| {
        let line = line?;
        serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    })
}
```

Validation (config first, config once) happens at the call site.

## Reconstructing State on Resume

```rust
type PendingTasks = HashMap<LogTaskId, TaskSubmitted>;

#[derive(Debug, thiserror::Error)]
enum ReconstructError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("empty log file")]
    EmptyLog,
    #[error("first entry must be Config")]
    FirstEntryNotConfig,
    #[error("Config appeared more than once")]
    DuplicateConfig,
    #[error("duplicate task_id {0:?}")]
    DuplicateTaskId(LogTaskId),
    #[error("TaskCompleted for unknown task_id {0:?}")]
    UnknownTaskId(LogTaskId),
    #[error("task {task_id:?} has retries={retries} which exceeds max_retries={max}")]
    RetriesExceeded { task_id: LogTaskId, retries: u32, max: u32 },
}

fn reconstruct(mut entries: impl Iterator<Item = io::Result<StateLogEntry>>, max_retries: u32) -> Result<(Config, PendingTasks), ReconstructError> {
    // First entry must be Config
    let config = match entries.next() {
        Some(Ok(StateLogEntry::Config(c))) => c.config,
        Some(Ok(_)) => return Err(ReconstructError::FirstEntryNotConfig),
        Some(Err(e)) => return Err(e.into()),
        None => return Err(ReconstructError::EmptyLog),
    };

    let mut pending = PendingTasks::new();

    for entry in entries {
        match entry? {
            StateLogEntry::Config(_) => {
                return Err(ReconstructError::DuplicateConfig);
            }
            StateLogEntry::TaskSubmitted(task) => {
                if task.retries > max_retries {
                    return Err(ReconstructError::RetriesExceeded {
                        task_id: task.task_id,
                        retries: task.retries,
                        max: max_retries,
                    });
                }
                if pending.contains_key(&task.task_id) {
                    return Err(ReconstructError::DuplicateTaskId(task.task_id));
                }
                pending.insert(task.task_id, task);
            }
            StateLogEntry::TaskCompleted(c) => {
                if pending.remove(&c.task_id).is_none() {
                    return Err(ReconstructError::UnknownTaskId(c.task_id));
                }
            }
        }
    }

    Ok((config, pending))
}
```

## CLI

```bash
# Normal run (no persistence)
gsd run config.jsonc --pool mypool --initial-state '[...]'

# Run with state logging
gsd run config.jsonc --pool mypool --initial-state '[...]' --state-log /tmp/myrun.ndjson

# Resume from state log
gsd run --resume-from /tmp/myrun.ndjson
```

`--state-log` and `--resume-from` are mutually exclusive. `--resume-from` is also mutually exclusive with config file and `--initial-state` (they come from the log).

## Implementation Phases

### Phase 1: Data Structures
- Add `state_log.rs` with types and read/write functions

### Phase 2: CLI Integration
- Add `--state-log <path>` flag
- Write config as first entry on startup
- Print: `State log: <path>. Resume with: gsd run --resume-from <path>`

### Phase 3: Task Logging
- Write `TaskSubmitted` when task queued
- Write `TaskCompleted` with `Success` or `Failed` outcome when task resolves
- Flush after each write

### Phase 4: Resume
- Add `--resume-from <path>` flag
- Parse log, reconstruct pending tasks with failure counts
- Check retries exhausted, fail run if so
- Continue with remaining pending tasks

## What We Don't Track (v1)

- **In-flight tasks**: Lost on crash. May cause duplicate work on resume.
- **Finally hook state**: Won't fire correctly if interrupted mid-fan-out.

## Future Work

- Visualize state logs (TUI or web UI)
- `gsd runs list` command to show run status
