# Refactor: GSD to Use CLI Instead of Internal APIs

## Motivation

Currently, `gsd_config` imports `agent_pool` as a Rust dependency and calls internal functions directly:

```rust
// gsd_config/runner.rs
use agent_pool::Response;

// Checking if daemon is running
agent_pool::is_daemon_running(pool_path)

// Submitting tasks
agent_pool::submit(&root, &agent_pool::Payload::inline(&payload))
```

This creates tight coupling between the crates. If GSD used the CLI instead, it would:
1. **Validate the CLI interface** - Ensure the CLI is complete and usable
2. **Enable language-agnostic orchestration** - Any language could implement a GSD-like runner
3. **Simplify the public API** - `agent_pool` could hide more implementation details
4. **Dogfood our own CLI** - Catch usability issues

## Current State (Exact Code)

### File: `gsd_config/src/runner.rs`

**Line 13 - Import Response type:**
```rust
use agent_pool::Response;
```

**Lines 169-174 - SubmitResult enum:**
```rust
enum SubmitResult {
    Pool(io::Result<Response>),
    Command(io::Result<String>),
    /// Pre hook failed before the action could run.
    PreHookError(String),
}
```

**Lines 192-209 - Daemon running check in `TaskRunner::new()`:**
```rust
// Check if the pool exists and is running (only if config uses Pool actions and has tasks)
let pool_path = runner_config.agent_pool_root;
if config.has_pool_actions() && !runner_config.initial_tasks.is_empty() {
    if !pool_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Pool not found: {}", pool_path.display()),
        ));
    }
    if !agent_pool::is_daemon_running(pool_path) {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            format!(
                "Pool daemon not running at: {} (directory exists but no daemon)",
                pool_path.display()
            ),
        ));
    }
}
```

**Lines 351-364 - Submit task in spawn thread:**
```rust
// Build payload with (possibly modified) value
let payload = build_agent_payload_with_value(
    &step_name,
    &effective_value,
    &docs,
    timeout,
);
debug!(payload = %payload, "task payload");

let result =
    agent_pool::submit(&root, &agent_pool::Payload::inline(&payload));
let _ = tx.send(InFlightResult {
    task,
    task_id,
    origin_id,
    step_name,
    effective_value,
    result: SubmitResult::Pool(result),
    post_hook,
    finally_hook,
});
```

**Lines 664-686 - Process pool response:**
```rust
fn process_pool_response(
    response: Response,
    task: &Task,
    effective_value: &serde_json::Value,
    step: &Step,
    schemas: &CompiledSchemas,
    effective: &EffectiveOptions,
) -> (TaskResult, Vec<Task>, PostHookInput) {
    match response {
        Response::Processed { stdout, .. } => {
            debug!(stdout = %stdout, "agent response");
            process_stdout(&stdout, task, effective_value, step, schemas, effective)
        }
        Response::NotProcessed { reason } => {
            warn!(step = %task.step, ?reason, "task outcome unknown");
            let (result, tasks) = process_retry(task, effective, FailureKind::Timeout);
            let post_input = PostHookInput::Timeout {
                input: effective_value.clone(),
            };
            (result, tasks, post_input)
        }
    }
}
```

## Proposed Changes

### Change 1: Remove `is_daemon_running` check entirely

**Before (lines 192-209):**
```rust
// Check if the pool exists and is running (only if config uses Pool actions and has tasks)
let pool_path = runner_config.agent_pool_root;
if config.has_pool_actions() && !runner_config.initial_tasks.is_empty() {
    if !pool_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Pool not found: {}", pool_path.display()),
        ));
    }
    if !agent_pool::is_daemon_running(pool_path) {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            format!(
                "Pool daemon not running at: {} (directory exists but no daemon)",
                pool_path.display()
            ),
        ));
    }
}
```

**After:**
```rust
// No check needed - submit_task CLI will fail with clear error if daemon isn't running
```

The CLI's `submit_task` already checks if the daemon is running and returns a clear error. We don't need to duplicate this check.

### Change 2: Replace `agent_pool::submit` with CLI call

**Before (lines 353-354):**
```rust
let result =
    agent_pool::submit(&root, &agent_pool::Payload::inline(&payload));
```

**After:**
```rust
let result = submit_via_cli(&root, &payload);
```

