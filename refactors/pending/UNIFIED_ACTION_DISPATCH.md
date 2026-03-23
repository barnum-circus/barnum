# Unified Action Dispatch

**Prerequisite to:** [PLUGGABLE_ACTION_KINDS.md](./PLUGGABLE_ACTION_KINDS.md)

## Motivation

Pool and Command actions are handled asymmetrically in the runner. The most visible gap: **Command actions have no timeout enforcement.** A misbehaving shell command hangs forever — no timeout, no retry, no recourse. Pool actions get timeouts because troupe enforces them server-side. But the config schema exposes `options.timeout` for all steps, regardless of action kind, creating a contract the runner doesn't honor for commands.

Beyond timeouts, the two action kinds are dispatched through parallel but structurally different code paths. They share pre-hook handling and ultimately feed into the same `process_stdout` function, but the plumbing between dispatch and response processing is kind-specific: separate dispatch functions, separate `SubmitResult` variants, separate response processing branches.

This refactor makes Pool and Command opaque to the runner's dispatch logic, handling them identically through a common `SubmitResult` shape. This is the mechanical prerequisite to introducing the `Executor` trait in PLUGGABLE_ACTION_KINDS.md — once Pool and Command produce the same result type, extracting a trait is a small diff.

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

### 1. Add timeout to `run_shell_command`

**File:** `runner/shell.rs`

Add an optional timeout parameter. When set, spawn a reaper thread that kills the child process after the deadline. Use `child.try_wait()` in a loop with a short sleep, or use the `wait-timeout` crate (which wraps platform-specific APIs for timed waits on child processes).

The cleanest approach uses a background reaper thread:

```rust
pub fn run_shell_command(
    script: &str,
    stdin_input: &str,
    working_dir: Option<&Path>,
    timeout: Option<Duration>,
) -> ShellResult {
    let mut child = spawn_shell(script, stdin_input, working_dir)?;

    if let Some(duration) = timeout {
        let child_id = child.id();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        // Reaper thread: kill the child if it exceeds the deadline
        thread::spawn(move || {
            if done_rx.recv_timeout(duration).is_err() {
                // Timeout expired before the child finished — kill it
                unsafe { libc::kill(child_id as i32, libc::SIGKILL); }
            }
        });

        let output = child.wait_with_output()
            .map_err(|e| format!("wait failed: {e}"))?;

        // Signal reaper to stop (child finished before timeout)
        let _ = done_tx.send(());

        classify_output(output, true)
    } else {
        let output = child.wait_with_output()
            .map_err(|e| format!("wait failed: {e}"))?;
        classify_output(output, false)
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

This keeps the caller from needing to guess whether a SIGKILL exit was a timeout or something else.

### 2. Unify `SubmitResult`

**File:** `runner/dispatch.rs`

Replace the per-kind variants with a single action result shape:

```rust
pub(super) enum SubmitResult {
    Action {
        value: StepInputValue,
        outcome: ActionOutcome,
    },
    Finally {
        value: StepInputValue,
        output: Result<String, String>,
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

### 3. Pass timeout to `dispatch_command_task`

**File:** `runner/dispatch.rs`

Update the signature to accept `timeout: Option<u64>` and pass it through to `run_shell_command` (as `timeout.map(Duration::from_secs)`).

**File:** `runner/mod.rs`

Pass `step.options.timeout` to the command dispatch, same as pool.

### 4. Simplify response processing

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

This removes the `process_pool_response` and `process_command_response` functions entirely, since their logic is now unified in the match arm above. The `troupe::Response` type no longer leaks into response processing — it's handled inside `dispatch_pool_task` where it's mapped to `ActionOutcome`.

### 5. Remove `troupe::Response` dependency from response.rs

After unification, `response.rs` no longer needs `use troupe::Response`. The troupe dependency is confined to `dispatch.rs` and `submit.rs`, where it belongs.

## What doesn't change

- **Config types:** `ActionFile` and `Action` enums remain as-is. The closed enum is fine until PLUGGABLE_ACTION_KINDS introduces the trait.
- **Pre-hooks:** Already handled identically for both kinds.
- **Post-hooks:** Already handled uniformly in `process_and_finalize`.
- **Finally tasks:** Keep their own `SubmitResult::Finally` variant. They have different semantics (no schema validation, triggered on subtree completion).
- **State logging:** Unchanged. The log captures task submission and completion regardless of action kind.

## Open Questions

1. **`libc::kill` vs portable timeout.** The reaper approach uses `libc::kill` which is Unix-only. The `wait-timeout` crate provides cross-platform timed waits. Since barnum currently targets Unix (macOS/Linux), `libc::kill` is fine. If Windows support matters later, switch to `wait-timeout`.

2. **SIGKILL vs SIGTERM.** SIGKILL is immediate and non-catchable. SIGTERM gives the process a chance to clean up. For a timeout scenario, SIGKILL is appropriate — the process exceeded its deadline and the runner needs to move on. If graceful shutdown matters for specific commands, users can set a longer timeout.

3. **Should hooks also get timeouts?** Pre-hooks and post-hooks run shell commands too, but they don't currently have timeout support. This refactor scopes timeout to action execution only. Hook timeouts could be added later using the same `run_shell_command` mechanism.
