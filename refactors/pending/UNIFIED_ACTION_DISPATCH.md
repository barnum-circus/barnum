# Unified Action Dispatch

**Prerequisite to:** PLUGGABLE_ACTION_KINDS (future)

## Motivation

Everything that executes — tasks, pre-hooks, post-hooks, finally hooks — should go through a single dispatch trait. Today, Pool and Command actions have separate dispatch functions, hooks run through ad-hoc code paths, and timeouts only exist for Pool actions. Commands block forever. Hooks don't participate in concurrency limits.

The core principle: **if it runs, it goes through the trait.** Every executable unit — Pool task, Command task, pre-hook, post-hook, finally hook — is scheduled, timed out, and tracked through the same interface. All contribute to the concurrency limit.

## Current State

### Dispatch fork (`runner/mod.rs:707-744`)

The runner pattern-matches on `Action` and calls different functions with different signatures:

```rust
match &step.action {
    Action::Pool { .. } => {
        let timeout = step.options.timeout;  // ← used
        thread::spawn(move || {
            dispatch_pool_task(task_id, task, pre_hook, &docs, timeout, &pool, &tx);
        });
    }
    Action::Command { script } => {
        // ← no timeout
        thread::spawn(move || {
            dispatch_command_task(task_id, task, pre_hook, &script, &working_dir, &tx);
        });
    }
}
```

### `SubmitResult` variants (`runner/dispatch.rs:35-49`)

```rust
pub(super) enum SubmitResult {
    Pool { value: StepInputValue, response: io::Result<Response> },
    Command { value: StepInputValue, output: io::Result<String> },
    Finally { value: StepInputValue, output: Result<String, String> },
    PreHookError(String),
}
```

Pool wraps a `troupe::Response` (which has `Processed` and `NotProcessed` variants). Command wraps a raw `io::Result<String>`. The response processing in `response.rs:52-127` must then branch again to unwrap each shape.

### Hooks

Pre-hooks and the action currently execute in the same worker thread (one concurrency slot for both). Post-hooks execute on the main thread. Finally hooks get their own worker thread. None of them have timeout enforcement.

### Response processing (`runner/response.rs:52-127`)

Pool's success case unwraps `Response::Processed { stdout }` and calls `process_stdout`. Command's success case calls `process_stdout` directly. They converge on the same function — but the match arms to get there are per-kind.

Pool's timeout case (`Response::NotProcessed`) produces `FailureKind::Timeout`. Command has no equivalent — it can't timeout.

### Command execution (`runner/shell.rs:10-47`)

```rust
let output = child
    .wait_with_output()  // ← blocks forever
    .map_err(|e| format!("wait failed: {e}"))?;
```

No timeout enforcement. The thread blocks until the child process exits.

## Proposed Changes

### 1. Introduce the `Executor` trait

**File:** `runner/executor.rs` (new)

This is the central abstraction. Every executable unit goes through this trait. The trait itself is deliberately minimal — it just produces a string:

```rust
/// Trait for executing work units.
///
/// Pool tasks, Command tasks, pre-hooks, post-hooks, and finally hooks
/// all go through this interface. The runner schedules, times out, and
/// tracks concurrency for every `Executor::execute` call identically.
///
/// Timeout enforcement and working directory are the runner's/construction's
/// responsibility, not the trait's. The trait takes input, produces output.
pub trait Executor: Send + Sync {
    fn execute(&self, input: &str) -> Result<String, String>;
}
```

The return type is `Result<String, String>` — success stdout or error message. No typed outcome enum. Working directory is captured at construction time. The runner adds timeout semantics on top:

```rust
/// Runner-level outcome, wrapping the executor's raw result.
pub enum ActionOutcome {
    Success(String),
    Timeout,
    Error(String),
}

fn run_with_timeout(
    executor: &dyn Executor,
    input: &str,
    timeout: Option<Duration>,
) -> ActionOutcome {
    // Spawn thread, call executor.execute(), wait with timeout.
    // On timeout: kill thread, return ActionOutcome::Timeout.
    // On Ok(stdout): return ActionOutcome::Success(stdout).
    // On Err(e): return ActionOutcome::Error(e).
}
```

This keeps the trait at the lowest common denominator. Type safety can be layered on later without changing the trait contract.

### 2. Implement `Executor` for Shell and Pool

**`ShellExecutor`** — used by Command actions, pre-hooks, post-hooks, and finally hooks:

```rust
pub struct ShellExecutor {
    pub script: String,
    pub working_dir: PathBuf,
}

impl Executor for ShellExecutor {
    fn execute(&self, input: &str) -> Result<String, String> {
        run_shell_command(&self.script, input, Some(&self.working_dir))
    }
}
```

**`PoolExecutor`** — used by Pool actions. Both working directory and pool timeout are captured at construction:

```rust
pub struct PoolExecutor {
    pub root: PathBuf,
    pub invoker: Invoker<TroupeCli>,
    pub docs: String,
    pub step_name: StepName,
    pub pool_timeout: Option<u64>,
}

impl Executor for PoolExecutor {
    fn execute(&self, input: &str) -> Result<String, String> {
        let payload = build_agent_payload(
            &self.step_name, /* ... */, &self.docs, self.pool_timeout,
        );
        match submit_via_cli(&self.root, &payload, &self.invoker) {
            Ok(Response::Processed { stdout, .. }) => Ok(stdout),
            Ok(Response::NotProcessed { .. }) => Err("not processed".into()),
            Err(e) => Err(e.to_string()),
        }
    }
}
```

