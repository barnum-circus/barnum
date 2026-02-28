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
1. A new subdirectory is created (e.g., `submissions/<uuid>/`)
2. inotify needs to add a watch for the new subdirectory
3. If files are written to the subdirectory before the watch is added, events are missed

**FSEvents doesn't have this problem** because it watches at the filesystem level, not per-directory.

**FIXED:** By flattening submissions to `<id>.request.json` files, we eliminated the subdirectory creation entirely.

## Original Architecture Issues (RESOLVED)

### 1. ~~Nested directories in `submissions/`~~ - FIXED

Previously each submission created `submissions/<uuid>/task.json`. Now uses flat files:
```
submissions/<uuid>.request.json
submissions/<uuid>.response.json
```

### 2. ~~Three notification methods with different reliability~~ - FIXED

`NotifyMethod::Raw` now works reliably with flat file structure.

### 3. ~~Temp file pattern~~ - FIXED

Atomic writes use temp files with rename, generating `Modify(Name)` events that watchers handle correctly.

### 4. ~~Watcher sync only proves startup readiness~~ - RESOLVED

With flat files, there's no per-submission race. The startup watcher sync is sufficient.

## Cleanup Tasks

### Task 1: Flatten submissions directory structure - **DONE**

Now uses:
```
submissions/<uuid>.request.json
submissions/<uuid>.response.json
```

### Task 2: Audit all temp file usage - **DONE**

Atomic writes use temp files in the same directory, then rename. This is intentional - the rename generates events that watchers handle.

### Task 3: SubmissionsDir fallback - **OBSOLETE**

No longer needed since we flattened the structure.

### Task 4: Replace polling with notify in submit_file - **TODO**

`submit_file.rs` still polls every 100ms for `response.json`. Should use file watcher instead. Tracked in `todos.md`.

### Task 5: Document Linux vs macOS differences - **LOW PRIORITY**

Could be useful but not blocking anything now that the race is fixed.

### Task 6: inotify-specific handling - **NOT NEEDED**

The flat file structure works on both platforms. No special handling required.

### Task 7: Flatten agent directory - **FUTURE**

Agent structure could be flattened like submissions. See `ANONYMOUS_WORKERS.md` for the proposed three-file protocol (`<id>.ready.json`, `<id>.task.json`, `<id>.response.json`).

Low priority - agents work fine as-is since the directory exists before files are written (causal chain prevents race).

## Quick Wins - **DONE**

1. ~~Verify temp files are gone~~ - Temp files are intentional for atomic writes
2. ~~Add logging around task registration~~ - Debug logging added
3. ~~Test deduplication~~ - Working correctly with flat structure
4. ~~Increase test timeout logging~~ - Tests pass now

## Questions Answered

1. **Is the flat file structure worth the protocol change?** - YES. It fixed the race condition.
2. **Should we deprecate `NotifyMethod::Raw` on Linux?** - NO. Works fine now with flat structure.
3. **Is polling acceptable as a fallback?** - For now, yes. Long-term, replace with notify (tracked in todos.md).
