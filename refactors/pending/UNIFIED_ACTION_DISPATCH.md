# Unified Action Dispatch

Prerequisite to PLUGGABLE_ACTION_KINDS (future).

## Motivation

Pool actions, Command actions, and finally hooks each have their own dispatch function, their own result type, and their own response-processing path. Timeouts only exist for Pool (via troupe). Commands block forever. Finally hooks ignore concurrency limits.

All of these run work in a thread and send a result back on a channel. They should go through one dispatch path with one trait. If it runs, it goes through the trait. Concurrency and result routing wrap the trait from the outside.

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

### Naming: `Action` trait, config enum becomes `ActionKind`

The existing `Action` enum (`Action::Pool`, `Action::Command`) in config types is an enum of action kinds. Rename it to `ActionKind`. The `Action` name belongs to the trait — the thing that performs work. Similarly `ActionFile` becomes `ActionKindFile`.

### `Action` trait

```rust
pub trait Action: Send {
    fn perform(&self, value: &serde_json::Value) -> Result<String, String>;
}
```

Takes a value, runs something, returns stdout or an error string. The engine calls `action.perform()` in a spawned thread and collects the result on a channel. One thread per action.

### Barnum owns the timeout

The timeout comes from step config. Barnum passes it to the action at construction time. The action is responsible for honoring it — `perform()` must return within the deadline.

- `ShellAction`: `run_shell_command` takes an `Option<Duration>`. On timeout, a reaper thread kills the child process. The dispatch thread blocks on `wait_with_output`, which returns immediately once the child is killed.
- `PoolAction`: the struct holds `timeout: Option<Duration>` (from step config). In `perform()`, it passes this value to troupe as `pool_timeout` in the payload (agent lifecycle) and also uses it as its own deadline for the CLI invocation. Two timeouts, same value: one is troupe's concern (agent management), one is Barnum's (the action must return).

`action.perform()` is synchronous. The engine spawns one thread, which calls `perform()`, which returns. No wrapper threads, no abandoned threads.

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

### Phase 1: `Action` trait + `PoolAction`

Introduce the trait. Move Pool dispatch through it. Command stays as-is.

Requires Phase 0 (unified result types).

Also rename config `Action` enum to `ActionKind` (and `ActionFile` to `ActionKindFile`) to free up the name.

#### 1a. Rename config enum

`Action::Pool` / `Action::Command` becomes `ActionKind::Pool` / `ActionKind::Command` everywhere: `config.rs`, `resolved.rs`, `runner/mod.rs`, tests. Mechanical find-and-replace.

#### 1b. Create `runner/action.rs`

```rust
pub trait Action: Send {
    fn perform(&self, value: &serde_json::Value) -> Result<String, String>;
}
```

#### 1c. `PoolAction`

Move the `Response` to `Result<String, String>` conversion from `dispatch_pool_task` (Phase 0d) into the action:

```rust
pub struct PoolAction {
    pub root: PathBuf,
    pub invoker: Invoker<TroupeCli>,
    pub docs: String,
    pub step_name: StepName,
    pub timeout: Option<Duration>,
}

impl Action for PoolAction {
    fn perform(&self, value: &serde_json::Value) -> Result<String, String> {
        let pool_timeout = self.timeout.map(|d| d.as_secs());
        let payload = build_agent_payload(
            &self.step_name, value, &self.docs, pool_timeout,
        );
        match submit_via_cli(&self.root, &payload, &self.invoker) {
            Ok(Response::Processed { stdout, .. }) => Ok(stdout),
            Ok(Response::NotProcessed { .. }) => Err("not processed by pool".into()),
            Err(e) => Err(e.to_string()),
        }
    }
}
```

#### 1d. `dispatch_action`

Replaces `dispatch_pool_task`. Takes `WorkerKind` so it works for both tasks and finally hooks (Phase 3).

```rust
pub fn dispatch_action(
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    action: Box<dyn Action>,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = task.value.clone();
    let output = action.perform(&value.0);
    let _ = tx.send(WorkerResult {
        task_id,
        task,
        kind,
        result: ActionResult { value, output },
    });
}
```

#### 1e. Update `dispatch_task` Pool branch

```rust
ActionKind::Pool(..) => {
    let docs = generate_step_docs(step, self.config);
    let action = Box::new(PoolAction {
        root: self.pool.root.clone(),
        invoker: self.pool.invoker.clone(),
        docs,
        step_name: task.step.clone(),
        timeout: step.options.timeout.map(Duration::from_secs),
    });

    info!(step = %task.step, "submitting task to pool");
    thread::spawn(move || {
        dispatch_action(
            task_id, task, WorkerKind::Task, action, &tx,
        );
    });
}
```

#### 1f. Delete `dispatch_pool_task`

Compile, run full suite.

