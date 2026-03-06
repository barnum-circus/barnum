# State Persistence and Resume

**Status:** Not started

## Motivation

Long-running GSD jobs can be interrupted (network issues, machine restart, user Ctrl+C). Currently, all progress is lost and users must restart from scratch. State persistence enables:

1. **Resume**: Continue from where you left off after interruption
2. **Debugging**: Inspect state file to understand what happened
3. **Monitoring**: External tools can read state file to track progress

## Current State

### TaskRunner (`crates/gsd_config/src/runner.rs`)

```rust
pub struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a str, &'a Step>,
    queue: VecDeque<QueuedTask>,           // Pending tasks
    agent_pool_root: &'a Path,
    config_base_path: &'a Path,
    invoker: &'a Invoker<AgentPoolCli>,
    max_concurrency: usize,
    in_flight: usize,                       // Count of tasks currently being processed
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    next_task_id: u64,                      // Counter for unique IDs
    finally_tracking: HashMap<u64, FinallyState>,
}

struct QueuedTask {
    task: Task,
    id: u64,
    origin_id: Option<u64>,  // For finally hook tracking
}
```

### run() function

```rust
pub fn run(config: &Config, schemas: &CompiledSchemas, runner_config: RunnerConfig<'_>) -> io::Result<()> {
    let mut runner = TaskRunner::new(config, schemas, runner_config)?;
    let mut completed_count = 0u32;
    let mut dropped_count = 0u32;

    while let Some(outcome) = runner.next() {
        completed_count += 1;
        if matches!(outcome.result, TaskResult::Dropped { .. }) {
            dropped_count += 1;
        }
        // ... logging ...
    }
    // ... final status ...
}
```

### Problems with current design

- No way to serialize queue state
- No way to resume from saved state
- In-flight tasks are lost on interruption
- No tracking of which tasks completed vs failed

## Proposed Changes

