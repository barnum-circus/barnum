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

### The executor is just a function that runs in a thread

The trait is minimal:
```rust
pub trait Executor: Send {
    fn execute(&self, value: &serde_json::Value) -> Result<String, String>;
}
```

No timeout, no concurrency, no hooks. It takes a value, runs something, returns a string or an error. Everything else — timeout, pre-hooks, post-hooks, concurrency accounting, result routing — is handled by free functions external to the trait that take `Box<dyn Executor>`.

### Timeout wraps the executor from the outside

The executor doesn't know about timeout. A `run_with_timeout` function spawns the executor in a thread and waits on a channel with `recv_timeout`:

```rust
fn run_with_timeout(
    executor: Box<dyn Executor>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, String> {
    let value = value.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(executor.execute(&value));
    });
    match timeout {
        Some(d) => rx.recv_timeout(d).unwrap_or(Err("timed out".into())),
        None => rx.recv().unwrap_or(Err("executor panicked".into())),
    }
}
```

On timeout, the executor's thread keeps running (can't kill threads in Rust). This is acceptable: both executors spawn child processes that terminate on their own. The troupe pool eventually times out the agent, and shell commands finish or get reaped. The leak is bounded.

This means:
- No `timeout` field on any executor
- No changes to `run_shell_command` signature
- No changes to `cli_invoker` crate
- `PoolExecutor` still sends `pool_timeout` to troupe in the payload (an implementation detail for agent lifecycle management — eventually removable)

### Concurrency is managed by the engine, not the executor

The `in_flight` counter is managed by `Engine::flush_dispatches` (increments before spawning) and `Engine::process_worker_result` (decrements on completion). Both run on the main thread. Executors run in spawned threads and have no access to `Engine`.

### All failures retry the same way

Today, retry behavior depends on the error type: `retry_on_timeout`, `retry_on_invalid_response`, always-retry for submit errors. This is unnecessary complexity.

After Phase 0: **all failures retry up to `max_retries`.** No `retry_on_timeout`, no `retry_on_invalid_response`. Failed = retry. The failure reason is logged and recorded in the state log for observability, but doesn't affect retry logic.

### Hooks need state log entries

Pre-hooks and post-hooks currently run without being recorded in the NDJSON state log. They can execute, time out, and fail — but there's no record of it. They should be first-class work units in the state log, with their own `StateLogEntry` variants. This enables:
- Visibility into hook execution (did the pre-hook run? did it time out?)
- Resume correctness (on replay, know whether a hook already ran)
- Retry accounting (hooks can be retried independently of the action)

This is addressed in Phase 5.

## Proposed Changes — Phased Implementation

### Phase 0: Unify retry logic and result types

**Goal:** All failure types retry the same way. `SubmitResult` collapses to two variants. Lands independently before the Executor trait.

#### 0a. Remove `retry_on_timeout` and `retry_on_invalid_response` from `Options`

In `resolved.rs`, remove:
```rust
pub retry_on_timeout: bool,
pub retry_on_invalid_response: bool,
```

And their serde defaults, `default_true()`, and `Default` impl entries. `Options` becomes:
```rust
pub struct Options {
    pub timeout: Option<u64>,
    pub max_retries: u32,
}
```

Remove from `config.rs` (the file-level config type) as well. Update the JSON schema and Zod files (`cargo run -p barnum_config --bin build_barnum_schema`).

#### 0b. Simplify `process_retry`

In `response.rs`, `process_retry` currently branches on `FailureKind` to check `retry_on_timeout`/`retry_on_invalid_response`. Remove the branching:

```rust
pub fn process_retry(task: &Task, options: &Options, failure_kind: FailureKind) -> TaskOutcome {
    let mut retry_task = task.clone();
    retry_task.retries += 1;

    if retry_task.retries <= options.max_retries {
        info!(
            step = %task.step,
            retry = retry_task.retries,
            max = options.max_retries,
            failure = ?failure_kind,
            "requeuing task"
        );
        TaskOutcome::Retry(retry_task, failure_kind)
    } else {
        error!(step = %task.step, retries = retry_task.retries, "max retries exceeded");
        TaskOutcome::Dropped(failure_kind)
    }
}
```

