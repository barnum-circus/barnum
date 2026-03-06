# State Persistence and Resume

**Status:** Not started

## Motivation

Long-running GSD jobs can be interrupted (crash, Ctrl+C, OOM). State persistence enables resuming from where you left off.

## CLI Changes

### Output state: `--state-output <path>`

```bash
gsd run config.jsonc --pool mypool --state-output /tmp/myrun.state.json
```

Writes state after each task completion. Simple, explicit, no magic.

### Input state: `--initial-state <path-or-json>`

Already works! The existing `--initial-state` flag detects whether the argument is a file path or inline JSON by checking if the file exists. So this already works:

```bash
# Inline JSON (starts with '[')
gsd run config.jsonc --pool mypool --initial-state '[{"kind": "Start", "value": {}}]'

# File path (file exists)
gsd run config.jsonc --pool mypool --initial-state /tmp/myrun.state.json
```

No new flags needed for input.

## State File Format

```rust
// crates/gsd_config/src/state.rs

use crate::types::StepName;
use serde::{Deserialize, Serialize};

/// Unique identifier for a task instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(u64);

/// Unique identifier for finally-hook tracking (the origin task that spawned descendants).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OriginId(u64);

/// Number of retry attempts for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RetryCount(u32);

/// Count of pending descendants for finally-hook tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PendingCount(usize);

/// Persistent state for a GSD run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    /// GSD version that created this state (e.g., "0.1.0").
    pub gsd_version: String,

    /// Next task ID to assign (ensures unique IDs across resume).
    pub next_task_id: TaskId,

    /// Tasks waiting to be processed.
    pub pending: Vec<PersistedTask>,

    /// Finally tracking state (for resuming mid-fan-out).
    pub finally_tracking: Vec<PersistedFinallyState>,
}

/// A task waiting to be processed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTask {
    pub id: TaskId,
    pub step: StepName,
    pub value: serde_json::Value,
    pub retries: RetryCount,
    pub origin_id: Option<OriginId>,
}

/// State for tracking when a finally hook should run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedFinallyState {
    pub origin_id: OriginId,
    pub pending_count: PendingCount,
    pub original_value: serde_json::Value,
    pub finally_command: String,
}

impl RunState {
    pub fn new() -> Self {
        Self {
            gsd_version: env!("GSD_VERSION").to_string(),
            next_task_id: TaskId(0),
            pending: Vec::new(),
            finally_tracking: Vec::new(),
        }
    }
}
```

The `gsd_version` uses the same `GSD_VERSION` env var that's already set in `build.rs`.

## Code Changes

### 1. Add `--state-output` flag to CLI

**File:** `crates/gsd_cli/src/main.rs`

```rust
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
    /// Write state to this file after each task completion.
    #[arg(long)]
    state_output: Option<PathBuf>,
},
```

### 2. Add `state_output` to `RunnerConfig`

**File:** `crates/gsd_config/src/runner.rs`

```rust
pub struct RunnerConfig<'a> {
    pub agent_pool_root: &'a Path,
    pub config_base_path: &'a Path,
    pub wake_script: Option<&'a str>,
    pub initial_tasks: Vec<Task>,
    pub invoker: &'a Invoker<AgentPoolCli>,
    /// If set, write state to this file after each task completion.
    pub state_output: Option<&'a Path>,
}
```

### 3. Add `snapshot()` method to `TaskRunner`

**File:** `crates/gsd_config/src/runner.rs`

```rust
impl<'a> TaskRunner<'a> {
    /// Snapshot current state for persistence.
    pub fn snapshot(&self) -> RunState {
        RunState {
            gsd_version: env!("GSD_VERSION").to_string(),
            next_task_id: TaskId(self.next_task_id),
            pending: self.queue.iter().map(|q| PersistedTask {
                id: TaskId(q.id),
                step: q.task.step.clone(),
                value: q.task.value.clone(),
                retries: RetryCount(q.task.retries),
                origin_id: q.origin_id.map(OriginId),
            }).collect(),
            finally_tracking: self.finally_tracking.iter().map(|(id, s)| {
                PersistedFinallyState {
                    origin_id: OriginId(*id),
                    pending_count: PendingCount(s.pending_count),
                    original_value: s.original_value.clone(),
                    finally_command: s.finally_command.clone(),
                }
            }).collect(),
        }
    }
}
```

### 4. Write state in `run()` loop

**File:** `crates/gsd_config/src/runner.rs`

```rust
pub fn run(config: &Config, schemas: &CompiledSchemas, runner_config: RunnerConfig<'_>) -> io::Result<()> {
    let state_output = runner_config.state_output.map(|p| p.to_path_buf());
    let mut runner = TaskRunner::new(config, schemas, runner_config)?;

    while let Some(outcome) = runner.next() {
        // ... existing logging ...

        // Persist state if output path configured
        if let Some(ref path) = state_output {
            let state = runner.snapshot();
            let json = serde_json::to_vec_pretty(&state)?;
            std::fs::write(path, json)?;
        }
    }

    // Delete state file on successful completion
    if let Some(ref path) = state_output {
        let _ = std::fs::remove_file(path);
    }

    // ... rest of function ...
}
```

### 5. Modify `parse_initial_tasks` to handle state files

**File:** `crates/gsd_cli/src/main.rs`

```rust
fn parse_initial_tasks(initial: &str) -> io::Result<Vec<Task>> {
    let path = PathBuf::from(initial);
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        initial.to_string()
    };

    // Try parsing as RunState first (resuming from state file)
    if let Ok(state) = serde_json::from_str::<RunState>(&content) {
        return Ok(state.pending.into_iter().map(|p| Task {
            step: p.step,
            value: p.value,
            retries: p.retries.0,
        }).collect());
    }

    // Fall back to parsing as Vec<Task> (normal initial state)
    json5::from_str(&content).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("[E057] invalid initial tasks JSON: {e}"),
        )
    })
}
```

## Files to Change

| File | Changes |
|------|---------|
| `crates/gsd_config/src/state.rs` | **New file** - state types with newtypes |
| `crates/gsd_config/src/lib.rs` | Export `state` module |
| `crates/gsd_config/src/runner.rs` | Add `state_output` to config, `snapshot()` method, write in loop |
| `crates/gsd_cli/src/main.rs` | Add `--state-output` flag, modify `parse_initial_tasks` |

## Implementation Plan

1. **Add state types** - Create `state.rs` with newtypes
2. **Add snapshot** - Add `TaskRunner::snapshot()` method
3. **Add CLI flag** - Add `--state-output` to CLI
4. **Write state** - Persist after each task in `run()` loop
5. **Parse state** - Modify `parse_initial_tasks` to handle state files
6. **Tests** - Integration test: run partially, resume, verify completion

## Future Work (TODO)

Add to `todos.md`:

### Config Hash for Resume Validation

When resuming from a state file, we should validate that the config hasn't changed. This requires:

1. Hash the config content (SHA-256 of normalized JSON)
2. Store hash in state file
3. On resume, compare stored hash with current config hash
4. If mismatch, error with "[E070] config has changed since state was saved"

This is deferred because:
- Initial implementation works without it
- Users can manually manage state files
- Hash calculation needs careful normalization (ignore whitespace, sort keys)
