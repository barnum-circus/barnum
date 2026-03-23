# Unified Action Dispatch

**Prerequisite to:** PLUGGABLE_ACTION_KINDS (future)

## Motivation

Everything that executes — tasks, pre-hooks, post-hooks, finally hooks — should go through a single dispatch trait. Today, Pool and Command actions have separate dispatch functions, hooks run through ad-hoc code paths, and timeouts only exist for Pool actions. Commands block forever. Hooks don't participate in concurrency limits.

The core principle: **if it runs, it goes through the trait.** Every executable unit — Pool task, Command task, pre-hook, post-hook, finally hook — is scheduled, timed out, and tracked through the same interface. All contribute to the concurrency limit.

## Current State

### `dispatch_task` (`runner/mod.rs`)

The engine's `dispatch_task` pattern-matches on `Action` and calls different functions:

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let tx = self.tx.clone();
    match &step.action {
        Action::Pool(..) => {
            // constructs docs, timeout, pool — calls dispatch_pool_task in a thread
        }
        Action::Command(CommandAction { script }) => {
            // constructs script, working_dir — calls dispatch_command_task in a thread
        }
    }
}
```

### `dispatch_pool_task` (`runner/dispatch.rs:124-154`)

Runs in a spawned thread. Calls `run_pre_hook_or_error`, then `build_agent_payload` + `submit_via_cli`. Sends `SubmitResult::Pool(PoolResult { value, response: io::Result<Response> })`.

### `dispatch_command_task` (`runner/dispatch.rs:160-192`)

Runs in a spawned thread. Calls `run_pre_hook_or_error`, then `run_command_action` (which calls `run_shell_command`). Sends `SubmitResult::Command(CommandResult { value, output: io::Result<String> })`.

### `dispatch_finally_task` (`runner/dispatch.rs:198-214`)

Runs in a spawned thread. Calls `run_shell_command` directly (no pre-hook). Sends `SubmitResult::Finally(FinallyResult { value, output: Result<String, String> })`.

### `SubmitResult` (`runner/dispatch.rs:53-58`)

```rust
pub(super) enum SubmitResult {
    Pool(PoolResult),
    Command(CommandResult),
    Finally(FinallyResult),
    PreHookError(String),
}
```

Each variant wraps a different result type: `io::Result<Response>` for Pool, `io::Result<String>` for Command, `Result<String, String>` for Finally.

### `process_submit_result` (`runner/response.rs:46-127`)

Has separate match arms for Pool, Command, Finally, and PreHookError. Pool and Command both converge on `process_stdout` for the success case, but take different paths to get there:
- Pool: unwraps `Response::Processed { stdout }` → `process_stdout`; `Response::NotProcessed` → `FailureKind::Timeout`
- Command: unwraps `io::Result<String>` → `process_stdout`; no timeout variant

### Hooks (`runner/hooks.rs`)

- `run_pre_hook`: runs a shell command, returns transformed `serde_json::Value`
- `run_post_hook`: runs a shell command on the main thread, returns modified `PostHookInput`
- Neither has timeout. Neither contributes to `in_flight` count.

### `run_shell_command` (`runner/shell.rs`)

Blocks forever on `child.wait_with_output()`. No timeout.

## Design Decisions

### Why timeout is NOT a default method on the trait

The user's instinct was to put a default method on `Executor` that wraps `execute()` with timeout enforcement, so everything goes through the same path. After analysis, this doesn't work because timeout enforcement is inherently executor-specific:

- **PoolExecutor**: Timeout is part of the troupe protocol. The `pool_timeout` is sent in the JSON payload, and the pool daemon enforces it server-side by returning `Response::NotProcessed` when the agent doesn't respond in time. There's no child process to kill.
- **ShellExecutor**: Timeout requires `wait-timeout` crate to call `child.wait_timeout(duration)` on the spawned shell process, then `child.kill()` if it expires. The executor has the `Child` handle; a wrapper doesn't.

A default method wrapping `execute()` would have to spawn a thread, join with a timeout, and... leak the blocking thread if it expires. There's no way to cancel a blocking `execute()` from outside without cooperation from the executor.

### Where the unification happens instead

`dispatch_via_executor` is the single dispatch point. Every executable unit goes through this function, which:
1. Runs the pre-hook (if any)
2. Calls `executor.execute()`
3. Maps the result to `ActionOutcome` (Success/Timeout/Error)
4. Sends the `WorkerResult` on the channel

This is the "if it runs, it goes through the trait" guarantee — it's a free function wrapping the trait call, not a trait method. It's correct that the function is external to the trait because it has access to the pre-hook, the channel, and the task metadata that executors shouldn't know about.

### Why concurrency is NOT on the trait

The `in_flight` counter is managed by `Engine::flush_dispatches` (increments before spawning) and `Engine::process_worker_result` (decrements on completion). Both run on the main thread. Executors run in spawned threads and have no access to `Engine`. Concurrency is a scheduling concern, not an execution concern. The executor shouldn't know or care about concurrency limits.

### Timeout is enforced by barnum, not the agent pool

Today, timeout only exists for Pool actions, and it's enforced by troupe (the pool daemon). The `pool_timeout` is sent in the JSON payload, and the pool returns `Response::NotProcessed` when the agent doesn't respond in time. Commands have no timeout at all — they block forever.

After this refactor, **barnum enforces timeout for all executor types.** The timeout clock starts when `executor.execute()` is called inside the spawned worker thread — i.e., after the task has been dequeued from `pending_dispatches` and dispatched. Time spent waiting in the queue does NOT count toward the timeout.

Each executor handles timeout internally using whatever mechanism fits its execution context:
- **ShellExecutor**: `wait-timeout` crate on the child process (`child.wait_timeout(duration)`, then `child.kill()` on expiry)
- **PoolExecutor**: `wait-timeout` on the CLI child process that `submit_via_cli` spawns. If barnum's local timeout fires before troupe responds, barnum kills the CLI process and moves on. Troupe's internal timeout (`pool_timeout` in the payload) is kept as a secondary mechanism so the pool daemon can clean up its agent, but barnum does not rely on it.

### Trait return type evolution

Phases 1-3 use `Result<String, String>` with a sentinel constant (`TIMEOUT_SENTINEL = "timeout"`) to distinguish timeout from other errors. `dispatch_via_executor` maps `Err(TIMEOUT_SENTINEL)` to `ActionOutcome::Timeout`, preserving the timeout→retry distinction:

- PoolExecutor returns `Err(TIMEOUT_SENTINEL)` when `Response::NotProcessed` is received
- ShellExecutor returns `Err(TIMEOUT_SENTINEL)` when `wait_timeout` expires (Phase 4)

This keeps the trait simple (`Result<String, String>`) while preserving behavioral correctness from Phase 1 onward.

## Proposed Changes — Phased Implementation

### Phase 1: Executor trait + PoolExecutor

**Goal:** Pool actions go through the trait. Everything else stays as-is. Tests pass.

#### 1a. Create `runner/executor.rs`

```rust
use std::path::Path;