`FailureKind` stays as a parameter for logging/state-log purposes. It just doesn't affect retry logic anymore.

#### 0c. Unify `SubmitResult` to two variants

Replace:
```rust
pub(super) enum SubmitResult {
    Pool(PoolResult),
    Command(CommandResult),
    Finally(FinallyResult),
    PreHookError(String),
}
```

With:
```rust
pub(super) struct ActionResult {
    pub value: StepInputValue,
    pub output: Result<String, String>,
}

pub(super) enum SubmitResult {
    Action(ActionResult),
    PreHookError(String),
}
```

Delete `PoolResult`, `CommandResult`, `FinallyResult`.

#### 0d. Convert results in dispatch functions

Each dispatch function converts its native result type to `Result<String, String>` before sending:

**`dispatch_pool_task`**: Convert `io::Result<Response>` → `Result<String, String>`:
```rust
let output = match submit_via_cli(&pool.root, &payload, &pool.invoker) {
    Ok(Response::Processed { stdout, .. }) => Ok(stdout),
    Ok(Response::NotProcessed { .. }) => Err("not processed by pool".into()),
    Err(e) => Err(e.to_string()),
};
let _ = tx.send(WorkerResult {
    task_id,
    task,
    result: SubmitResult::Action(ActionResult { value, output }),
});
```

**`dispatch_command_task`**: Convert `io::Result<String>` → `Result<String, String>`:
```rust
let output = run_command_action(script, &task_json, working_dir)
    .map_err(|e| e.to_string());
let _ = tx.send(WorkerResult {
    task_id,
    task,
    result: SubmitResult::Action(ActionResult { value, output }),
});
```

**`dispatch_finally_task`**: Already `Result<String, String>`, just wrap:
```rust
let _ = tx.send(WorkerResult {
    task_id,
    task,
    result: SubmitResult::Action(ActionResult { value, output }),
});
```

#### 0e. Add `WorkerKind` to `WorkerResult`

`process_worker_result` in `mod.rs` currently routes `SubmitResult::Finally` to `convert_finally_result`. Since `SubmitResult::Finally` is gone, add a `kind` field:

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

`dispatch_pool_task` and `dispatch_command_task` send `WorkerKind::Task`. `dispatch_finally_task` sends `WorkerKind::Finally { parent_id }` (it already receives `parent_id` as a parameter — the `task_id` parameter is currently the parent_id).

Update `process_worker_result` to dispatch on `kind`:
```rust
fn process_worker_result(&mut self, result: WorkerResult) -> Vec<StateLogEntry> {
    self.in_flight = self.in_flight.saturating_sub(1);

    let entries = match result.kind {
        WorkerKind::Task => self.convert_task_result(result.task_id, &result.task, result.result),
        WorkerKind::Finally { parent_id } => {
            let output = match result.result {
                SubmitResult::Action(ActionResult { value, output }) => output,
                SubmitResult::PreHookError(e) => Err(e),
            };
            self.convert_finally_result(parent_id, result.task.value.clone(), output)
        }
    };

    for entry in &entries {
        self.state.apply_entry(entry, self.config);
    }
    self.flush_dispatches();
    entries
}
```

#### 0f. Collapse `process_submit_result`

