# Stop File Cancellation

**Depends on:**
- `CROSSBEAM_CHANNELS.md` (completed)
- `WAIT_FOR_POOL_READY_WATCHER.md` (completed)

## Motivation

Multiple blocking operations need cancellation support:
- Workers waiting for tasks
- Submitters waiting for responses
- Tests that need clean shutdown

Currently these use timeout-based polling, which is error-prone (we've broken it twice).

## Goal

Bake stop file detection into `VerifiedWatcher`. When the stop file is written, all blocking operations return `Err(Interrupted)` immediately. No API changes needed - cancellation is automatic.

## Key Insight

After `SINGLE_WATCHER_AT_ENTRY_POINT`, the `VerifiedWatcher` is always created at the pool root and passed down. All blocking operations go through it:

| Function | Uses VerifiedWatcher |
|----------|---------------------|
| `wait_for_file` | ✓ |
| `wait_for_file_with_timeout` | ✓ |
| `into_receiver` | ✓ |
| `wait_for_task` | ✓ (passed in) |
| `submit_file` | ✓ (passed in) |
| `submit` (socket) | ✓ + blocking socket read (punted) |

Since everything uses VerifiedWatcher, baking stop detection into it covers all cases.

## Implementation

### VerifiedWatcher changes

Store the stop file path and check for it on every event:

```rust
pub struct VerifiedWatcher {
    watcher: RecommendedWatcher,
    rx: Receiver<notify::Event>,
    remaining_canaries: Vec<CanaryGuard>,
    stop_path: PathBuf,  // NEW: pool_root/status.json
}

impl VerifiedWatcher {
    pub fn new(watch_dir: &Path, canary_dirs: &[PathBuf]) -> io::Result<Self> {
        // ... existing setup ...

        // Assume watch_dir is pool root (true after SINGLE_WATCHER_AT_ENTRY_POINT)
        let stop_path = watch_dir.join(STATUS_FILE);

        Ok(Self {
            watcher,
            rx,
            remaining_canaries,
            stop_path,
        })
    }
}
```

### wait_for_file_impl changes

Check for stop file on every event:

```rust
fn wait_for_file_impl(
    &mut self,
    target: &Path,
    timeout: Option<Duration>,
) -> Result<(), WaitError> {
    if target.exists() {
        return Ok(());
    }

    let deadline = timeout.map(|t| Instant::now() + t);

    loop {
        let wait_time = match deadline {
            Some(d) => {
                let remaining = d.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(WaitError::Io(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("[E046] timed out waiting for {}", target.display()),
                    )));
                }
                remaining.min(Duration::from_millis(100))
            }
            None => Duration::from_millis(100),
        };

        match self.rx.recv_timeout(wait_time) {
            Ok(event) => {
                // Check for stop file
                if event.paths.iter().any(|p| p == &self.stop_path) {
                    if is_stop_requested(&self.stop_path) {
                        return Err(WaitError::Stopped);
                    }
                }

                // Check for target file
                if event.paths.iter().any(|p| p == target) || target.exists() {
                    return Ok(());
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if target.exists() {
                    return Ok(());
                }
                for canary in &mut self.remaining_canaries {
                    canary.retry().map_err(WaitError::Io)?;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(WaitError::Io(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "[E003] watcher disconnected",
                )));
            }
        }
    }
}

/// Error type for wait operations.
#[derive(Debug)]
pub enum WaitError {
    /// Pool stop was requested (stop file written).
    Stopped,
    /// I/O error (timeout, disconnect, file error, etc).
    Io(io::Error),
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "pool stopped"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WaitError {}

fn is_stop_requested(stop_path: &Path) -> bool {
    std::fs::read_to_string(stop_path)
        .map(|s| s.trim().starts_with(STATUS_STOP))
        .unwrap_or(false)
}
```

### wait_for_task cleanup

Still need to clean up ready file on error (including Interrupted):

```rust
pub fn wait_for_task(
    watcher: &mut VerifiedWatcher,
    pool_root: &Path,
    name: Option<&str>,
    timeout: Option<Duration>,
) -> io::Result<TaskAssignment> {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let uuid = Uuid::new_v4().to_string();

    let ready = ready_path(&agents_dir, &uuid);
    let task = task_path(&agents_dir, &uuid);

    let metadata = name.map_or_else(|| "{}".to_string(), |n| format!(r#"{{"name":"{n}"}}"#));
    fs::write(&ready, &metadata)?;

    // Wait for task, clean up ready file on any error
    if let Err(e) = match timeout {
        Some(t) => watcher.wait_for_file_with_timeout(&task, t),
        None => watcher.wait_for_file(&task),
    } {
        let _ = fs::remove_file(&ready);
        return Err(e);
    }

    let content = fs::read_to_string(&task)?;
    Ok(TaskAssignment { uuid, content })
}
```

## Usage

### Stopping everything

To cancel all operations, just write the stop file:

```rust
// This causes all wait_for_file calls to return Err(Interrupted)
stop(&pool_root);
```

### Test Agent (simplified)

```rust
pub struct GsdTestAgent {
    handle: Option<thread::JoinHandle<Vec<String>>>,
    pool_root: PathBuf,
}

impl GsdTestAgent {
    pub fn start<F>(root: &Path, processor: F) -> Self
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        let pool_root = root.to_path_buf();

        let handle = thread::spawn(move || {
            let mut watcher = VerifiedWatcher::new(&pool_root, &[pool_root.join(AGENTS_DIR)])
                .expect("create watcher");
            let mut processed = Vec::new();

            loop {
                match wait_for_task(&mut watcher, &pool_root, None, None) {
                    Ok(assignment) => {
                        let response = processor(&assignment.content);
                        processed.push(assignment.content);
                        let _ = write_response(&pool_root, &assignment.uuid, &response);
                    }
                    Err(WaitError::Stopped) => break,  // Clean exit
                    Err(e) => {
                        eprintln!("[test-agent] error: {e}");
                        break;
                    }
                }
            }

            processed
        });

        Self { handle: Some(handle), pool_root }
    }

    pub fn stop(mut self) -> Vec<String> {
        // Write stop file - agent will see it and return Interrupted
        let _ = stop(&self.pool_root);
        self.handle.take().unwrap().join().unwrap()
    }
}
```

No cancel channels needed - just write the stop file.

## Migration Steps

1. Add `WaitError` enum to lib.rs (Stopped, Io variants)
2. Add `stop_path: PathBuf` field to `VerifiedWatcher`
3. Store `watch_dir.join(STATUS_FILE)` in constructor
4. Change `wait_for_file` return type from `io::Result<()>` to `Result<(), WaitError>`
5. Update `wait_for_file_impl` to check for stop file events, return `WaitError::Stopped`
6. Update all callers to handle `WaitError` (match on Stopped vs Io)
7. Ensure `wait_for_task` cleans up ready file on all errors
8. Update daemon to delete entire pool folder on stop
9. Simplify test agents to match on `WaitError::Stopped`

## Cleanup: Rename "shutdown" to "stop"

Existing code uses "shutdown" in several places. Rename for consistency:

| Current | New |
|---------|-----|
| `NotProcessedReason::Shutdown` | `NotProcessedReason::Stopped` |
| `ShutdownNotifier` | `StopNotifier` |
| `shutdown.shutdown()` | `stop_notifier.stop()` |
| `shutdown_signaled` variable | `stopped` |
| Comments mentioning "shutdown" | Update to "stop" |

Files to update:
- `crates/agent_pool/src/response.rs` - enum variant
- `crates/agent_pool/src/daemon/io.rs` - StopNotifier struct
- `crates/agent_pool/src/daemon/wiring.rs` - variable names, comments

## Design Decisions

1. **No CancelRx parameter:** The stop file IS the cancellation signal. VerifiedWatcher detects it internally. No API changes to downstream functions.

2. **Assume watch_dir is pool root:** After SINGLE_WATCHER_AT_ENTRY_POINT, the watcher is always created at pool root. We can assume `watch_dir.join(STATUS_FILE)` is the stop file.

3. **Socket read:** TODO for future work. The `submit()` function blocks on socket read. For now, accept the limitation (socket reads are typically fast).

4. **Cleanup on cancel:** `wait_for_task` cleans up its ready file on any error including Interrupted. `submit_file` does NOT clean up request files (daemon handles them).

5. **Daemon stop cleanup:** When daemon stops, it deletes the entire pool folder. This automatically cleans up all orphaned files.

## Testing

- `wait_for_file` returns `WaitError::Stopped` when stop file written before call
- `wait_for_file` returns `WaitError::Stopped` when stop file written during wait
- `wait_for_file` returns `WaitError::Io` for timeout, disconnect, etc.
- Test agent stops promptly when `stop()` called
- `wait_for_task` cleans up ready file on any error (including Stopped)
- Daemon stop deletes entire pool folder
