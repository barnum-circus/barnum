# Unified Action Dispatch

**Prerequisite to:** [PLUGGABLE_ACTION_KINDS.md](./PLUGGABLE_ACTION_KINDS.md)

## Motivation

Everything that executes — tasks, pre-hooks, post-hooks, finally hooks — should go through a single dispatch trait. Today, Pool and Command actions are handled asymmetrically, hooks run through their own ad-hoc code paths, and timeouts only exist for Pool actions. The result: inconsistent timeout enforcement, separate code paths for structurally identical work, and no unified concurrency accounting.

The core principle: **if it runs, it goes through the trait.** Every executable unit — whether it's a Pool task, a Command, a pre-hook, a post-hook, or a finally task — gets scheduled, timed out, and tracked identically. All of them contribute to concurrency limits.

This refactor unifies all execution into a common `ActionOutcome` shape and adds timeout support everywhere. This is the mechanical prerequisite to extracting the `Executor` trait in PLUGGABLE_ACTION_KINDS.md.

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

Pre-hooks, post-hooks, and finally hooks all run shell commands but through separate code paths with no timeout enforcement. They don't contribute to concurrency tracking.

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

### 1. Add cross-platform timeout to `run_shell_command`

**File:** `runner/shell.rs`

**Dependency:** Add [`wait-timeout`](https://crates.io/crates/wait-timeout) crate for cross-platform timed waits on child processes. This wraps platform-specific APIs (waitpid on Unix, WaitForSingleObject on Windows) so timeout enforcement works on all platforms.

```rust
use wait_timeout::ChildExt;

pub fn run_shell_command(
    script: &str,
    stdin_input: &str,
    working_dir: Option<&Path>,
    timeout: Option<Duration>,
) -> ShellResult {
    let mut child = spawn_shell(script, stdin_input, working_dir)?;

    match timeout {
        Some(duration) => {
            match child.wait_timeout(duration) {
                Ok(Some(status)) => {
                    // Child exited within the deadline
                    let output = collect_output(child, status);
                    classify_output(output, false)
                }
                Ok(None) => {
                    // Timeout expired — kill and reap
                    let _ = child.kill();
                    let _ = child.wait();
                    ShellResult::Timeout
                }
                Err(e) => ShellResult::Error(format!("wait failed: {e}")),
            }
        }
        None => {
            let output = child.wait_with_output()
                .map_err(|e| format!("wait failed: {e}"))?;
            classify_output(output, false)
        }
    }
}
```

The return type distinguishes timeout from other failures:

```rust
pub enum ShellResult {
    Success(String),
    Timeout,
    Error(String),
}
```

`child.kill()` is cross-platform (SIGKILL on Unix, TerminateProcess on Windows). No `libc` dependency needed.

### 2. Unify `SubmitResult`

**File:** `runner/dispatch.rs`

Replace the per-kind variants with a single action result shape:

```rust
pub(super) enum SubmitResult {
    Action {
        value: StepInputValue,
        outcome: ActionOutcome,
    },
    PreHookError(String),
}

pub(super) enum ActionOutcome {
    /// Action produced stdout (JSON array of follow-up tasks).
    Success(String),
    /// Action timed out.
    Timeout,
    /// Action failed.
    Error(String),
}
```

Both `dispatch_pool_task` and `dispatch_command_task` produce `SubmitResult::Action`. Pool maps `Response::Processed` to `ActionOutcome::Success` and `Response::NotProcessed` to `ActionOutcome::Timeout`. Command maps `ShellResult::Success` to `ActionOutcome::Success`, `ShellResult::Timeout` to `ActionOutcome::Timeout`, and `ShellResult::Error` to `ActionOutcome::Error`.

**Finally tasks also produce `SubmitResult::Action`.** They run shell commands (or pool tasks) and produce the same outcome shape. The `SubmitResult::Finally` variant is removed — finally tasks are dispatched identically, with the "this is a finally" semantics handled by the caller when processing the outcome, not by a separate result type.

### 3. Dispatch hooks through the same path

**All hooks get timeouts.** Pre-hooks, post-hooks, and finally hooks all call `run_shell_command` with an optional timeout. The timeout can come from:
- Step-level `options.timeout` (inherited by hooks associated with that step)
- A global hook timeout config (future work if needed)

**All hooks contribute to concurrency.** When a hook is running, it occupies a slot in the concurrency limit, same as a task action. This prevents a burst of hooks from overwhelming the system.

The dispatch path for hooks:
1. Hook runs through `run_shell_command` with timeout
2. Result is mapped to `ActionOutcome`
3. Timeout/failure handling is identical to task actions

### 4. Pass timeout to `dispatch_command_task`

**File:** `runner/dispatch.rs`

Update the signature to accept `timeout: Option<u64>` and pass it through to `run_shell_command` (as `timeout.map(Duration::from_secs)`).

**File:** `runner/mod.rs`

Pass `step.options.timeout` to the command dispatch, same as pool.

### 5. Simplify response processing

**File:** `runner/response.rs`

The `process_submit_result` function currently has separate arms for `Pool` and `Command`. With the unified `SubmitResult::Action`, there's a single arm:

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

This removes the `process_pool_response` and `process_command_response` functions entirely. The `troupe::Response` type no longer leaks into response processing — it's handled inside `dispatch_pool_task` where it's mapped to `ActionOutcome`.

### 6. Remove `troupe::Response` dependency from response.rs

After unification, `response.rs` no longer needs `use troupe::Response`. The troupe dependency is confined to `dispatch.rs` and `submit.rs`, where it belongs.

## What doesn't change

- **Config types:** `ActionFile` and `Action` enums remain as-is. The closed enum is fine until PLUGGABLE_ACTION_KINDS introduces the trait.
- **State logging:** Unchanged. The log captures task submission and completion regardless of action kind.

## Open Questions

1. **Hook timeout source.** Step-level `options.timeout` is the natural choice for hooks associated with a step. Should there be a separate global config for hook timeouts? Or is the step timeout sufficient? Leaning toward step timeout only — keep it simple.

2. **SIGKILL vs graceful kill.** `child.kill()` is SIGKILL on Unix, TerminateProcess on Windows — both are immediate and non-catchable. For a timeout scenario this is appropriate: the process exceeded its deadline. If graceful shutdown matters for specific commands, users can set a longer timeout.
