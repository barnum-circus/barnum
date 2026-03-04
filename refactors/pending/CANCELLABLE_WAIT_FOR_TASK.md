# Universal Cancellation Channel

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

Add a `CancelRx` parameter to all blocking operations. When a message arrives on the cancel channel, return `Err(Interrupted)` immediately.

## Prerequisite

This refactor assumes `CROSSBEAM_CHANNELS.md` is complete:
- `crossbeam` dependency added
- `VerifiedWatcher` uses `crossbeam::channel` internally
- Daemon uses `crossbeam::select!` instead of forwarder threads

## Blocking Operations That Need Cancellation

After `WAIT_FOR_POOL_READY_WATCHER.md`, these functions exist but without cancel support:

| Function | Location | Currently |
|----------|----------|-----------|
| `VerifiedWatcher::wait_for_file` | verified_watcher.rs | recv_timeout loop |
| `VerifiedWatcher::wait_for_file_with_timeout` | verified_watcher.rs | recv_timeout loop |
| `VerifiedWatcher::into_receiver` | verified_watcher.rs | recv_timeout loop |
| `wait_for_task` | worker.rs | Uses wait_for_file |
| `submit_file` | submit/file.rs | Uses wait_for_file |
| `submit` | submit/socket.rs | Uses wait_for_file + blocking socket |
| `wait_for_pool_ready` | submit/mod.rs | Uses wait_for_file_with_timeout |

## API

### Type Alias

```rust
// lib.rs or verified_watcher.rs
pub type CancelRx = crossbeam::channel::Receiver<()>;
```

### VerifiedWatcher

```rust
impl VerifiedWatcher {
    pub fn wait_for_file(
        &mut self,
        target: &Path,
        cancel: Option<&CancelRx>,
    ) -> io::Result<()>;

    pub fn wait_for_file_with_timeout(
        &mut self,
        target: &Path,
        timeout: Duration,
        cancel: Option<&CancelRx>,
    ) -> io::Result<()>;

    pub fn into_receiver(
        self,
        cancel: Option<&CancelRx>,
    ) -> io::Result<(RecommendedWatcher, Receiver<notify::Event>)>;
}
```

### Worker

```rust
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    cancel: Option<&CancelRx>,
) -> io::Result<TaskAssignment>;
```

### Submission

```rust
pub fn submit_file(
    root: impl AsRef<Path>,
    payload: &Payload,
    cancel: Option<&CancelRx>,
) -> io::Result<Response>;

pub fn submit(
    root: impl AsRef<Path>,
    payload: &Payload,
    cancel: Option<&CancelRx>,
) -> io::Result<Response>;

pub fn wait_for_pool_ready(
    root: impl AsRef<Path>,
    timeout: Duration,
    cancel: Option<&CancelRx>,
) -> io::Result<()>;
```

## Implementation

### VerifiedWatcher methods

Update the private impl to accept cancel, public methods delegate:

```rust
pub fn wait_for_file(
    &mut self,
    target: &Path,
    cancel: Option<&CancelRx>,
) -> io::Result<()> {
    self.wait_for_file_impl(target, None, cancel)
}

pub fn wait_for_file_with_timeout(
    &mut self,
    target: &Path,
    timeout: Duration,
    cancel: Option<&CancelRx>,
) -> io::Result<()> {
    self.wait_for_file_impl(target, Some(timeout), cancel)
}

fn wait_for_file_impl(
    &mut self,
    target: &Path,
    timeout: Option<Duration>,
    cancel: Option<&CancelRx>,
) -> io::Result<()> {
    if target.exists() {
        return Ok(());
    }

    let never = crossbeam::channel::never();
    let cancel = cancel.unwrap_or(&never);
    let deadline = timeout.map(|t| Instant::now() + t);

    loop {
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

        crossbeam::select! {
            recv(self.state.rx) -> event => {
                match event {
                    Ok(e) => {
                        if e.paths.iter().any(|p| p == target) || target.exists() {
                            return Ok(());
                        }
                    }
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "watcher disconnected",
                        ));
                    }
                }
            }
            recv(cancel) -> _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            default(wait_time) => {
                if target.exists() {
                    return Ok(());
                }
                for canary in &mut self.state.remaining_canaries {
                    canary.retry()?;
                }
            }
        }
    }
}
```

### wait_for_task

