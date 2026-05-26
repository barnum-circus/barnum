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

### Frame tree (`crates/barnum_engine/src/lib.rs`)

The engine already tracks in-flight tasks via `task_to_frame: BTreeMap<TaskId, FrameId>`. This tells us exactly which tasks are in-flight at any moment.

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

### 1. Extract PID from spawned child

`execute_typescript` currently consumes the `Child` internally. Modify it to capture `child.id()` (returns `Option<u32>`) immediately after spawn and return it alongside the result.

Two approaches:

**Option A: Return PID before awaiting.**

Split into `spawn_typescript` (returns `(u32, Child)` — sets up stdin, returns PID and child ready for await) and `await_typescript` (reads stdout, waits for exit, returns result). The scheduler calls spawn, records PID, then spawns a tokio task that calls await.

```rust
// barnum_typescript_handler/src/lib.rs

pub struct SpawnedHandler {
    pub pid: u32,
    pub child: Child,
    pub stderr_task: JoinHandle<Vec<u8>>,
}

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

    // Write input and close stdin
    let mut stdin = child.stdin.take().expect("no stdin");
    let input = serde_json::to_vec(&serde_json::json!({ "value": value }))
        .expect("serialize failed");
    // Note: stdin write needs to be async — handle via spawn_blocking or
    // restructure to write in the caller's tokio task before spawning the
    // await task.

    // ... stderr forwarding setup ...

    SpawnedHandler { pid, child, stderr_task }
}

pub async fn await_typescript(
    spawned: SpawnedHandler,
    module: &str,
    func: &str,
) -> Result<Value, TypeScriptHandlerError> {
    let output = spawned.child.wait_with_output().await.expect("wait failed");
    let stderr_bytes = spawned.stderr_task.await.expect("stderr task failed");
    // ... same exit code / JSON parsing logic ...
}
```

**Option B: Callback/channel for PID notification.**

Keep `execute_typescript` as one function, but pass a `oneshot::Sender<u32>` that receives the PID immediately after spawn. Simpler change, no API split.

```rust
pub async fn execute_typescript(
    executor: &str,
    worker_path: &str,
    module: &str,
    func: &str,
    value: &Value,
    pid_tx: oneshot::Sender<u32>,
) -> Result<Value, TypeScriptHandlerError> {
    let mut child = Command::new("sh") /* ... */ .spawn().expect("...");
    let pid = child.id().expect("child has no pid");
    let _ = pid_tx.send(pid);
    // ... rest unchanged ...
}
```

**Recommendation: Option A.** The split makes the lifecycle explicit — spawn is synchronous (or nearly so), await is async. The scheduler owns the spawn step and naturally sees the PID. Option B threads a channel through for no structural benefit.

### 2. PID registry in Scheduler

Add a `HashMap<TaskId, u32>` to `Scheduler` tracking in-flight child PIDs. Wrap in `Arc<Mutex<_>>` so both the main loop (for signal handler access) and the spawned tokio tasks (for cleanup on completion) can access it.

```rust
pub struct Scheduler {
    result_tx: mpsc::UnboundedSender<(TaskId, Result<Value, HandlerError>)>,
    result_rx: mpsc::UnboundedReceiver<(TaskId, Result<Value, HandlerError>)>,
    executor: String,
    worker_path: String,
    in_flight_pids: Arc<Mutex<HashMap<TaskId, u32>>>,
}
```

- On dispatch: `in_flight_pids.insert(task_id, pid)`
- On completion received (in `run_workflow` loop): `in_flight_pids.remove(&task_id)`

### 3. SIGTERM handler

Install a signal handler (via `tokio::signal` or `ctrlc` crate) in the CLI entry point (`crates/barnum_cli/src/main.rs`). On SIGTERM/SIGINT:

1. Lock `in_flight_pids`
2. For each PID: `unsafe { libc::kill(pid as i32, libc::SIGTERM) }`
3. Exit the process

```rust
// In main.rs, before run_workflow:
let pids = scheduler.in_flight_pids();
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    let pids = pids.lock().unwrap();
    for &pid in pids.values() {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
    }
    std::process::exit(130); // 128 + SIGINT
});
```

For SIGTERM specifically (not just Ctrl+C):

```rust
use tokio::signal::unix::{signal, SignalKind};

let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
tokio::spawn(async move {
    sigterm.recv().await;
    // kill children, exit
});
```

### 4. Worker-side EPIPE handling stays

Keep the existing EPIPE suppression in `worker.ts` as a belt-and-suspenders fallback. If the signal handler somehow misses a child (race between spawn and signal), EPIPE still provides eventual cleanup.

## Edge cases

- **Race: signal arrives between spawn and PID registration.** The child exists but isn't in the registry. EPIPE fallback handles it. Acceptable.
- **Race: child exits naturally the same instant as signal.** `kill()` on a dead PID is a no-op (ESRCH). Safe.
- **`sh -c` wrapper PID vs actual node PID.** `child.id()` returns the PID of `sh`, not `node`/`tsx`. Killing `sh` should propagate to its child because `sh -c` exec's the command (replaces itself). If for some reason it doesn't, the actual node process becomes orphaned and hits EPIPE. Belt-and-suspenders.
- **Process group kill alternative.** Instead of tracking individual PIDs, spawn all children in a dedicated process group and kill the group. Simpler registry (just the pgid), one kill call. Trade-off: requires `.process_group(0)` on spawn (creates new group) or inheriting a shared group. Worth considering as a simplification.

## Open questions

1. **Process group vs individual PID tracking?** A process group kill is one `killpg()` call instead of iterating. Simpler. But requires all children to share a group, which means either: (a) barnum creates a new process group for itself at startup (`setsid`), or (b) each child is spawned into a shared group. Option (a) means killing barnum's group also kills barnum itself — which is fine since we're in the signal handler and about to exit anyway.

2. **Graceful timeout?** Send SIGTERM, wait N ms, then SIGKILL stragglers? Or just SIGTERM and exit immediately? For handler subprocesses that are stateless (no persistent side effects), immediate SIGTERM is fine. Handlers that write to disk (like the queue demos) might leave partial state — but that's already the crash-recovery concern that the claim-and-complete strategy addresses.

3. **Should the scheduler expose a `kill_all()` method** for use in non-signal contexts (e.g., workflow timeout, fatal error)? Probably yes — the signal handler is just one caller.
