# FS Watcher / File Protocol Cleanup

## Problem Summary

Tests pass on macOS (FSEvents) but fail/hang on Linux (inotify). The issue stems from how inotify handles recursive watching of newly-created subdirectories.

## Root Cause Analysis

**inotify race condition**: When watching a directory recursively with inotify:
1. A new subdirectory is created (e.g., `pending/<uuid>/`)
2. inotify needs to add a watch for the new subdirectory
3. If files are written to the subdirectory before the watch is added, events are missed

**FSEvents doesn't have this problem** because it watches at the filesystem level, not per-directory.

## Current Architecture Issues

### 1. Nested directories in `pending/`

Each submission creates `pending/<uuid>/task.json`. This means EVERY submission triggers the inotify race:
- Client creates `pending/<uuid>/`
- Client writes `pending/<uuid>/task.json`
- If (2) happens before inotify watches `<uuid>/`, event is missed

**Suggested fix**: Flatten the structure to `pending/<uuid>.json` (single file, no subdirectory).

**Trade-offs**:
- Pro: Eliminates per-submission race
- Con: Requires protocol change
- Con: Response handling becomes trickier (currently uses `response.json` in same dir)

### 2. Three notification methods with different reliability

Currently:
- `Socket`: Client sends socket message → daemon notified directly → **reliable**
- `File` (CLI): Uses socket for notification but file for response
- `Raw`: Pure file-based, relies entirely on FS watcher → **unreliable on Linux**

**Suggested fix**: Consider deprecating `Raw` or adding a polling fallback.

### 3. Temp file pattern in Transport::write() (FIXED)

We removed the atomic write pattern (temp file + rename). However, FSEvents still shows `.task.json.tmp` events in local logs, suggesting there may be another code path or stale binaries.

**Action**: Verify no temp files are being created anywhere.

### 4. Watcher sync only proves startup readiness

The watcher sync with canary file proves the watcher is working AT STARTUP. It doesn't help with per-submission races.

**Suggested fix**: Could add periodic polling of `pending/` to catch missed events, or bring back PendingDir fallback with proper deduplication.

## Proposed Cleanup Tasks

### Task 1: Flatten pending directory structure (protocol change)

Instead of:
```
pending/<uuid>/task.json
pending/<uuid>/response.json
```

Use:
```
pending/<uuid>.task.json
pending/<uuid>.response.json
```

This eliminates the subdirectory creation race entirely.

### Task 2: Audit all temp file usage

Search for any remaining temp file patterns:
- `tempfile::`
- `.tmp`
- Rename patterns

Ensure we're writing directly to final locations.

### Task 3: Add PendingDir fallback with deduplication

When we see a `PendingDir` event:
1. Check if `task.json` exists
2. If yes, call `register_pending_task`
3. The existing deduplication via `get_id_by_path` should prevent double registration

Need to verify deduplication is actually working.

### Task 4: Consider polling fallback for file protocol

For `submit_file`, add a fallback that polls `pending/<uuid>/response.json` if the FS watcher doesn't see it within N seconds.

### Task 5: Document Linux vs macOS differences

Add documentation about:
- inotify vs FSEvents behavior differences
- Why tests might pass locally but fail in CI
- Recommended testing approach (run in Linux Docker container)

### Task 6: Add inotify-specific handling

Consider detecting Linux and using different strategies:
- More aggressive polling
- Different watcher configuration
- Or just accept that file protocol is less reliable on Linux

### Task 7: Improve agent watcher

Current state (workaround committed):
- Agent watches entire directory with `RecursiveMode::NonRecursive`
- Filters for `Close(Write)` events only to avoid reading empty files
- Gets events for both `task.json` and `response.json`

Better approach:
- Watch specific file (`task.json`) instead of whole directory
- Or flatten agent structure like submissions: `agents/<name>.task.json`, `agents/<name>.response.json`
- Consider if agents even need subdirectories or if flat files would work

## Quick Wins (Immediate Actions)

1. **Verify temp files are gone**: Run tests on Linux and check logs for `.tmp` files
2. **Add logging around task registration**: Make it clearer when/why tasks are registered
3. **Test deduplication**: Add a test that explicitly tests double-registration prevention
4. **Increase test timeout logging**: When tests time out, dump state of pending/ and agents/

## Questions to Answer

1. Is the flat file structure (`<uuid>.json` vs `<uuid>/task.json`) worth the protocol change?
2. Should we deprecate `NotifyMethod::Raw` on Linux?
3. Is polling acceptable as a fallback, or should we invest in fixing the FS watcher?