/// Trait for executing work units.
///
/// Every action kind implements this. The runner dispatches all work through
/// this interface, giving every unit timeout enforcement and concurrency accounting.
///
/// Input is the task's value (as a serde_json::Value). Each executor knows
/// how to package it for its execution context.
///
/// Working directory and other configuration are captured at construction time.
pub trait Executor: Send {
    fn execute(&self, value: &serde_json::Value) -> Result<String, String>;
}

/// Outcome of executing a work unit (runner-level, not trait-level).
pub enum ActionOutcome {
    Success(String),
    Timeout,
    Error(String),
}
```

#### 1b. Create `PoolExecutor` in `runner/executor.rs`

```rust
use std::path::PathBuf;
use cli_invoker::Invoker;
use troupe::Response;
use troupe_cli::TroupeCli;
use crate::types::StepName;
use super::submit::{build_agent_payload, submit_via_cli};

pub struct PoolExecutor {
    pub root: PathBuf,
    pub invoker: Invoker<TroupeCli>,
    pub docs: String,
    pub step_name: StepName,
    pub pool_timeout: Option<u64>,
}

impl Executor for PoolExecutor {
    fn execute(&self, value: &serde_json::Value) -> Result<String, String> {
        let payload = build_agent_payload(
            &self.step_name, value, &self.docs, self.pool_timeout,
        );
        match submit_via_cli(&self.root, &payload, &self.invoker) {
            Ok(Response::Processed { stdout, .. }) => Ok(stdout),
            Ok(Response::NotProcessed { .. }) => Err(TIMEOUT_SENTINEL.into()),
            Err(e) => Err(e.to_string()),
        }
    }
}
```

`TIMEOUT_SENTINEL` is a module-level constant string (`"timeout"`) that `dispatch_via_executor` uses to distinguish timeout from other errors (see 1c). This is a pragmatic choice: the trait stays simple (`Result<String, String>`) while preserving the timeout→retry distinction that exists today. The sentinel is an internal implementation detail between executors and the dispatcher.

This moves `troupe::Response` handling INTO PoolExecutor (out of `response.rs`).

#### 1c. Add `TIMEOUT_SENTINEL` and `dispatch_via_executor` in `runner/dispatch.rs`

Add the sentinel constant and the unified dispatch function:

```rust
/// Sentinel error string returned by executors to indicate timeout.
/// dispatch_via_executor maps this to ActionOutcome::Timeout.
pub(super) const TIMEOUT_SENTINEL: &str = "timeout";

