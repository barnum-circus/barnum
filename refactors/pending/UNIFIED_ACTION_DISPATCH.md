# Unified Action Dispatch

Prerequisite to PLUGGABLE_ACTION_KINDS (future).

## Motivation

Pool actions, Command actions, and finally hooks each have their own dispatch function, their own result type, and their own response-processing path. Timeouts only exist for Pool (via troupe). Commands block forever. Finally hooks ignore concurrency limits.

All of these run work in a thread and send a result back on a channel. They should go through one dispatch path with one trait. If it runs, it goes through the trait. Concurrency and result routing wrap the trait from the outside.

## Current State

### `dispatch_task` (`runner/mod.rs:709-734`)

Pattern-matches on `ActionKind` and calls different functions:

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let tx = self.tx.clone();
    match &step.action {
        ActionKind::Pool(..) => {
            let docs = generate_step_docs(step, self.config);
            let timeout = step.options.timeout;
            let pool = self.pool.clone();
            info!(step = %task.step, "submitting task to pool");
            thread::spawn(move || {
                dispatch_pool_task(task_id, task, &docs, timeout, &pool, &tx);
            });
        }
        ActionKind::Command(CommandAction { script }) => {
            let script = script.clone();
            let working_dir = self.pool.working_dir.clone();
            info!(step = %task.step, script = %script, "executing command");
            thread::spawn(move || {
                dispatch_command_task(task_id, task, &script, &working_dir, &tx);
            });
        }
    }
}
```

### `dispatch_finally` (`runner/mod.rs:738-751`)

Separate method with its own thread::spawn and dispatch function:

```rust
fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let script = step.finally_hook.clone().expect("[P073]...");
    let working_dir = self.pool.working_dir.clone();
    let tx = self.tx.clone();
    info!(step = %task.step, parent = ?parent_id, "dispatching finally worker");
    thread::spawn(move || {
        dispatch_finally_task(parent_id, task, &script, &working_dir, &tx);
    });
}
```

### `dispatch_pool_task` (`runner/dispatch.rs:56-74`)

Spawned thread. Calls `build_agent_payload` + `submit_via_cli`. Sends `SubmitResult::Pool(PoolResult { value, response: io::Result<Response> })`.

### `dispatch_command_task` (`runner/dispatch.rs:80-100`)

Spawned thread. Calls `run_command_action` (which wraps `run_shell_command`, converting `Result<String, String>` to `io::Result<String>`). Sends `SubmitResult::Command(CommandResult { value, output: io::Result<String> })`.

### `dispatch_finally_task` (`runner/dispatch.rs:106-122`)

Spawned thread. Calls `run_shell_command` directly. Sends `SubmitResult::Finally(FinallyResult { value, output: Result<String, String> })`.

### `SubmitResult` and result types (`runner/dispatch.rs:21-50`)

```rust
pub struct WorkerResult {
    pub task_id: LogTaskId,
    pub task: Task,
    pub result: SubmitResult,
}

pub(super) struct PoolResult {
    pub value: StepInputValue,
    pub response: io::Result<Response>,
}

pub(super) struct CommandResult {
    pub value: StepInputValue,
    pub output: io::Result<String>,
}

pub(super) struct FinallyResult {
    pub value: StepInputValue,
    pub output: Result<String, String>,
}

