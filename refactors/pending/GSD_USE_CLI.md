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

## Current State

### Internal API Usage in `gsd_config/runner.rs`

```rust
// Line 13: Import Response type
use agent_pool::Response;

// Line 96: Resolve pool path
let pool_path = pool.map_or_else(
    || { ... },
    |p| agent_pool::resolve_pool(&agent_pool::default_pool_root(), &p),
);

// Line 200: Check if daemon is running
if !agent_pool::is_daemon_running(pool_path) { ... }

// Lines 353-354: Submit task and get response
let result = agent_pool::submit(&root, &agent_pool::Payload::inline(&payload));
// result: io::Result<Response>

// Lines 672-686: Match on Response enum
match response {
    Response::Processed { stdout, .. } => { ... }
    Response::NotProcessed { reason } => { ... }
}
```

### What the CLI Provides

The `submit_task` command already does what GSD needs:

```bash
agent_pool submit_task \
  --pool /path/to/pool \
  --data '{"task": ..., "instructions": ...}' \
  --timeout-secs 60
```

Output is JSON:
```json
{"kind": "Processed", "stdout": "..."}
// or
{"kind": "NotProcessed", "reason": "timeout"}
```

## Proposed Changes

### Before (Internal API)

```rust
// runner.rs

use agent_pool::Response;

// In TaskRunner::new()
if !agent_pool::is_daemon_running(pool_path) {
    return Err(...);
}

// In spawn thread
let result = agent_pool::submit(&root, &agent_pool::Payload::inline(&payload));
let _ = tx.send(InFlightResult {
    result: SubmitResult::Pool(result),
    ...
});

// In process_result()
match result {
    SubmitResult::Pool(Ok(response)) => process_pool_response(response, ...),
    SubmitResult::Pool(Err(e)) => { ... }
}
```

### After (CLI)

```rust
// runner.rs

use std::process::Command;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(tag = "kind")]
enum CliResponse {
    Processed { stdout: String },
    NotProcessed { reason: String },
}

// In TaskRunner::new()
// Check daemon by checking status file exists and contains "ready"
let status_path = pool_path.join("status");
if !status_path.exists() || fs::read_to_string(&status_path)?.trim() != "ready" {
    return Err(...);
}

// In spawn thread
let output = Command::new("agent_pool")
    .args(["submit_task", "--pool", &root.display().to_string()])
    .arg("--data")
    .arg(&payload)
    .arg("--timeout-secs")
    .arg(&timeout.to_string())
    .output()?;

let response: CliResponse = serde_json::from_slice(&output.stdout)?;

let _ = tx.send(InFlightResult {
    result: SubmitResult::Pool(response),
    ...
});
```

## Architectural Implications

### Pros

1. **Decoupling** - `gsd_config` no longer depends on `agent_pool` internals
2. **CLI validation** - We discover CLI gaps before users do
3. **Consistent interface** - Same interface for Rust and non-Rust users
4. **Easier testing** - Can mock CLI responses with fake binaries

### Cons

1. **Process overhead** - Spawning processes instead of function calls
2. **Binary dependency** - Need `agent_pool` binary in PATH or specified
3. **Error handling** - Must parse CLI output/errors instead of Rust types
4. **Timeout handling** - CLI has its own timeout; need to coordinate

### Performance Impact

Each task submission spawns a process. For GSD's use case (agent tasks that take seconds to minutes), the ~10ms process spawn overhead is negligible.

## Complexity Analysis

**This is a moderate change.** The core logic stays the same; we're just changing the transport layer.

### Changes Required

1. **Remove `agent_pool` dependency from `gsd_config/Cargo.toml`**
   - Or keep it but only for types (Response enum)

2. **Update `runner.rs`**
   - Replace `agent_pool::submit()` with `Command::new("agent_pool")`
   - Replace `agent_pool::is_daemon_running()` with status file check
   - Parse JSON output instead of using Rust types directly

3. **Binary resolution**
   - Add config option for `agent_pool` binary path
   - Or require it in PATH
   - Or embed path from build time

4. **Error handling**
   - Parse stderr for error messages
   - Handle non-zero exit codes

### Lines of Code

- `runner.rs`: ~50 lines changed (transport code)
- New: ~20 lines for CLI response parsing
- Remove: `use agent_pool::*` imports

## Missing Tests

Before this refactor, we should ensure test coverage for:

### GSD Tests (`gsd_config/tests/`)

| Test File | What it Tests | Uses Internal API? |
|-----------|--------------|-------------------|
| `simple_termination.rs` | Basic task completion | Yes, via `run()` |
| `linear_transitions.rs` | A -> B -> C flow | Yes |
| `branching_transitions.rs` | A -> [B, C] fan-out | Yes |
| `concurrency.rs` | Parallel task execution | Yes |
| `retry_behavior.rs` | Retry on timeout/error | Yes |
| `schema_validation.rs` | Input/output schemas | Yes |
| `invalid_transitions.rs` | Invalid step references | Yes |
| `edge_cases.rs` | Edge cases | Yes |

**All tests use internal APIs via `TaskRunner::new()` and `run()`.**

### What's Missing

1. **CLI integration tests** - No tests that invoke `gsd run` as a subprocess
2. **Error path tests** - What happens when CLI fails?
3. **Timeout coordination tests** - CLI timeout vs GSD timeout

### Tests to Add Before Refactor

```rust
// gsd_config/tests/cli_integration.rs

#[test]
fn gsd_run_via_cli() {
    // Start agent pool
    // Start agent
    // Run `gsd run config.json --initial '[...]' --pool /path`
    // Verify output
}

#[test]
fn submit_task_returns_processed() {
    // Start pool + agent
    // Run `agent_pool submit_task --pool ... --data '...'`
    // Parse JSON output, verify structure
}

#[test]
fn submit_task_returns_not_processed_on_timeout() {
    // Start pool (no agent)
    // Run `agent_pool submit_task --timeout-secs 1 ...`
    // Verify NotProcessed response
}
```

## Implementation Plan

### Phase 1: Add Missing Tests (First)

1. Add CLI integration test for `agent_pool submit_task`
2. Add CLI integration test for `gsd run`
3. Verify all existing tests pass

### Phase 2: Refactor

1. Define `CliResponse` enum in `runner.rs`
2. Add helper function `submit_via_cli(pool: &Path, payload: &str, timeout: u64) -> io::Result<CliResponse>`
3. Replace `agent_pool::submit()` call with `submit_via_cli()`
4. Replace `agent_pool::is_daemon_running()` with status file check
5. Update `gsd_config/Cargo.toml` to remove internal API dependency (keep types if needed)

### Phase 3: Cleanup

1. Remove unused `agent_pool` public exports
2. Update documentation

## Open Questions

1. **Binary path resolution** - How do we find `agent_pool` binary?
   - Require in PATH?
   - Config option?
   - Relative to `gsd` binary?

2. **Keep types dependency?** - Should `gsd_config` still depend on `agent_pool` for the `Response` type?
   - Pro: Type sharing, no duplication
   - Con: Still creates crate dependency

3. **Timeout handling** - CLI has `--timeout-secs`. GSD has per-step timeout. How to coordinate?
   - Pass GSD timeout to CLI?
   - CLI timeout = GSD timeout + buffer?

---

## Summary

| Aspect | Assessment |
|--------|-----------|
| **Complexity** | Moderate - transport change, not logic change |
| **Risk** | Low - existing tests cover behavior |
| **Blockers** | Need CLI integration tests first |
| **LOC Changed** | ~70 lines in runner.rs |
| **Breaking** | No external API changes |
