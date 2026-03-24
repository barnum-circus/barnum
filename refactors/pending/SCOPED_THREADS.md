# Scoped Threads and Reference-Based Worker Dispatch

## Motivation

The runner's worker dispatch uses `thread::spawn`, which requires `'static` bounds on everything moved into the thread. This forces `ShellAction` to own clones of data that the `Engine` already holds (config JSON, working directory). Meanwhile, `ShellAction::start` uses `Arc<Mutex<Child>>` to share a child process between a reader thread and a drop guard. Both of these are ownership workarounds for the `'static` constraint. `thread::scope` eliminates that constraint by guaranteeing all spawned threads join before the scope exits, so threads can borrow from the enclosing stack.

## Current state

### spawn_worker (action.rs:126-144)

```rust
pub fn spawn_worker(
    tx: mpsc::Sender<WorkerResult>,
    action: Box<dyn Action>,
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    timeout: Option<Duration>,
) {
    thread::spawn(move || {
        let value = task.value.clone();
        let output = run_action(action, &value.0, timeout);
        let _ = tx.send(WorkerResult { task_id, task, kind, result: ActionResult { value, output } });
    });
}
```

Uses `thread::spawn`. Everything is moved in.

### ShellAction (action.rs:171-176)

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub config: serde_json::Value,    // cloned from Engine
    pub working_dir: PathBuf,         // cloned from Engine
}
```

Both `config` and `working_dir` are clones of data the Engine already owns. These clones exist because `thread::spawn` requires `'static`.

### Arc\<Mutex\<Child\>\> (action.rs:148-247)

```rust
struct ProcessGuard {
    child: Arc<Mutex<Child>>,
}

// In ShellAction::start:
let child = Arc::new(Mutex::new(child));
let child_for_reader = Arc::clone(&child);
thread::spawn(move || {
    // ... read stdout/stderr ...
    let status = child_for_reader.lock().expect("...").wait();
    // ...
});
ActionHandle::new(rx, ProcessGuard { child })
```

The `Child` is shared between two threads: a reader thread (calls `.wait()`) and a `ProcessGuard` (calls `.kill()` on drop for timeout cancellation). `Arc<Mutex<Child>>` exists because both threads need access to the same process handle.

### Engine dispatch (mod.rs:694-741)

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    // ...
    let action = Box::new(ShellAction {
        script: script.clone(),
        step_name: task.step.clone(),
        config: self.config_json.clone(),      // clone
        working_dir: self.working_dir.clone(),  // clone
    });
    spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
}
```

Both `dispatch_task` and `dispatch_finally` clone config and working_dir into every ShellAction.

## Changes

### 1. Extract immutable context from Engine

**File:** `crates/barnum_config/src/runner/mod.rs`

The Engine holds both mutable state (RunState, in_flight, etc.) and immutable data (config, config_json, working_dir, step_map). Scoped threads need shared borrows to the immutable data while the main loop mutates the Engine. Splitting these avoids borrow conflicts.

```rust
/// Immutable data shared by all worker threads via reference.
struct RunContext<'a> {
    config: &'a Config,
    config_json: serde_json::Value,
    step_map: HashMap<&'a StepName, &'a Step>,
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
}

struct Engine<'a> {
    ctx: RunContext<'a>,
    state: RunState,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

`RunContext` is constructed once and never mutated. The main loop borrows `&self.ctx` (shared) while mutating `self.state` and `self.in_flight` (exclusive). Scoped threads borrow from `RunContext`.

### 2. Scoped threads in run_loop

**File:** `crates/barnum_config/src/runner/mod.rs`

Wrap the main loop in `thread::scope`. Pass the scope handle to `flush_dispatches` and down to `spawn_worker`.

```rust
fn run_loop(
    engine: &mut Engine<'_>,
    rx: &mpsc::Receiver<WorkerResult>,
    log_writer: &mut io::BufWriter<std::fs::File>,
) -> io::Result<()> {
    thread::scope(|scope| {
        let mut completed_count = 0u32;
        loop {
            if engine.is_done() { break; }
            engine.flush_dispatches(scope);
            let result = rx.recv().expect("[P062] channel closed while tasks in flight");
            let entries = engine.process_worker_result(result);
            for entry in &entries {
                write_log(log_writer, entry);
            }
            completed_count += 1;
            // ... logging ...
        }
        engine.compute_result()
    })
}
```