```rust
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    cancel: Option<&CancelRx>,
) -> io::Result<TaskAssignment> {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let uuid = Uuid::new_v4().to_string();

    let ready = ready_path(&agents_dir, &uuid);
    let task = task_path(&agents_dir, &uuid);

    let metadata = name.map_or_else(|| "{}".to_string(), |n| format!(r#"{{"name":"{n}"}}"#));
    fs::write(&ready, &metadata)?;

    let mut watcher = VerifiedWatcher::new(&agents_dir, std::slice::from_ref(&agents_dir))?;

    // Wait for task, clean up ready file on cancel
    if let Err(e) = watcher.wait_for_file(&task, cancel) {
        let _ = fs::remove_file(&ready);  // Clean up on cancel/error
        return Err(e);
    }

    let content = fs::read_to_string(&task)?;
    Ok(TaskAssignment { uuid, content })
}
```

## Usage Example

### Test Agent

```rust
pub struct GsdTestAgent {
    cancel_tx: crossbeam::channel::Sender<()>,
    handle: Option<thread::JoinHandle<Vec<String>>>,
    pool_root: PathBuf,
}

impl GsdTestAgent {
    pub fn start<F>(root: &Path, processor: F) -> Self
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        let (cancel_tx, cancel_rx) = crossbeam::channel::bounded(1);
        let pool_root = root.to_path_buf();

        let handle = thread::spawn(move || {
            let mut processed = Vec::new();

            loop {
                match wait_for_task(&pool_root, None, Some(&cancel_rx)) {
                    Ok(assignment) => {
                        let response = processor(&assignment.content);
                        processed.push(assignment.content);
                        let _ = write_response(&pool_root, &assignment.uuid, &response);
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => break,
                    Err(e) => {
                        eprintln!("[test-agent] error: {e}");
                        break;
                    }
                }
            }

            processed
        });

        Self { cancel_tx, handle: Some(handle), pool_root }
    }

    pub fn stop(mut self) -> Vec<String> {
        // Signal cancellation
        let _ = self.cancel_tx.send(());

        // Also stop daemon
        let _ = stop(&self.pool_root);

        self.handle.take().unwrap().join().unwrap()
    }
}
```

## Migration Steps

(Assumes `WAIT_FOR_POOL_READY_WATCHER.md` is complete, which provides `wait_for_file` and `wait_for_file_with_timeout` methods)

1. Add `CancelRx` type alias
2. Update `VerifiedWatcher::wait_for_file` to accept cancel
3. Update `VerifiedWatcher::wait_for_file_with_timeout` to accept cancel
4. Update `VerifiedWatcher::into_receiver` to accept cancel
5. Update `wait_for_task` to accept and pass through cancel
6. Update `submit_file` to accept and pass through cancel
7. Update `submit` to accept cancel (for wait_for_file phase)
8. Update `wait_for_pool_ready` to accept and pass through cancel
9. Update all call sites to pass `None` for cancel (or actual channel)
10. Update test agents to use cancel channel instead of AtomicBool + timeout

## Design Decisions

1. **Timeout vs cancel:** Keep both as separate parameters. Timeout is for actual deadlines, cancel is for graceful shutdown. The existing `wait_for_file` and `wait_for_file_with_timeout` methods are correctly named and will gain a cancel parameter.

2. **Socket read:** TODO for future work. The `submit()` function blocks on socket read after connecting. This can't be cancelled with channels. For now, accept the limitation (socket reads are typically fast). Future options: non-blocking socket with `select!` or socket timeout.

3. **Cleanup on cancel:** YES, clean up. When `wait_for_task` is cancelled, delete the ready file to avoid orphans.

4. **Daemon stop cleanup:** When the daemon writes the stop file and shuts down, it should delete the entire pool folder. This simplifies cleanup since:
   - All workers are dead anyway
   - Any ready files are orphaned
   - Any pending submissions will fail
   - Starting a new daemon creates fresh directories

   This means individual cancellation cleanup (point 3) only matters for non-stop scenarios like worker timeouts.

## Testing

- `wait_for_file` returns `Interrupted` when cancel signal sent before call
- `wait_for_file` returns `Interrupted` when cancel signal sent during wait
- Test agent stops within 100ms of `stop()` call (not 500ms timeout)
- File submission can be cancelled mid-wait
- `wait_for_task` cleans up ready file when cancelled
- Daemon stop deletes entire pool folder