/// Execute a task through an Executor (runs in spawned thread).
///
/// Runs pre-hook, calls executor.execute(), sends the result on the channel.
/// Maps the executor's Result into ActionOutcome, using TIMEOUT_SENTINEL
/// to distinguish timeout from other errors.
pub fn dispatch_via_executor(
    task_id: LogTaskId,
    task: Task,
    pre_hook: Option<&HookScript>,
    executor: Box<dyn Executor>,
    working_dir: &Path,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = match run_pre_hook_or_error(pre_hook, &task.value, working_dir) {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(WorkerResult {
                task_id,
                task,
                result: SubmitResult::PreHookError(e),
            });
            return;
        }
    };

    let result = executor.execute(&value.0);
    let outcome = match result {
        Ok(stdout) => ActionOutcome::Success(stdout),
        Err(e) if e == TIMEOUT_SENTINEL => ActionOutcome::Timeout,
        Err(e) => ActionOutcome::Error(e),
    };
    let _ = tx.send(WorkerResult {
        task_id,
        task,
        result: SubmitResult::Action(ActionResult { value, outcome }),
    });
}
```

#### 1d. Add `SubmitResult::Action` variant

```rust
/// Raw result from an executor.
pub(super) struct ActionResult {
    pub value: StepInputValue,
    pub outcome: ActionOutcome,
}