Note: `PoolExecutor` returns `Err` for `NotProcessed`. The runner's timeout wrapper handles the *local* deadline (killing work that exceeds it). The pool's own timeout is separate — it tells the pool how long the agent has, and is captured at construction. These are independent mechanisms.

### 3. Decompose task lifecycle into independent work units

Currently a worker thread runs the pre-hook and action together as one unit. In the unified model, each phase is a separate work unit dispatched through the trait:

**Task lifecycle phases:**
1. **Pre-hook** — dispatched through `ShellExecutor`, occupies a concurrency slot, has a timeout. On success, the transformed value feeds into the action. On failure, `PreHookError` is produced.
2. **Action** — dispatched through `PoolExecutor` or `ShellExecutor`, occupies a concurrency slot, has a timeout.
3. **Post-hook** — dispatched through `ShellExecutor`, occupies a concurrency slot, has a timeout.
4. **Finally hook** — dispatched through `ShellExecutor`, occupies a concurrency slot, has a timeout.

Each phase completes before the next is scheduled. The `PendingDispatch` enum grows to represent the pipeline:

```rust
enum PendingDispatch {
    /// Run a pre-hook for a task.
    PreHook { task_id: LogTaskId },
    /// Run the task's action (pre-hook already completed).
    Action { task_id: LogTaskId, value: StepInputValue },
    /// Run a post-hook after action completion.
    PostHook { task_id: LogTaskId, post_input: PostHookInput },
    /// Run a finally hook for a parent whose children completed.
    Finally { parent_id: LogTaskId },
}
```

Tasks without a pre-hook skip directly to `Action`. Tasks without a post-hook skip directly to completion processing. The engine dispatches whichever phase is next, and each phase occupies exactly one concurrency slot while executing.

### 4. Add timeout infrastructure to the runner

**File:** `runner/executor.rs`

The `run_with_timeout` function wraps any executor call with a deadline. This is runner infrastructure, separate from the executor trait.

**Dependency:** Add [`wait-timeout`](https://crates.io/crates/wait-timeout) for cross-platform timed waits (waitpid on Unix, WaitForSingleObject on Windows). `child.kill()` is cross-platform (SIGKILL on Unix, TerminateProcess on Windows).

The `run_shell_command` function in `shell.rs` keeps its current signature (`Result<String, String>`). Timeout enforcement is layered on by the runner when it calls the executor.

### 5. Unify `SubmitResult`

**File:** `runner/dispatch.rs`

Replace the per-kind variants with a single shape:

```rust
pub(super) enum SubmitResult {
    /// Any executor completed (pool, command, hook, finally).
    Action {
        value: StepInputValue,
        outcome: ActionOutcome,
    },
    /// Pre-hook failed before the action could run.
    PreHookError(String),
}
```

`PreHookError` remains separate because it carries semantically different information: the original (untransformed) value, and the post-hook sees `"kind": "PreHookError"` rather than `"kind": "Error"`.

### 6. Simplify response processing

**File:** `runner/response.rs`

With the unified `SubmitResult::Action`, there's a single arm:

```rust
SubmitResult::Action { value, outcome } => match outcome {
    ActionOutcome::Success(stdout) => {
        process_stdout(&stdout, task, &value, step, schemas)
    }
    ActionOutcome::Timeout => {
        let outcome = process_retry(task, &step.options, FailureKind::Timeout);
        (outcome, PostHookInput::Timeout { input: value })
    }
    ActionOutcome::Error(error) => {
        error!(step = %task.step, %error, "action failed");
        let outcome = process_retry(task, &step.options, FailureKind::SubmitError);
        (outcome, PostHookInput::Error { input: value, error })
    }
}
```

`process_pool_response` and `process_command_response` are removed. `troupe::Response` no longer leaks into `response.rs` — it's handled inside `PoolExecutor`.

### 7. Remove `troupe::Response` dependency from response.rs

After unification, `response.rs` no longer needs `use troupe::Response`. The troupe dependency is confined to the `PoolExecutor` implementation.

## Dispatch flow summary

```
                    ┌──────────────┐
                    │ PendingDispatch│
                    └──────┬───────┘
                           │
            ┌──────────────┼──────────────┐
            ▼              ▼              ▼
        PreHook         Action         PostHook / Finally
            │              │              │
            ▼              ▼              ▼
     ShellExecutor    Pool/Shell     ShellExecutor
            │         Executor            │
            ▼              ▼              ▼
     Result<String>   Result<String>  Result<String>
            │              │              │
            └──────────────┼──────────────┘
                           ▼
                    run_with_timeout
                           │
                           ▼
                     ActionOutcome
```

Every box in the middle row implements `Executor` (returns `Result<String, String>`). The runner wraps each call with `run_with_timeout` to produce `ActionOutcome`. Every execution occupies a concurrency slot.

## What doesn't change

- **Config types:** `ActionFile` and `Action` enums remain as-is. The closed enum is fine until PLUGGABLE_ACTION_KINDS introduces user-defined action kinds.
- **State logging:** Unchanged. The log captures task submission and completion regardless of action kind.

## Open Questions

1. **Hook timeout source.** Step-level `options.timeout` is the natural choice for hooks associated with a step. A separate hook-specific timeout config could be added later if needed.

2. **Concurrency cost of pipeline decomposition.** Decomposing pre-hook + action into two separate concurrency slots means a task with a pre-hook occupies 2 slots sequentially (never simultaneously). This is correct — each phase is real work. But it means a workflow heavy on pre-hooks will see lower action throughput at the same concurrency limit. This is a factual tradeoff, not necessarily a problem.
