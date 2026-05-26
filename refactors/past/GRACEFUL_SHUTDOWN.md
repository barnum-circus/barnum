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

### 1. `ProcessGroup` newtype

```rust
#[derive(Debug, Clone, Copy)]
pub struct ProcessGroup(pub u32);
```

### 2. Scheduler holds the process group

The Scheduler already holds `executor` and `worker_path`. It also holds the `ProcessGroup` and passes it to spawn:

```rust
// crates/barnum_event_loop/src/lib.rs

pub struct Scheduler {
    executor: String,
    worker_path: String,
    process_group: ProcessGroup,  // <-- new field
    result_tx: mpsc::Sender<(TaskId, Result<Value, HandlerError>)>,
    result_rx: mpsc::Receiver<(TaskId, Result<Value, HandlerError>)>,
}
```

```rust
// In execute_typescript, the only spawn-site change:
use std::os::unix::process::CommandExt;

let mut child = Command::new("sh")
    .arg("-c")
    .arg(format!("{executor} {worker_path} {module} {func}"))
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .process_group(process_group.0)  // <-- one line added
    .spawn()
    .expect("failed to spawn handler process");
```

### 3. SIGTERM/SIGINT handler

The CLI creates the `ProcessGroup`, passes it to the Scheduler, and uses the same value in signal handlers:

```rust
// crates/barnum_cli/src/main.rs
use tokio::signal::unix::{signal, SignalKind};

let process_group = ProcessGroup::current();

// Ctrl+C
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    unsafe { libc::killpg(process_group.0 as i32, libc::SIGTERM); }
    std::process::exit(130);
});

// SIGTERM
let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
tokio::spawn(async move {
    sigterm.recv().await;
    unsafe { libc::killpg(process_group.0 as i32, libc::SIGTERM); }
    std::process::exit(143);
});
```

### 4. Worker-side EPIPE handling stays

Keep the existing EPIPE suppression in `worker.ts` as belt-and-suspenders.

## Edge cases

- **Child exits naturally the same instant as signal.** `killpg()` on a group with dead members is fine — dead PIDs are already removed from the group by the OS.
- **`sh -c` wrapper.** All processes in the group get the signal — both the shell and its child. No reliance on exec behavior.
- **Grandchildren.** If a handler spawns its own subprocesses, they inherit the process group and are also killed. This is desirable.

## Open questions

1. **`kill_all()` for non-signal contexts?** Workflow timeout, fatal validation error, or user cancellation could all use `killpg()`. The signal handler is just one caller.