pub(super) enum SubmitResult {
    Pool(PoolResult),         // ← still exists (Command still uses old path)
    Command(CommandResult),   // ← still exists
    Finally(FinallyResult),   // ← still exists
    Action(ActionResult),     // ← NEW: Pool goes through this now
    PreHookError(String),
}
```

#### 1e. Add `SubmitResult::Action` arm to `process_submit_result`

In `runner/response.rs`, add:

```rust
SubmitResult::Action(ActionResult { value, outcome }) => match outcome {
    ActionOutcome::Success(stdout) => {
        let (outcome, post_input) = process_stdout(&stdout, task, &value, step, schemas);
        ProcessedSubmit { outcome, post_input }
    }
    ActionOutcome::Timeout => {
        let outcome = process_retry(task, &step.options, FailureKind::Timeout);
        ProcessedSubmit {
            outcome,
            post_input: PostHookInput::Timeout(PostHookTimeout { input: value }),
        }
    }
    ActionOutcome::Error(error) => {
        error!(step = %task.step, %error, "action failed");
        let outcome = process_retry(task, &step.options, FailureKind::SubmitError);
        ProcessedSubmit {
            outcome,
            post_input: PostHookInput::Error(PostHookError { input: value, error }),
        }
    }
},
```

#### 1f. Modify `dispatch_task` Pool branch

In `runner/mod.rs`, change the `Action::Pool` arm:

```rust
Action::Pool(..) => {
    let pre_hook = step.pre.clone();
    let docs = generate_step_docs(step, self.config);
    let working_dir = self.pool.working_dir.clone();
    let executor = Box::new(PoolExecutor {
        root: self.pool.root.clone(),
        invoker: self.pool.invoker.clone(),
        docs,
        step_name: task.step.clone(),
        pool_timeout: step.options.timeout,
    });

    info!(step = %task.step, "submitting task to pool");
    thread::spawn(move || {
        dispatch_via_executor(
            task_id, task, pre_hook.as_ref(), executor, &working_dir, &tx,
        );
    });
}
```

#### 1g. Delete `dispatch_pool_task`

It's now unused. Remove from `dispatch.rs`. Remove `PoolResult` struct.

#### 1h. Remove `SubmitResult::Pool` variant

Remove from `SubmitResult`. Remove the `SubmitResult::Pool` arm from `process_submit_result`. Remove `process_pool_response` function. Remove `use troupe::Response` from `response.rs`.

**At this point:** Pool goes through `Executor` trait → `dispatch_via_executor` → `SubmitResult::Action`. Command still uses `dispatch_command_task` → `SubmitResult::Command`. Tests pass. Compile, run full suite.

---

### Phase 2: ShellExecutor + Command through trait

**Goal:** Command actions also go through the trait. The `match &step.action` in `dispatch_task` constructs an executor and calls the same `dispatch_via_executor` for both. No more per-kind dispatch functions.

#### 2a. Add `ShellExecutor` to `runner/executor.rs`

```rust
use std::path::PathBuf;
use crate::types::StepName;
use super::shell::run_shell_command;

pub struct ShellExecutor {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
}

impl Executor for ShellExecutor {
    fn execute(&self, value: &serde_json::Value) -> Result<String, String> {
        let task_json = serde_json::to_string(&serde_json::json!({
            "kind": &self.step_name,
            "value": value,
        }))
        .unwrap_or_default();

        run_shell_command(&self.script, &task_json, Some(&self.working_dir))
    }
}
```

#### 2b. Modify `dispatch_task` Command branch

```rust
Action::Command(CommandAction { script }) => {
    let pre_hook = step.pre.clone();
    let working_dir = self.pool.working_dir.clone();
    let executor = Box::new(ShellExecutor {
        script: script.clone(),
        step_name: task.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });

    info!(step = %task.step, script = %script, "executing command");
    thread::spawn(move || {
        dispatch_via_executor(
            task_id, task, pre_hook.as_ref(), executor, &working_dir, &tx,
        );
    });
}
```

#### 2c. Delete `dispatch_command_task`

Remove from `dispatch.rs`. Remove `CommandResult` struct. Remove `run_command_action` from `hooks.rs` (it was a thin wrapper).

#### 2d. Remove `SubmitResult::Command` variant

Remove from `SubmitResult`. Remove the `SubmitResult::Command` arm from `process_submit_result`. Remove `process_command_response` function.

#### 2e. Observe: `dispatch_task` no longer cares about action kind for dispatch

Both branches now do the same thing: construct a `Box<dyn Executor>`, call `dispatch_via_executor`. The match is only for choosing which executor to construct. This is the seam for PLUGGABLE_ACTION_KINDS.

**At this point:** Pool and Command both go through `Executor` trait → `dispatch_via_executor` → `SubmitResult::Action`. `SubmitResult` has only `Action`, `Finally`, and `PreHookError`. `response.rs` has no `troupe::Response` dependency. Tests pass. Compile, run full suite.

---

### Phase 3: Finally hooks through trait

**Goal:** Finally hooks also go through `dispatch_via_executor` with a `ShellExecutor`.

#### 3a. Modify `dispatch_finally` in `runner/mod.rs`

Currently:
```rust
fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("...");
    let script = step.finally_hook.clone().expect("...");
    let working_dir = self.pool.working_dir.clone();
    let tx = self.tx.clone();
    thread::spawn(move || {
        dispatch_finally_task(parent_id, task, &script, &working_dir, &tx);
    });
}
```

Change to:
```rust
fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("...");
    let script = step.finally_hook.clone().expect("...");
    let working_dir = self.pool.working_dir.clone();
    let tx = self.tx.clone();
    let executor = Box::new(ShellExecutor {
        script: script.to_string(),
        step_name: task.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });
    thread::spawn(move || {
        // Note: no pre-hook for finally
        dispatch_via_executor(parent_id, task, None, executor, &working_dir, &tx);
    });
}
```

**Problem:** `dispatch_via_executor` currently sends `SubmitResult::Action`. But `process_worker_result` in `mod.rs` checks `SubmitResult::Finally` to route to `convert_finally_result`. We need a way to distinguish finally results from action results.

**Options:**
1. Add a `is_finally: bool` field to `ActionResult`
2. Keep the Finally routing in `process_worker_result` by using WorkerResult metadata
3. Unify finally processing into the standard action path

Option 3 is cleanest: finally hooks produce the same output shape (stdout = JSON array of tasks). The only difference is how the result is recorded in the state log (`FinallyRun` vs `TaskCompleted`). This can be driven by metadata in `WorkerResult` (e.g., a `WorkerKind` enum: `Task` vs `Finally { parent_id }`).

```rust
pub enum WorkerKind {
    Task,
    Finally { parent_id: LogTaskId },
}

