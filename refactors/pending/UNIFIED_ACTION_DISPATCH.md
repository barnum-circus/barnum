# Unified Action Dispatch

Prerequisite to PLUGGABLE_ACTION_KINDS (future).

## Motivation

Pool actions, Command actions, and finally hooks each have their own dispatch function, their own result type, and their own response-processing path. Timeouts only exist for Pool (via troupe). Commands block forever. Finally hooks ignore concurrency limits.

All of these run work in a thread and send a result back on a channel. They should go through one dispatch path with one trait. If it runs, it goes through the trait. Timeout, concurrency, and result routing wrap the trait from the outside.

## Current State

### `dispatch_task` (`runner/mod.rs`)

Pattern-matches on `Action` and calls different functions:

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

### `dispatch_pool_task` (`runner/dispatch.rs:56-74`)

Spawned thread. Calls `build_agent_payload` + `submit_via_cli`. Sends `SubmitResult::Pool(PoolResult { value, response: io::Result<Response> })`.

### `dispatch_command_task` (`runner/dispatch.rs:80-100`)

Spawned thread. Calls `run_command_action` (which calls `run_shell_command`). Sends `SubmitResult::Command(CommandResult { value, output: io::Result<String> })`.

### `dispatch_finally_task` (`runner/dispatch.rs:106-122`)

Spawned thread. Calls `run_shell_command` directly. Sends `SubmitResult::Finally(FinallyResult { value, output: Result<String, String> })`.

### `SubmitResult` (`runner/dispatch.rs:46-50`)

```rust
pub(super) enum SubmitResult {
    Pool(PoolResult),
    Command(CommandResult),
    Finally(FinallyResult),
}
```

Three variants, three different result types: `io::Result<Response>` for Pool, `io::Result<String>` for Command, `Result<String, String>` for Finally.

### `process_submit_result` (`runner/response.rs:42-71`)

Separate match arms for Pool, Command, Finally. Pool and Command both converge on `process_stdout` for success but take different paths there:
- Pool: unwraps `Response::Processed { stdout }` into `process_stdout`; `Response::NotProcessed` becomes `FailureKind::Timeout`
- Command: unwraps `io::Result<String>` into `process_stdout`; no timeout variant

### `run_shell_command` (`runner/shell.rs`)

Blocks forever on `child.wait_with_output()`. No timeout.

## Design Decisions

### Executor trait

Minimal:
```rust
pub trait Executor: Send {
    fn execute(&self, value: &serde_json::Value) -> Result<String, String>;
}
```

Takes a value, runs something, returns stdout or an error string. The executor knows nothing about timeout or concurrency. Those are handled by free functions that take `Box<dyn Executor>`.

### External timeout

The executor doesn't know about timeout. `run_with_timeout` spawns the executor in a thread and waits on a channel:

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

On timeout, the executor's thread keeps running (can't kill threads in Rust). Both executors spawn child processes that terminate on their own: the troupe pool eventually times out the agent, and shell commands finish or get reaped. The leak is bounded.

Consequences:
- No `timeout` field on any executor
- No changes to `run_shell_command` or `cli_invoker`
- `PoolExecutor` still sends `pool_timeout` to troupe in the payload (implementation detail for agent lifecycle, eventually removable)

### Concurrency stays in the engine

`in_flight` is managed by `Engine::flush_dispatches` (increment before spawn) and `Engine::process_worker_result` (decrement on completion). Both run on the main thread. Executors run in spawned threads with no access to `Engine`.

### Uniform retry

Today retry behavior branches on error type: `retry_on_timeout`, `retry_on_invalid_response`, always-retry for submit errors. After Phase 0, all failures retry up to `max_retries`. The failure reason is logged and recorded in the state log, but doesn't gate retry decisions.

## Phased Implementation

### Phase 0: Unify retry logic and result types

Collapse `SubmitResult`'s three variants into one struct. Make all failures retry the same way. Lands independently before the Executor trait.

#### 0a. Remove `retry_on_timeout` and `retry_on_invalid_response` from `Options`

In `resolved.rs`, remove both fields, their serde defaults, `default_true()`, and their `Default` impl entries. `Options` becomes:
```rust
pub struct Options {
    pub timeout: Option<u64>,
    pub max_retries: u32,
}
```

Same removal in `config.rs`. Regenerate schema/zod files (`cargo run -p barnum_cli --bin build_schemas`).

