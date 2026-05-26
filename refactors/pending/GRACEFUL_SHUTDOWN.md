# Graceful Shutdown: Kill In-Flight Children on SIGTERM

## Motivation

When the barnum binary is killed (e.g., `pkill barnum`, Ctrl+C), in-flight handler subprocesses continue running until they try to write to stdout and hit EPIPE. A handler executing a long-running operation (LLM call, network request, `sleep`) can zombie for seconds or minutes — consuming resources with nobody listening for its result.

The parent process owns the children. The parent should kill the children.

## Current state

### Process spawning (`crates/barnum_typescript_handler/src/lib.rs`)

`execute_typescript` spawns a child, writes stdin, reads stdout, and awaits exit — all in one async function. The `Child` handle is consumed internally. No PID is retained or returned.

```rust
let mut child = Command::new("sh")
    .arg("-c")
    .arg(format!("{executor} {worker_path} {module} {func}"))
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .spawn()
    .expect("failed to spawn handler process");

// ... writes stdin, reads stdout, awaits exit ...
let output = child.wait_with_output().await.expect("wait failed");
```

### Scheduler dispatch (`crates/barnum_event_loop/src/lib.rs:129-135`)

Each TypeScript handler dispatch spawns a tokio task that calls `execute_typescript` and sends the result through an mpsc channel. The spawned task owns the child's lifetime. No PID tracking exists.

```rust
tokio::spawn(async move {
    let result = execute_typescript(&executor, &worker_path, &module, &func, &value)
        .await
        .map_err(HandlerError::from);
    let _ = result_tx.send((task_id, result));
});
```

### Frame tree (`crates/barnum_engine/src/frame.rs`)

`FrameKind::Invoke` is the leaf frame for in-flight handler invocations:

```rust
Invoke {
    handler: HandlerId,
},
```

The engine tracks which tasks are in-flight via `task_to_frame: BTreeMap<TaskId, FrameId>`. Each `TaskId` maps to a `FrameId` pointing at an `Invoke` frame. A handler is a *definition* (module, func, schemas); a frame is an *invocation* (a specific execution of that handler). The same handler can have many concurrent invocations, each with its own frame and its own subprocess PID.

### Current orphan handling (`libs/barnum/src/worker.ts:22-29`)

Workers suppress EPIPE on stdout and exit cleanly. This is the lazy fallback — workers self-detect their own orphaning when they try to write, which may be arbitrarily delayed.

```typescript
process.stdout.on("error", (error: NodeJS.ErrnoException) => {
  if (error.code === "EPIPE") {
    process.exit(0);
  }
  throw error;
});
```

## Proposed changes

### 1. Store PID on the Invoke frame

The PID is per-invocation state. It belongs on `FrameKind::Invoke` — the frame that represents "a handler invocation in flight." Add a `pid` field:

```rust
// crates/barnum_engine/src/frame.rs
Invoke {
    handler: HandlerId,
    pid: Option<u32>,  // None for builtins, Some for TypeScript subprocesses
},
```

`None` for builtins (they run inline in a tokio task, no subprocess). `Some(pid)` for TypeScript handlers (each invocation spawns a subprocess).

This is the only change to the engine crate. No new data structures, no parallel maps. The frame tree already represents "what's in flight" — the PID is just one more field on the invocation.

### 2. Extract PID from spawned child

`execute_typescript` currently consumes the `Child` internally. Split it so the PID is available before the await:

```rust
// crates/barnum_typescript_handler/src/lib.rs

pub struct SpawnedHandler {
    pub pid: u32,
    pub child: Child,
    pub stderr_task: JoinHandle<Vec<u8>>,
}

/// Spawn the subprocess and write input to stdin. Returns immediately
/// with the PID and child handle ready for awaiting.
pub fn spawn_typescript(
    executor: &str,
    worker_path: &str,
    module: &str,
    func: &str,
    value: &Value,
) -> SpawnedHandler {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("{executor} {worker_path} {module} {func}"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn handler process");

    let pid = child.id().expect("child has no pid");

    // Write input and close stdin synchronously (small payload, won't block)
    let mut stdin = child.stdin.take().expect("no stdin");
    let input = serde_json::to_vec(&serde_json::json!({ "value": value }))
        .expect("serialize failed");
    // stdin write is blocking here — acceptable for small JSON payloads.
    // If this becomes a concern, move to spawn_blocking or async write
    // before handing off to the await task.

    // stderr forwarding setup...
    let mut stderr_handle = child.stderr.take().expect("no stderr");
    let stderr_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stderr_handle.read(&mut buf).await.unwrap_or(0);
            if n == 0 { break; }
            collected.extend_from_slice(&buf[..n]);
            tokio::io::stderr().write_all(&buf[..n]).await.ok();
        }
        collected
    });

    SpawnedHandler { pid, child, stderr_task }
}

/// Await the spawned handler's completion and parse its result.
pub async fn await_typescript(
    spawned: SpawnedHandler,
    module: &str,
    func: &str,
) -> Result<Value, TypeScriptHandlerError> {
    let output = spawned.child.wait_with_output().await.expect("wait failed");
    let stderr_bytes = spawned.stderr_task.await.expect("stderr task failed");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_owned();
        return Err(TypeScriptHandlerError::SubprocessFailed {
            module: module.to_owned(),
            func: func.to_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            stderr,
        });
    }

    serde_json::from_slice(&output.stdout).map_err(|source| {
        TypeScriptHandlerError::InvalidOutput {
            module: module.to_owned(),
            func: func.to_owned(),
            source,
        }
    })
}
```