pub(super) enum SubmitResult {
    Pool(PoolResult),
    Command(CommandResult),
    Finally(FinallyResult),
}
```

Three variants, three different result types.

### `process_submit_result` (`runner/response.rs:42-71`)

Separate match arms for Pool, Command, Finally. Pool and Command both converge on `process_stdout` for success:
- Pool: `Response::Processed { stdout }` → `process_stdout`; `Response::NotProcessed` → `FailureKind::Timeout`
- Command: `io::Result<String>` → `process_stdout`; no timeout variant
- Finally: parses stdout as `Vec<Task>`; this arm is dead code (Finally is routed to `convert_finally_result` in `mod.rs:521`, never reaches `process_submit_result`)

### `process_worker_result` routing (`runner/mod.rs:511-532`)

Routes Finally to `convert_finally_result`, everything else to `convert_task_result`:

```rust
let entries = match submit_result {
    dispatch::SubmitResult::Finally(dispatch::FinallyResult { value, output }) => {
        self.convert_finally_result(task_id, value, output)
    }
    other => self.convert_task_result(task_id, &task, other),
};
```

### `run_command_action` (`runner/hooks.rs:25-28`)

Thin wrapper around `run_shell_command` that converts `Result<String, String>` to `io::Result<String>`:

```rust
pub fn run_command_action(script: &str, task_json: &str, working_dir: &Path) -> io::Result<String> {
    run_shell_command(script, task_json, Some(working_dir))
        .map_err(|e| io::Error::other(format!("[E021] command {e}")))
}
```

### `run_shell_command` (`runner/shell.rs`)

Blocks forever on `child.wait_with_output()`. No timeout.

## Design Decisions

### Naming: `Action` trait, config enum is `ActionKind`

The config enum was already renamed from `Action` to `ActionKind` on master. The `Action` name belongs to the trait.

### `Action` trait

```rust
pub trait Action: Send {
    fn perform(&self, value: &serde_json::Value) -> Result<String, ActionError>;
}
```

Takes a value, runs something, returns stdout or an `ActionError`.

### `ActionError` preserves failure classification

The existing retry logic distinguishes `FailureKind::Timeout` (checked against `options.retry_on_timeout`) from `FailureKind::SubmitError` (always retries). Without a typed error, all failures from `perform` become `SubmitError` and we lose the timeout classification — changing retry behavior for users with `retry_on_timeout: false`.

`ActionError` preserves the distinction:

```rust
pub enum ActionError {
    /// The action timed out or was not processed.
    TimedOut,
    /// The action failed with an error message.
    Failed(String),
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => write!(f, "action timed out"),
            Self::Failed(msg) => write!(f, "{msg}"),
        }
    }
}
```

Sources of each variant:
- `TimedOut`: `run_action`'s `recv_timeout` fires; `PoolAction::perform` gets `Response::NotProcessed` from troupe
- `Failed`: shell command exits non-zero; `submit_via_cli` returns `io::Error`; any other operational failure

`process_submit_result` maps `ActionError::TimedOut` → `FailureKind::Timeout` and `ActionError::Failed` → `FailureKind::SubmitError`, preserving existing retry semantics.

### `run_action`: timeout via `recv_timeout`

`run_action` takes a boxed action, a value, and an optional timeout. Without a timeout, it calls `perform` directly (one thread, no overhead). With a timeout, it spawns an inner thread for `perform` and uses `recv_timeout` on a one-shot channel:

```rust
fn run_action(
    action: Box<dyn Action>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, ActionError> {
    match timeout {
        None => action.perform(value),
        Some(duration) => {
            let (tx, rx) = mpsc::channel();
            let value = value.clone();
            thread::spawn(move || {
                let _ = tx.send(action.perform(&value));
            });
            match rx.recv_timeout(duration) {
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => Err(ActionError::TimedOut),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Err(ActionError::Failed("action panicked".into()))
                }
            }
        }
    }
}
```

On timeout, the inner thread keeps running (can't kill threads in Rust). Both action types spawn child processes that terminate independently: troupe eventually times out the agent, and shell commands finish or get reaped. The inner thread's eventual `tx.send()` writes to a channel nobody is listening to.

### `spawn_worker`: the single dispatch path

Every dispatch site constructs an action and calls `spawn_worker`. Thread spawn, `run_action` call, and `WorkerResult` send live in one place:

```rust
fn spawn_worker(
    tx: Sender<WorkerResult>,
    action: Box<dyn Action>,
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    timeout: Option<Duration>,
) {
    thread::spawn(move || {
        let value = task.value.clone();
        let output = run_action(action, &value.0, timeout);
        let _ = tx.send(WorkerResult {
            task_id, task, kind,
            result: ActionResult { value, output },
        });
    });
}
```

`WorkerResult` is sent exactly once — either with the action's output or with a timeout error.

### Timeout flow end-to-end

1. **Source**: `step.options.timeout: Option<u64>` (seconds), configured per-step in the barnum config.
2. **Conversion**: `dispatch_task` and `dispatch_finally` convert to `Option<Duration>` via `step.options.timeout.map(Duration::from_secs)` and pass to `spawn_worker`.
3. **Enforcement**: `spawn_worker`'s thread calls `run_action`, which either calls `perform` directly (no timeout) or spawns an inner thread and waits with `recv_timeout` (timeout set).
4. **On timeout**: `run_action` returns `Err(ActionError::TimedOut)`. The inner thread continues running but only writes to the dead inner channel.
5. **Result routing**: `spawn_worker` sends `WorkerResult` with `output: Err(ActionError::TimedOut)`. The engine's main loop receives it via `rx.recv()`.
6. **Retry**: `process_submit_result` maps `ActionError::TimedOut` → `FailureKind::Timeout`. `process_retry` checks `options.retry_on_timeout` to decide whether to retry or drop.

For Pool actions specifically: `pool_timeout` (troupe's agent lifecycle timeout, passed in the payload via `build_agent_payload`) and the Barnum-level timeout (enforced by `run_action`) both derive from `step.options.timeout`. The troupe timeout controls how long the agent has to process the task. The Barnum timeout controls how long the engine waits for the result. They happen to be the same value. If troupe times out first, `PoolAction::perform` returns `Err(ActionError::TimedOut)` from the `NotProcessed` response. If Barnum times out first, `run_action` returns `Err(ActionError::TimedOut)` from `recv_timeout`.

### Concurrency stays in the engine

`in_flight` is managed by `Engine::flush_dispatches` (increment before spawn) and `Engine::process_worker_result` (decrement on completion). Both run on the main thread. Actions run in spawned threads with no access to `Engine`.

## Phased Implementation

### Phase 0: Unify result types

Collapse `SubmitResult`'s three payload types into one `ActionResult`. Retry logic is unchanged.

#### 0a. Replace types in `runner/dispatch.rs`

Delete `PoolResult`, `CommandResult`, `FinallyResult`, `SubmitResult`. Replace with:

```rust
use super::action::ActionError;

