# State Persistence and Resume

**Status:** Not started

**Depends on:** ROOT_FLAG_REFACTOR (complete), INLINED_CONFIG (complete), FINALLY_TRACKING (complete), FINALLY_SCHEDULING

## Motivation

Long-running GSD jobs can be interrupted (crash, Ctrl+C, OOM). State persistence enables resuming from where you left off.

## Core Idea

A run creates a single NDJSON log file. First entry is config, then task events. On resume, replay log to reconstruct pending tasks.

**Two types of logs (don't confuse them):**
- **Debug logs** (`--log-file`): Tracing output for debugging
- **State logs** (this feature): Machine-readable NDJSON for persistence/resume

## Current Runner Architecture (as of 2026-03-08)

The runner module (`crates/gsd_config/src/runner/`) has these submodules:
- `mod.rs` - TaskRunner struct, main loop, `run()` public function
- `types.rs` - TaskEntry, TaskState, Continuation, InFlight, TaskIdentity, InFlightResult, RunnerConfig
- `dispatch.rs` - TaskContext, dispatch_pool_task, dispatch_command_task
- `finally.rs` - `run_finally_hook_direct()` (synchronous finally execution)
- `hooks.rs` - run_pre_hook, run_post_hook
- `shell.rs` - `run_shell_command()` helper
- `response.rs` - Response processing and retry logic
- `submit.rs` - CLI invocation for agent_pool

Key current state:
- `initial_tasks` passed separately from `RunnerConfig`
- `RunnerConfig` passed by reference
- Unified task state in `BTreeMap<LogTaskId, TaskEntry>`:
  - `TaskEntry { parent_id: Option<LogTaskId>, state: TaskState }`
  - `TaskState::Pending(Task)` - queued, waiting for dispatch
  - `TaskState::InFlight(InFlight)` - currently executing
  - `TaskState::Waiting { pending_count, continuation }` - waiting for children
- `parent_id` is always the immediate parent (tree structure for proper finally tracking)

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

// Defined in crate::types - shown here for reference
#[derive(Debug, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LogTaskId(pub u32);

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
type PendingTasks = BTreeMap<LogTaskId, TaskSubmitted>;

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
# Normal run - creates state log in default folder
gsd run config.jsonc --pool mypool --initial-state '[...]'
# Creates: <root>/runs/<pool>/<timestamp>.ndjson

# Explicit state log path
gsd run config.jsonc --pool mypool --initial-state '[...]' --state-log /tmp/myrun.ndjson

# Resume to same default folder (new log file)
gsd run --resume-from /tmp/old.ndjson
# Creates new log, copies entries from old.ndjson, continues

# Resume to explicit path
gsd run --resume-from /tmp/old.ndjson --state-log /tmp/new.ndjson
# Copies entries from old.ndjson to new.ndjson, continues writing to new.ndjson

# PANIC: same path for both (no in-place mutation)
gsd run --resume-from /tmp/run.ndjson --state-log /tmp/run.ndjson  # panic!
```

`--resume-from` is incompatible with config file, `--initial-state`, and `--entrypoint-value` (panic if any combination provided).

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
- **Finally hook state**: ✅ **Fixed by FINALLY_TRACKING** - tree-based tracking with `parent_id` enables reconstruction. Once FINALLY_SCHEDULING is complete, finally hooks will be loggable tasks that can be resumed.

## Prerequisite Refactors

Before implementing state persistence:

1. **FINALLY_TRACKING** ✅ COMPLETE - Changed to `parent_id` (always immediate parent) in unified `BTreeMap<LogTaskId, TaskEntry>`. Tree structure enables reconstructing task relationships from the log.

2. **FINALLY_SCHEDULING** - Make finally hooks go through the task queue instead of running synchronously. This makes them loggable/resumable.

3. **Task ID Registry** (optional but recommended) - Consider whether needed now that we have unified `BTreeMap<LogTaskId, TaskEntry>`. The current structure already provides a central task registry.

## Future Work

- Visualize state logs (TUI or web UI)
- `gsd runs list` command to show run status
