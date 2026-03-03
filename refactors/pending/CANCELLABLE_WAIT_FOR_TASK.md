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

`wait_for_task` should accept a cancellation channel and use `select!` to wait on either:
1. A task file appearing (return `Ok(TaskAssignment)`)
2. A cancellation signal (return `Err(Cancelled)`)

No timeouts, no polling, no AtomicBool checks.

## Proposed Design: Channel + Select

Use `crossbeam::select!` to wait on multiple channels simultaneously:

```rust
use crossbeam::channel::{self, Receiver};

pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    cancel_rx: Option<Receiver<()>>,
) -> io::Result<TaskAssignment> {
    // ... setup ...

    let mut watcher = VerifiedWatcher::new(&agents_dir, ...)?;
    watcher.wait_for(&task, cancel_rx)?;

    // ... read task ...
}
```

Inside `VerifiedWatcher::wait_for`:

```rust
pub fn wait_for(
    &mut self,
    target: &Path,
    cancel_rx: Option<Receiver<()>>,
) -> io::Result<()> {
    if target.exists() {
        return Ok(());
    }

    loop {
        crossbeam::select! {
            recv(self.rx) -> event => {
                // Handle watcher event
                if let Ok(event) = event {
                    if event.paths.contains(target) || target.exists() {
                        return Ok(());
                    }
                }
            }
            recv(cancel_rx.unwrap_or_else(channel::never)) -> _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
        }
    }
}
```

Caller creates a channel and sends to cancel:
```rust
let (cancel_tx, cancel_rx) = crossbeam::channel::bounded(1);

// Spawn worker
let handle = thread::spawn(move || {
    wait_for_task(&pool, None, Some(cancel_rx))
});

// To cancel
let _ = cancel_tx.send(());
handle.join();
```

## Why crossbeam?

`std::mpsc` doesn't have a `select!` macro. Options:
1. **crossbeam** - Mature, widely used, has `select!` macro
2. **async/tokio** - Overkill for this use case, adds async complexity
3. **polling with timeout** - What we do now, error-prone

crossbeam is the right choice.

## Changes Required

### 1. Add crossbeam dependency

```toml
# crates/agent_pool/Cargo.toml
[dependencies]
crossbeam = "0.8"
```

### 2. Update VerifiedWatcher

Change internal `mpsc::Receiver` to `crossbeam::channel::Receiver` and add select:

```rust
use crossbeam::channel::{self, Receiver, Sender};

pub struct VerifiedWatcher {
    watcher: RecommendedWatcher,
    rx: Receiver<notify::Event>,
    // ...
}

impl VerifiedWatcher {
    pub fn wait_for(
        &mut self,
        target: &Path,
        cancel_rx: Option<&Receiver<()>>,
    ) -> io::Result<()> {
        if target.exists() {
            return Ok(());
        }

        let never = channel::never();
        let cancel = cancel_rx.unwrap_or(&never);

        loop {
            crossbeam::select! {
                recv(self.rx) -> event => {
                    match event {
                        Ok(e) => {
                            // Check if target appeared
                            for path in &e.paths {
                                if path == target {
                                    return Ok(());
                                }
                            }
                            if target.exists() {
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
            }
        }
    }
}
```

### 3. Update wait_for_task

```rust
use crossbeam::channel::Receiver;

pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    cancel_rx: Option<&Receiver<()>>,
) -> io::Result<TaskAssignment> {
    // ... setup ...

    let mut watcher = VerifiedWatcher::new(&agents_dir, ...)?;
    watcher.wait_for(&task, cancel_rx)?;

    let content = fs::read_to_string(&task)?;
    Ok(TaskAssignment { uuid, content })
}
```

### 4. Update test agents

```rust
use crossbeam::channel;

pub struct GsdTestAgent {
    cancel_tx: channel::Sender<()>,
    handle: Option<thread::JoinHandle<Vec<String>>>,
    pool_root: PathBuf,
}

impl GsdTestAgent {
    pub fn start<F>(...) -> Self {
        let (cancel_tx, cancel_rx) = channel::bounded(1);

        let handle = thread::spawn(move || {
            loop {
                match wait_for_task(&pool_root, None, Some(&cancel_rx)) {
                    Ok(assignment) => { /* process */ }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => break,
                    Err(e) => {
                        eprintln!("[test-agent] error: {e}");
                        break;
                    }
                }
            }
            processed_tasks
        });

        Self { cancel_tx, handle: Some(handle), pool_root }
    }

    pub fn stop(mut self) -> Vec<String> {
        // Send cancellation signal
        let _ = self.cancel_tx.send(());

        // Also stop daemon
        let bin = find_agent_pool_binary();
        let _ = Command::new(&bin).arg("stop").arg("--pool").arg(&self.pool_root).output();

        self.handle.take().unwrap().join().unwrap()
    }
}
```

## Canary Handling

The current code retries canary writes on timeout. With select, we need to handle this differently:

Option 1: Use `select_timeout!` with short timeout, retry canaries on timeout
Option 2: Spawn a background thread that periodically retries canaries
Option 3: Remove canary retries (if verification completes quickly enough)

Option 1 is simplest:

```rust
loop {
    crossbeam::select! {
        recv(self.rx) -> event => { /* handle */ }
        recv(cancel) -> _ => { return Err(Interrupted); }
        default(Duration::from_millis(100)) => {
            // Retry canaries
            for canary in &mut self.remaining_canaries {
                canary.retry()?;
            }
        }
    }
}
```

## Testing

- Unit test: `wait_for_task` returns `Interrupted` when cancel signal sent before call
- Unit test: `wait_for_task` returns `Interrupted` when cancel signal sent during wait
- Integration test: Test agent stops cleanly when `stop()` is called
- Integration test: Verify no timeout-based polling (no 100ms/500ms sleeps in agent code)