**New helper function (add near bottom of file):**
```rust
/// Submit a task via the CLI instead of internal API.
fn submit_via_cli(pool: &Path, payload: &str) -> io::Result<Response> {
    let binary = resolve_agent_pool_binary();

    // Use 24-hour timeout. TODO: Add --no-timeout support to CLI.
    let output = Command::new(&binary)
        .arg("submit_task")
        .arg("--pool")
        .arg(pool)
        .arg("--notify")
        .arg("file")
        .arg("--timeout-secs")
        .arg("86400")
        .arg("--data")
        .arg(payload)
        .output()
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Failed to run agent_pool binary '{}': {e}", binary.display()),
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("agent_pool submit_task failed: {}", stderr.trim()),
        ));
    }

    serde_json::from_slice(&output.stdout).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse agent_pool output: {e}"),
        )
    })
}

/// Resolve the agent_pool binary path.
fn resolve_agent_pool_binary() -> PathBuf {
    // 1. Environment variable override
    if let Ok(path) = std::env::var("AGENT_POOL_BINARY") {
        return PathBuf::from(path);
    }

    // 2. Default: assume it's in PATH
    PathBuf::from("agent_pool")
}
```

### Change 3: Update imports

**Before (line 13):**
```rust
use agent_pool::Response;
```

**After:**
```rust
use agent_pool::Response;  // Keep - we still use this type for parsing CLI output
```

**No change needed** - we keep the type dependency since `Response` is used to parse the CLI's JSON output.

## What Stays The Same

These parts **do not change**:

1. **`SubmitResult` enum** (lines 169-174) - Still wraps `io::Result<Response>`
2. **`process_pool_response`** (lines 664-686) - Still matches on `Response` variants
3. **All error handling logic** - Same retry behavior, same error propagation
4. **All hook handling** - Pre/post/finally hooks unchanged

## Resolved Questions

1. **Binary path resolution** - Resolution order:
   1. `AGENT_POOL_BINARY` environment variable (explicit override)
   2. Assume `agent_pool` is in PATH (simplest default)

2. **Keep types dependency?** - **Yes.** Keep using `agent_pool::Response` to parse CLI output. This avoids duplicating the type definition and ensures compatibility.

3. **Timeout handling** - GSD passes a very large `--timeout-secs` (e.g., 86400 = 24 hours).
   - The CLI's `submit_task` defaults to 5 minutes if not specified, which isn't enough
   - GSD handles its own per-step timeouts separately via the task payload
   - **TODO:** Add `--no-timeout` or `--timeout-secs 0` support for truly infinite waits
   - **TODO:** CLI commands should check status file contains "ready" (not just exists) and understand shutdown signal ("stop")

4. **Pool readiness** - Don't check. Let `submit_task` fail with its own error if daemon isn't running.
   - Removes duplicate logic and keeps error handling in one place

5. **Error handling** - Non-zero exit code → `io::Error`. Parse stderr for message.

## Open Questions / Concerns

1. **CLI exit codes** - What exit codes does `submit_task` return?
   - Need to verify: does it return non-zero on `NotProcessed`? Or only on actual errors?
   - Current assumption: non-zero = error (connection failed, invalid args, etc.)
   - Zero + JSON `NotProcessed` = timeout/shutdown (not an error from CLI perspective)

2. **Binary not found** - What's the user experience when `agent_pool` binary isn't available?
   - Should we check at startup and give a clear error message?
   - Or let it fail on first submit with a helpful error?

3. **Cargo.toml changes** - Do we need to update `gsd_config/Cargo.toml`?
   - We still depend on `agent_pool` for the `Response` type
   - But we can remove features or limit the dependency scope

## Implementation Plan

### Task 1: Add `submit_via_cli` helper function

**File:** `gsd_config/src/runner.rs`

Add after line 960 (after `build_agent_payload_with_value`):

```rust
/// Submit a task via the CLI instead of internal API.
fn submit_via_cli(pool: &Path, payload: &str) -> io::Result<Response> {
    // ... implementation from above
}

/// Resolve the agent_pool binary path.
fn resolve_agent_pool_binary() -> PathBuf {
    // ... implementation from above
}
```

### Task 2: Replace `agent_pool::submit` call

**File:** `gsd_config/src/runner.rs`

**Lines 353-354**, change:
```rust
// Before
let result =
    agent_pool::submit(&root, &agent_pool::Payload::inline(&payload));

// After
let result = submit_via_cli(&root, &payload);
```

### Task 3: Remove `is_daemon_running` check

**File:** `gsd_config/src/runner.rs`

**Lines 192-209**, delete the entire daemon check block. The CLI handles this.

### Task 4: Update imports

**File:** `gsd_config/src/runner.rs`

**Line 13** - Keep `use agent_pool::Response;` but we can remove other unused imports if any.

### Task 5: Verify tests pass

Run:
```bash
cargo test -p gsd_config
SKIP_IPC_TESTS=1 cargo test -p gsd_cli
```

## Summary

| Aspect | Assessment |
|--------|-----------|
| **Complexity** | Low-moderate - only 2 call sites change |
| **Risk** | Low - same behavior, different transport |
| **Type dependency** | Keep `agent_pool::Response` |
| **LOC Changed** | ~40 lines in runner.rs |
| **Breaking** | No external API changes |
