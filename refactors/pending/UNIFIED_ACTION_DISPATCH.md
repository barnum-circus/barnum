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

### `Action` trait and `ActionHandle`

```rust
pub trait Action: Send {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle;
}

pub struct ActionHandle {
    pub rx: mpsc::Receiver<Result<String, String>>,
    drop_guard: Box<dyn Send>,
}
```

`start` kicks off the work (typically by spawning a thread) and returns an `ActionHandle` immediately — it does not block. The handle has two parts:

- `rx`: receives the action's result (`Ok(stdout)` or `Err(message)`) when the work completes.
- `drop_guard`: type-erased cleanup. When the handle is dropped, the guard's `Drop` impl stops the work (best-effort). For shell actions, this kills the child process. For pool actions, this is a no-op (troupe manages its own lifecycle).

**Contract**: dropping the `ActionHandle` signals "I don't want the result, clean up." The guard's `Drop` does best-effort cancellation — the action should stop, but if it doesn't, the closed channel discards any late sends harmlessly (userland code, not UB). It's cooperative cancellation with a safety net.

**Future migration**: this is a hand-rolled future. The mapping is 1:1:

| Channel + guard | Future |
|---|---|
| `start()` returns `ActionHandle` | `perform()` returns `impl Future` |
| `rx.recv()` | `.await` |
| `rx.recv_timeout(d)` | `timeout(d, future).await` |
| Guard's `Drop` kills work | Future's `Drop` cancels (stops polling + drops state) |

When we adopt async, the guard's cleanup logic moves into the future's captured state. Dropping the future triggers the same `Drop` impls.

### Cancellation guards

Each action provides its own guard type. The guard is boxed as `Box<dyn Send>` in `ActionHandle`, so `run_action` is agnostic — it just drops the handle.

**`ProcessGuard`** (for `ShellAction`):

```rust
struct ProcessGuard(u32); // child PID

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        unsafe { libc::kill(self.0 as i32, libc::SIGKILL); }
    }
}
```

Sends SIGKILL to the child process. If the process already exited, `kill` returns an error that we ignore. PID recycling is theoretically possible but negligible for a CLI tool with short-lived children — the window between child exit and guard drop is typically microseconds. If subprocesses are a concern, `Command::pre_exec` + `setsid` + negative-PID kill can be added to `ShellAction::start` without changing the trait.

**No-op guard** (for `PoolAction`):

```rust
ActionHandle { rx, drop_guard: Box::new(()) }
```

`PoolAction` calls `invoker.run()`, which encapsulates the child process. There's no PID to kill. On timeout, the invoker thread keeps running until troupe returns. This is the same behavior as today — troupe manages its own agent lifecycle.

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

**Behavior change for `Response::NotProcessed`** (already landed in Phase 0): maps to `ActionError::Failed("not processed by pool")` → `FailureKind::SubmitError` (always retries), instead of the old `FailureKind::Timeout` (gated on `retry_on_timeout`). This is more correct: Barnum now owns timeout enforcement via `run_action`, and `NotProcessed` is an operational failure from the pool.

### `run_action`: timeout via deadline + `recv_timeout`

`run_action` takes a boxed action, a value, and an optional timeout. It computes a deadline *before* calling `start`, so time spent in `start` (spawning child processes, etc.) counts against the timeout:

```rust
pub fn run_action(
    action: Box<dyn Action>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, ActionError> {
    let deadline = timeout.map(|d| Instant::now() + d);
    let handle = action.start(value.clone());
    let channel_result = match deadline {
        None => handle.rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            handle.rx.recv_timeout(remaining)
        }
    };
    match channel_result {
        Ok(result) => result.map_err(ActionError::Failed),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(ActionError::TimedOut),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(ActionError::Failed("action panicked".into()))
        }
    }
    // handle drops here — guard's Drop kills the action if still running
}
```

If `start` consumes the entire timeout budget, `remaining` is zero and `recv_timeout(Duration::ZERO)` returns `Timeout` immediately — which drops the handle, triggering the guard's kill.

`run_action` does not spawn any threads — `start` handles that internally. On timeout, `handle` drops at function exit, triggering the guard's `Drop` which kills the underlying work. On success, the work already completed, so the guard's `Drop` is a no-op (process already exited).

Both the timeout and no-timeout paths use the same logic. The only difference is `recv()` vs `recv_timeout()`. This is a slight overhead increase for the no-timeout case (always a channel + thread, even without timeout), but trivial compared to spawning a child process.

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

`spawn_worker` spawns one thread that blocks on `run_action` (which blocks on `handle.rx.recv()`). The action's `start` spawns its own work thread internally. Total threads per action: 2 (spawn_worker thread + action's work thread), plus the child process. `WorkerResult` is sent exactly once — either with the action's output or with a timeout error.

### Timeout flow end-to-end