With one `SubmitResult::Action` variant, `process_submit_result` becomes:
```rust
pub fn process_submit_result(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
) -> ProcessedSubmit {
    match result {
        SubmitResult::Action(ActionResult { value, output }) => match output {
            Ok(stdout) => {
                let (outcome, post_input) = process_stdout(&stdout, task, &value, step, schemas);
                ProcessedSubmit { outcome, post_input }
            }
            Err(error) => {
                error!(step = %task.step, %error, "action failed");
                let outcome = process_retry(task, &step.options, FailureKind::SubmitError);
                ProcessedSubmit {
                    outcome,
                    post_input: PostHookInput::Error(PostHookError { input: value, error }),
                }
            }
        },
        SubmitResult::PreHookError(e) => {
            error!(step = %task.step, error = %e, "pre hook failed");
            let outcome = process_retry(task, &step.options, FailureKind::SubmitError);
            ProcessedSubmit {
                outcome,
                post_input: PostHookInput::PreHookError(PostHookPreHookError {
                    input: task.value.clone(),
                    error: e,
                }),
            }
        }
    }
}
```

Delete `process_pool_response`, `process_command_response`, `process_finally_response`. Remove `use troupe::Response` from `response.rs`.

#### 0g. Delete dead code

- `PoolResult`, `CommandResult`, `FinallyResult` structs
- `process_pool_response`, `process_command_response`, `process_finally_response` functions
- `run_command_action` from `hooks.rs` (thin wrapper, inline the `run_shell_command` call in `dispatch_command_task`)
- `retry_on_timeout`, `retry_on_invalid_response` from config types and schema

**At this point:** All dispatch functions produce `SubmitResult::Action(ActionResult)`. One code path in `process_submit_result`. Retry logic is uniform. `WorkerKind` distinguishes task from finally. Tests pass. Compile, run full suite, regenerate schemas.

---

### Phase 1: Executor trait + PoolExecutor

**Goal:** Pool actions go through the trait. Everything else stays as-is. Tests pass.

**Prerequisite:** Phase 0 (unified result types) is complete. `SubmitResult` already has `Action(ActionResult)` and `PreHookError(String)`. `WorkerKind` already exists on `WorkerResult`.

#### 1a. Create `runner/executor.rs`

```rust
/// Trait for executing work units.
///
/// Every action kind implements this. The runner dispatches all work through
/// this interface, giving every unit the same dispatch path.
///
/// Input is the task's value (as a serde_json::Value). Each executor knows
/// how to package it for its execution context.
///
/// Working directory and other configuration are captured at construction time.
pub trait Executor: Send {
    fn execute(&self, value: &serde_json::Value) -> Result<String, String>;
}
```

#### 1b. Create `PoolExecutor` in `runner/executor.rs`

Move the `Response` → `Result<String, String>` conversion from `dispatch_pool_task` (Phase 0d) into the executor:

```rust
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
            Ok(Response::NotProcessed { .. }) => Err("not processed by pool".into()),
            Err(e) => Err(e.to_string()),
        }
    }
}
```

#### 1c. Add `dispatch_via_executor` in `runner/dispatch.rs`

Replaces `dispatch_pool_task`:

```rust
/// Execute a task through an Executor (runs in spawned thread).
///
/// Runs pre-hook, calls executor.execute(), sends the result on the channel.
pub fn dispatch_via_executor(
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
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
                kind,
                result: SubmitResult::PreHookError(e),
            });
            return;
        }
    };

    let output = executor.execute(&value.0);
    let _ = tx.send(WorkerResult {
        task_id,
        task,
        kind,
        result: SubmitResult::Action(ActionResult { value, output }),
    });
}
```

Note: `dispatch_via_executor` takes `WorkerKind` as a parameter so the same function works for both tasks and finally hooks (Phase 3).

#### 1d. Modify `dispatch_task` Pool branch

