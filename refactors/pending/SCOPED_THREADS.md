# Replace Arc\<Mutex\<Child\>\> with PID-Based Kill Guard

## Motivation

`ShellAction::start` uses `Arc<Mutex<Child>>` to share a child process handle between a reader thread and a drop guard. The reader thread calls `.wait()`, the drop guard calls `.kill()` for timeout cancellation. This is the last `Arc` in the runner. The two concerns (waiting for exit vs. killing on timeout) don't need shared access to the same `Child` handle — the guard only needs the PID.

## Current state

### ProcessGuard and ShellAction::start (action.rs:148-247)

```rust
struct ProcessGuard {
    child: Arc<Mutex<Child>>,
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}
```

In `ShellAction::start`:

```rust
let stdout = child.stdout.take();
let stderr = child.stderr.take();
let child = Arc::new(Mutex::new(child));

let child_for_reader = Arc::clone(&child);
thread::spawn(move || {
    let stdout_data = stdout.map(|mut r| { /* read */ }).unwrap_or_default();
    let stderr_data = stderr.map(|mut r| { /* read */ }).unwrap_or_default();

    let status = child_for_reader
        .lock()
        .expect("[P080] child mutex poisoned")
        .wait();
    let result = match status {
        Ok(s) if s.success() => Ok(stdout_data),
        Ok(_) => Err(stderr_data),
        Err(e) => Err(e.to_string()),
    };
    let _ = tx.send(result);
});

ActionHandle::new(rx, ProcessGuard { child })
```

The `Arc<Mutex<Child>>` exists because two threads access the same `Child`: the reader thread (`.wait()`) and the `ProcessGuard` (`.kill()` on drop). The `Mutex` serializes access. This is correct but heavier than necessary.

## Changes

### 1. PID-based ProcessGuard

**File:** `crates/barnum_config/src/runner/action.rs`

The guard stores the PID instead of a shared handle. It calls `libc::kill` with `SIGKILL` to terminate the process on drop.

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

`Child::kill()` in the stdlib does the same thing — it calls `libc::kill` on the stored PID. The PID race (OS reuses the PID after the process exits) exists in both implementations equally. If the process already exited, `kill` returns `ESRCH`, which we ignore.

### 2. Reader thread owns Child

**File:** `crates/barnum_config/src/runner/action.rs`

The reader thread takes sole ownership of `Child`. No shared state, no mutex.

```rust
let pid = child.id();

// Take pipes before moving child into the reader thread.
let stdout = child.stdout.take();
let stderr = child.stderr.take();

thread::spawn(move || {
    let stdout_data = stdout
        .map(|mut r| {
            let mut s = String::new();
            r.read_to_string(&mut s).ok();
            s
        })
        .unwrap_or_default();
    let stderr_data = stderr
        .map(|mut r| {
            let mut s = String::new();
            r.read_to_string(&mut s).ok();
            s
        })
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

### 3. Add libc dependency

**File:** `crates/barnum_config/Cargo.toml`

Add `libc` as a direct dependency. It's already in the dependency tree transitively.

### 4. Remove Arc import

**File:** `crates/barnum_config/src/runner/action.rs`

`Arc` and `Mutex` are no longer used in this file. Remove both imports.

## Tests

The existing timeout tests in `retry_behavior.rs` (`timeout_retry_exhausts_max_retries`, `retry_on_timeout_false_drops_task`) exercise the cancellation path — they verify that timed-out actions are killed and retried. These tests validate the PID-based kill without modification.

## What this does NOT do

- Does not change the Action trait or ActionHandle API.
- Does not change spawn_worker or the Engine's dispatch logic.
- Does not change the envelope format or any user-facing behavior.