### 1. State file format (`crates/gsd_config/src/state.rs`)

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persistent state for a GSD run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    /// Version of state file format (for future migrations)
    pub version: u32,
    /// Hash of config content (SHA-256 of normalized JSON)
    pub config_hash: String,
    /// Next task ID to assign (to ensure unique IDs across resume)
    pub next_task_id: u64,
    /// Tasks waiting to be processed
    pub pending: Vec<PersistedTask>,
    /// Tasks that completed successfully (for tracking/debugging)
    pub completed: Vec<CompletedTask>,
    /// Tasks that were dropped (retries exhausted)
    pub dropped: Vec<DroppedTask>,
    /// Finally tracking state (for resuming mid-fan-out)
    pub finally_tracking: Vec<PersistedFinallyState>,
    /// When this state was last updated
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTask {
    pub id: u64,
    pub step: String,
    pub value: serde_json::Value,
    pub retries: u32,
    pub origin_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedTask {
    pub id: u64,
    pub step: String,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroppedTask {
    pub id: u64,
    pub step: String,
    pub reason: String,
    pub dropped_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedFinallyState {
    pub origin_id: u64,
    pub pending_count: usize,
    pub original_value: serde_json::Value,
    pub finally_command: String,
}

impl RunState {
    pub const VERSION: u32 = 1;

    pub fn new(config_hash: String) -> Self {
        Self {
            version: Self::VERSION,
            config_hash,
            next_task_id: 0,
            pending: Vec::new(),
            completed: Vec::new(),
            dropped: Vec::new(),
            finally_tracking: Vec::new(),
            updated_at: Utc::now(),
        }
    }
}
```

### 2. Config hash calculation

```rust
// crates/gsd_config/src/state.rs

use sha2::{Sha256, Digest};

pub fn calculate_config_hash(config: &Config) -> String {
    // Serialize to canonical JSON (sorted keys)
    let json = serde_json::to_string(config).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    format!("{:x}", hasher.finalize())
}
```

### 3. State persistence in TaskRunner

**Before:**
```rust
impl<'a> TaskRunner<'a> {
    pub fn new(config: &'a Config, schemas: &'a CompiledSchemas, runner_config: RunnerConfig<'a>) -> io::Result<Self> {
        // ... setup ...
        let queue: VecDeque<QueuedTask> = runner_config.initial_tasks.into_iter().map(|task| {
            let id = next_task_id;
            next_task_id += 1;
            QueuedTask { task, id, origin_id: None }
        }).collect();
        // ...
    }
}
```

**After:**
```rust
impl<'a> TaskRunner<'a> {
    pub fn new(
        config: &'a Config,
        schemas: &'a CompiledSchemas,
        runner_config: RunnerConfig<'a>,
    ) -> io::Result<Self> {
        // ... setup ...

        let (queue, next_task_id, finally_tracking) = if let Some(state) = runner_config.resume_state {
            // Validate config hash matches
            let current_hash = calculate_config_hash(config);
            if state.config_hash != current_hash {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "[E070] config has changed since state was saved, cannot resume",
                ));
            }

            // Restore state
            let queue = state.pending.into_iter().map(|p| QueuedTask {
                task: Task { step: p.step.into(), value: p.value, retries: p.retries },
                id: p.id,
                origin_id: p.origin_id,
            }).collect();

            let finally = state.finally_tracking.into_iter().map(|f| {
                (f.origin_id, FinallyState {
                    pending_count: f.pending_count,
                    original_value: f.original_value,
                    finally_command: f.finally_command,
                })
            }).collect();

            (queue, state.next_task_id, finally)
        } else {
            // Fresh start from initial_tasks
            let mut next_id = 0u64;
            let queue = runner_config.initial_tasks.into_iter().map(|task| {
                let id = next_id;
                next_id += 1;
                QueuedTask { task, id, origin_id: None }
            }).collect();
            (queue, next_id, HashMap::new())
        };
        // ...
    }

    /// Persist current state to file.
    fn persist_state(&self, state_file: &Path, config_hash: &str) -> io::Result<()> {
        let state = RunState {
            version: RunState::VERSION,
            config_hash: config_hash.to_string(),
            next_task_id: self.next_task_id,
            pending: self.queue.iter().map(|q| PersistedTask {
                id: q.id,
                step: q.task.step.to_string(),
                value: q.task.value.clone(),
                retries: q.task.retries,
                origin_id: q.origin_id,
            }).collect(),
            completed: Vec::new(), // Populated by caller
            dropped: Vec::new(),   // Populated by caller
            finally_tracking: self.finally_tracking.iter().map(|(id, s)| PersistedFinallyState {
                origin_id: *id,
                pending_count: s.pending_count,
                original_value: s.original_value.clone(),
                finally_command: s.finally_command.clone(),
            }).collect(),
            updated_at: Utc::now(),
        };

        agent_pool::atomic_write(
            state_file.parent().unwrap_or(Path::new(".")),
            state_file,
            &serde_json::to_vec_pretty(&state)?,
        )
    }
}
```

### 4. RunnerConfig changes

**Before:**
```rust
pub struct RunnerConfig<'a> {
    pub agent_pool_root: &'a Path,
    pub config_base_path: &'a Path,
    pub wake_script: Option<&'a str>,
    pub initial_tasks: Vec<Task>,
    pub invoker: &'a Invoker<AgentPoolCli>,
}
```

**After:**
```rust
pub struct RunnerConfig<'a> {
    pub agent_pool_root: &'a Path,
    pub config_base_path: &'a Path,
    pub wake_script: Option<&'a str>,
    pub initial_tasks: Vec<Task>,
    pub invoker: &'a Invoker<AgentPoolCli>,
    /// State file for persistence. If Some, state is saved after each task.
    pub state_file: Option<&'a Path>,
    /// Resume from this state instead of using initial_tasks.
    pub resume_state: Option<RunState>,
}
```

### 5. CLI changes (`crates/gsd_cli/src/main.rs`)

**Before:**
```rust
#[derive(Subcommand)]
enum Command {
    Run {
        config: String,
        #[arg(long)]
        initial_state: Option<String>,
        #[arg(long)]
        entrypoint_value: Option<String>,
        #[arg(long)]
        pool: Option<String>,
        #[arg(long)]
        wake: Option<String>,
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    Config { ... },
    Version { ... },
}
```

**After:**
```rust
#[derive(Subcommand)]
enum Command {
    Run {
        config: String,
        #[arg(long)]
        initial_state: Option<String>,
        #[arg(long)]
        entrypoint_value: Option<String>,
        #[arg(long)]
        pool: Option<String>,
        #[arg(long)]
        wake: Option<String>,
        #[arg(long)]
        log_file: Option<PathBuf>,
        /// State file for persistence. Saves state after each task completion.
        /// If file exists and matches config, resumes from saved state.
        #[arg(long)]
        state_file: Option<PathBuf>,
    },
    /// Resume a run from a state file
    Resume {
        /// State file to resume from
        state_file: PathBuf,
        /// Agent pool ID or path
        #[arg(long)]
        pool: Option<String>,
        /// Wake script to call before starting
        #[arg(long)]
        wake: Option<String>,
        /// Log file path
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    Config { ... },
    Version { ... },
}
```

## CLI Flag Incompatibilities

### `--state-file` is incompatible with:

| Flag | Reason |
|------|--------|
| `--initial-state` | State file contains its own pending tasks; providing both is ambiguous |
| `--entrypoint-value` | Same as above - state file has the tasks |

### `gsd resume` is incompatible with:

| Flag | Reason |
|------|--------|
| `--initial-state` | Resume uses state from file, not new initial state |
| `--entrypoint-value` | Same as above |
| config argument | Config path is stored in state file |

### Validation logic:

```rust
fn run_command(...) -> io::Result<()> {
    // If --state-file is provided and file exists with matching config hash,
    // ignore --initial-state and --entrypoint-value (resume mode)

    // If --state-file is provided but file doesn't exist or hash differs,
    // use --initial-state/--entrypoint-value normally (fresh start, will persist)

    // Error if resume subcommand used with --initial-state or --entrypoint-value
}
```

## Signal Handling

State should be persisted before exit on SIGTERM/SIGINT:

```rust
// In run() or TaskRunner
use signal_hook::{consts::SIGTERM, iterator::Signals};

let mut signals = Signals::new(&[SIGTERM, SIGINT])?;
// Set up handler to persist state and exit gracefully
```

## Implementation Plan

### Phase 1: State types and serialization (no runtime changes)

1. Create `crates/gsd_config/src/state.rs`
2. Add `RunState`, `PersistedTask`, etc. types
3. Add `calculate_config_hash()` function
4. Add `chrono` and `sha2` dependencies
5. Write unit tests for serialization/deserialization
6. **Commit**: "Add state persistence types"

### Phase 2: TaskRunner state extraction

1. Add `fn snapshot(&self) -> RunState` to TaskRunner
2. Add `fn from_state(state: RunState, ...) -> io::Result<Self>` constructor
3. Unit test round-trip: create runner -> snapshot -> restore -> verify queue matches
4. **Commit**: "Add TaskRunner state snapshot and restore"

### Phase 3: Persistence during run

1. Add `state_file: Option<&Path>` to `RunnerConfig`
2. Call `persist_state()` after each `TaskOutcome` in `run()`
3. Use `atomic_write` to prevent corruption
4. Add integration test: run with state file, verify file contents
5. **Commit**: "Persist state to file during run"

### Phase 4: Resume support

1. Add `resume_state: Option<RunState>` to `RunnerConfig`
2. Modify `TaskRunner::new` to accept resume state
3. Add config hash validation
4. Add integration test: partial run -> interrupt -> resume -> complete
5. **Commit**: "Support resuming from state file"

### Phase 5: CLI integration

1. Add `--state-file` flag to `run` command
2. Add `resume` subcommand
3. Add flag incompatibility validation
4. Add CLI tests
5. **Commit**: "Add --state-file flag and resume subcommand"

### Phase 6: Signal handling (optional)

1. Add `signal-hook` dependency
2. Set up SIGTERM/SIGINT handler
3. Persist state on signal
4. Test graceful shutdown
5. **Commit**: "Graceful shutdown with state persistence"

## Files to Change

| File | Changes |
|------|---------|
| `crates/gsd_config/Cargo.toml` | Add `chrono`, `sha2` dependencies |
| `crates/gsd_config/src/lib.rs` | Export `state` module |
| `crates/gsd_config/src/state.rs` | **New file** - state types |
| `crates/gsd_config/src/runner.rs` | Add snapshot/restore, state_file handling |
| `crates/gsd_cli/src/main.rs` | Add `--state-file`, `resume` subcommand |

## Open Questions

1. **In-flight task handling**: When interrupted, in-flight tasks are lost. On resume, they'll be requeued from pending state. This could cause duplicate work if the task partially completed. Options:
   - Document this behavior (simplest)
   - Track in-flight separately and re-submit on resume
   - Add idempotency requirements to tasks

2. **State file location**: Default to `.gsd-state/<config-hash>.json` in config dir? Or require explicit `--state-file`?

3. **State file cleanup**: Should we auto-delete state file on successful completion? Probably yes, with `--keep-state` flag to preserve for debugging.

4. **Multiple concurrent runs**: What if user starts two runs with same config? Options:
   - Lock file to prevent concurrent runs
   - Include run ID in state file name
   - Document "don't do this"
