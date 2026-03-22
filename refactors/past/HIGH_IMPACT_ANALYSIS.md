# High-Impact Refactors and Cleanups

An analysis of the highest-impact improvements, considering effort vs. benefit.

---

## Tier 1: High Impact, Low Effort

*All Tier 1 items completed.*

---

## Tier 2: High Impact, Medium Effort

### 1. Re-enable Multi-Threaded Tests

**Impact:** Faster CI (currently ~2 minutes with --test-threads=1).
**Effort:** 2-4 hours investigation, unknown fix time.
**Risk:** Medium - root cause unclear.

The issue is CLI spawn overhead. Potential solutions:
1. **Connection pooling** - Keep daemon connections open across test cases
2. **Batch mode** - Submit multiple tasks in one CLI call
3. **In-process testing** - Use library API instead of CLI for some tests
4. **Reduce test count** - Remove redundant test cases

Investigation needed to understand where time goes.

---

## Tier 3: Medium Impact, Medium Effort

### 2. Sync Testing Harness

**Impact:** Deterministic tests, faster execution, easier debugging.
**Effort:** 8-16 hours.
**Risk:** Low - new test infrastructure, doesn't replace existing tests.

See `SYNC_TESTING_HARNESS.md`. In-memory testing without real I/O.

Would help with:
- Testing edge cases (timeouts, crashes)
- Debugging flaky tests
- Testing protocol changes in isolation

### 3. Socket-Based Agent Protocol

**Impact:** Faster task dispatch (no file I/O for agents).
**Effort:** 8-16 hours.
**Risk:** Medium - significant protocol change.

Currently agents use files for tasks. Socket-based:
- Daemon pushes tasks to connected agents
- No task/response files for agents
- Faster, lower latency

Requires careful design for reconnection and failure handling.

### 4. Documentation Improvements

**Impact:** Better onboarding, fewer user questions.
**Effort:** 2-4 hours.
**Risk:** None.

- Document Linux vs macOS differences
- Add architecture overview
- Improve error messages
- Add troubleshooting guide

---

## Tier 4: Lower Priority

### 5. KQueue Investigation

**Impact:** Potentially faster file watching on macOS.
**Effort:** 2-4 hours investigation.
**Risk:** Low.

May not be worth it - FSEvents works fine. Only investigate if file watching becomes a bottleneck.

### 6. Barnum Multi-Pool Support

**Impact:** Enable workflows spanning multiple pools.
**Effort:** 4-8 hours.
**Risk:** Low - additive feature.

See `todos.md`. Allows mixing AI agents with command pools in same workflow.

---

## Recommended Order

### Short Term
1. Investigate multi-threaded test timeouts

### Medium Term
2. Sync testing harness

### Long Term / As Needed
3. Socket-based agent protocol
4. Documentation improvements
5. Everything else

---

## What NOT to Do

### Over-engineer the transport layer

The current file-based protocol is simple and works. Socket-based agents would be faster but adds complexity. Only pursue if latency becomes a real problem.

### Premature optimization

KQueue, connection pooling, etc. - measure first. The current system handles reasonable workloads fine.

### Break backward compatibility unnecessarily

Agent scripts depend on current CLI. Changes like renaming flags have cost but little benefit.

---

## Metrics to Track

Before optimizing, establish baselines:

1. **Test suite time** - `time cargo test --workspace -- --test-threads=1`
2. **Single task latency** - Time from submit to response
3. **Throughput** - Tasks per second with N agents
4. **CLI spawn overhead** - Time to run `troupe --help`

Optimize what matters, not what's theoretically improvable.
