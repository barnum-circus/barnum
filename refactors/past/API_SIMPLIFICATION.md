# API Simplification Analysis

## Status: COMPLETE

Phases 1 and 2 are done. Phase 3 (rename File → Poll) is optional and low priority.

## Current State

### CLI Commands (11 total)

| Command | Purpose | Used by |
|---------|---------|---------|
| `start` | Start daemon | Users/scripts |
| `stop` | Stop daemon | Users/scripts |
| `submit_task` | Submit task, wait for result | Submitters |
| `register` | Join pool, get first task | Agents |
| `next_task` | Submit response, get next task | Agents |
| `deregister_agent` | Leave pool | Agents (external) |
| `list` | List pools | Users |
| `cleanup` | Clean stale pools | Users |
| `protocol` | Print protocol docs | Users |
| `help` | Help | Users |

### Submission Modes

**DataSource** (how content is delivered):
- `Inline` - Content inline in command (`--data`)
- `FileReference` - Content in separate file (`--file`)

**NotifyMethod** (how to wait for response):
- `Socket` - IPC socket (fast, default)
- `File` - File-based polling (sandbox fallback)
- `Raw` - Direct file protocol, no CLI

This creates a 2×3 = 6 mode matrix for tests.

### Agent Protocol

Agents cycle through:
```
register → (next_task --data <response>)* → deregister_agent
```

---

## Problems

### 1. Raw Mode is Redundant

`NotifyMethod::Raw` calls `submit_file()` directly, bypassing CLI. This exists only for testing the low-level file protocol.

**Issues:**
- Exposes implementation detail in public API
- Creates additional test surface without user benefit
- Tests using Raw don't exercise CLI code paths
- Confusing distinction between "File" (CLI with file notify) and "Raw" (direct file protocol)

**Recommendation:** Remove `Raw` from public API. Keep `submit_file()` as internal implementation detail. Tests should use CLI modes only.

### 2. DataSource Complexity

`--data` vs `--file` distinction exists because:
- Large payloads may exceed command line limits
- File references let daemon read from submitter's filesystem

**Issues:**
- `--file` passes a path that daemon must be able to read
- If submitter and daemon don't share filesystem, `--file` fails silently
- The distinction leaks into the internal protocol (Inline vs FileReference payloads)

**Recommendation:** Keep for now, but document the shared filesystem requirement. Consider auto-detecting and falling back to inline if file is small enough.

### 3. Agent Command Lifecycle is Awkward

Current lifecycle:
```
register → next_task → next_task → ... → deregister_agent
```

**Issues:**
- `deregister_agent` is a separate command (can't submit final response AND leave)
- To submit final response then leave requires two CLI calls
- External deregistration (daemon kicks agent) can happen mid-task

**Related TODOs in todos.md:**
- "Agent CLI: Respond-and-Deregister / Abort Support"
- "Rethink Agent Deregistration"

### 4. NotifyMethod Naming

- `Socket` = IPC socket-based submission (fast)
- `File` = CLI submission with file-based response wait (slow)

The name "File" is confusing because both use files - the distinction is notification mechanism.

**Better names:**
- `Socket` → `Ipc` or keep as-is
- `File` → `Poll` or `Blocking`

---

## Proposed Simplifications

### Phase 1: Remove Raw Mode from Tests

**Impact:** Low risk, reduces test matrix from 6 to 4 modes.

1. Remove `NotifyMethod::Raw` enum variant
2. Remove `submit_raw()` function from test utils
3. Update all `#[rstest]` test cases to only use Socket/File
4. Keep `submit_file()` library function (used by File mode internally)

**Test matrix after:**
| DataSource | NotifyMethod | Description |
|------------|--------------|-------------|
| Inline | Socket | `--data --notify socket` |
| Inline | File | `--data --notify file` |
| FileReference | Socket | `--file --notify socket` |
| FileReference | File | `--file --notify file` |

### ~~Phase 2: Consolidate Agent Commands~~ ✓ DONE

Implemented `--deregister` flag on `next_task`:
```bash
# Submit final response and deregister in one command
agent_pool next_task --pool $POOL --name $NAME --data '{"result": "done"}' --deregister
```

- `--deregister` waits for daemon to acknowledge the response before deregistering
- `deregister_agent` remains for emergency abort (external kick or immediate shutdown)

### Phase 3: Rename File → Poll (Optional)

Low priority cosmetic change. Would require updating:
- CLI flag values
- Documentation
- All scripts using `--notify file`

---

## Related TODOs to Consolidate

From `todos.md`:

1. **Agent CLI: Respond-and-Deregister / Abort Support** → Phase 2 above
2. **Rethink Agent Deregistration** → Broader design question, keep separate
3. **Socket-Based Agent Notifications** → Future enhancement, keep separate
4. **Replace Polling with Notify in File Transport** → Performance fix, keep separate

---

## Implementation Order

1. ~~**Remove Raw mode**~~ ✓ DONE - Completed: Removed `NotifyMethod::Raw` and `submit_raw()`
2. ~~**Add --deregister to next_task**~~ ✓ DONE - Simplifies agent shutdown
3. **(Optional) Rename File → Poll** - Only if we're doing breaking changes anyway

---

## Questions

1. **Should Raw mode be removed entirely or just hidden from tests?**
   - Recommendation: Remove from test matrix, keep `submit_file()` as internal API for File mode.

2. **Should FileReference be removed?**
   - No. It's genuinely useful for large payloads. Just document the shared filesystem requirement.

3. **Should we combine register + next_task into a single streaming command?**
   - Future consideration. Would require protocol changes. Out of scope for now.