/// Unified action output.
pub(super) struct ActionResult {
    pub value: StepInputValue,
    pub output: Result<String, ActionError>,
}

/// Routing tag: determines whether result goes to convert_task_result or convert_finally_result.
pub(super) enum WorkerKind {
    Task,
    Finally { parent_id: LogTaskId },
}

/// What the engine's main loop receives from worker threads.
pub struct WorkerResult {
    pub task_id: LogTaskId,
    pub task: Task,
    pub kind: WorkerKind,
    pub result: ActionResult,
}
```

**Complication**: `ActionResult.output` uses `ActionError`, which is introduced in Phase 1. For Phase 0 to compile independently, either: (a) define `ActionError` in Phase 0 (in `dispatch.rs` or a new `action.rs`), or (b) use `Result<String, String>` in Phase 0 and change to `Result<String, ActionError>` in Phase 1. Option (a) is cleaner — define `ActionError` in a new `runner/action.rs` in this phase, even though the trait comes later.

Create `runner/action.rs` with just `ActionError` and its `Display` impl (as shown in Design Decisions). Add `mod action;` to `runner/mod.rs`.

#### 0b. Convert results in dispatch functions

**File: `runner/dispatch.rs`**

Each dispatch function normalizes its native result type to `Result<String, ActionError>` before wrapping in `ActionResult`.

`dispatch_pool_task` — before:
```rust
let response = submit_via_cli(&pool.root, &payload, &pool.invoker);
let _ = tx.send(WorkerResult {
    task_id, task,
    result: SubmitResult::Pool(PoolResult { value, response }),
});
```

After:
```rust
let output = match submit_via_cli(&pool.root, &payload, &pool.invoker) {
    Ok(Response::Processed { stdout, .. }) => Ok(stdout),
    Ok(Response::NotProcessed { .. }) => Err(ActionError::TimedOut),
    Err(e) => Err(ActionError::Failed(e.to_string())),
};
let _ = tx.send(WorkerResult {
    task_id, task,
    kind: WorkerKind::Task,
    result: ActionResult { value, output },
});
```

`dispatch_command_task` — before:
```rust
let output = run_command_action(script, &task_json, working_dir);
let _ = tx.send(WorkerResult {
    task_id, task,
    result: SubmitResult::Command(CommandResult { value, output }),
});
```

After (inline `run_shell_command` directly, drop the `io::Result` wrapper):
```rust
let output = run_shell_command(script, &task_json, Some(working_dir))
    .map_err(ActionError::Failed);
let _ = tx.send(WorkerResult {
    task_id, task,
    kind: WorkerKind::Task,
    result: ActionResult { value, output },
});
```

`dispatch_finally_task` — before:
```rust
let output = run_shell_command(finally_script.as_str(), &input_json, Some(working_dir));
let _ = tx.send(WorkerResult {
    task_id, task,
    result: SubmitResult::Finally(FinallyResult { value, output }),
});
```

After:
```rust
let output = run_shell_command(finally_script.as_str(), &input_json, Some(working_dir))
    .map_err(ActionError::Failed);