### 3. Scheduler returns PID, run_workflow stores it

`Scheduler::dispatch` returns `Option<u32>` — the child PID for TypeScript handlers, `None` for builtins. The Scheduler doesn't touch WorkflowState. `run_workflow` (which already has `&mut WorkflowState`) stores the PID on the frame:

```rust
// crates/barnum_event_loop/src/lib.rs — Scheduler::dispatch

/// Returns the child PID if a subprocess was spawned (TypeScript handlers),
/// or None for builtins (inline tokio tasks).
pub fn dispatch(&self, dispatch_event: &DispatchEvent, handler: &HandlerKind) -> Option<u32> {
    match handler {
        HandlerKind::Builtin(_) => {
            // ... existing builtin dispatch (tokio::spawn inline) ...
            None
        }
        HandlerKind::TypeScript(ts) => {
            let module = ts.module.lookup().to_owned();
            let func = ts.func.lookup().to_owned();
            let value = dispatch_event.value.clone();
            let executor = self.executor.clone();
            let worker_path = self.worker_path.clone();

            let spawned = spawn_typescript(&executor, &worker_path, &module, &func, &value);
            let pid = spawned.pid;

            let result_tx = self.result_tx.clone();
            tokio::spawn(async move {
                let result = await_typescript(spawned, &module, &func)
                    .await
                    .map_err(HandlerError::from);
                let _ = result_tx.send((task_id, result));
            });

            Some(pid)
        }
    }
}
```

```rust
// In run_workflow's dispatch path:
let pid = scheduler.dispatch(&dispatch_event, handler);
if let Some(pid) = pid {
    workflow_state.set_invoke_pid(dispatch_event.task_id, pid);
}
```

### 4. Clear PID on completion

In the `run_workflow` loop, when a completion is received and processed, clear the PID from the frame. This happens naturally when the frame is removed from the arena during `complete()`. No explicit cleanup needed — the frame (and its PID field) is deallocated.

### 5. SIGTERM/SIGINT handler

Install signal handlers in the CLI entry point. On signal, walk the frame arena and kill every `Invoke` frame's PID:

```rust
// crates/barnum_cli/src/main.rs

// The frame arena needs to be accessible from the signal handler.
// Since run_workflow owns &mut WorkflowState, expose the PID list
// via a shared handle.

use tokio::signal::unix::{signal, SignalKind};

// Before run_workflow:
let pid_list = workflow_state.pid_list_handle(); // Arc<Mutex<Vec<u32>>> or similar

// Ctrl+C
let pids = Arc::clone(&pid_list);
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    for &pid in pids.lock().unwrap().iter() {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
    }
    std::process::exit(130);
});

// SIGTERM
let pids = Arc::clone(&pid_list);
let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
tokio::spawn(async move {
    sigterm.recv().await;
    for &pid in pids.lock().unwrap().iter() {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
    }
    std::process::exit(143);
});
```

The `pid_list_handle()` method on WorkflowState returns a shared reference to the list of active PIDs. Implementation options:
- WorkflowState owns an `Arc<Mutex<Vec<u32>>>` that's updated on frame create/destroy
- Or: the signal handler directly accesses the frame arena (requires Arc<Mutex<Arena>>)
- Or: a standalone `Arc<Mutex<Vec<u32>>>` maintained by `run_workflow` at dispatch/completion boundaries (keeps WorkflowState free of concurrency primitives)

The last option is cleanest — WorkflowState stays a plain struct, and `run_workflow` maintains the shared PID list as a local concern of the event loop.

### 6. Worker-side EPIPE handling stays

Keep the existing EPIPE suppression in `worker.ts` as belt-and-suspenders. If the signal handler misses a child (race between spawn and PID registration), EPIPE still provides eventual cleanup.

## Edge cases

- **Race: signal arrives between spawn and PID registration.** The child exists but isn't in the list yet. EPIPE fallback handles it.
- **Race: child exits naturally the same instant as signal.** `kill()` on a dead PID returns ESRCH. Safe — ignore the error.
- **`sh -c` wrapper PID vs actual node PID.** `child.id()` returns the PID of `sh`. On most systems, `sh -c <single command>` exec's the command (replaces itself), so the PID is the actual node process. If it doesn't exec (e.g., the command has pipes/redirects), killing the shell sends SIGTERM to it, which terminates the shell and its child gets SIGHUP. Either way, cleanup happens.
- **Frame torn down before signal.** If a race is won and losing frames are deallocated, their PIDs are gone from the arena. The orphaned workers hit EPIPE and self-terminate. No stale PIDs in the list to accidentally kill a recycled PID.

## Open questions

1. **Process group vs individual PID tracking?** Spawning all children into a shared process group means one `killpg()` call instead of iterating. Requires `.process_group(pgid)` on spawn or barnum calling `setsid` at startup. Simpler kill path, but adds spawn-time complexity. Individual PID kill is straightforward and the number of concurrent handlers is small (typically < 20).

2. **Graceful timeout?** SIGTERM then wait N ms then SIGKILL? For stateless handler subprocesses, immediate SIGTERM is sufficient. Handlers with persistent side effects (disk writes) are already addressed by crash-recovery patterns (claim-and-complete).

3. **`kill_all()` for non-signal contexts?** Workflow timeout, fatal validation error, or user cancellation could all use the same kill-all-children mechanism. The signal handler is just one caller. Worth exposing as a method on whatever owns the PID list.