pub struct WorkerResult {
    pub task_id: LogTaskId,
    pub task: Task,
    pub kind: WorkerKind,
    pub result: SubmitResult,
}
```

`process_worker_result` dispatches based on `kind`:
- `WorkerKind::Task` → `convert_task_result` (existing path)
- `WorkerKind::Finally` → `convert_finally_result` (existing path, but now receives `SubmitResult::Action` instead of `SubmitResult::Finally`)

#### 3b. Delete `dispatch_finally_task`

Remove from `dispatch.rs`. Remove `FinallyResult` struct. Remove `SubmitResult::Finally` variant. Remove `SubmitResult::Finally` arm from `process_submit_result`. Remove `process_finally_response` function.

**At this point:** `SubmitResult` has only two variants: `Action(ActionResult)` and `PreHookError(String)`. All executable units go through `dispatch_via_executor`. Tests pass.

---

### Phase 4: Timeout enforcement

**Goal:** Barnum enforces timeout for all executor types. Commands can timeout. Pool timeout is enforced locally (not just by troupe).

**Dependency:** Add `wait-timeout` crate to `barnum_config/Cargo.toml`.

#### 4a. Add timeout to `run_shell_command`

Refactor `runner/shell.rs` to separate spawning from waiting, then add timeout:

```rust
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

use super::dispatch::TIMEOUT_SENTINEL;

/// Spawn a shell command with stdin input. Returns the child process.
fn spawn_shell(
    script: &str,
    stdin_input: &str,
    working_dir: Option<&Path>,
) -> Result<Child, String> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_input.as_bytes());
    }

    Ok(child)
}

/// Check an Output for success and extract stdout.
fn check_output(output: Output) -> Result<String, String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("exited with status {}: {}", output.status, stderr.trim()));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("output is not valid UTF-8: {e}"))
}