let _ = tx.send(WorkerResult {
    task_id: parent_id, task,
    kind: WorkerKind::Finally { parent_id },
    result: ActionResult { value, output },
});
```

Remove import of `run_command_action` from `dispatch.rs`. Add import of `ActionError` from `super::action`.

#### 0c. Collapse `process_submit_result`

**File: `runner/response.rs`**

Before (handles three variants):
```rust
pub fn process_submit_result(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
) -> TaskOutcome {
    match result {
        SubmitResult::Pool(PoolResult { value, response }) => match response {
            Ok(response) => process_pool_response(response, task, &value, step, schemas),
            Err(e) => { ... process_retry(task, &step.options, FailureKind::SubmitError) }
        },
        SubmitResult::Command(CommandResult { value, output }) => match output {
            Ok(stdout) => process_command_response(&stdout, task, &value, step, schemas),
            Err(e) => { ... process_retry(task, &step.options, FailureKind::SubmitError) }
        },
        SubmitResult::Finally(FinallyResult { value, output }) => ...  // dead code
    }
}
```

After:
```rust
pub fn process_submit_result(
    result: ActionResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
) -> TaskOutcome {
    match result.output {
        Ok(stdout) => process_stdout(&stdout, task, &result.value, step, schemas),
        Err(ActionError::TimedOut) => {
            warn!(step = %task.step, "action timed out");
            process_retry(task, &step.options, FailureKind::Timeout)
        }
        Err(ActionError::Failed(error)) => {
            error!(step = %task.step, %error, "action failed");
            process_retry(task, &step.options, FailureKind::SubmitError)
        }
    }
}
```

Delete `process_pool_response`, `process_command_response`, `process_finally_response`. Update imports: remove `SubmitResult`, `PoolResult`, `CommandResult`, `FinallyResult`; add `ActionResult` and `ActionError`.

#### 0d. Update routing in `process_worker_result`

**File: `runner/mod.rs`**

Before (`mod.rs:510-532`):
```rust
fn process_worker_result(&mut self, result: WorkerResult) -> Vec<StateLogEntry> {
    let WorkerResult { task_id, task, result: submit_result } = result;
    self.in_flight = self.in_flight.saturating_sub(1);
    let entries = match submit_result {
        dispatch::SubmitResult::Finally(dispatch::FinallyResult { value, output }) => {
            self.convert_finally_result(task_id, value, output)
        }
        other => self.convert_task_result(task_id, &task, other),
    };
    ...
}
```

After:
```rust
fn process_worker_result(&mut self, result: WorkerResult) -> Vec<StateLogEntry> {
    self.in_flight = self.in_flight.saturating_sub(1);
    let entries = match result.kind {
        dispatch::WorkerKind::Task => {
            self.convert_task_result(result.task_id, &result.task, result.result)
        }
        dispatch::WorkerKind::Finally { parent_id } => {
            self.convert_finally_result(parent_id, result.task.value.clone(), result.result.output)
        }
    };
    for entry in &entries {
        self.state.apply_entry(entry, self.config);
    }
    self.flush_dispatches();
    entries
}
```

Update `convert_task_result` signature — takes `ActionResult` instead of `SubmitResult`:

```rust
fn convert_task_result(
    &mut self,
    task_id: LogTaskId,
    task: &Task,
    action_result: dispatch::ActionResult,
) -> Vec<StateLogEntry> {
    let step = self.step_map.get(&task.step).expect("[P015] task step must exist");
    let outcome = process_submit_result(action_result, task, step, self.schemas);
    ...  // rest unchanged
}
```

Update `convert_finally_result` — takes `Result<String, ActionError>` instead of `Result<String, String>`:

```rust
fn convert_finally_result(
    &mut self,
    parent_id: LogTaskId,
    _value: StepInputValue,
    output: Result<String, ActionError>,
) -> Vec<StateLogEntry> {
    let raw_children = match output {
        Ok(stdout) => match json5::from_str::<Vec<Task>>(&stdout) { ... },
        Err(e) => {
            error!(parent = ?parent_id, error = %e, "finally hook failed");
            vec![]
        }
    };
    ...  // rest unchanged
}
```

The `%e` formatting works because `ActionError` implements `Display`.

Update imports in `mod.rs`: remove `dispatch_command_task`, `dispatch_finally_task`, `dispatch_pool_task` from the `use dispatch::` line. Add `dispatch::WorkerKind`. Keep `dispatch::WorkerResult`. Add `use action::ActionError;` (for `convert_finally_result` signature).

#### 0e. Delete dead code

- `PoolResult`, `CommandResult`, `FinallyResult` structs (from `dispatch.rs`)
- `SubmitResult` enum (from `dispatch.rs`)
- `process_pool_response`, `process_command_response`, `process_finally_response` (from `response.rs`)
- `run_command_action` (from `hooks.rs` — the only caller was `dispatch_command_task`, now inlined)
- Remove `use super::hooks::run_command_action;` from `dispatch.rs`

After this phase: all dispatch functions produce `ActionResult` with `Result<String, ActionError>`, `WorkerKind` handles routing, one code path in `process_submit_result`. Compile, run full suite.

---

### Phase 1: `Action` trait + `run_action` + `spawn_worker` + `PoolAction`

Introduce the trait, the dispatch infrastructure, and move Pool through it. Command stays as-is.

Requires Phase 0 (unified result types).

#### 1a. Expand `runner/action.rs`

Phase 0 created this file with just `ActionError`. Add the trait, `run_action`, `spawn_worker`, and `PoolAction`:

```rust
//! Action trait and dispatch infrastructure.

