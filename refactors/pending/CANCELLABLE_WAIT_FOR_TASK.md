# Universal Cancellation Channel

**Depends on:**
- `CROSSBEAM_CHANNELS.md` (completed)
- `WAIT_FOR_POOL_READY_WATCHER.md` (must be completed first)

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

| Function | Location | Currently |
|----------|----------|-----------|
| `VerifiedWatcher::wait_for` | verified_watcher.rs | recv_timeout loop |
| `VerifiedWatcher::into_receiver` | verified_watcher.rs | recv_timeout loop |
| `wait_for_task` | worker.rs | Uses wait_for |
| `submit_file` | submit/file.rs | Uses wait_for |
| `submit` | submit/socket.rs | Uses wait_for + blocking socket |
| `wait_for_pool_ready` | submit/mod.rs | sleep loop |

## API

### Type Alias

```rust
// constants.rs or lib.rs
pub type CancelRx = crossbeam::channel::Receiver<()>;
```

### VerifiedWatcher

```rust
impl VerifiedWatcher {
    pub fn wait_for(
        &mut self,
        target: &Path,
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

### VerifiedWatcher::wait_for

```rust
use crossbeam::channel::{self, Receiver};
use crossbeam::select;

pub fn wait_for(
    &mut self,
    target: &Path,
    cancel: Option<&CancelRx>,
) -> io::Result<()> {
    if target.exists() {
        return Ok(());
    }

    let never = channel::never();
    let cancel = cancel.unwrap_or(&never);

    loop {
        select! {
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
            default(Duration::from_millis(100)) => {
                // Check file exists (edge case) and retry canaries
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
    watcher.wait_for(&task, cancel)?;  // Pass through cancel

    let content = fs::read_to_string(&task)?;
    Ok(TaskAssignment { uuid, content })
}
```

### wait_for_pool_ready

After `WAIT_FOR_POOL_READY_WATCHER.md` is complete, this function will already use `VerifiedWatcher`. We just need to add the cancel parameter:

```rust
pub fn wait_for_pool_ready(
    root: impl AsRef<Path>,
    timeout: Duration,
    cancel: Option<&CancelRx>,
) -> io::Result<()> {
    let root = root.as_ref();
    let status_path = root.join(STATUS_FILE);

    if status_path.exists() {
        return Ok(());
    }

    let mut watcher = VerifiedWatcher::new(root, std::slice::from_ref(&root))?;
    watcher.wait_for_timeout(&status_path, timeout, cancel)  // Pass cancel through
}
```

The `wait_for_timeout` method signature changes to accept cancel:

```rust
pub fn wait_for_timeout(
    &mut self,
    target: &Path,
    timeout: Duration,
    cancel: Option<&CancelRx>,
) -> io::Result<()>;
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

1. Add `CancelRx` type alias
2. Update `VerifiedWatcher::wait_for` signature and implementation
3. Add `VerifiedWatcher::wait_for_with_timeout` for deadline-based waiting
4. Update `VerifiedWatcher::into_receiver` signature and implementation
5. Update `wait_for_task` to accept and pass through cancel
6. Update `submit_file` to accept and pass through cancel
7. Update `submit` to accept cancel (for wait_for phase)
8. Update `wait_for_pool_ready` to use `VerifiedWatcher` instead of polling
9. Update test agents to use cancel channel instead of AtomicBool + timeout
10. Remove timeout parameter from functions where it was only used for cancellation polling

## Open Questions

1. **Timeout vs cancel:** Keep timeout as separate parameter for actual deadlines?
   ```rust
   fn wait_for(&mut self, target: &Path, timeout: Option<Duration>, cancel: Option<&CancelRx>)
   ```

2. **Socket read:** The `submit()` function blocks on socket read after connecting. This can't be cancelled with channels. Options:
   - Accept limitation (socket reads are typically fast)
   - Use non-blocking socket with `select!` (complex)
   - Set socket timeout

3. **Cleanup on cancel:** When cancelled mid-wait, should we clean up the ready file in `wait_for_task`? Currently we don't, which could leave orphaned files.

## Testing

- `wait_for` returns `Interrupted` when cancel signal sent before call
- `wait_for` returns `Interrupted` when cancel signal sent during wait
- Test agent stops within 100ms of `stop()` call (not 500ms timeout)
- File submission can be cancelled mid-wait
- Cancellation doesn't leave orphaned files
