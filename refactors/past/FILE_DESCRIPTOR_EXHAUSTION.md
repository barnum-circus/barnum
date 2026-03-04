# File Descriptor Exhaustion Investigation

## Status: ROOT CAUSE IDENTIFIED, FIX IMPLEMENTED

## Symptoms

Agents running in the sandbox fail with:
```
Failed to create watcher: [E044] failed to create filesystem watcher: No file descriptors available (os error 24)
```

This happens when calling `get_task` - the watcher creation fails before any work begins.

## Context

- Error code 24 = EMFILE (too many open files)
- Host system showed 88117 open file descriptors (`lsof | wc -l` outside container)
- `lsof` not available in sandbox, so can't diagnose there directly
- Agents run via `pnpm dlx @gsd-now/agent-pool@main get_task ...`

## Possible Causes

### 1. Our Code is Leaking (needs investigation)

- Each `get_task` invocation creates a `VerifiedWatcher` (inotify on Linux)
- Watcher should be dropped when CLI process exits
- BUT: If processes are hanging or not exiting cleanly, watchers accumulate
- Check: Are agent processes staying alive unexpectedly?

### 2. Sandbox FD Limit is Low

- Sandbox environments often have restricted ulimits
- Many concurrent agent processes could exhaust a low limit quickly
- Each `pnpm dlx` invocation may also hold FDs open

### 3. External FD Pressure

- 88117 FDs at host level is high but may be normal
- Other processes on devapp could be consuming FDs
- Docker overlays visible in lsof warnings suggest complex environment

## Code Paths to Investigate

1. `agent_pool_cli/src/main.rs` - `GetTask` command creates watcher at line 441
2. `agent_pool/src/verified_watcher.rs` - `VerifiedWatcher::new()` creates inotify watch
3. `agent_pool/src/daemon/wiring.rs` - Daemon creates ONE watcher at startup (line 185)

## Current Understanding

- Daemon: Creates exactly one watcher, keeps it for daemon lifetime (not leaking)
- CLI get_task: Creates one watcher per invocation, should drop on exit
- CLI submit_task: Creates one watcher per invocation, should drop on exit

The watcher lifecycle looks correct - watchers are created, used, and dropped when the process exits. This suggests either:
- Processes aren't exiting cleanly
- Sandbox has very low FD limits
- External pressure from other processes

## Potential Fixes (if we confirm a leak)

1. **Graceful degradation**: If watcher creation fails with EMFILE, fall back to polling
2. **Add `--poll` flag**: Let agents opt into polling mode (no watcher)
3. **Long-running agent**: Keep single agent process, reuse one watcher
4. **Connection pooling**: Share watchers across operations

## Next Steps

1. Restart devapp and see if issue resolves (rules out accumulated leak)
2. Monitor FD count over time during agent operation
3. Check if agent processes are staying alive unexpectedly
4. Investigate sandbox ulimit settings
5. Consider adding `--poll` mode for constrained environments

## ROOT CAUSE FOUND

The issue is NOT a leak - it's **unbounded concurrency** in GSD's task submission.

### The Problem

```rust
// gsd_config/src/runner.rs:198
let max_concurrency = config.options.max_concurrency.unwrap_or(usize::MAX);
```

Default was unlimited. When GSD processes 200 files:
1. Spawns 200 threads
2. Each calls `submit_via_cli` → `agent_pool submit_task`
3. Each `submit_task` process creates an inotify watcher
4. 200 watchers > 128 `max_user_instances` limit → EMFILE

With only 3 agents, at most 3 tasks are actively processed. The other 197 submissions just sit there holding inotify instances while waiting for agents.

### System Limits

```
$ cat /proc/sys/fs/inotify/max_user_instances
128
```

This is the bottleneck - only 128 inotify instances allowed per user.

### Fix Implemented

Changed default to 20:

```rust
// Default to 20 concurrent submissions to avoid exhausting inotify instances.
// Each submit_task process creates an inotify watcher, and Linux defaults to
// max_user_instances=128. With 3 agents, only ~5 submissions can be actively
// processed at once anyway - the rest just queue up holding watchers.
// TODO: Query the pool for actual agent count and use that + small buffer.
let max_concurrency = config.options.max_concurrency.unwrap_or(20);
```

### Why 20?

- 3 agents = 3 tasks actively processed at once
- Ideal: `max_concurrency = agent_count + 2` (small buffer for pipeline smoothness)
- 20 is conservative and leaves headroom for:
  - Daemon watcher (1)
  - Agent watchers (3)
  - Other system processes
  - Future scaling

### Pending Refactors Don't Help

Checked `CONCURRENT_FILE_SUBMISSION_FIX.md` and `CANCELLABLE_WAIT_FOR_TASK.md` - neither reduces watcher count. They address different issues (race conditions, cancellation) but still create one watcher per operation.

### Future Improvement

Query the daemon for actual agent count and auto-tune:
```rust
let agent_count = query_pool_agent_count(&pool_root)?;
let max_concurrency = config.options.max_concurrency
    .unwrap_or(agent_count + 2);
```

## Related

- Error ID: E044
- File: `crates/agent_pool/src/verified_watcher.rs:188`
- Fix: `crates/gsd_config/src/runner.rs:198`
