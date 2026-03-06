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
{"kind":"TaskSubmitted","task_id":1,"step":"Analyze","value":{...},"origin_id":null}
{"kind":"TaskSubmitted","task_id":2,"step":"Analyze","value":{...},"origin_id":null}
{"kind":"TaskCompleted","task_id":1,"new_task_ids":[3]}
{"kind":"TaskSubmitted","task_id":3,"step":"Process","value":{...},"origin_id":1}
{"kind":"TaskFailed","task_id":2}
{"kind":"TaskFailed","task_id":2}
{"kind":"TaskCompleted","task_id":2,"new_task_ids":[]}
```

## Data Structures

```rust
use serde::{Deserialize, Serialize};
use crate::resolved::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
    TaskFailed(TaskFailed),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateLogConfig {
    pub config: Config,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSubmitted {
    pub task_id: u64,
    pub step: String,
    pub value: serde_json::Value,
    pub origin_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCompleted {
    pub task_id: u64,
    pub new_task_ids: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFailed {
    pub task_id: u64,
}
```

## Writing/Reading

Just functions, no wrapper classes:

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

Validation (config first, config once) happens at the call site, not baked into read/write.

## Reconstructing State on Resume

```rust
struct PendingTask {
    submitted: TaskSubmitted,
    failure_count: u32,
}

fn reconstruct(entries: Vec<StateLogEntry>) -> Result<(Config, HashMap<u64, PendingTask>), Error> {
    let mut config: Option<Config> = None;
    let mut pending: HashMap<u64, PendingTask> = HashMap::new();

    for entry in entries {
        match entry {
            StateLogEntry::Config(c) => {
                if config.is_some() {
                    return Err("Config appeared twice");
                }
                config = Some(c.config);
            }
            StateLogEntry::TaskSubmitted(task) => {
                pending.insert(task.task_id, PendingTask {
                    submitted: task,
                    failure_count: 0,
                });
            }
            StateLogEntry::TaskCompleted(c) => {
                pending.remove(&c.task_id);
            }
            StateLogEntry::TaskFailed(f) => {
                if let Some(task) = pending.get_mut(&f.task_id) {
                    task.failure_count += 1;
                }
            }
        }
    }

    let config = config.ok_or("No config entry")?;
    Ok((config, pending))
}
```

On resume, check each pending task: if `failure_count >= max_retries` (from config), fail the run.

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
- Write `TaskCompleted` or `TaskFailed` when task resolves
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