#### 0b. Simplify `process_retry`

Remove the `FailureKind`-based branching that checks `retry_on_timeout`/`retry_on_invalid_response`:

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

`FailureKind` stays as a parameter for logging. It just doesn't affect retry logic anymore.

#### 0c. Collapse `SubmitResult` to a struct

Replace the enum:
```rust
pub(super) enum SubmitResult {
    Pool(PoolResult),
    Command(CommandResult),
    Finally(FinallyResult),
}
```

With:
```rust
pub(super) struct ActionResult {
    pub value: StepInputValue,
    pub output: Result<String, String>,
}
```

Delete `PoolResult`, `CommandResult`, `FinallyResult`.

#### 0d. Convert results in dispatch functions

Each dispatch function normalizes its native result type to `Result<String, String>` before sending.

`dispatch_pool_task` converts `io::Result<Response>`:
```rust
let output = match submit_via_cli(&pool.root, &payload, &pool.invoker) {
    Ok(Response::Processed { stdout, .. }) => Ok(stdout),
    Ok(Response::NotProcessed { .. }) => Err("not processed by pool".into()),
    Err(e) => Err(e.to_string()),
};
let _ = tx.send(WorkerResult {
    task_id,
    task,
    result: ActionResult { value, output },
});
```

`dispatch_command_task` converts `io::Result<String>`:
```rust
let output = run_command_action(script, &task_json, working_dir)
    .map_err(|e| e.to_string());
let _ = tx.send(WorkerResult {
    task_id,
    task,
    result: ActionResult { value, output },
});
```

`dispatch_finally_task` already returns `Result<String, String>`, so it just wraps directly.

#### 0e. Add `WorkerKind` to `WorkerResult`

`process_worker_result` currently routes `SubmitResult::Finally` to `convert_finally_result`. With the `Finally` variant gone, replace that with a `kind` field:

```rust
pub enum WorkerKind {
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

Pool and Command send `WorkerKind::Task`. Finally sends `WorkerKind::Finally { parent_id }` (it already receives `parent_id` as a parameter).

`process_worker_result` dispatches on `kind`:
```rust
fn process_worker_result(&mut self, result: WorkerResult) -> Vec<StateLogEntry> {
    self.in_flight = self.in_flight.saturating_sub(1);

    let entries = match result.kind {
        WorkerKind::Task => self.convert_task_result(result.task_id, &result.task, result.result),
        WorkerKind::Finally { parent_id } => {
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

#### 0f. Collapse `process_submit_result`

With `ActionResult`, this becomes:
```rust
pub fn process_submit_result(
    result: ActionResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
) -> TaskOutcome {
    match result.output {
        Ok(stdout) => process_stdout(&stdout, task, &result.value, step, schemas),
        Err(error) => {
            error!(step = %task.step, %error, "action failed");
            process_retry(task, &step.options, FailureKind::SubmitError)
        }
    }
}
```

Delete `process_pool_response` and `process_command_response`.

#### 0g. Delete dead code

- `PoolResult`, `CommandResult`, `FinallyResult` structs
- `process_pool_response`, `process_command_response` functions
- `run_command_action` from `hooks.rs` (thin wrapper; inline the `run_shell_command` call in `dispatch_command_task`)
- `retry_on_timeout`, `retry_on_invalid_response` from config types and schema

After this phase: all dispatch functions produce `ActionResult`, one code path in `process_submit_result`, uniform retry logic, `WorkerKind` distinguishes task from finally. Compile, run full suite, regenerate schemas.

---

### Phase 1: Executor trait + PoolExecutor

Introduce the trait. Move Pool dispatch through it. Command stays as-is.

Requires Phase 0 (unified result types).

#### 1a. Create `runner/executor.rs`

```rust
pub trait Executor: Send {
    fn execute(&self, value: &serde_json::Value) -> Result<String, String>;
}
```

#### 1b. `PoolExecutor`

Move the `Response` to `Result<String, String>` conversion from `dispatch_pool_task` (Phase 0d) into the executor:

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

#### 1c. `dispatch_via_executor`

Replaces `dispatch_pool_task`. Takes `WorkerKind` so it works for both tasks and finally hooks (Phase 3).

```rust
pub fn dispatch_via_executor(
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    executor: Box<dyn Executor>,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = task.value.clone();
    let output = executor.execute(&value.0);
    let _ = tx.send(WorkerResult {
        task_id,
        task,
        kind,
        result: ActionResult { value, output },
    });
}
```

#### 1d. Update `dispatch_task` Pool branch

```rust
Action::Pool(..) => {
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
            task_id, task, WorkerKind::Task, executor, &tx,
        );
    });
}
```

#### 1e. Delete `dispatch_pool_task`

Compile, run full suite.

---

### Phase 2: ShellExecutor + Command through trait

Both branches of `dispatch_task` now construct a `Box<dyn Executor>` and call `dispatch_via_executor`.

#### 2a. `ShellExecutor`

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

#### 2b. Update `dispatch_task` Command branch

```rust
Action::Command(CommandAction { script }) => {
    let working_dir = self.pool.working_dir.clone();
    let executor = Box::new(ShellExecutor {
        script: script.clone(),
        step_name: task.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });

    info!(step = %task.step, script = %script, "executing command");
    thread::spawn(move || {
        dispatch_via_executor(
            task_id, task, WorkerKind::Task, executor, &tx,
        );
    });
}
```

#### 2c. Delete `dispatch_command_task`

After this, both branches of `dispatch_task` do the same thing: construct an executor, call `dispatch_via_executor`. The match only picks which executor. This is the seam for PLUGGABLE_ACTION_KINDS.

Compile, run full suite.

---

### Phase 3: Finally hooks through trait

Finally hooks also go through `dispatch_via_executor` with a `ShellExecutor`. `WorkerKind` (added in Phase 0e) routes the result to `convert_finally_result`.

#### 3a. Update `dispatch_finally`

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
            executor, &tx,
        );
    });
}
```

#### 3b. Delete `dispatch_finally_task`

Three dispatch functions replaced by one. Compile, run full suite.

---

### Phase 4: Timeout enforcement

Barnum-enforced timeout for all executor types, using `mpsc::recv_timeout` (stdlib, no new deps).

#### 4a. `run_with_timeout`

```rust
use std::time::Duration;

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