---

### Phase 2: `ShellAction` + Command through trait

Both branches of `dispatch_task` now construct a `Box<dyn Action>` and call `dispatch_action`.

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
    let action = Box::new(ShellAction {
        script: script.clone(),
        step_name: task.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });

    info!(step = %task.step, script = %script, "executing command");
    thread::spawn(move || {
        dispatch_action(
            task_id, task, WorkerKind::Task, action, &tx,
        );
    });
}
```

#### 2c. Delete `dispatch_command_task`

After this, both branches of `dispatch_task` do the same thing: construct an action, call `dispatch_action`. The match only picks which action to construct. This is the seam for PLUGGABLE_ACTION_KINDS.

Compile, run full suite.

---

### Phase 3: Finally hooks through trait

Finally hooks go through `dispatch_action` with a `ShellAction`. `WorkerKind` (Phase 0a) already handles the routing — `dispatch_action` takes `kind: WorkerKind` and passes it through to `WorkerResult`.

#### 3a. Update `dispatch_finally`

```rust
fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("...");
    let script = step.finally_hook.clone().expect("...");
    let tx = self.tx.clone();
    let action = Box::new(ShellAction {
        script: script.to_string(),
        step_name: task.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });
    thread::spawn(move || {
        dispatch_action(
            parent_id, task, WorkerKind::Finally { parent_id },
            action, &tx,
        );
    });
}
```

#### 3c. Delete `dispatch_finally_task`

Three dispatch functions replaced by one. Compile, run full suite.

---

### Phase 4: Timeout

Barnum enforces timeout for all action types. The timeout value comes from step config and is passed to the action at construction time.

#### 4a. Add timeout to `run_shell_command`

`run_shell_command` gains an `Option<Duration>` parameter. When set, a reaper thread kills the child after the deadline:

```rust
pub fn run_shell_command(
    script: &str,
    stdin_input: &str,
    working_dir: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<String, String> {
    // ... spawn child, write stdin as before ...

    if let Some(duration) = timeout {
        let child_id = child.id();
        thread::spawn(move || {
            thread::sleep(duration);
            unsafe { libc::kill(-(child_id as i32), libc::SIGKILL); }
        });
    }

    let output = child.wait_with_output()
        .map_err(|e| format!("wait failed: {e}"))?;

    if !output.status.success() {
        if timeout.is_some() && output.status.code().is_none() {
            return Err("timed out".into());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("exited with status {}: {}", output.status, stderr.trim()));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("not valid UTF-8: {e}"))
}
```

The reaper thread is fire-and-forget. If the child exits before the deadline, the `kill` fails harmlessly (process gone). If the deadline fires, the child dies and `wait_with_output` returns immediately with a signal status.

#### 4b. Add `timeout` to `ShellAction`

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
    pub timeout: Option<Duration>,
}

impl Action for ShellAction {
    fn perform(&self, value: &serde_json::Value) -> Result<String, String> {
        let task_json = serde_json::to_string(&serde_json::json!({
            "kind": &self.step_name,
            "value": value,
        }))
        .unwrap_or_default();

        run_shell_command(&self.script, &task_json, Some(&self.working_dir), self.timeout)
    }
}
```

#### 4c. Construct with timeout at dispatch sites

In `dispatch_task` (Command branch):
```rust
let action = Box::new(ShellAction {
    script: script.clone(),
    step_name: task.step.clone(),
    working_dir: self.pool.working_dir.clone(),
    timeout: step.options.timeout.map(Duration::from_secs),
});
```

In `dispatch_finally`, pass `None` (finally hooks have no timeout config).

`dispatch_action` is unchanged. It calls `action.perform()` and sends the result. The timeout is between the action and `run_shell_command` — `dispatch_action` doesn't know or care.

Compile, run full suite.

## Summary

| Phase | What | Deletes | Adds |
|-------|------|---------|------|
| 0 | Unify result types | `SubmitResult`, `PoolResult`, `CommandResult`, `FinallyResult`, `process_pool_response`, `process_command_response`, `run_command_action` | `ActionResult`, `WorkerKind` |
| 1 | Pool through trait | `dispatch_pool_task`, `Action` enum (renamed) | `Action` trait, `PoolAction`, `dispatch_action`, `ActionKind` enum |
| 2 | Command through trait | `dispatch_command_task` | `ShellAction` |
| 3 | Finally through trait | `dispatch_finally_task` | (reuses `ShellAction` + `WorkerKind::Finally`) |
| 4 | Timeout | | `timeout` param on `run_shell_command`, `timeout` field on `ShellAction` |

Each phase compiles and passes tests before moving to the next.

## What doesn't change

`ActionKind` enum variants stay as-is (just renamed from `Action`). State logging (task submission and completion) is unchanged.