In `runner/mod.rs`, change the `Action::Pool` arm to construct a `PoolExecutor` and call `dispatch_via_executor`:

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
            task_id, task, WorkerKind::Task,
            pre_hook.as_ref(), executor, &working_dir, &tx,
        );
    });
}
```

#### 1e. Delete `dispatch_pool_task`

It's now unused. Remove from `dispatch.rs`.

**At this point:** Pool goes through `Executor` trait → `dispatch_via_executor`. Command still uses `dispatch_command_task`. Tests pass. Compile, run full suite.

---

### Phase 2: ShellExecutor + Command through trait

**Goal:** Command actions also go through the trait. Both branches of `dispatch_task` construct a `Box<dyn Executor>` and call `dispatch_via_executor`.

#### 2a. Add `ShellExecutor` to `runner/executor.rs`

```rust
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
            task_id, task, WorkerKind::Task,
            pre_hook.as_ref(), executor, &working_dir, &tx,
        );
    });
}
```

#### 2c. Delete `dispatch_command_task`

Remove from `dispatch.rs`.

#### 2d. Observe: `dispatch_task` no longer cares about action kind for dispatch

Both branches now do the same thing: construct a `Box<dyn Executor>`, call `dispatch_via_executor`. The match is only for choosing which executor to construct. This is the seam for PLUGGABLE_ACTION_KINDS.

**At this point:** Pool and Command both go through `Executor` trait → `dispatch_via_executor`. Tests pass. Compile, run full suite.

---

### Phase 3: Finally hooks through trait

**Goal:** Finally hooks also go through `dispatch_via_executor` with a `ShellExecutor`. `WorkerKind` (already added in Phase 0e) routes the result to `convert_finally_result`.

#### 3a. Modify `dispatch_finally` in `runner/mod.rs`

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
        dispatch_via_executor(
            parent_id, task, WorkerKind::Finally { parent_id },
            None, executor, &working_dir, &tx,
        );
    });
}
```

No pre-hook for finally (passed as `None`).

#### 3b. Delete `dispatch_finally_task`

Remove from `dispatch.rs`.

**At this point:** All executable units go through `dispatch_via_executor`. Three dispatch functions replaced by one. Tests pass.

---

### Phase 4: Timeout enforcement

**Goal:** Barnum enforces timeout for all executor types. The timeout wraps the executor from the outside.

**No new dependencies needed.** Timeout uses `mpsc::recv_timeout` on the standard library channel.

#### 4a. Add `run_with_timeout` to `runner/dispatch.rs`

```rust
use std::time::Duration;

/// Run an executor with optional timeout.
///
/// Spawns a thread for the executor. If timeout expires, returns Err
/// without waiting for the thread (which will finish on its own).
fn run_with_timeout(
    executor: Box<dyn Executor>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, String> {
    let value = value.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(executor.execute(&value));
    });
    match timeout {
        Some(d) => rx.recv_timeout(d).unwrap_or(Err("timed out".into())),
        None => rx.recv().unwrap_or(Err("executor panicked".into())),
    }
}
```

On timeout, the executor's thread keeps running. Both executor types spawn child processes that terminate on their own (shell commands finish, troupe pool eventually times out the agent). The thread leak is bounded and transient.

#### 4b. Modify `dispatch_via_executor` to use `run_with_timeout`

Add a `timeout` parameter:

```rust
pub fn dispatch_via_executor(
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    pre_hook: Option<&HookScript>,
    executor: Box<dyn Executor>,
    timeout: Option<Duration>,
    working_dir: &Path,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = match run_pre_hook_or_error(pre_hook, &task.value, working_dir) {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(WorkerResult {
                task_id, task, kind,
                result: SubmitResult::PreHookError(e),
            });
            return;
        }
    };

    let output = run_with_timeout(executor, &value.0, timeout);
    let _ = tx.send(WorkerResult {
        task_id, task, kind,
        result: SubmitResult::Action(ActionResult { value, output }),
    });
}
```

#### 4c. Pass `step.options.timeout` from dispatch sites

In `dispatch_task` (both Pool and Command branches), pass the timeout:
```rust
let timeout = step.options.timeout.map(Duration::from_secs);
// ...
dispatch_via_executor(
    task_id, task, WorkerKind::Task,
    pre_hook.as_ref(), executor, timeout, &working_dir, &tx,
);
```

In `dispatch_finally`, pass `None` (finally hooks have no timeout config):
```rust
dispatch_via_executor(
    parent_id, task, WorkerKind::Finally { parent_id },
    None, executor, None, &working_dir, &tx,
);
```

