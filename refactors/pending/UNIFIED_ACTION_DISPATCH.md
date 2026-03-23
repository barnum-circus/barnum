# Unified Action Dispatch

Prerequisite to PLUGGABLE_ACTION_KINDS (future).

## Motivation

Pool actions, Command actions, and finally hooks each have their own dispatch function, their own result type, and their own response-processing path. Timeouts only exist for Pool (via troupe). Commands block forever. Finally hooks ignore concurrency limits.

All of these run work in a thread and send a result back on a channel. They should go through one dispatch path with one trait. If it runs, it goes through the trait. Concurrency and result routing wrap the trait from the outside.

## Current State

Phase 0 (unified result types) is landed on master. All dispatch functions produce unified `ActionResult` with `Result<String, ActionError>`, and `WorkerKind` handles routing between task and finally paths.

### `runner/action.rs`

Defines `ActionError` — the unified error type for action dispatch:

```rust
pub enum ActionError {
    #[expect(dead_code, reason = "constructed by run_action in Phase 1")]
    TimedOut,
    Failed(String),
}
```

`TimedOut` is dead code until Phase 1 introduces `run_action` with `recv_timeout`. `Failed` wraps all current error paths.

### `runner/dispatch.rs`

Unified result types and three dispatch functions:

```rust
pub(super) struct ActionResult {
    pub value: StepInputValue,
    pub output: Result<String, ActionError>,
}

pub(super) enum WorkerKind {
    Task,
    Finally { parent_id: LogTaskId },
}

pub struct WorkerResult {
    pub task_id: LogTaskId,
    pub task: Task,
    pub kind: WorkerKind,
    pub result: ActionResult,
}
```

`dispatch_pool_task` normalizes `Response::Processed` → `Ok(stdout)`, `Response::NotProcessed` → `Err(ActionError::Failed("not processed by pool"))`, `Err(e)` → `Err(ActionError::Failed(e.to_string()))`.

`dispatch_command_task` calls `run_shell_command` directly (the `run_command_action` wrapper was deleted), maps errors via `.map_err(ActionError::Failed)`.

`dispatch_finally_task` calls `run_shell_command`, maps errors via `.map_err(ActionError::Failed)`, sends `WorkerKind::Finally { parent_id }`.

### `runner/response.rs`

Single `process_submit_result` matches on `ActionResult.output`:

