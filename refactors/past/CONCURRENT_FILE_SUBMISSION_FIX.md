# Fix Concurrent File-Based Submissions Race Condition

## Status: IMPLEMENTED

## Problem

The `agent_joins_mid_processing::case_4` (FileReference + File notify mode) test fails intermittently. Root cause analysis reveals a race condition when multiple CLI processes submit tasks concurrently using file-based notification.

### Evidence from CI logs

- Test spawns 6 parallel submissions
- 3 succeed (create `request.json`), 3 fail (timeout waiting for `response.json`)
- Failing submissions create and delete canary files very quickly (~0.3ms)
- No `request.json` files are ever created for failing submissions
- Error message says "timed out waiting for response.json" (misleading - actual issue is earlier)

### Timeline Analysis

```
844.330ms - Failing canary #1 (3b7ac5d1) CREATE
844.676ms - Failing canary #1 DELETE (0.3ms later)
844.985ms - Failing canary #2 (ee4dc904) CREATE
845.023ms - Failing canary #2 DELETE
845.056ms - Failing canary #3 (27493cc6) CREATE
845.081ms - Failing canary #3 DELETE
848.002ms - Successful canary #1 (d6034cd3) CREATE
848.066ms - Successful submission #1 request.json MOVED_TO
```

The failing submissions' canaries are deleted immediately, but no `request.json` follows. The successful submissions happen 3ms later and work normally.

### Root Cause

The `VerifiedWatcher` has a design flaw when multiple watchers observe the same directory concurrently:

1. Each CLI process creates a `VerifiedWatcher` on the pool root
2. All watchers receive events for ALL filesystem activity
3. When `wait_for(status)` receives an event for ANOTHER submission's file:
   - Sets `canary = None` (deletes its own canary)
   - Checks if event matches status path - NO
   - Original code didn't check if `status.exists()` immediately after receiving any event
4. High inotify event throughput may cause queue issues or event delivery delays

## Implemented Fix

Modified `VerifiedWatcher::wait_for` in `crates/agent_pool/src/fs.rs`:

1. **Added retry loop for fast path**: Check `target.exists()` multiple times with small delays (100µs each) to handle filesystem sync delays when multiple watchers are active.

2. **Prioritize target existence check in event handler**: After receiving ANY event, immediately check if the target file exists before processing the event path. This ensures we don't miss the target appearing concurrently with other events.

```rust
pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
    // Fast path: file already exists - check multiple times to handle filesystem sync
    for _ in 0..3 {
        if target.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_micros(100));
    }

    // ... event loop ...
    match rx.recv_timeout(Duration::from_millis(100)) {
        Ok(path) => {
            // First check if target exists before doing anything else
            if target.exists() {
                // Clean up canary if still present
                if canary.is_some() {
                    *canary = None;
                }
                return Ok(());
            }
            // ... rest of handling
        }
        // ...
    }
}
```

## Test Results

- CI run 22611422550: `agent_joins_mid_processing::case_4` now **PASSES** (was failing before)

## Additional Discovery

During this investigation, discovered that the gsd_cli cli_integration tests were **not running in CI** before. These tests are now running and exposing a separate issue where `FileWriterAgent` hangs when `stop()` is called before any task is assigned. This was fixed by adding a timeout to `wait_for_task` so the agent thread periodically checks its `running` flag.