1. **Source**: `step.options.timeout: Option<u64>` (seconds), configured per-step in the barnum config.
2. **Conversion**: `dispatch_task` and `dispatch_finally` convert to `Option<Duration>` via `step.options.timeout.map(Duration::from_secs)` and pass to `spawn_worker`.
3. **Start**: `spawn_worker`'s thread calls `run_action`, which calls `action.start(value)`. The action spawns its work thread and returns an `ActionHandle`.
4. **Wait**: `run_action` calls `handle.rx.recv_timeout(duration)` (or `recv()` if no timeout).
5. **On timeout**: `recv_timeout` returns `Err(Timeout)`. `run_action` returns `Err(ActionError::TimedOut)`. The `ActionHandle` drops, triggering the guard's `Drop` — for shell actions, this sends SIGKILL to the child process. The action's work thread eventually sees the child exit (or its `tx.send()` fails on the closed channel) and exits.
6. **Result routing**: `spawn_worker` sends `WorkerResult` with `output: Err(ActionError::TimedOut)`. The engine's main loop receives it via `rx.recv()`.
7. **Retry**: `process_submit_result` maps `ActionError::TimedOut` → `FailureKind::Timeout`. `process_retry` checks `options.retry_on_timeout` to decide whether to retry or drop.

For Pool actions specifically: `pool_timeout` (troupe's agent lifecycle timeout, passed in the payload via `build_agent_payload`) and the Barnum-level timeout (enforced by `run_action`) both derive from `step.options.timeout`. The troupe timeout controls how long the agent has to process the task. The Barnum timeout controls how long the engine waits for the result. They use the same value. If troupe times out first, `PoolAction::start`'s thread sends `Err("not processed by pool")` on the handle's channel. If Barnum times out first, `run_action` returns `Err(ActionError::TimedOut)` from `recv_timeout`, the handle drops (no-op guard for pool), and the pool thread continues until troupe returns.

### Concurrency stays in the engine

`in_flight` is managed by `Engine::flush_dispatches` (increment before spawn) and `Engine::process_worker_result` (decrement on completion). Both run on the main thread. Actions run in spawned threads with no access to `Engine`.

## Phased Implementation

### Phase 1: `Action` trait + `run_action` + `spawn_worker` + `PoolAction`

Introduce the trait, the dispatch infrastructure, and move Pool through it. Command stays as-is.

Builds on the unified result types (ActionResult, WorkerKind, ActionError) already on master.

#### 1a. Expand `runner/action.rs`

Phase 0 created this file with `ActionError`. Add the trait, `ActionHandle`, `run_action`, `spawn_worker`, and `PoolAction`. This is the complete file after Phase 1:

```rust
//! Action trait and dispatch infrastructure.

use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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

/// Handle returned by `Action::start`. Dropping this handle cancels the action.
///
/// Contract:
/// - Call `rx.recv()` or `rx.recv_timeout()` to get the result.
/// - Drop the handle to cancel the action (best-effort via guard's `Drop`).
/// - Late sends to a dropped handle are silently discarded.
pub struct ActionHandle {
    pub rx: mpsc::Receiver<Result<String, String>>,
    drop_guard: Box<dyn Send>,
}

impl ActionHandle {
    /// Create a handle with a type-erased cleanup guard.
    pub fn new(rx: mpsc::Receiver<Result<String, String>>, guard: impl Send + 'static) -> Self {
        Self { rx, drop_guard: Box::new(guard) }
    }
}

/// An executable action. Constructed per dispatch, consumed once by `start`.
///
/// `start` kicks off work (typically by spawning a thread) and returns an
/// `ActionHandle` immediately. It does not block. The handle's guard
/// provides cancellation on drop.
pub trait Action: Send {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle;
}

// ==================== run_action ====================

/// Run an action with an optional timeout.
///
/// Computes a deadline before calling `start`, so time spent in `start`
/// counts against the timeout. On timeout, the handle drops — the guard's
/// `Drop` kills the underlying work.
pub fn run_action(
    action: Box<dyn Action>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, ActionError> {
    let deadline = timeout.map(|d| Instant::now() + d);
    let handle = action.start(value.clone());
    let channel_result = match deadline {
        None => handle.rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            handle.rx.recv_timeout(remaining)
        }
    };
    match channel_result {
        Ok(result) => result.map_err(ActionError::Failed),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(ActionError::TimedOut),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(ActionError::Failed("action panicked".into()))
        }
    }
    // handle drops here — guard's Drop kills the action if still running
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
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let payload = build_agent_payload(
                &self.step_name, &value, &self.docs, self.pool_timeout,
            );
            let result = match submit_via_cli(&self.root, &payload, &self.invoker) {
                Ok(Response::Processed { stdout, .. }) => Ok(stdout),
                Ok(Response::NotProcessed { .. }) => Err("not processed by pool".into()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });
        // No-op guard: troupe manages its own agent lifecycle.
        ActionHandle::new(rx, ())
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

#### 2a. Add `ShellAction` and `ProcessGuard` to `runner/action.rs`

`ShellAction::start` spawns the child process, writes stdin, spawns a reader thread, and returns a handle with a `ProcessGuard` that kills the child on drop:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

/// Guard that kills a child process on drop.
struct ProcessGuard(u32);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        unsafe { libc::kill(self.0 as i32, libc::SIGKILL); }
    }
}

/// Shell action: runs a shell script with the task value on stdin.
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
}

impl Action for ShellAction {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
        let (tx, rx) = mpsc::channel();
        let task_json = serde_json::to_string(&serde_json::json!({
            "kind": &self.step_name,
            "value": &value,
        }))
        .unwrap_or_default();

        let child = Command::new("sh")
            .arg("-c")
            .arg(&self.script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&self.working_dir)
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
                return ActionHandle::new(rx, ());
            }
        };

        let pid = child.id();

        // Write stdin, then drop to close the pipe.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(task_json.as_bytes());
        }

        // Reader thread: waits for child, sends result.
        thread::spawn(move || {
            let result = match child.wait_with_output() {
                Ok(out) if out.status.success() => {
                    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
                }
                Ok(out) => Err(String::from_utf8_lossy(&out.stderr).into_owned()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });

        ActionHandle::new(rx, ProcessGuard(pid))
    }
}
```

This replaces `run_shell_command` for the action path. The key difference from the old blocking approach: `start` returns immediately after spawning, and the `ProcessGuard` enables kill-on-timeout via the handle's drop.

`run_shell_command` (`runner/shell.rs`) stays for now — `dispatch_finally_task` still uses it until Phase 3. It will be deleted once all callers go through the trait.

**New dependency**: `libc` (for `kill` in `ProcessGuard::Drop`). Add to `Cargo.toml` for `barnum_config`.

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

**Complication**: `dispatch_finally_task` currently uses `serde_json::to_string(&value.0)` for stdin (raw JSON value). `ShellAction::start` uses `serde_json::to_string(&json!({"kind": step_name, "value": value}))` (kind+value envelope). This changes the stdin format for finally hooks. Check whether any existing finally hooks depend on the raw format. If so, create a `FinallyShellAction` that preserves the raw format, or add a stdin format parameter to `ShellAction`.

#### 3b. Delete `dispatch_finally_task` and clean up

**File: `runner/dispatch.rs`** — remove the function (lines 106-122).

**File: `runner/mod.rs`** — remove `dispatch_finally_task` from the `use dispatch::` line.

After deleting all three dispatch functions, `dispatch.rs` contains only type definitions: `ActionResult`, `WorkerKind`, `WorkerResult`. Move these to `action.rs` and delete `dispatch.rs`. Update `runner/mod.rs`: remove `mod dispatch;`, change `use dispatch::WorkerResult` to `use action::WorkerResult`, etc.

`runner/shell.rs` can be deleted — `ShellAction::start` spawns the child directly and no other callers remain.

All dispatch sites call `spawn_worker`. Compile, run full suite.

## Summary

| Phase | What | Deletes | Adds |
|-------|------|---------|------|
| 1 | Trait + dispatch infra + Pool | `dispatch_pool_task` | `Action` trait, `ActionHandle`, `run_action`, `spawn_worker`, `PoolAction` |
| 2 | Command through trait | `dispatch_command_task` | `ShellAction`, `ProcessGuard`, `libc` dep |
| 3 | Finally + cleanup | `dispatch_finally_task`, `runner/dispatch.rs`, `runner/shell.rs` | (reuses `ShellAction` + `WorkerKind::Finally`) |

Each phase compiles and passes tests before moving to the next.

## Behavior changes

`ActionKind` enum variants stay as-is (already renamed from `Action`). State logging and retry logic are unchanged.

**Timeout**: `run_action` enforces `step.options.timeout` for all action kinds via `handle.rx.recv_timeout()`. Commands and finally hooks, which previously blocked forever, now timeout when configured. On timeout, the `ActionHandle` drops — for shell actions, the `ProcessGuard` sends SIGKILL to the child process. For pool actions, the guard is a no-op (troupe manages its own lifecycle). The engine treats the action as failed and moves on.

**Retry classification for `NotProcessed`** (already landed in Phase 0): maps to `ActionError::Failed` → `FailureKind::SubmitError` (always retries), instead of the old `FailureKind::Timeout`.

**Finally hook stdin** (Phase 3 complication): The stdin format for finally hooks changes from raw JSON value to `{"kind": "step_name", "value": ...}` envelope, because `ShellAction` uses the same envelope as command actions. Must verify existing finally hooks aren't broken by this.

**New dependency**: `libc` is added in Phase 2 for `ProcessGuard`'s SIGKILL. This is contained to `ShellAction`'s guard implementation.
