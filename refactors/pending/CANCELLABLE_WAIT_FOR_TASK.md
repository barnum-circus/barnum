# Cancellable wait_for_task

## Motivation

`wait_for_task` blocks indefinitely (or until timeout) with no way to cancel from outside. This forces callers to poll with short timeouts to enable clean shutdown:

```rust
while running.load(Ordering::SeqCst) {
    let Ok(task) = wait_for_task(&pool, None, Some(Duration::from_millis(500))) else {
        continue;  // Timeout - check running flag
    };
    // ...
}
```

This pattern is error-prone. Commit `54549e3` accidentally removed the timeout, assuming CLI stop would make the watcher fail. It didn't, causing 20-second test hangs. We've now fixed this twice (`5894f96`, `f6404cd`).

## Goal

`wait_for_task` should accept a cancellation signal and return immediately when cancelled, eliminating the need for timeout-based polling.

## Current State

### worker.rs (lines 46-67)

```rust
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    timeout: Option<Duration>,
) -> io::Result<TaskAssignment> {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let uuid = Uuid::new_v4().to_string();

    let ready = ready_path(&agents_dir, &uuid);
    let task = task_path(&agents_dir, &uuid);

    // Write ready file with optional metadata
    let metadata = name.map_or_else(|| "{}".to_string(), |n| format!(r#"{{"name":"{n}"}}"#));
    fs::write(&ready, &metadata)?;

    // Wait for task file using VerifiedWatcher
    let mut watcher = VerifiedWatcher::new(&agents_dir, std::slice::from_ref(&agents_dir))?;
    watcher.wait_for(&task, timeout)?;

    let content = fs::read_to_string(&task)?;
    Ok(TaskAssignment { uuid, content })
}
```

### VerifiedWatcher::wait_for (verified_watcher.rs, lines 205-261)

```rust
pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
    // Fast path: file already exists
    if target.exists() {
        return Ok(());
    }

    let start = Instant::now();
    loop {
        // Check timeout
        if let Some(t) = timeout && start.elapsed() > t {
            return Err(io::Error::new(io::ErrorKind::TimedOut, ...));
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                // Check if target appeared
                // ...
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Retry canaries
                // ...
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, ...));
            }
        }
    }
}
```

### Test agent usage (gsd_config/tests/common/mod.rs, lines 144-152)

```rust
while running_clone.load(Ordering::SeqCst) {
    // Use timeout so we can check running flag periodically.
    // CLI stop may not reliably cause the watcher to error.
    let Ok(assignment) =
        wait_for_task(&pool_root, None, Some(Duration::from_millis(500)))
    else {
        // Timeout or error - check running flag and retry
        continue;
    };
    // ...
}
```

## Proposed Design

### Option A: Channel-based cancellation

Add an optional `mpsc::Receiver<()>` parameter. When a message arrives on the channel, return `Err(Cancelled)`.

```rust
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    cancel_rx: Option<&mpsc::Receiver<()>>,
) -> io::Result<TaskAssignment> {
    // ...
    watcher.wait_for(&task, cancel_rx)?;
    // ...
}
```

Caller creates a channel and sends to cancel:
```rust
let (cancel_tx, cancel_rx) = mpsc::channel();
let cancel_rx_clone = cancel_rx.clone();  // Problem: Receiver can't be cloned

// In worker thread
wait_for_task(&pool, None, Some(&cancel_rx))?;

// To cancel
let _ = cancel_tx.send(());
```

**Problem:** `mpsc::Receiver` cannot be cloned, so this doesn't work well for multi-threaded use. Could use `crossbeam` channels or `Arc<AtomicBool>`.

### Option B: AtomicBool cancellation flag

Accept an `Arc<AtomicBool>` that the caller sets to cancel.

```rust
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    cancel: Option<&AtomicBool>,
) -> io::Result<TaskAssignment> {
    // ...
    watcher.wait_for(&task, cancel)?;
    // ...
}
```

Caller creates the flag and sets it to cancel:
```rust
let cancel = Arc::new(AtomicBool::new(false));
let cancel_clone = cancel.clone();

// In worker thread
wait_for_task(&pool, None, Some(&*cancel))?;

// To cancel
cancel.store(true, Ordering::SeqCst);
```

This is what the test agents already use internally. Making `wait_for_task` accept it directly removes the polling layer.

### Option C: Stop file

Watch for both the task file AND a stop file. When stop file appears, return `Err(Stopped)`.

```rust
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    stop_file: Option<&Path>,
) -> io::Result<TaskAssignment> {
    // ...
    watcher.wait_for_any(&[&task, stop_file], ...)?;
    // ...
}
```

**Benefit:** Works across process boundaries (parent can write stop file, child sees it).
**Drawback:** Requires filesystem coordination, more complex cleanup.

## Recommendation

**Option B (AtomicBool)** is the simplest and matches existing patterns in the codebase. The test agents already use `AtomicBool` for their running flag - we just move the check inside `wait_for_task`.

## Changes Required

### 1. Update VerifiedWatcher::wait_for

Add `cancel` parameter:

```rust
pub fn wait_for(
    &mut self,
    target: &Path,
    timeout: Option<Duration>,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    loop {
        // Check cancellation first
        if let Some(c) = cancel && c.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }

        // Rest of existing logic...
    }
}
```

### 2. Update wait_for_task

```rust
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    cancel: Option<&AtomicBool>,
) -> io::Result<TaskAssignment> {
    // ...
    watcher.wait_for(&task, None, cancel)?;  // No timeout needed when cancel is provided
    // ...
}
```

### 3. Update test agents

```rust
while running_clone.load(Ordering::SeqCst) {
    match wait_for_task(&pool_root, None, Some(&*running_clone)) {
        Ok(assignment) => { /* process */ }
        Err(e) if e.kind() == io::ErrorKind::Interrupted => break,  // Cancelled
        Err(e) => {
            eprintln!("[test-agent] wait_for_task error: {e}");
            break;
        }
    }
}
```

Or simpler - just pass the flag and let `wait_for_task` handle it:

```rust
let assignment = match wait_for_task(&pool_root, None, Some(&*running_clone)) {
    Ok(a) => a,
    Err(_) => break,  // Cancelled or error
};
```

### 4. Update CLI (if needed)

The CLI's `get_task` command may want to accept a signal handler for Ctrl+C. This is a separate concern from the library API.

## Open Questions

1. **Error kind for cancellation:** `Interrupted` seems right but could use a custom error type.

2. **Timeout vs cancel:** Should `wait_for_task` accept both, or only cancel? With cancellation, timeout can be implemented by the caller spawning a thread that sets the flag after N seconds.

3. **Backwards compatibility:** Changing the function signature breaks existing callers. Could add a new `wait_for_task_cancellable` function, or change the signature and update all call sites (there aren't many).

4. **Channel vs AtomicBool:** AtomicBool is simpler but channel allows sending a reason for cancellation. For our use case (just "stop"), AtomicBool suffices.

## Testing

- Unit test: `wait_for_task` returns `Interrupted` when cancel flag is set before call
- Unit test: `wait_for_task` returns `Interrupted` when cancel flag is set during wait
- Integration test: Test agent stops cleanly when `stop()` is called
- Integration test: No timeout-based polling in test output (grep for "Timeout")