/// Run a shell command with optional timeout.
///
/// Returns TIMEOUT_SENTINEL on timeout, enabling ActionOutcome::Timeout mapping.
pub fn run_shell_command(
    script: &str,
    stdin_input: &str,
    working_dir: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<String, String> {
    let mut child = spawn_shell(script, stdin_input, working_dir)?;

    match timeout {
        Some(duration) => match child.wait_timeout(duration) {
            Ok(Some(status)) => {
                // Process finished within timeout. Read remaining stdout/stderr.
                let output = child.wait_with_output()
                    .map_err(|e| format!("wait failed: {e}"))?;
                check_output(output)
            }
            Ok(None) => {
                // Timeout expired. Kill the process.
                let _ = child.kill();
                let _ = child.wait(); // reap zombie
                Err(TIMEOUT_SENTINEL.into())
            }
            Err(e) => Err(format!("wait failed: {e}")),
        },
        None => {
            let output = child.wait_with_output()
                .map_err(|e| format!("wait failed: {e}"))?;
            check_output(output)
        }
    }
}
```

`child.kill()` is cross-platform (SIGKILL on Unix, TerminateProcess on Windows).

**Note on `wait_timeout` + `wait_with_output`:** After `wait_timeout` returns `Ok(Some(status))`, the process has exited but stdout/stderr pipes may still have unread data. We call `wait_with_output()` to drain them. This is safe because the process is already dead.

#### 4b. Add timeout to `ShellExecutor`

```rust
pub struct ShellExecutor {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
    pub timeout: Option<Duration>,  // ← new field
}

impl Executor for ShellExecutor {
    fn execute(&self, value: &serde_json::Value) -> Result<String, String> {
        let task_json = serde_json::to_string(&serde_json::json!({
            "kind": &self.step_name,
            "value": value,
        }))
        .unwrap_or_default();

        run_shell_command(&self.script, &task_json, Some(&self.working_dir), self.timeout)
    }
}
```

#### 4c. Pass `step.options.timeout` when constructing ShellExecutor

In `dispatch_task`'s Command branch (already modified in Phase 2b):
```rust
let executor = Box::new(ShellExecutor {
    script: script.clone(),
    step_name: task.step.clone(),
    working_dir: self.pool.working_dir.clone(),
    timeout: step.options.timeout.map(Duration::from_secs),
});
```

In `dispatch_finally` (already modified in Phase 3a), pass `None` for timeout (finally hooks don't have their own timeout config):
```rust
let executor = Box::new(ShellExecutor {
    script: script.to_string(),
    step_name: task.step.clone(),
    working_dir: self.pool.working_dir.clone(),
    timeout: None,
});
```

#### 4d. Add local timeout to `PoolExecutor`

Currently, `PoolExecutor::execute` calls `submit_via_cli`, which calls `invoker.invoke()`, which spawns a CLI process and blocks on its output. To enforce barnum-local timeout, we need to timeout on this CLI process.

Two approaches:
1. **Wrap `submit_via_cli`** in a child-process timeout (requires `submit_via_cli` to return a `Child` handle instead of blocking)
2. **Add timeout to the invoker API** (changes `cli_invoker` crate)

Approach 1 is less invasive. Modify `submit.rs` to expose a lower-level API:

```rust
/// Spawn the CLI invocation as a child process.
pub fn spawn_submit(
    root: &Path,
    payload: &str,
    invoker: &Invoker<TroupeCli>,
) -> io::Result<Child> {
    // Build the command but return the Child instead of waiting
    invoker.spawn(root, payload)
}

/// Wait for the CLI process with optional timeout.
pub fn wait_submit(
    child: &mut Child,
    timeout: Option<Duration>,
) -> io::Result<Response> {
    // Similar to run_shell_command's timeout logic
    match timeout {
        Some(duration) => match child.wait_timeout(duration) {
            Ok(Some(_status)) => {
                let output = child.wait_with_output()?;
                parse_response(output)
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                Ok(Response::NotProcessed { reason: "barnum timeout".into() })
            }
            Err(e) => Err(io::Error::other(format!("wait failed: {e}"))),
        },
        None => {
            let output = child.wait_with_output()?;
            parse_response(output)
        }
    }
}
```

Then `PoolExecutor` becomes:
```rust
pub struct PoolExecutor {
    pub root: PathBuf,
    pub invoker: Invoker<TroupeCli>,
    pub docs: String,
    pub step_name: StepName,
    pub pool_timeout: Option<u64>,
    pub local_timeout: Option<Duration>,  // ← barnum-enforced timeout
}

impl Executor for PoolExecutor {
    fn execute(&self, value: &serde_json::Value) -> Result<String, String> {
        let payload = build_agent_payload(
            &self.step_name, value, &self.docs, self.pool_timeout,
        );
        let mut child = spawn_submit(&self.root, &payload, &self.invoker)
            .map_err(|e| e.to_string())?;
        match wait_submit(&mut child, self.local_timeout) {
            Ok(Response::Processed { stdout, .. }) => Ok(stdout),
            Ok(Response::NotProcessed { .. }) => Err(TIMEOUT_SENTINEL.into()),
            Err(e) => Err(e.to_string()),
        }
    }
}
```

**Note:** `pool_timeout` (sent to troupe in the payload) and `local_timeout` (enforced by barnum on the CLI process) serve different purposes:
- `pool_timeout` tells the pool daemon how long the agent has to respond. The pool manages agent lifecycle.
- `local_timeout` is barnum's hard limit. If the CLI process doesn't return in time (e.g., pool is unresponsive), barnum kills it and moves on.

Both come from `step.options.timeout`. The `local_timeout` should be slightly longer than `pool_timeout` to give the pool a chance to respond gracefully before barnum kills the process. A simple heuristic: `local_timeout = pool_timeout + 10s` (or some fixed buffer).

**This requires changes to the `cli_invoker` crate** to expose a `spawn()` method that returns a `Child` instead of blocking. If this is too invasive, an alternative is to wrap the existing `submit_via_cli` call in a `thread::scope` with a join timeout — but this leaks the thread on timeout, which is acceptable for a v1.

#### 4e. Update all `run_shell_command` call sites

`run_shell_command` gained a `timeout` parameter. Update all existing callers:

- `run_pre_hook` in `hooks.rs`: pass `None` (pre-hook timeout is a Phase 5 concern)
- `run_post_hook` in `hooks.rs`: pass `None` (post-hook timeout is a Phase 5 concern)

**At this point:** Both Pool and Command actions have barnum-enforced timeout. The timeout clock starts when `executor.execute()` runs in the spawned worker thread — time spent waiting in `pending_dispatches` does not count. The sentinel pattern (`TIMEOUT_SENTINEL`) cleanly separates timeout from error. Tests pass.

---

### Phase 5: Hooks as separate concurrency slots (future)

**Goal:** Pre-hooks, post-hooks, and finally hooks each occupy a concurrency slot.

This is the most structurally invasive phase. Currently:
- Pre-hooks run inside the worker thread (same slot as the action)
- Post-hooks run on the main thread (no slot)
- Finally hooks have their own thread (already a slot)

**Changes:**
1. Expand `PendingDispatch` to include `PreHook`, `PostHook` phases
2. When a task is dispatched, if it has a pre-hook, first dispatch `PendingDispatch::PreHook`
3. On pre-hook completion, dispatch `PendingDispatch::Action`
4. On action completion, if there's a post-hook, dispatch `PendingDispatch::PostHook`
5. Each phase occupies one `in_flight` slot

This requires the engine to track which phase each task is in, which is a significant state machine change. Details TBD after Phases 1-4 are implemented.

## Implementation Order Summary

| Phase | What | Key deletions | Key additions |
|-------|------|---------------|---------------|
| 1 | Pool through trait | `dispatch_pool_task`, `PoolResult`, `SubmitResult::Pool`, `process_pool_response` | `Executor` trait, `PoolExecutor`, `dispatch_via_executor`, `TIMEOUT_SENTINEL`, `ActionOutcome`, `ActionResult`, `SubmitResult::Action` |
| 2 | Command through trait | `dispatch_command_task`, `CommandResult`, `SubmitResult::Command`, `process_command_response`, `run_command_action` | `ShellExecutor` |
| 3 | Finally through trait | `dispatch_finally_task`, `FinallyResult`, `SubmitResult::Finally`, `process_finally_response` | `WorkerKind` enum on `WorkerResult` |
| 4 | Barnum-enforced timeout | — | `wait-timeout` dep, `timeout` param on `run_shell_command`, `timeout` on `ShellExecutor`, `local_timeout` on `PoolExecutor`, `spawn_submit`/`wait_submit` in `submit.rs` |
| 5 | Hooks as concurrency slots | Pre-hook code inside `dispatch_via_executor` | `PendingDispatch::PreHook`, `PendingDispatch::PostHook`, phase tracking in engine |

Each phase compiles and passes tests before moving to the next.

## What doesn't change

- **Config types:** `ActionFile` and `Action` enums remain as-is. The closed enum is fine until PLUGGABLE_ACTION_KINDS introduces user-defined action kinds.
- **State logging:** Unchanged. The log captures task submission and completion regardless of action kind.
- **Pre-commit hooks (Phases 1-4):** Pre-hooks still run inside the worker thread, same as today. Phase 5 separates them.