use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use cli_invoker::Invoker;
use troupe::Response;
use troupe_cli::TroupeCli;

use crate::types::{LogTaskId, StepName};
use crate::value_schema::Task;

use super::dispatch::{ActionResult, WorkerKind, WorkerResult};
use super::submit::{build_agent_payload, submit_via_cli};

/// Error from an action's execution.
pub enum ActionError {
    /// The action timed out or was not processed.
    TimedOut,
    /// The action failed with an error message.
    Failed(String),
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => write!(f, "action timed out"),
            Self::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

/// An executable action. Constructed per dispatch, called once in a worker thread.
pub trait Action: Send {
    fn perform(&self, value: &serde_json::Value) -> Result<String, ActionError>;
}

/// Run an action with an optional timeout.
///
/// Without a timeout, calls `perform` directly (single thread, no overhead).
/// With a timeout, spawns an inner thread and waits on a channel with `recv_timeout`.
/// Returns the action's result or `ActionError::TimedOut`.
pub fn run_action(
    action: Box<dyn Action>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, ActionError> {
    match timeout {
        None => action.perform(value),
        Some(duration) => {
            let (tx, rx) = mpsc::channel();
            let value = value.clone();
            thread::spawn(move || {
                let _ = tx.send(action.perform(&value));
            });
            match rx.recv_timeout(duration) {
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => Err(ActionError::TimedOut),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Err(ActionError::Failed("action panicked".into()))
                }
            }
        }
    }
}

/// Spawn a worker thread that runs an action and sends the result to the engine.
///
/// This is the single dispatch path. Every dispatch site constructs an action,
/// picks the WorkerKind, and calls this function. Thread spawn, run_action call,
/// and WorkerResult send all live here.
pub fn spawn_worker(
    tx: mpsc::Sender<WorkerResult>,
    action: Box<dyn Action>,
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    timeout: Option<Duration>,
) {
    thread::spawn(move || {
        let value = task.value.clone();
        let output = run_action(action, &value.0, timeout);
        let _ = tx.send(WorkerResult {
            task_id,
            task,
            kind,
            result: ActionResult { value, output },
        });
    });
}

/// Pool action: submits a task to the troupe agent pool.
pub struct PoolAction {
    pub root: PathBuf,
    pub invoker: Invoker<TroupeCli>,
    pub docs: String,
    pub step_name: StepName,
    /// Troupe's agent lifecycle timeout (seconds), passed through in the payload.
    pub pool_timeout: Option<u64>,
}

impl Action for PoolAction {
    fn perform(&self, value: &serde_json::Value) -> Result<String, ActionError> {
        let payload = build_agent_payload(
            &self.step_name, value, &self.docs, self.pool_timeout,
        );
        match submit_via_cli(&self.root, &payload, &self.invoker) {
            Ok(Response::Processed { stdout, .. }) => Ok(stdout),
            Ok(Response::NotProcessed { .. }) => Err(ActionError::TimedOut),
            Err(e) => Err(ActionError::Failed(e.to_string())),
        }
    }
}
```

#### 1b. Update `dispatch_task` Pool branch

**File: `runner/mod.rs`**

Add import: `use action::{PoolAction, spawn_worker};` and `use std::time::Duration;`.

Before (`mod.rs:714-723`):
```rust
ActionKind::Pool(..) => {
    let docs = generate_step_docs(step, self.config);
    let timeout = step.options.timeout;
    let pool = self.pool.clone();
    info!(step = %task.step, "submitting task to pool");
    thread::spawn(move || {
        dispatch_pool_task(task_id, task, &docs, timeout, &pool, &tx);
    });
}
```

After:
```rust
ActionKind::Pool(..) => {
    let docs = generate_step_docs(step, self.config);
    info!(step = %task.step, "submitting task to pool");
    let action = Box::new(PoolAction {
        root: self.pool.root.clone(),
        invoker: self.pool.invoker.clone(),
        docs,
        step_name: task.step.clone(),
        pool_timeout: step.options.timeout,
    });
    spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
}
```

Where `timeout` and `tx` are set up before the match:
```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();
    match &step.action {
        ActionKind::Pool(..) => { ... }  // as above
        ActionKind::Command(..) => { ... }  // unchanged for now
    }
}
```

**Complication**: `timeout` is currently extracted inside the Pool branch as `step.options.timeout` (raw `Option<u64>`). It now moves before the match as `step.options.timeout.map(Duration::from_secs)` so both branches share it.

#### 1c. Delete `dispatch_pool_task`

**File: `runner/dispatch.rs`** — remove the `dispatch_pool_task` function (lines 56-74). Remove the `build_agent_payload`, `submit_via_cli` imports from `dispatch.rs` (they're now only used in `action.rs`). Remove the `Response` import (same reason).

**File: `runner/mod.rs`** — remove `dispatch_pool_task` from the `use dispatch::` line.

Compile, run full suite.

---

### Phase 2: `ShellAction` + Command through trait

#### 2a. Add `ShellAction` to `runner/action.rs`

```rust
use super::shell::run_shell_command;