**No changes to executors, `run_shell_command`, `cli_invoker`, or `submit.rs`.** The timeout is entirely external.

**At this point:** Both Pool and Command actions have barnum-enforced timeout. The timeout clock starts when `run_with_timeout` is called in the dispatch thread — time in `pending_dispatches` doesn't count. All errors retry the same way. Tests pass.

---

### Phase 5: Hooks as first-class work units

**Goal:** Pre-hooks and post-hooks are logged in the state log, occupy concurrency slots, have timeout, and can be retried.

Currently:
- Pre-hooks run inside the dispatch thread (same slot as the action, no state log entry)
- Post-hooks run on the main thread (no slot, no state log entry)
- Neither has timeout. Neither is recorded.

**Changes:**

#### 5a. Add `StateLogEntry` variants for hooks

New entries in `barnum_state`:
```rust
StateLogEntry::PreHookStarted { task_id, script }
StateLogEntry::PreHookCompleted { task_id, outcome: Result<Value, String> }
StateLogEntry::PostHookStarted { task_id, script }
StateLogEntry::PostHookCompleted { task_id, outcome: Result<PostHookInput, String> }
```

These enable resume correctness (on replay, know whether a hook already ran) and observability (did the pre-hook run? did it time out?).

#### 5b. Expand `PendingDispatch` to include hook phases

```rust
enum PendingDispatch {
    PreHook { task_id: LogTaskId },
    Action { task_id: LogTaskId },
    PostHook { task_id: LogTaskId },
    Finally { parent_id: LogTaskId },
}
```

When a task is dispatched:
1. If it has a pre-hook → dispatch `PendingDispatch::PreHook`
2. On pre-hook completion → dispatch `PendingDispatch::Action`
3. On action completion, if there's a post-hook → dispatch `PendingDispatch::PostHook`
4. Each phase occupies one `in_flight` slot

#### 5c. Hooks go through the executor trait

Pre-hooks and post-hooks become executors (e.g., `PreHookExecutor`, `PostHookExecutor`) that go through `dispatch_via_executor` → `run_with_timeout`. They get the same timeout and concurrency treatment as actions.

This requires the engine to track which phase each task is in — a significant state machine change. Details TBD after Phases 0-4 are implemented and stable.

## Implementation Order Summary

| Phase | What | Key deletions | Key additions |
|-------|------|---------------|---------------|
| 0 | Unify results + retry | `PoolResult`, `CommandResult`, `FinallyResult`, `SubmitResult::Pool/Command/Finally`, `process_pool_response`, `process_command_response`, `process_finally_response`, `run_command_action`, `retry_on_timeout`, `retry_on_invalid_response` | `ActionResult`, `WorkerKind` |
| 1 | Pool through trait | `dispatch_pool_task` | `Executor` trait, `PoolExecutor`, `dispatch_via_executor` |
| 2 | Command through trait | `dispatch_command_task` | `ShellExecutor` |
| 3 | Finally through trait | `dispatch_finally_task` | — (reuses `ShellExecutor` + `WorkerKind::Finally`) |
| 4 | Timeout | — | `run_with_timeout`, `timeout` param on `dispatch_via_executor` |
| 5 | Hooks as work units | Pre-hook code inside dispatch thread, post-hook on main thread | `StateLogEntry::PreHook*`, `StateLogEntry::PostHook*`, `PendingDispatch::PreHook/Action/PostHook`, phase tracking in engine |

Each phase compiles and passes tests before moving to the next.

## What doesn't change

- **Config types:** `ActionFile` and `Action` enums remain as-is. The closed enum is fine until PLUGGABLE_ACTION_KINDS introduces user-defined action kinds.
- **State logging (Phases 0-4):** Task submission and completion logging unchanged. Phase 5 adds hook entries.
- **Pre/post hooks (Phases 0-4):** Still run inside the dispatch thread (pre) and on the main thread (post). Phase 5 separates them into their own concurrency slots.
