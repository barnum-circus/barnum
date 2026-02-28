# FS Watcher / File Protocol Cleanup

## Status: MOSTLY COMPLETE

The primary issue (inotify race) was fixed by flattening the submissions directory structure. See `../past/INOTIFY_RACE_ANALYSIS.md` for details.

**Remaining work:**
- Replace polling with notify in `submit_file.rs` (see `todos.md`)
- Document Linux vs macOS differences (low priority)

---

## Problem Summary (FIXED)

Tests passed on macOS (FSEvents) but failed/hung on Linux (inotify). The issue stemmed from how inotify handles recursive watching of newly-created subdirectories.

## Root Cause Analysis

**inotify race condition**: When watching a directory recursively with inotify:
1. A new subdirectory is created (e.g., `pending/<uuid>/`)
2. inotify needs to add a watch for the new subdirectory
3. If files are written to the subdirectory before the watch is added, events are missed

**FSEvents doesn't have this problem** because it watches at the filesystem level, not per-directory.

**FIXED:** By flattening submissions to `<id>.request.json` files, we eliminated the subdirectory creation entirely.

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

## Cleanup Tasks

### Task 1: Flatten pending directory structure - **DONE**

Now uses:
```
pending/<uuid>.request.json
pending/<uuid>.response.json
```

This eliminated the subdirectory creation race entirely.

### Task 2: Audit all temp file usage - **DONE**

Atomic writes now use temp files in `/tmp` with UUID-based names, then rename to final location. This is intentional and correct - the rename generates a `Modify(Name)` event that watchers handle.

### Task 3: PendingDir fallback - **OBSOLETE**

No longer needed since we flattened the structure.

### Task 4: Replace polling with notify in submit_file - **TODO**

`submit_file.rs` still polls every 100ms for `response.json`. Should use file watcher instead. Tracked in `todos.md`.

### Task 5: Document Linux vs macOS differences - **LOW PRIORITY**

Could be useful but not blocking anything now that the race is fixed.

### Task 6: inotify-specific handling - **NOT NEEDED**

The flat file structure works on both platforms. No special handling required.

### Task 7: Improve agent watcher - **FUTURE**

Agent structure could be flattened like submissions. Low priority - agents work fine as-is since the directory exists before files are written (causal chain prevents race).

## Quick Wins - **DONE**

1. ~~Verify temp files are gone~~ - Temp files are intentional for atomic writes
2. ~~Add logging around task registration~~ - Debug logging added
3. ~~Test deduplication~~ - Working correctly with flat structure
4. ~~Increase test timeout logging~~ - Tests pass now

## Questions Answered

1. **Is the flat file structure worth the protocol change?** - YES. It fixed the race condition.
2. **Should we deprecate `NotifyMethod::Raw` on Linux?** - NO. Works fine now with flat structure.
3. **Is polling acceptable as a fallback?** - For now, yes. Long-term, replace with notify (tracked in todos.md).