All worker threads join when the scope exits (after the loop breaks).

### 3. ShellAction borrows instead of cloning

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
pub struct ShellAction<'a> {
    pub script: String,
    pub step_name: StepName,
    pub config: &'a serde_json::Value,
    pub working_dir: &'a Path,
}
```

`config` and `working_dir` are now references into `RunContext`. The lifetime `'a` is the scope's lifetime, which is valid because `RunContext` outlives the scope.

### 4. spawn_worker takes a Scope

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
pub fn spawn_worker<'scope, 'env: 'scope>(
    scope: &'scope thread::Scope<'scope, 'env>,
    tx: mpsc::Sender<WorkerResult>,
    action: Box<dyn Action + 'env>,
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    timeout: Option<Duration>,
) {
    scope.spawn(move || {
        let value = task.value.clone();
        let output = run_action(action, &value.0, timeout);
        let _ = tx.send(WorkerResult { task_id, task, kind, result: ActionResult { value, output } });
    });
}
```

The `Action` trait bound changes from `Send` to `Send + 'env` (the environment lifetime of the scope). This allows actions to hold references to data in the enclosing scope.

### 5. Action trait gains a lifetime

**File:** `crates/barnum_config/src/runner/action.rs`

The `Action` trait currently requires `Send` (which implies `'static` for `thread::spawn`). With scoped threads, actions can hold non-`'static` references.

The trait itself doesn't need a lifetime parameter — the lifetime constraint lives on `spawn_worker`'s `Box<dyn Action + 'env>`. The trait stays:

```rust
pub trait Action: Send {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle;
}
```

`ShellAction<'a>` implements `Action` and is `Send` as long as `'a` references are `Sync` (which `serde_json::Value` and `Path` are).

### 6. Replace Arc\<Mutex\<Child\>\> with PID-based kill

**File:** `crates/barnum_config/src/runner/action.rs`

The reader thread inside `ShellAction::start` is not a scoped thread — it's a nested `thread::spawn` inside the worker thread. The reader thread must outlive `run_action`'s timeout so that dropping the `ActionHandle` can trigger cancellation while the reader continues until the child dies. Scoped threads don't help here because the scope would block until the reader finishes, preventing the timeout return.

Instead, eliminate `Arc<Mutex<Child>>` by splitting the child into two concerns: the reader thread owns the `Child` handle (for `.wait()`), and the drop guard stores only the PID (for `kill`).

```rust
struct ProcessGuard {
    pid: u32,
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(self.pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
```

In `ShellAction::start`:

```rust
let pid = child.id();

// Reader thread owns the child directly.
thread::spawn(move || {
    // ... read stdout/stderr ...
    let status = child.wait();
    let result = match status {
        Ok(s) if s.success() => Ok(stdout_data),
        Ok(_) => Err(stderr_data),
        Err(e) => Err(e.to_string()),
    };
    let _ = tx.send(result);
});

ActionHandle::new(rx, ProcessGuard { pid })
```

No `Arc`, no `Mutex`. The reader thread has sole ownership of `Child`. The guard kills by PID, which is safe because:
- If the process already exited, `kill` returns an error that we ignore.
- If the PID was reused by the OS, we might kill the wrong process, but this is the same race condition that `Child::kill()` has (the stdlib implementation calls `kill` on the PID).

This requires a `libc` dependency (already in the dependency tree via other crates, but needs a direct dependency if not already present).

## Tests

Existing integration tests (`branching_transitions`, `concurrency`, `edge_cases`, `retry_behavior`, etc.) exercise the full `run` and `resume` paths. If scoped threads are wired correctly, all existing tests pass without modification. No new tests are needed — the behavioral contract is unchanged.

The `Arc<Mutex<Child>>` to PID-based kill change should be verified by the existing timeout tests in `retry_behavior.rs` (`timeout_retry_exhausts_max_retries`, `retry_on_timeout_false_drops_task`), which exercise the cancellation path.

## What this does NOT do

- Does not change the Engine's scheduling logic or state machine.
- Does not change the Action trait's API contract (start, ActionHandle, cancellation via drop).
- Does not change the envelope format or any user-facing behavior.
- Does not restructure the reader thread inside `ShellAction::start` to use scoped threads (that thread must outlive the timeout for cancellation to work).