/// Shell action: runs a shell script with the task value on stdin.
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
}

impl Action for ShellAction {
    fn perform(&self, value: &serde_json::Value) -> Result<String, ActionError> {
        let task_json = serde_json::to_string(&serde_json::json!({
            "kind": &self.step_name,
            "value": value,
        }))
        .unwrap_or_default();

        run_shell_command(&self.script, &task_json, Some(&self.working_dir))
            .map_err(ActionError::Failed)
    }
}
```

#### 2b. Update `dispatch_task` Command branch

**File: `runner/mod.rs`**

Add import: `use action::ShellAction;`.

Before (`mod.rs:724-732`):
```rust
ActionKind::Command(CommandAction { script }) => {
    let script = script.clone();
    let working_dir = self.pool.working_dir.clone();
    info!(step = %task.step, script = %script, "executing command");
    thread::spawn(move || {
        dispatch_command_task(task_id, task, &script, &working_dir, &tx);
    });
}
```

After:
```rust
ActionKind::Command(CommandAction { script }) => {
    info!(step = %task.step, script = %script, "executing command");
    let action = Box::new(ShellAction {
        script: script.clone(),
        step_name: task.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });
    spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
}
```

#### 2c. Delete `dispatch_command_task`

**File: `runner/dispatch.rs`** — remove the function (lines 80-100). Remove `use super::shell::run_shell_command;` (now only used in `action.rs`).

**File: `runner/mod.rs`** — remove `dispatch_command_task` from the `use dispatch::` line.

After this, `dispatch_task` has one shape: the match constructs the action, and one `spawn_worker` call at the end:

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();

    let action: Box<dyn Action> = match &step.action {
        ActionKind::Pool(..) => {
            let docs = generate_step_docs(step, self.config);
            info!(step = %task.step, "submitting task to pool");
            Box::new(PoolAction {
                root: self.pool.root.clone(),
                invoker: self.pool.invoker.clone(),
                docs,
                step_name: task.step.clone(),
                pool_timeout: step.options.timeout,
            })
        }
        ActionKind::Command(CommandAction { script }) => {
            info!(step = %task.step, script = %script, "executing command");
            Box::new(ShellAction {
                script: script.clone(),
                step_name: task.step.clone(),
                working_dir: self.pool.working_dir.clone(),
            })
        }
    };

    spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
}
```

