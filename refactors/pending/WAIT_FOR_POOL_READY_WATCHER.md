# Convert wait_for_pool_ready to use VerifiedWatcher

## Motivation

`wait_for_pool_ready` currently polls every 10ms:

```rust
// crates/agent_pool/src/submit/mod.rs:27-46
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    while !status_path.exists() {
        if start.elapsed() > timeout {
            return Err(...);
        }
        thread::sleep(Duration::from_millis(10));  // Polling!
    }
    Ok(())
}
```

Similarly, `wait_for_status_file` in the CLI polls every 100ms:

```rust
// crates/agent_pool_cli/src/main.rs:470-468
fn wait_for_status_file(status_file: &std::path::Path) -> bool {
    while start.elapsed() < TIMEOUT {
        if status_file.exists() {
            return true;
        }
        thread::sleep(POLL_INTERVAL);  // 100ms polling!
    }
    false
}
```

This is inconsistent with the rest of the codebase which uses `VerifiedWatcher` for file watching.

## Current Call Sites

| Location | Timeout | Used For |
|----------|---------|----------|
| `agent_pool/tests/common/mod.rs:686` | 10s | Test daemon startup |
| `gsd_cli/tests/common/mod.rs:272` | 10s | Test daemon startup |
| `gsd_config/tests/common/mod.rs:328` | 10s | Test daemon startup |
| `agent_pool_cli/src/main.rs:428` | 5s | CLI `get_task` command |

## Proposed Solution

Use `VerifiedWatcher` to watch for the status file. Rename existing method for clarity.

### New VerifiedWatcher Methods

One private implementation, two public conveniences:

```rust
// crates/agent_pool/src/verified_watcher.rs

impl VerifiedWatcher {
    /// Wait for a target file to exist (no timeout).
    pub fn wait_for_file(&mut self, target: &Path) -> io::Result<()> {
        self.wait_for_file_impl(target, None)
    }

    /// Wait for a target file to exist, with a timeout.
    pub fn wait_for_file_with_timeout(
        &mut self,
        target: &Path,
        timeout: Duration,
    ) -> io::Result<()> {
        self.wait_for_file_impl(target, Some(timeout))
    }

    fn wait_for_file_impl(
        &mut self,
        target: &Path,
        timeout: Option<Duration>,
    ) -> io::Result<()> {
        if target.exists() {
            return Ok(());
        }

        let deadline = timeout.map(|t| Instant::now() + t);

        loop {
            // Check timeout if set
            let wait_time = match deadline {
                Some(d) => {
                    let remaining = d.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("timed out waiting for {}", target.display()),
                        ));
                    }
                    remaining.min(Duration::from_millis(100))
                }
                None => Duration::from_millis(100),
            };

            match self.state.rx.recv_timeout(wait_time) {
                Ok(event) => {
                    if event.paths.iter().any(|p| p == target) || target.exists() {
                        return Ok(());
                    }
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                    if target.exists() {
                        return Ok(());
                    }
                    for canary in &mut self.state.remaining_canaries {
                        canary.retry()?;
                    }
                }
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "watcher disconnected",
                    ));
                }
            }
        }
    }
}
```

### Watcher Reuse

Currently each function creates its own `VerifiedWatcher`:
- `wait_for_pool_ready` creates one
- `wait_for_task` creates one
- `submit_file` creates one

If multiple are called in the same process, we waste resources creating multiple watchers for the same directory.

**Solution:** Accept an optional watcher reference, create one only if not provided:

```rust
pub fn wait_for_pool_ready(
    root: impl AsRef<Path>,
    timeout: Duration,
    watcher: Option<&mut VerifiedWatcher>,
) -> io::Result<()> {
    let root = root.as_ref();
    let status_path = root.join(STATUS_FILE);

    if status_path.exists() {
        return Ok(());
    }

    match watcher {
        Some(w) => w.wait_for_file_with_timeout(&status_path, timeout),
        None => {
            let mut w = VerifiedWatcher::new(root, std::slice::from_ref(&root))?;
            w.wait_for_file_with_timeout(&status_path, timeout)
        }
    }
}
```

Same pattern for `wait_for_task` and `submit_file`. Callers that need multiple operations can create one watcher and pass it to all.

### Convenience wrapper (no watcher param)

For simple use cases, keep zero-config versions:

```rust
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    wait_for_pool_ready_with_watcher(root, timeout, None)
}

pub fn wait_for_pool_ready_with_watcher(
    root: impl AsRef<Path>,
    timeout: Duration,
    watcher: Option<&mut VerifiedWatcher>,
) -> io::Result<()> {
    // ... implementation above
}
```

### CLI Update

Replace `wait_for_status_file` with the library function:

```rust
// crates/agent_pool_cli/src/main.rs

// Before:
let status_file = root.join(STATUS_FILE);
if !wait_for_status_file(&status_file) {
    eprintln!("Daemon not ready...");
    return ExitCode::FAILURE;
}

// After:
if let Err(e) = wait_for_pool_ready(&root, Duration::from_secs(5)) {
    eprintln!("Daemon not ready: {e}");
    return ExitCode::FAILURE;
}
```

Then delete `wait_for_status_file` function entirely.

### Update Existing wait_for Callers

Rename `wait_for` → `wait_for_file` at call sites:

```rust
// worker.rs
watcher.wait_for_file(&task)?;

// submit/file.rs
watcher.wait_for_file(&response_path)?;
```

## Migration Steps

1. Add private `wait_for_file_impl` with `Option<Duration>`
2. Add public `wait_for_file` and `wait_for_file_with_timeout` that call impl
3. Add `wait_for_pool_ready_with_watcher` that accepts optional watcher
4. Add convenience `wait_for_pool_ready` that passes `None`
5. Update `wait_for_task` similarly (accept optional watcher)
6. Update `submit_file` similarly (accept optional watcher)
7. Update all `wait_for` call sites to use new names
8. Update CLI to use `wait_for_pool_ready` instead of `wait_for_status_file`
9. Delete `wait_for_status_file` from CLI
10. Run tests to verify

## Testing

- Daemon startup in tests still works
- CLI `get_task` waits properly for daemon
- Timeout fires correctly if daemon never starts
- No polling in hot path (verify with tracing)

## Notes

The original comment said polling was needed because "the daemon clears and recreates the pool directory on startup, which would race with watcher setup." This is not actually a problem:

1. The pool root directory must exist before calling `wait_for_pool_ready` (caller creates it or it already exists)
2. We watch the root directory, not the status file specifically
3. The daemon creates subdirectories then writes the status file
4. Our watcher sees the status file creation event

The race condition concern was about watching a directory that gets deleted, but we watch the parent which persists.
