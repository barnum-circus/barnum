# Daemon Event Loop Refactor

## Dependencies

**Must complete before**: TRANSPORT_ABSTRACTION.md Phase 2 (async)
- Converting traits from sync to async requires tokio runtime

**Can happen in parallel with**: TRANSPORT_ABSTRACTION.md Phase 1 (sync)
- Sync traits don't need tokio
- Both refactors are independent until async conversion

## Scope: agent_pool Only

This refactor applies specifically to the **agent_pool daemon**, not GSD.

**Why agent_pool needs `select!`:**
- Long-running daemon with multiple independent event sources
- Must respond to task submissions, agent responses, and timers concurrently
- Cannot predict which source will fire next

**Why GSD runner doesn't need this:**
```rust
// GSD runner - simple single-channel model
pub fn next(&mut self) -> Option<TaskOutcome> {
    self.submit_pending();              // Spawn worker threads
    let result = self.rx.recv().ok()?;  // Block on single results channel
    // ...
}
```
- GSD spawns worker threads that call `agent_pool::submit()` or `agent_pool::submit_file()`
- Collects results via a single channel
- Blocking `recv()` is sufficient

---

## Current Architecture (Problems)

```rust
loop {
    // Non-blocking socket accept
    if let Some(task) = accept_task(listener)? { ... }

    // Block with timeout waiting for fs events
    match fs_events.recv_timeout(poll_timeout) { ... }

    // Periodic scans every 500ms (!)
    if last_scan.elapsed() >= scan_interval {
        state.scan_agents()?;
        state.scan_outputs()?;
        state.scan_pending()?;
        state.check_health_check_timeouts()?;
    }

    state.dispatch_pending()?;
}
```

**Problems:**
1. Periodic polling wastes CPU
2. Scans are a crutch for not trusting FS events
3. Not proper event-driven architecture

---

## Target Architecture

Use **tokio** with `select!` to wait on multiple async event sources. No polling, no scans.

### Clients

The **client** is GSD's `TaskRunner`. When GSD runs a workflow:

1. `TaskRunner` spawns threads to submit tasks
2. Each thread calls `agent_pool::submit()` (socket) or `agent_pool::submit_file()` (file)
3. The daemon receives the submission, dispatches to an agent
4. Agent completes, daemon sends response back to client
5. Client thread returns result to `TaskRunner`

### Event Sources

| Source | Trigger | Handler |
|--------|---------|---------|
| Socket accept | Client calls `submit()` | Read task, enqueue |
| FS: `pending/<uuid>/task.json` | Client calls `submit_file()` | Read task, enqueue |
| FS: `agents/<name>/response.json` | Agent completes work | Complete task, respond to client |
| FS: `agents/<name>/` created | Agent registers | Add to available pool |
| FS: `agents/<name>/` removed | Agent deregisters | Remove from pool |
| Timer tick | Periodic interval | Check health check timeouts |

### Architecture Diagram

```
                    GSD TaskRunner
                          |
            +-------------+-------------+
            |                           |
      submit() [socket]          submit_file() [fs]
            |                           |
            v                           v
    ┌───────────────┐           ┌───────────────┐
    │ UnixListener  │           │  pending/     │
    │   .accept()   │           │  task.json    │
    └───────┬───────┘           └───────┬───────┘
            │                           │
            │      ┌─────────────┐      │
            └─────>│             │<─────┘
                   │   tokio     │
                   │  select!    │<──── agents/<name>/response.json
                   │             │<──── agents/<name>/ (created/removed)
                   │             │<──── health check timer
                   └──────┬──────┘
                          │
                          v
                   ┌─────────────┐
                   │ PoolState   │
                   │             │
                   │ - enqueue   │
                   │ - dispatch  │
                   │ - complete  │
                   └─────────────┘
```

### Main Event Loop

```rust
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use notify::Event;

async fn event_loop(
    listener: UnixListener,
    mut fs_rx: mpsc::Receiver<Event>,
    health_check_interval: Option<Duration>,
    state: &mut PoolState,
    shutdown: CancellationToken,
) -> io::Result<()> {
    // Only create health check timer if configured
    let mut health_check = health_check_interval.map(tokio::time::interval);

    loop {
        // Dispatch any pending tasks before waiting
        state.dispatch_pending()?;

        tokio::select! {
            // Graceful shutdown
            _ = shutdown.cancelled() => {
                return drain_and_shutdown(fs_rx, state).await;
            }

            // Socket-based task submission
            result = listener.accept() => {
                let (stream, _) = result?;
                if let Some(task) = read_task(stream).await? {
                    state.enqueue(task);
                }
            }

            // All FS events (submissions, responses, registration)
            Some(event) = fs_rx.recv() => {
                handle_fs_event(&event, state).await?;
            }

            // Heartbeat timeout checking (only if configured)
            _ = async {
                match &mut health_check {
                    Some(interval) => interval.tick().await,
                    None => std::future::pending().await,
                }
            } => {
                state.check_health_check_timeouts()?;
            }
        }
    }
}
```

### FS Event Handler

One handler for all filesystem events:

