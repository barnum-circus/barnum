# High-Impact Refactors and Cleanups

An analysis of the highest-impact improvements, considering effort vs. benefit.

---

## Tier 1: High Impact, Low Effort

### ~~1. Remove Raw Test Mode~~ ✓ DONE

~~**Impact:** Reduces test matrix by 33% (6 → 4 modes), faster CI, simpler test code.~~
~~**Effort:** ~30 minutes.~~
~~**Risk:** None - Raw mode has no users outside tests.~~

Completed: Removed `NotifyMethod::Raw` variant and `submit_raw()` function. Test matrix reduced from 6 to 4 combinations.

### 2. Replace Polling in submit_file.rs

**Impact:** File-based submissions become responsive instead of polling every 100ms.
**Effort:** ~1 hour.
**Risk:** Low - isolated change to one file.

Current code polls for response:
```rust
loop {
    if response_path.exists() { ... }
    thread::sleep(POLL_INTERVAL);  // 100ms
}
```

Should use notify watcher:
```rust
let (tx, rx) = mpsc::channel();
let mut watcher = RecommendedWatcher::new(move |event| {
    if event matches response file { tx.send(()); }
})?;
watcher.watch(pending_dir)?;
rx.recv_timeout(timeout)?;
```

### ~~3. Add --deregister to next_task~~ ✓ DONE

~~**Impact:** Simplifies agent scripts (one command instead of two for graceful exit).~~
~~**Effort:** ~30 minutes.~~
~~**Risk:** None - additive change.~~

Completed: Added `--deregister` flag to `next_task` command.

---

## Tier 2: High Impact, Medium Effort

### 4. Re-enable Multi-Threaded Tests

**Impact:** Faster CI (currently ~2 minutes with --test-threads=1).
**Effort:** 2-4 hours investigation, unknown fix time.
**Risk:** Medium - root cause unclear.

The issue is CLI spawn overhead. Potential solutions:
1. **Connection pooling** - Keep daemon connections open across test cases
2. **Batch mode** - Submit multiple tasks in one CLI call
3. **In-process testing** - Use library API instead of CLI for some tests
4. **Reduce test count** - Remove redundant test cases

Investigation needed to understand where time goes.

### ~~5. Anonymous Worker Model~~ ✓ DONE

~~**Impact:** Simpler agent protocol, eliminates directory-per-agent overhead.~~
~~**Effort:** 4-8 hours.~~
~~**Risk:** Medium - protocol change affects agent scripts.~~

Completed: Agents now use `get_task` which returns task directly. No persistent identity, no agent directories. Removed `register`, `next_task`, `deregister_agent` commands.

### 6. Clean Shutdown

**Impact:** No stale pool directories, cleaner user experience.
**Effort:** 2-3 hours.
**Risk:** Low - additive behavior.

On `agent_pool stop` or SIGTERM:
1. Mark pool as shutting down
2. Wait for in-flight tasks (with timeout)
3. Remove pool directory

Currently pools leave orphaned directories that require manual `cleanup`.

---

## Tier 3: Medium Impact, Medium Effort

### 7. Sync Testing Harness

**Impact:** Deterministic tests, faster execution, easier debugging.
**Effort:** 8-16 hours.
**Risk:** Low - new test infrastructure, doesn't replace existing tests.

See `SYNC_TESTING_HARNESS.md`. In-memory testing without real I/O.

Worth doing after Tier 1-2 items. Would help with:
- Testing edge cases (timeouts, crashes)
- Debugging flaky tests
- Testing protocol changes in isolation

### 8. Socket-Based Agent Protocol

**Impact:** Faster task dispatch (no file I/O for agents).
**Effort:** 8-16 hours.
**Risk:** Medium - significant protocol change.

Currently agents poll files for tasks. Socket-based:
- Daemon pushes tasks to connected agents
- No `task.json` / `response.json` files
- Faster, lower latency

Requires careful design for reconnection and failure handling.

### 9. Documentation Improvements

**Impact:** Better onboarding, fewer user questions.
**Effort:** 2-4 hours.
**Risk:** None.

- Document Linux vs macOS differences
- Add architecture overview
- Improve error messages
- Add troubleshooting guide

---

## Tier 4: Lower Priority

### 10. KQueue Investigation

**Impact:** Potentially faster file watching on macOS.
**Effort:** 2-4 hours investigation.
**Risk:** Low.

May not be worth it - FSEvents works fine. Only investigate if file watching becomes a bottleneck.

### 11. Rename pending → submissions

**Impact:** Clearer naming.
**Effort:** 1-2 hours (find/replace + update docs).
**Risk:** Low - internal change, no protocol impact.

Cosmetic cleanup, do opportunistically.

### 12. GSD Multi-Pool Support

**Impact:** Enable workflows spanning multiple pools.
**Effort:** 4-8 hours.
**Risk:** Low - additive feature.

See `todos.md`. Allows mixing AI agents with command pools in same workflow.

---

## Recommended Order

### Immediate
1. ~~Remove Raw test mode~~ ✓ DONE
2. Replace polling in submit_file.rs
3. ~~Add --deregister to next_task~~ ✓ DONE

### Short Term
4. Investigate multi-threaded test timeouts
5. Clean shutdown implementation

### Medium Term
6. ~~Anonymous worker model~~ ✓ DONE
7. Sync testing harness

### Long Term / As Needed
8. Socket-based agent protocol
9. Documentation improvements
10. Everything else

---

## What NOT to Do

### Over-engineer the transport layer

The current file-based protocol is simple and works. Socket-based agents would be faster but adds complexity. Only pursue if latency becomes a real problem.

### Premature optimization

KQueue, connection pooling, etc. - measure first. The current system handles reasonable workloads fine.

### Break backward compatibility unnecessarily

Agent scripts depend on current CLI. Changes like renaming `--notify file` to `--notify poll` have cost but little benefit.

---

## Metrics to Track

Before optimizing, establish baselines:

1. **Test suite time** - `time cargo test --workspace -- --test-threads=1`
2. **Single task latency** - Time from submit to response
3. **Throughput** - Tasks per second with N agents
4. **CLI spawn overhead** - Time to run `agent_pool --help`

Optimize what matters, not what's theoretically improvable.
