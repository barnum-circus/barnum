# State Persistence and Resume

**Status:** Not started

**Depends on:** ROOT_FLAG_REFACTOR (complete), INLINED_CONFIG (complete), FINALLY_TRACKING (complete), FINALLY_SCHEDULING (complete)

## Motivation

Long-running GSD jobs can be interrupted (crash, Ctrl+C, OOM). State persistence enables resuming from where you left off.

## Core Idea

A run creates a single NDJSON log file. First entry is config, then task events. On resume, replay log to reconstruct pending tasks.

**Two types of logs (don't confuse them):**
- **Debug logs** (`--log-file`): Tracing output for debugging
- **State logs** (this feature): Machine-readable NDJSON for persistence/resume

## Current Runner Architecture (as of 2026-03-08, updated after FINALLY_SCHEDULING)

The runner module (`crates/gsd_config/src/runner/`) has these submodules:
- `mod.rs` - TaskRunner struct, main loop, `run()` public function, RunnerConfig, TaskEntry, TaskState, InFlight
- `dispatch.rs` - TaskContext, TaskIdentity, InFlightResult, SubmitResult, dispatch_*_task functions
- `hooks.rs` - run_pre_hook, run_post_hook
- `shell.rs` - `run_shell_command()` helper
- `response.rs` - Response processing and retry logic
- `submit.rs` - CLI invocation for agent_pool

Key current state:
- `initial_tasks` passed separately from `RunnerConfig`
- `RunnerConfig` passed by reference
- Unified task state in `BTreeMap<LogTaskId, TaskEntry>`:
  - `TaskEntry { step, parent_id, finally_script, state, retries_remaining }`
  - `TaskState::Pending { value }` - queued, waiting for dispatch
  - `TaskState::InFlight(InFlight)` - currently executing
  - `TaskState::WaitingForChildren { pending_children_count, finally_data }` - waiting for children
- `parent_id` is always the immediate parent (tree structure for proper finally tracking)
- `finally_script: Option<HookScript>` identifies finally tasks (same step name as parent, dispatched differently)

## State Log Format

Newline-delimited JSON. First entry MUST be `Config` (exactly once). Uses `#[serde(tag = "kind")]`.

```json
{"kind":"Config","config":{...}}
{"kind":"TaskSubmitted","task_id":1,"step":"Analyze","value":{...},"parent_id":null,"origin":{"kind":"Initial"}}
{"kind":"TaskSubmitted","task_id":2,"step":"Analyze","value":{...},"parent_id":null,"origin":{"kind":"Initial"}}
{"kind":"TaskCompleted","task_id":1,"outcome":{"kind":"Success","value":{"spawned_task_ids":[3]}}}
{"kind":"TaskSubmitted","task_id":3,"step":"Process","value":{...},"parent_id":1,"origin":{"kind":"Spawned"}}
{"kind":"TaskCompleted","task_id":2,"outcome":{"kind":"Failed","value":{"reason":{"kind":"Timeout"},"retry_task_id":4}}}
{"kind":"TaskSubmitted","task_id":4,"step":"Analyze","value":{...},"parent_id":null,"origin":{"kind":"Retry","replaces":2}}
{"kind":"TaskCompleted","task_id":4,"outcome":{"kind":"Success","value":{"spawned_task_ids":[]}}}
{"kind":"TaskSubmitted","task_id":5,"step":"Analyze","value":{...},"parent_id":1,"origin":{"kind":"Finally","finally_for":1}}
```

**Task origins:**
- `Initial`: From `--initial-state`
- `Spawned`: Created by parent task's output (regular child)
- `Retry`: Replacement for a failed task
- `Finally`: Finally hook task for a completed task

## Data Structures

```rust
use serde::{Deserialize, Serialize};
use crate::resolved::Config;
use crate::types::StepName;

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
    pub step: StepName,
    pub value: serde_json::Value,
    /// Task waiting for this one to complete (tree parent).
    pub parent_id: Option<LogTaskId>,
    /// How this task came to exist.
    pub origin: TaskOrigin,
}

/// How a task came to be created.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TaskOrigin {
    /// From `--initial-state` (root task).
    Initial,
    /// Spawned by parent task's action output.
    Spawned,
    /// Retry of a failed task.
    Retry {
        /// The task this replaces.
        replaces: LogTaskId,
    },
    /// Finally hook for a completed task.
    Finally {
        /// The task whose finally hook this is.
        finally_for: LogTaskId,
    },
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
    Failed(TaskFailed),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskSuccess {
    /// IDs of tasks spawned by this task's output.
    pub spawned_task_ids: Vec<LogTaskId>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskFailed {
    pub reason: FailureReason,
    /// If the task was retried, the ID of the retry task.
    /// None if retries were exhausted or disabled.
    pub retry_task_id: Option<LogTaskId>,
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
}

fn reconstruct(
    mut entries: impl Iterator<Item = io::Result<StateLogEntry>>,
) -> Result<(Config, PendingTasks), ReconstructError> {
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

**Notes on retry counting:** The number of retries is not stored in the log. Instead, it can be computed by following the `Retry { replaces }` chain backwards. On resume, we validate against `max_retries` from the config.

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
- **Finally hook state**: [DONE] **Fixed by FINALLY_TRACKING + FINALLY_SCHEDULING** - finally hooks are now regular tasks that go through the queue and can be logged/resumed like any other task.

## Prerequisite Refactors

All prerequisites are now complete:

1. **FINALLY_TRACKING** [DONE] COMPLETE - Changed to `parent_id` (always immediate parent) in unified `BTreeMap<LogTaskId, TaskEntry>`. Tree structure enables reconstructing task relationships from the log.

2. **FINALLY_SCHEDULING** [DONE] COMPLETE - Finally hooks now go through the task queue as regular tasks. Key implementation details:
   - Finally tasks are identified by `finally_script: Option<HookScript>` on `TaskEntry`
   - Finally tasks use the **same step name** as their parent (NOT a special `"__finally__"` step)
   - Finally tasks are scheduled as **siblings** (child of same parent), not child of the original task
   - `TaskState::WaitingForChildren { pending_children_count, finally_data }` stores the hook + value to schedule when children complete

3. **Task ID Registry** - Not needed. The unified `BTreeMap<LogTaskId, TaskEntry>` already provides this.

## Future Work

- Visualize state logs (TUI or web UI)
- `gsd runs list` command to show run status