```rust
async fn handle_fs_event(event: &Event, state: &mut PoolState) -> io::Result<()> {
    for path in &event.paths {
        match categorize_path(path, state) {
            Some(PathKind::PendingTask { submission_id }) => {
                // File-based submission: pending/<uuid>/task.json created
                if matches!(event.kind, EventKind::Create(_)) {
                    state.accept_file_submission(&submission_id)?;
                }
            }
            Some(PathKind::AgentResponse { agent_id }) => {
                // Agent completed: agents/<name>/response.json created
                if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    state.complete_task(&agent_id)?;
                }
            }
            Some(PathKind::AgentDir { agent_id }) => {
                // Agent registration change
                if path.is_dir() {
                    state.register(&agent_id);
                } else {
                    state.unregister(&agent_id);
                }
            }
            None => {
                // Ignore unrecognized paths
            }
        }
    }
    Ok(())
}

enum PathKind {
    PendingTask { submission_id: String },
    AgentResponse { agent_id: String },
    AgentDir { agent_id: String },
}

fn categorize_path(path: &Path, state: &PoolState) -> Option<PathKind> {
    // Check if path is under pending/
    if let Ok(relative) = path.strip_prefix(&state.pending_dir) {
        let components: Vec<_> = relative.components().collect();
        if components.len() == 2 {
            let submission_id = components[0].as_os_str().to_str()?;
            let filename = components[1].as_os_str().to_str()?;
            if filename == "task.json" {
                return Some(PathKind::PendingTask {
                    submission_id: submission_id.to_string(),
                });
            }
        }
        return None;
    }

    // Check if path is under agents/
    if let Ok(relative) = path.strip_prefix(&state.agents_dir) {
        let components: Vec<_> = relative.components().collect();
        if components.is_empty() {
            return None;
        }
        let agent_id = components[0].as_os_str().to_str()?.to_string();

        if components.len() == 1 {
            // agents/<name>/ directory itself
            return Some(PathKind::AgentDir { agent_id });
        } else if components.len() == 2 {
            let filename = components[1].as_os_str().to_str()?;
            if filename == "response.json" {
                return Some(PathKind::AgentResponse { agent_id });
            }
        }
    }

    None
}
```

### Bridging notify to tokio

The `notify` crate uses std channels. Bridge to tokio:

```rust
fn create_watcher(
    agents_dir: &Path,
    pending_dir: &Path,
) -> io::Result<(RecommendedWatcher, mpsc::Receiver<Event>)> {
    let (sync_tx, sync_rx) = std::sync::mpsc::channel();
    let (async_tx, async_rx) = mpsc::channel(100);

    let watcher = notify::recommended_watcher(move |event: Result<Event, _>| {
        if let Ok(event) = event {
            let _ = sync_tx.send(event);
        }
    })?;

    // Bridge thread: forwards from std channel to tokio channel
    tokio::spawn(async move {
        loop {
            match tokio::task::spawn_blocking({
                let sync_rx = sync_rx.clone(); // Won't compile - need different approach
                move || sync_rx.recv()
            }).await {
                Ok(Ok(event)) => {
                    if async_tx.send(event).await.is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    });

    // Actually, simpler approach - just use blocking recv in a dedicated thread:
    std::thread::spawn(move || {
        while let Ok(event) = sync_rx.recv() {
            if async_tx.blocking_send(event).is_err() {
                break;
            }
        }
    });

    watcher.watch(agents_dir, RecursiveMode::Recursive)?;
    watcher.watch(pending_dir, RecursiveMode::Recursive)?;

    Ok((watcher, async_rx))
}
```

Note: This bridge thread is internal to the watcher setup, similar to how notify itself spawns internal threads. It's not visible to the main event loop.

### Removing Periodic Scans

With proper FS event handling, we remove all scans:

| Removed | Replaced by |
|---------|-------------|
| `scan_agents()` | FS event: `agents/<name>/` created/removed |
| `scan_outputs()` | FS event: `agents/<name>/response.json` created |
| `scan_pending()` | FS event: `pending/<uuid>/task.json` created |

The only periodic operation is health check checking, handled by the timer in `select!`.

### Shutdown Handling

```rust
async fn drain_and_shutdown(
    mut fs_rx: mpsc::Receiver<Event>,
    state: &mut PoolState,
) -> io::Result<()> {
    info!(in_flight = state.in_flight_count(), "draining in-flight tasks");

    while state.in_flight_count() > 0 {
        match tokio::time::timeout(Duration::from_secs(30), fs_rx.recv()).await {
            Ok(Some(event)) => {
                handle_fs_event(&event, state).await?;
            }
            Ok(None) => {
                warn!("fs channel closed during shutdown");
                break;
            }
            Err(_) => {
                warn!(
                    in_flight = state.in_flight_count(),
                    "shutdown drain timeout"
                );
                break;
            }
        }
    }

    info!("shutdown complete");
    Ok(())
}
```

---

## Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
tokio = { version = "1", features = ["net", "sync", "time", "rt-multi-thread", "macros"] }
tokio-util = "0.7"  # For CancellationToken
```

Remove or keep `interprocess` depending on whether we still need cross-platform named pipes.

---

## Migration Steps

1. Add tokio dependencies
2. Create `async fn event_loop()` with `tokio::select!`
3. Update `create_watcher()` to bridge notify → tokio channel
4. Convert `read_task()` to async
5. Update `run()` entry point to use `#[tokio::main]` or `Runtime::block_on()`
6. Remove `scan_agents()`, `scan_outputs()`, `scan_pending()` from periodic calls
7. Update tests to use tokio test runtime

---

## Open Questions

1. **Keep `interprocess` crate?** It provides cross-platform socket abstraction. With tokio, we could use `tokio::net::UnixListener` directly on Unix. Windows would need separate handling.

2. **Debouncing?** Should we debounce FS events like Isograph does with `notify_debouncer_full`? Probably not necessary - we process events immediately, and duplicate events are harmless (idempotent handlers).

3. **Error handling in select arms?** Currently using `?` which will exit the loop. May want more granular error handling per event type.
