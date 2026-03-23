# Unified Action Dispatch

Prerequisite to PLUGGABLE_ACTION_KINDS (future).

## Motivation

Pool actions, Command actions, and finally hooks each have their own dispatch function, their own result type, and their own response-processing path. Timeouts only exist for Pool (via troupe). Commands block forever. Finally hooks ignore concurrency limits.

All of these run work in a thread and send a result back on a channel. They should go through one dispatch path with one trait. If it runs, it goes through the trait. Concurrency and result routing wrap the trait from the outside.

## Current State

### `dispatch_task` (`runner/mod.rs`)

Pattern-matches on `ActionKind` and calls different functions:

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let tx = self.tx.clone();
    match &step.action {
        ActionKind::Pool(..) => {
            // constructs docs, timeout, pool — calls dispatch_pool_task in a thread
        }
        ActionKind::Command(CommandAction { script }) => {
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

### Naming: `Action` trait, config enum is `ActionKind`

The config enum was already renamed from `Action` to `ActionKind` on master. The `Action` name belongs to the trait.

### `Action` trait

```rust
pub trait Action: Send {
    fn perform(&self, value: &serde_json::Value) -> Result<String, String>;
}
```

Takes a value, runs something, returns stdout or an error string.

### `run_action` handles timeout

`run_action` takes a boxed action, a value, and an optional timeout. Without a timeout, it calls `perform` directly. With a timeout, it spawns an inner thread for `perform` and uses `recv_timeout`:

```rust
fn run_action(
    action: Box<dyn Action>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, String> {
    match timeout {
        None => action.perform(value),
        Some(duration) => {
            let (tx, rx) = mpsc::channel();
            let value = value.clone();
            thread::spawn(move || {
                let _ = tx.send(action.perform(&value));
            });
            rx.recv_timeout(duration)
                .unwrap_or_else(|_| Err("action timed out".into()))
        }
    }
}
```

On timeout, the inner thread keeps running (can't kill threads in Rust). Both action types spawn child processes that terminate independently: troupe eventually times out the agent, and shell commands finish or get reaped. The leak is bounded.

### `spawn_worker` is the single dispatch path

Every dispatch site constructs an action and calls `spawn_worker`. The thread spawn, `run_action` call, and `WorkerResult` send live in one place:

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

Each dispatch site reduces to: construct the action, pick the `WorkerKind`, call `spawn_worker`. The `WorkerResult` is sent exactly once — either with the action's output or with a timeout error. A timed-out inner thread may eventually complete, but it only writes to the inner channel that nobody is listening to anymore.

### Concurrency stays in the engine

`in_flight` is managed by `Engine::flush_dispatches` (increment before spawn) and `Engine::process_worker_result` (decrement on completion). Both run on the main thread. Actions run in spawned threads with no access to `Engine`.

## Phased Implementation

### Phase 0: Unify result types

Collapse `SubmitResult`'s three payload types into one `ActionResult`. Retry logic (`retry_on_timeout`, `retry_on_invalid_response`, `process_retry`, `FailureKind`) is unchanged — timeout vs invalid response are different failure classes that warrant separate retry controls.

#### 0a. Replace `SubmitResult` with `ActionResult` + `WorkerKind`

`SubmitResult`'s three variants serve two purposes: carrying different payload types (Pool/Command/Finally each have different result types) and routing (`Finally` goes to `convert_finally_result`, everything else to `convert_task_result`). These are separate concerns. Split them:

```rust
pub(super) struct ActionResult {
    pub value: StepInputValue,
    pub output: Result<String, String>,
}

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

Delete `SubmitResult`, `PoolResult`, `CommandResult`, `FinallyResult`.

#### 0b. Convert results in dispatch functions

Each dispatch function normalizes its native result type to `Result<String, String>` before wrapping in `ActionResult`.

`dispatch_pool_task` converts `io::Result<Response>`:
```rust
let output = match submit_via_cli(&pool.root, &payload, &pool.invoker) {
    Ok(Response::Processed { stdout, .. }) => Ok(stdout),
    Ok(Response::NotProcessed { .. }) => Err("not processed by pool".into()),
    Err(e) => Err(e.to_string()),
};
let _ = tx.send(WorkerResult {
    task_id, task,
    kind: WorkerKind::Task,
    result: ActionResult { value, output },
});
```

`dispatch_command_task` converts `io::Result<String>`:
```rust
let output = run_command_action(script, &task_json, working_dir)
    .map_err(|e| e.to_string());
let _ = tx.send(WorkerResult {
    task_id, task,
    kind: WorkerKind::Task,
    result: ActionResult { value, output },
});
```

`dispatch_finally_task` already returns `Result<String, String>`:
```rust
let _ = tx.send(WorkerResult {
    task_id, task,
    kind: WorkerKind::Finally { parent_id },
    result: ActionResult { value, output },
});
```

#### 0c. Collapse `process_submit_result`

`convert_task_result` currently takes `SubmitResult` and passes it to `process_submit_result`. With `ActionResult`, both simplify:

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

Delete `process_pool_response`, `process_command_response`. Update `process_worker_result`:

```rust
let entries = match result.kind {
    WorkerKind::Task => self.convert_task_result(result.task_id, &result.task, result.result),
    WorkerKind::Finally { parent_id } => {
        self.convert_finally_result(parent_id, result.task.value.clone(), result.result.output)
    }
};
```

#### 0d. Delete dead code

- `SubmitResult` enum
- `PoolResult`, `CommandResult`, `FinallyResult` structs
- `process_pool_response`, `process_command_response` functions
- `run_command_action` from `hooks.rs` (thin wrapper; inline the `run_shell_command` call in `dispatch_command_task`)

After this phase: all dispatch functions produce `ActionResult`, `WorkerKind` handles routing, one code path in `process_submit_result`. Compile, run full suite.

---

### Phase 1: `Action` trait + `run_action` + `spawn_worker` + `PoolAction`

Introduce the trait, the dispatch infrastructure, and move Pool through it. Command stays as-is.

Requires Phase 0 (unified result types). Config enum rename (`Action` → `ActionKind`) already landed on master.

#### 1a. Create `runner/action.rs`

Define the `Action` trait, `run_action`, and `spawn_worker` as described in the Design Decisions section above.

#### 1b. `PoolAction`

Move the `Response` to `Result<String, String>` conversion from `dispatch_pool_task` (Phase 0b) into the action:

```rust
pub struct PoolAction {
    pub root: PathBuf,
    pub invoker: Invoker<TroupeCli>,
    pub docs: String,
    pub step_name: StepName,
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

`pool_timeout` is troupe's agent lifecycle timeout, passed through in the payload. Existing behavior moved into the struct.

#### 1c. Update `dispatch_task` Pool branch

```rust
// Before the match (shared by both branches):
let timeout = step.options.timeout.map(Duration::from_secs);
let tx = self.tx.clone();

// Pool branch:
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

#### 1d. Delete `dispatch_pool_task`

Compile, run full suite.

---

### Phase 2: `ShellAction` + Command through trait

#### 2a. `ShellAction`

```rust
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

#### 2b. Update `dispatch_task` Command branch

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

After this, `dispatch_task` has one shape: construct the action, call `spawn_worker`. The match only picks which action to construct:

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

### Phase 3: Finally hooks through trait

Finally hooks use the same pattern: construct a `ShellAction`, call `spawn_worker` with `WorkerKind::Finally`.

#### 3a. Update `dispatch_finally`

```rust
fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("...");
    let script = step.finally_hook.clone().expect("...");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();

    let action = Box::new(ShellAction {
        script: script.to_string(),
        step_name: task.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });
    spawn_worker(tx, action, parent_id, task, WorkerKind::Finally { parent_id }, timeout);
}
```

#### 3b. Delete `dispatch_finally_task`

Three dispatch functions gone. All dispatch sites call `spawn_worker`. Compile, run full suite.

## Summary

| Phase | What | Deletes | Adds |
|-------|------|---------|------|
| 0 | Unify result types | `SubmitResult`, `PoolResult`, `CommandResult`, `FinallyResult`, `process_pool_response`, `process_command_response`, `run_command_action` | `ActionResult`, `WorkerKind` |
| 1 | Trait + dispatch infra + Pool | `dispatch_pool_task` | `Action` trait, `run_action`, `spawn_worker`, `PoolAction` |
| 2 | Command through trait | `dispatch_command_task` | `ShellAction` |
| 3 | Finally through trait | `dispatch_finally_task` | (reuses `ShellAction` + `WorkerKind::Finally`) |

Each phase compiles and passes tests before moving to the next.

## Behavior changes

`ActionKind` enum variants stay as-is (already renamed from `Action`). State logging and retry logic are unchanged.

Timeout behavior changes: `run_action` enforces `step.options.timeout` for all action kinds via `recv_timeout`. Commands and finally hooks, which previously blocked forever, now timeout. Pool actions get Barnum-level timeout enforcement in addition to troupe's existing agent lifecycle timeout (both derive from the same `step.options.timeout` value).