```rust
pub fn process_submit_result(
    result: ActionResult, task: &Task, step: &Step, schemas: &CompiledSchemas,
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

### `runner/mod.rs` routing

`process_worker_result` routes via `WorkerKind`:

```rust
let entries = match result.kind {
    dispatch::WorkerKind::Task => {
        self.convert_task_result(result.task_id, &result.task, result.result)
    }
    dispatch::WorkerKind::Finally { parent_id } => {
        self.convert_finally_result(parent_id, result.task.value.clone(), result.result.output)
    }
};
```

`convert_task_result` takes `dispatch::ActionResult`. `convert_finally_result` takes `Result<String, ActionError>`.

### `dispatch_task` and `dispatch_finally` (`runner/mod.rs`)

Still pattern-match on `ActionKind` and call separate dispatch functions with `thread::spawn`. Each dispatch function produces unified `WorkerResult` with `ActionResult` + `WorkerKind`.

### `runner/hooks.rs`

Only contains `call_wake_script`. `run_command_action` was deleted (inlined into `dispatch_command_task`).

### `runner/shell.rs`

`run_shell_command` blocks forever on `child.wait_with_output()`. No timeout.

## Design Decisions

### Naming: `Action` trait, config enum is `ActionKind`

The config enum was already renamed from `Action` to `ActionKind` on master. The `Action` name belongs to the trait.

### `Action` trait

```rust
pub trait Action: Send {
    fn perform(&self, value: &serde_json::Value) -> Result<String, String>;
}
```

Takes a value, runs something, returns stdout or an error message. The action knows nothing about timeouts or `ActionError` — those are `run_action`'s concerns.

### Why active kill is hard: `perform` is opaque

`perform` is a blocking call that typically spawns a child process (bash for commands, troupe CLI for pool). The outer thread in `run_action` can't see that child process because it's created inside `perform`, which runs in the inner thread. When `recv_timeout` fires, `run_action` can stop waiting — but the inner thread and its child process keep running.

This is a fundamental consequence of the trait abstraction. The outer code doesn't know what `perform` did: it might have spawned one process, multiple processes, or no process at all (PoolAction's child is managed by troupe's invoker). There's no general way to reach into `perform` and kill whatever it's blocking on.

Three approaches to deal with this:

**1. Don't kill — accept the leak.** `recv_timeout` fires, `run_action` returns `TimedOut`, the inner thread and child process keep running until the child exits on its own. The channel send fails silently (receiver dropped), and the thread exits. This is what pool actions already do today — troupe manages its own agent lifecycle. For a CLI tool that runs to completion, leaked children get cleaned up on process exit. No `KillHandle`, no `libc`, no `Arc`, no atomic — the simplest option.

**2. Cooperative PID registration (KillHandle).** The action registers its child PID after spawning, and `run_action` sends SIGKILL on timeout. This uses an `AtomicU32` because two threads share one integer: the inner thread writes the PID (after `child.spawn()`), the outer thread reads it (on timeout). `AtomicU32` is the lightest primitive for this — no heap allocation, no locking, no contention. Limitations: only works for the single registered process. If the shell script spawns subprocesses, killing just the `sh` process leaves orphans (fixable with process groups via `setsid` + negative PID kill). PoolAction can't register because `invoker.run()` encapsulates the child.

**3. Split the trait into spawn + wait.** Instead of one blocking `perform`, have `spawn() -> Handle` and `Handle::wait() -> Result`. The outer code gets the handle directly, can kill it on timeout. No atomics needed. But this doesn't generalize — PoolAction has no meaningful "spawn" step (it's a single blocking RPC via `invoker.run()`), so it would need a degenerate impl that starts a thread in `spawn` and joins it in `wait`, recreating the current pattern with more abstraction.

**Decision: approach 1 (don't kill).** Active kill adds a `libc` dependency, `Arc<KillHandle>` threading through `run_action`, a `kill: &KillHandle` parameter on every `Action::perform`, and only works for actions that spawn a single child process the action knows about. The benefit is marginal for a CLI tool: timed-out children finish on their own or get cleaned up on process exit. If active kill becomes necessary later (long-running daemon mode), it can be added as a separate concern without changing the `Action` trait — `run_action` can wrap actions in a process group using `Command::pre_exec` + `setsid` and kill the group on timeout, outside the trait entirely.

### `ActionError`: only `run_action` produces timeouts

```rust
pub enum ActionError {
    /// run_action's recv_timeout fired.
    TimedOut,
    /// The action returned Err(message).
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

`TimedOut` is produced exclusively by `run_action` when `recv_timeout` fires. Actions never return it — they return `Result<String, String>`, and `run_action` wraps `Err(msg)` into `ActionError::Failed(msg)`.

`process_submit_result` maps `ActionError::TimedOut` → `FailureKind::Timeout` and `ActionError::Failed` → `FailureKind::SubmitError`, preserving existing retry semantics for timeouts.

**Behavior change for `Response::NotProcessed`**: currently maps to `FailureKind::Timeout` (checked against `retry_on_timeout`). After this refactor, `PoolAction::perform` returns `Err("not processed by pool")` which becomes `ActionError::Failed` → `FailureKind::SubmitError` (always retries). This is more correct: Barnum now enforces timeout via `run_action`, and `NotProcessed` is an operational failure from the pool, not a Barnum timeout.

### `run_action`: timeout via `recv_timeout`

`run_action` takes a boxed action, a value, and an optional timeout. Without a timeout, it calls `perform` directly on the current thread — no channel, no extra thread. With a timeout, it spawns an inner thread, waits with `recv_timeout`, and returns `TimedOut` if the deadline fires:

```rust
pub fn run_action(
    action: Box<dyn Action>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, ActionError> {
    match timeout {
        None => action.perform(value).map_err(ActionError::Failed),
        Some(duration) => {
            let (tx, rx) = mpsc::channel();
            let value = value.clone();
            thread::spawn(move || {
                let _ = tx.send(action.perform(&value));
            });
            match rx.recv_timeout(duration) {
                Ok(result) => result.map_err(ActionError::Failed),
                Err(mpsc::RecvTimeoutError::Timeout) => Err(ActionError::TimedOut),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Err(ActionError::Failed("action panicked".into()))
                }
            }
        }
    }
}
```

In the timeout case: `recv_timeout` fires, `run_action` returns `Err(ActionError::TimedOut)`. The inner thread and its child process keep running until the child exits on its own. When `perform` eventually returns, `tx.send()` fails silently (receiver dropped) and the thread exits. No active kill — see "Why active kill is hard" above.

In the no-timeout case: `perform` is called directly on the current thread. No channel, no spawn, no overhead.

### `spawn_worker`: the single dispatch path

Every dispatch site constructs an action and calls `spawn_worker`. Thread spawn, `run_action` call, and `WorkerResult` send live in one place:

```rust
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
4. **On timeout**: `run_action` returns `Err(ActionError::TimedOut)`. The inner thread and its child process keep running until the child exits on its own. When `perform` returns, the channel send fails silently (receiver dropped) and the thread exits.
5. **Result routing**: `spawn_worker` sends `WorkerResult` with `output: Err(ActionError::TimedOut)`. The engine's main loop receives it via `rx.recv()`.
6. **Retry**: `process_submit_result` maps `ActionError::TimedOut` → `FailureKind::Timeout`. `process_retry` checks `options.retry_on_timeout` to decide whether to retry or drop.

For Pool actions specifically: `pool_timeout` (troupe's agent lifecycle timeout, passed in the payload via `build_agent_payload`) and the Barnum-level timeout (enforced by `run_action`) both derive from `step.options.timeout`. The troupe timeout controls how long the agent has to process the task. The Barnum timeout controls how long the engine waits for the result. They use the same value. If troupe times out first, `PoolAction::perform` returns `Err("not processed by pool")`. If Barnum times out first, `run_action` returns `Err(ActionError::TimedOut)` from `recv_timeout` and the pool thread continues until troupe returns.

### Concurrency stays in the engine

`in_flight` is managed by `Engine::flush_dispatches` (increment before spawn) and `Engine::process_worker_result` (decrement on completion). Both run on the main thread. Actions run in spawned threads with no access to `Engine`.

## Phased Implementation

### Phase 1: `Action` trait + `run_action` + `spawn_worker` + `PoolAction`

Introduce the trait, the dispatch infrastructure, and move Pool through it. Command stays as-is.

Builds on the unified result types (ActionResult, WorkerKind, ActionError) already on master.

#### 1a. Expand `runner/action.rs`

Phase 0 created this file with `ActionError`. Add the trait, `run_action`, `spawn_worker`, and `PoolAction`. This is the complete file after Phase 1:

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

// ==================== ActionError ====================

/// Error from action dispatch. Only `run_action` produces `TimedOut`.
pub enum ActionError {
    TimedOut,
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

// ==================== Action trait ====================

/// An executable action. Constructed per dispatch, called once in a worker thread.
///
/// `perform` returns `Result<String, String>` — stdout or error message.
/// It does not return `ActionError`; timeout is `run_action`'s concern.
pub trait Action: Send {
    fn perform(&self, value: &serde_json::Value) -> Result<String, String>;
}

// ==================== run_action ====================

/// Run an action with an optional timeout.
///
/// Without a timeout, calls `perform` directly on the current thread.
/// With a timeout, spawns an inner thread and waits with `recv_timeout`.
/// On timeout, the inner thread and its child process keep running until
/// the child exits on its own — see "Why active kill is hard" in the
/// design doc.
pub fn run_action(
    action: Box<dyn Action>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, ActionError> {
    match timeout {
        None => action.perform(value).map_err(ActionError::Failed),
        Some(duration) => {
            let (tx, rx) = mpsc::channel();
            let value = value.clone();
            thread::spawn(move || {
                let _ = tx.send(action.perform(&value));
            });
            match rx.recv_timeout(duration) {
                Ok(result) => result.map_err(ActionError::Failed),
                Err(mpsc::RecvTimeoutError::Timeout) => Err(ActionError::TimedOut),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Err(ActionError::Failed("action panicked".into()))
                }
            }
        }
    }
}

// ==================== spawn_worker ====================

/// Spawn a worker thread that runs an action and sends the result to the engine.
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

// ==================== PoolAction ====================

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
    fn perform(&self, value: &serde_json::Value) -> Result<String, String> {
        let payload = build_agent_payload(
            &self.step_name, value, &self.docs, self.pool_timeout,
        );
        match submit_via_cli(&self.root, &payload, &self.invoker) {
            Ok(Response::Processed { stdout, .. }) => Ok(stdout),
            Ok(Response::NotProcessed { .. }) => Err("not processed by pool".into()),
            Err(e) => Err(e.to_string()),
        }
    }
}
```

#### 1b. Update `dispatch_task` Pool branch

**File: `runner/mod.rs`**

Add imports: `use action::{PoolAction, spawn_worker};` and `use std::time::Duration;`.

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

**Complication**: `timeout` is currently extracted inside the Pool branch as `step.options.timeout` (raw `Option<u64>`). It moves before the match as `step.options.timeout.map(Duration::from_secs)` so both branches share it.

#### 1c. Delete `dispatch_pool_task`

**File: `runner/dispatch.rs`** — remove the `dispatch_pool_task` function (lines 56-74). Remove the `build_agent_payload`, `submit_via_cli` imports from `dispatch.rs` (now only used in `action.rs`). Remove the `Response` import.

**File: `runner/mod.rs`** — remove `dispatch_pool_task` from the `use dispatch::` line.

Compile, run full suite.

---

### Phase 2: `ShellAction` + Command through trait

#### 2a. Add `ShellAction` to `runner/action.rs`

`ShellAction` delegates to `run_shell_command` for the actual shell execution:

```rust
use super::shell::run_shell_command;

/// Shell action: runs a shell script with the task value on stdin.
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
}

impl Action for ShellAction {
    fn perform(&self, value: &serde_json::Value) -> Result<String, String> {
        let task_json = serde_json::to_string(&serde_json::json!({
            "kind": &self.step_name,
            "value": value,
        }))
        .unwrap_or_default();

        run_shell_command(&self.script, &task_json, Some(&self.working_dir))
    }
}
```

`run_shell_command` (`runner/shell.rs`) stays as-is — it spawns `sh -c`, writes stdin, calls `wait_with_output`, and returns `Result<String, String>`. No need to inline it since there's no kill handle to register with.

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

**File: `runner/dispatch.rs`** — remove the function (lines 80-100). Keep `use super::shell::run_shell_command;` — `dispatch_finally_task` still uses it until Phase 3.

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

**Complication**: `dispatch_finally_task` currently uses `serde_json::to_string(&value.0)` for stdin (raw JSON value). `ShellAction::perform` uses `serde_json::to_string(&json!({"kind": step_name, "value": value}))` (kind+value envelope). This changes the stdin format for finally hooks. Check whether any existing finally hooks depend on the raw format. If so, create a `FinallyShellAction` that preserves the raw format, or add a stdin format parameter to `ShellAction`.

#### 3b. Delete `dispatch_finally_task` and clean up

**File: `runner/dispatch.rs`** — remove the function (lines 106-122).

**File: `runner/mod.rs`** — remove `dispatch_finally_task` from the `use dispatch::` line.

After deleting all three dispatch functions, `dispatch.rs` contains only type definitions: `ActionResult`, `WorkerKind`, `WorkerResult`. Move these to `action.rs` and delete `dispatch.rs`. Update `runner/mod.rs`: remove `mod dispatch;`, change `use dispatch::WorkerResult` to `use action::WorkerResult`, etc.

`runner/shell.rs` stays — `ShellAction::perform` calls `run_shell_command`.

All dispatch sites call `spawn_worker`. Compile, run full suite.

## Summary

| Phase | What | Deletes | Adds |
|-------|------|---------|------|
| 1 | Trait + dispatch infra + Pool | `dispatch_pool_task` | `Action` trait, `run_action`, `spawn_worker`, `PoolAction` |
| 2 | Command through trait | `dispatch_command_task` | `ShellAction` |
| 3 | Finally + cleanup | `dispatch_finally_task`, `runner/dispatch.rs` | (reuses `ShellAction` + `WorkerKind::Finally`) |

Each phase compiles and passes tests before moving to the next.

## Behavior changes

`ActionKind` enum variants stay as-is (already renamed from `Action`). State logging and retry logic are unchanged.

**Timeout**: `run_action` enforces `step.options.timeout` for all action kinds via `recv_timeout`. Commands and finally hooks, which previously blocked forever, now timeout when configured. On timeout, the inner thread and its child process keep running until the child exits on its own (see "Why active kill is hard" in Design Decisions). The engine treats the action as failed and moves on.

**Retry classification for `NotProcessed`**: currently maps to `FailureKind::Timeout` (checked against `retry_on_timeout`). After this refactor, `PoolAction::perform` returns `Err("not processed by pool")` which becomes `ActionError::Failed` → `FailureKind::SubmitError` (always retries). This is more correct: Barnum now owns timeout enforcement via `run_action`, and `NotProcessed` is an operational failure from the pool.

**Finally hook stdin** (Phase 3 complication): The stdin format for finally hooks changes from raw JSON value to `{"kind": "step_name", "value": ...}` envelope, because `ShellAction` uses the same envelope as command actions. Must verify existing finally hooks aren't broken by this.