On timeout, the executor's thread keeps running. Both executor types spawn child processes that terminate on their own (shell commands finish, troupe pool times out the agent). Bounded, transient leak.

#### 4b. Add `timeout` param to `dispatch_via_executor`

```rust
pub fn dispatch_via_executor(
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    executor: Box<dyn Executor>,
    timeout: Option<Duration>,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = task.value.clone();
    let output = run_with_timeout(executor, &value.0, timeout);
    let _ = tx.send(WorkerResult {
        task_id, task, kind,
        result: ActionResult { value, output },
    });
}
```

#### 4c. Pass timeout from dispatch sites

In `dispatch_task` (both branches):
```rust
let timeout = step.options.timeout.map(Duration::from_secs);
dispatch_via_executor(
    task_id, task, WorkerKind::Task, executor, timeout, &tx,
);
```

In `dispatch_finally`, pass `None` (finally hooks have no timeout config):
```rust
dispatch_via_executor(
    parent_id, task, WorkerKind::Finally { parent_id },
    executor, None, &tx,
);
```

No changes to executors, `run_shell_command`, `cli_invoker`, or `submit.rs`. The timeout is entirely external. The clock starts when `run_with_timeout` is called in the dispatch thread; time spent in `pending_dispatches` doesn't count.

Compile, run full suite.

## Summary

| Phase | What | Deletes | Adds |
|-------|------|---------|------|
| 0 | Unify results + retry | `PoolResult`, `CommandResult`, `FinallyResult`, `SubmitResult` enum, `process_pool_response`, `process_command_response`, `run_command_action`, `retry_on_timeout`, `retry_on_invalid_response` | `ActionResult`, `WorkerKind` |
| 1 | Pool through trait | `dispatch_pool_task` | `Executor` trait, `PoolExecutor`, `dispatch_via_executor` |
| 2 | Command through trait | `dispatch_command_task` | `ShellExecutor` |
| 3 | Finally through trait | `dispatch_finally_task` | (reuses `ShellExecutor` + `WorkerKind::Finally`) |
| 4 | Timeout | | `run_with_timeout`, `timeout` param on `dispatch_via_executor` |

Each phase compiles and passes tests before moving to the next.

## What doesn't change

`ActionFile` and `Action` enums stay as-is. The closed enum is fine until PLUGGABLE_ACTION_KINDS. State logging (task submission and completion) is unchanged.
