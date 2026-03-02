# CLI Review

Review of `agent_pool` CLI commands, options, and public API for cleanup.

## Commands

### `start`
Start the agent pool server.

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool <POOL>` | String | Generated ID | Pool ID or path |
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory for pools |
| `-l, --log-level` | Enum | `info` | off/error/warn/info/debug/trace |
| `--json` | Flag | false | Output pool info as JSON |
| `--idle-timeout-secs` | u64 | 180 | Heartbeat timeout for idle workers |
| `--task-timeout-secs` | u64 | 300 | Default task timeout |
| `--no-heartbeat` | Flag | false | Disable heartbeats |
| `--stop` | Flag | false | Stop existing daemon before starting |

**Questions:**
- [ ] Do we need both `--pool` and `--pool-root`? Could simplify to just `--pool` being a path.
- [ ] Is `--json` used anywhere? What does it output?
- [ ] `--stop` is a convenience flag - keep or remove?

---

### `stop`
Stop a running agent pool server.

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool <POOL>` | String | Required | Pool ID or path |
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory for pools |

**Questions:**
- [ ] Same `--pool` vs `--pool-root` question

---

### `submit_task`
Submit a task and wait for the result.

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool <POOL>` | String | Required | Pool ID or path |
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory |
| `--data <DATA>` | String | None | Inline task JSON |
| `--file <FILE>` | Path | None | File containing task JSON |
| `--notify <NOTIFY>` | Enum | `socket` | socket/file notification mechanism |
| `--timeout-secs` | u64 | 300 (file) | Timeout in seconds |

**Questions:**
- [ ] `--notify` is implementation detail - should it be exposed?
- [ ] `--data` vs `--file` - both needed?

---

### `list`
List all pools.

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory |

**Questions:**
- [ ] Is this command used?

---

### `cleanup`
Clean up stopped pools.

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory |

**Questions:**
- [ ] Is this command used?

---

### `protocol`
Print the agent protocol documentation.

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool <POOL>` | String | None | Pool ID to include in instructions |
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory |
| `--low-level` | Flag | false | Show low-level file/socket protocol |

**Questions:**
- [ ] Is this command used? By who?

---

### `deregister_agent` ⚠️ DEPRECATED
Deregister an agent from the pool.

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool <POOL>` | String | Required | Pool ID or path |
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory |
| `--name <NAME>` | String | Required | Agent name |

**Status:** Marked deprecated. Workers are now anonymous. **Should be removed.**

---

### `get_task`
Wait for and return the next task (for agents).

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool <POOL>` | String | Required | Pool ID or path |
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory |
| `--name <NAME>` | String | None | Agent name (for debugging) |
| `-l, --log-level` | Enum | `off` | Log level |

**Questions:**
- [ ] Redundant with `register`?

---

### `register`
Register as an agent and wait for first task (alias for `get_task`).

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool <POOL>` | String | Required | Pool ID or path |
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory |
| `--name <NAME>` | String | None | Agent name (for debugging) |
| `-l, --log-level` | Enum | `off` | Log level |

**Questions:**
- [ ] Keep both `register` and `get_task`? Or just one?

---

### `next_task`
Submit response to current task and wait for next task.

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool <POOL>` | String | Required | Pool ID or path |
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Base directory |
| `--response-file <FILE>` | Path | Required | Response file from get_task |
| `--data <DATA>` | String | None | Inline response content |
| `--file <FILE>` | Path | None | File containing response |
| `--name <NAME>` | String | None | Agent name (for debugging) |
| `-l, --log-level` | Enum | `off` | Log level |
| `--deregister` | Flag | false | Submit and exit (don't wait) |

**Questions:**
- [ ] `--deregister` flag name is confusing now that workers are anonymous

---

## Global Options

| Option | Type | Default | Notes |
|--------|------|---------|-------|
| `--pool-root <POOL_ROOT>` | Path | `/tmp/agent_pool` | Repeated on every command |

---

## Public API (lib.rs exports)

```rust
// Constants
pub use constants::{AGENTS_DIR, RESPONSE_FILE, STATUS_FILE, TASK_FILE, response_path};

// Daemon
pub use daemon::{DaemonConfig, run_with_config};

// Lock
pub use lock::is_daemon_running;

// Pool utilities
pub use pool::{
    cleanup_stopped, default_pool_root, generate_id, id_to_path, list_pools, resolve_pool,
};

// Response type
pub use response::Response;

// Submit functions
pub use submit::{
    Payload, stop, submit, submit_file, submit_file_with_timeout, wait_for_pool_ready,
};

// Transport
pub use transport::Transport;

// Worker utilities (for agents)
pub use worker::{TaskAssignment, wait_for_task, write_response};
```

### Used by GSD

GSD (`gsd_config/runner.rs`) uses these internal APIs:
- `agent_pool::resolve_pool(&root, &id)` - resolve pool ID to path
- `agent_pool::default_pool_root()` - get default pool root
- `agent_pool::is_daemon_running(&path)` - check if daemon running
- `agent_pool::submit(&root, &payload)` - submit task (blocking)
- `agent_pool::Response` - response enum
- `agent_pool::Payload::inline(&json)` - create inline payload

### Questions

- [ ] Should GSD use CLI commands instead of internal APIs?
- [ ] Which exports are actually needed for external use?
- [ ] Constants like `AGENTS_DIR`, `TASK_FILE` - internal implementation details?

---

## Recommendations

### Remove
1. `deregister_agent` - deprecated, workers are anonymous

### Rename/Clarify
1. `--deregister` flag on `next_task` - confusing name
2. Consider merging `register` and `get_task`

### Simplify
1. `--pool` vs `--pool-root` - could be simplified
2. `--notify` on `submit_task` - implementation detail

### Review for usage
1. `list` - is it used?
2. `cleanup` - is it used?
3. `protocol` - is it used?
