# Scoped Threads and PID-Based Kill Guard

## Motivation

`ShellAction` clones `config: serde_json::Value` (the full workflow config) and `working_dir: PathBuf` on every dispatch because `thread::spawn` requires `'static`. With `thread::scope`, worker threads can borrow from the enclosing scope, eliminating per-dispatch clones of the config.

Separately, `ShellAction::start` uses `Arc<Mutex<Child>>` to share a child process handle between a reader thread and a drop guard. The guard only needs the PID to kill the process — it doesn't need the `Child` handle at all.

## Current state

### ShellAction (action.rs:171-176)

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub config: serde_json::Value,    // cloned from Engine every dispatch
    pub working_dir: PathBuf,         // cloned from Engine every dispatch
}
```

### Engine dispatch (mod.rs)

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    // ...
    let action = Box::new(ShellAction {
        script: script.clone(),
        step_name: task.step.clone(),
        config: self.config_json.clone(),      // full workflow config cloned
        working_dir: self.working_dir.clone(),
    });
    spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
}
```

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

Two threads access the same `Child`: the reader thread (`.wait()`) and `ProcessGuard` (`.kill()` on drop). The Mutex serializes access.

## Changes

### 1. Extract immutable context from Engine

**File:** `crates/barnum_config/src/runner/mod.rs`

Scoped threads hold shared borrows to Engine data while the main loop mutates Engine state. Splitting immutable data into a separate struct allows disjoint field borrowing.

```rust
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

The main loop borrows `&engine.ctx` (shared, for spawning threads) while mutating `engine.state` and `engine.in_flight` (exclusive). Rust allows disjoint field borrows.

### 2. Scoped threads in run_loop

**File:** `crates/barnum_config/src/runner/mod.rs`

Wrap the main loop in `thread::scope`. Pass the scope handle through to `spawn_worker`.

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
            // ...
        }
        engine.compute_result()
    })
}
```

### 3. ShellAction borrows instead of cloning

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
pub struct ShellAction<'a> {
    pub script: String,
    pub step_name: StepName,
    pub config: &'a serde_json::Value,
    pub working_dir: &'a Path,
    pub step_config: Option<serde_json::Value>,
}
```

`config` and `working_dir` are references into `RunContext`. The lifetime is the scope's lifetime, valid because `RunContext` outlives the scope.

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

### 5. Replace Arc\<Mutex\<Child\>\> with PID-based kill

**File:** `crates/barnum_config/src/runner/action.rs`

The reader thread inside `ShellAction::start` is a nested `thread::spawn` (not scoped) because it must outlive `run_action`'s timeout for cancellation to work. Scoped threads don't apply here. Instead, give the reader thread sole ownership of `Child` and use the PID for the kill guard.

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
let stdout = child.stdout.take();
let stderr = child.stderr.take();

// Reader thread owns child directly.
thread::spawn(move || {
    let stdout_data = stdout
        .map(|mut r| { let mut s = String::new(); r.read_to_string(&mut s).ok(); s })
        .unwrap_or_default();
    let stderr_data = stderr
        .map(|mut r| { let mut s = String::new(); r.read_to_string(&mut s).ok(); s })
        .unwrap_or_default();

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

### 6. Add libc dependency

**File:** `crates/barnum_config/Cargo.toml`

Add `libc` as a direct dependency (already transitive in the lock file).

## Tests

Existing integration tests exercise the full run/resume/timeout paths. The behavioral contract is unchanged. The timeout tests in `retry_behavior.rs` validate the PID-based kill.

## What this does NOT do

- Does not change the Engine's scheduling logic or state machine.
- Does not change the Action trait's API contract.
- Does not change the envelope format or any user-facing behavior.