This is the seam for PLUGGABLE_ACTION_KINDS. Compile, run full suite.

---

### Phase 3: Finally hooks through trait + cleanup

Finally hooks use `ShellAction` + `spawn_worker` with `WorkerKind::Finally`.

#### 3a. Update `dispatch_finally`

**File: `runner/mod.rs`**

Before (`mod.rs:738-751`):
```rust
fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let script = step.finally_hook.clone()
        .expect("[P073] finally parent's step must have finally_hook");
    let working_dir = self.pool.working_dir.clone();
    let tx = self.tx.clone();
    info!(step = %task.step, parent = ?parent_id, "dispatching finally worker");
    thread::spawn(move || {
        dispatch_finally_task(parent_id, task, &script, &working_dir, &tx);
    });
}
```

After:
```rust
fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let script = step.finally_hook.clone()
        .expect("[P073] finally parent's step must have finally_hook");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();

    info!(step = %task.step, parent = ?parent_id, "dispatching finally worker");
    let action = Box::new(ShellAction {
        script: script.to_string(),
        step_name: task.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });
    spawn_worker(tx, action, parent_id, task, WorkerKind::Finally { parent_id }, timeout);
}
```

**Complication**: `dispatch_finally_task` currently uses the value as `serde_json::to_string(&value.0)` for stdin (just the raw JSON value). `ShellAction::perform` uses `serde_json::to_string(&json!({"kind": step_name, "value": value}))` (wraps in kind+value envelope). This is a behavior change — finally hook stdin format changes from raw value to `{"kind": "...", "value": ...}`. Check whether any existing finally hooks depend on the raw format. If they do, either add a flag to `ShellAction` or create a separate `FinallyShellAction` that preserves the raw format.

#### 3b. Delete `dispatch_finally_task`

**File: `runner/dispatch.rs`** — remove the function (lines 106-122).

**File: `runner/mod.rs`** — remove `dispatch_finally_task` from the `use dispatch::` line.

#### 3c. Clean up `dispatch.rs`

After deleting all three functions, `dispatch.rs` contains only type definitions: `ActionResult`, `WorkerKind`, `WorkerResult`. Move these to `action.rs` and delete `dispatch.rs`.

**File: `runner/mod.rs`** — change `mod dispatch;` → remove. Update `use dispatch::WorkerResult` to `use action::WorkerResult`, etc.

All dispatch functions are gone. All dispatch sites call `spawn_worker`. Compile, run full suite.

## Summary

| Phase | What | Deletes | Adds |
|-------|------|---------|------|
| 0 | Unify result types | `SubmitResult`, `PoolResult`, `CommandResult`, `FinallyResult`, `process_pool_response`, `process_command_response`, `process_finally_response`, `run_command_action` | `ActionError`, `ActionResult`, `WorkerKind`, `runner/action.rs` |
| 1 | Trait + dispatch infra + Pool | `dispatch_pool_task` | `Action` trait, `run_action`, `spawn_worker`, `PoolAction` |
| 2 | Command through trait | `dispatch_command_task` | `ShellAction` |
| 3 | Finally through trait + cleanup | `dispatch_finally_task`, `runner/dispatch.rs` | (reuses `ShellAction` + `WorkerKind::Finally`) |

Each phase compiles and passes tests before moving to the next.

## Behavior changes

`ActionKind` enum variants stay as-is (already renamed from `Action`). State logging and retry logic are unchanged.

**Timeout**: `run_action` enforces `step.options.timeout` for all action kinds via `recv_timeout`. Commands and finally hooks, which previously blocked forever, now timeout when `timeout` is configured. Pool actions get Barnum-level timeout enforcement in addition to troupe's existing agent lifecycle timeout (both derive from `step.options.timeout`).

**Retry classification**: `ActionError::TimedOut` maps to `FailureKind::Timeout` (same as current `Response::NotProcessed` handling). `ActionError::Failed` maps to `FailureKind::SubmitError`. Existing `retry_on_timeout` / `retry_on_invalid_response` controls are unchanged.

**Finally hook stdin** (Phase 3 complication): The stdin format for finally hooks changes from raw JSON value to `{"kind": "step_name", "value": ...}` envelope, because `ShellAction` uses the same envelope as command actions. Must verify existing finally hooks aren't broken by this.
