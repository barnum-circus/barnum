# Single Watcher Created at Entry Point

**Depends on:** `WAIT_FOR_POOL_READY_WATCHER.md`

## Motivation

Multiple functions create their own `VerifiedWatcher` internally. This is wasteful. Create one at the entry point (CLI or daemon) and pass it down.

## Current State (Before)

### worker.rs - `wait_for_task`

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

    let metadata = name.map_or_else(|| "{}".to_string(), |n| format!(r#"{{"name":"{n}"}}"#));
    fs::write(&ready, &metadata)?;

    // CREATES ITS OWN WATCHER
    let mut watcher = VerifiedWatcher::new(&agents_dir, std::slice::from_ref(&agents_dir))?;
    watcher.wait_for(&task, timeout)?;

    let content = fs::read_to_string(&task)?;
    Ok(TaskAssignment { uuid, content })
}
```

### submit/file.rs - `submit_file_with_timeout`

```rust
pub fn submit_file_with_timeout(
    root: impl AsRef<Path>,
    payload: &Payload,
    timeout: Duration,
) -> io::Result<Response> {
    let root = fs::canonicalize(root.as_ref())?;
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let submission_id = Uuid::new_v4().to_string();
    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let status_path = root.join(STATUS_FILE);

    // CREATES ITS OWN WATCHER
    let mut watcher = VerifiedWatcher::new(&root, std::slice::from_ref(&root))?;

    watcher.wait_for(&status_path, Some(POOL_READY_TIMEOUT))?;
    atomic_write_str(&root, &request_path, &content)?;
    watcher.wait_for(&response_path, Some(timeout))?;
    // ... read response, cleanup
}
```

### submit/socket.rs - `submit`

```rust
pub fn submit(root: impl AsRef<Path>, payload: &Payload) -> io::Result<Response> {
    let root = fs::canonicalize(root.as_ref())?;
    let status_path = root.join(STATUS_FILE);

    // CREATES ITS OWN WATCHER
    let mut watcher = VerifiedWatcher::new(&root, std::slice::from_ref(&root))?;
    watcher.wait_for(&status_path, Some(POOL_READY_TIMEOUT))?;

    // ... socket connection and communication
}
```

### submit/mod.rs - `wait_for_pool_ready`

```rust
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    let root = root.as_ref();
    let status_path = root.join(STATUS_FILE);
    let start = Instant::now();

    // POLLS WITH THREAD::SLEEP - NO WATCHER AT ALL
    while !status_path.exists() {
        if start.elapsed() > timeout {
            return Err(io::Error::new(io::ErrorKind::TimedOut, ...));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
```

### agent_pool_cli/src/main.rs - `wait_for_status_file`

```rust
fn wait_for_status_file(status_file: &std::path::Path) -> bool {
    const TIMEOUT: Duration = Duration::from_secs(5);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    let start = std::time::Instant::now();
    // POLLS WITH THREAD::SLEEP
    while start.elapsed() < TIMEOUT {
        if status_file.exists() {
            return true;
        }
        thread::sleep(POLL_INTERVAL);
    }
    false
}
```

### agent_pool_cli/src/main.rs - `Command::GetTask`

```rust
Command::GetTask { pool, name } => {
    let root = resolve_pool(&pool_root, &pool);

    // Uses polling function
    let status_file = root.join(STATUS_FILE);
    if !wait_for_status_file(&status_file) {
        eprintln!("Daemon not ready");
        return ExitCode::FAILURE;
    }

    // Then calls wait_for_task which creates ANOTHER watcher
    match wait_for_task(&root, name.as_deref(), None) {
        Ok(assignment) => { ... }
        Err(e) => { ... }
    }
}
```

### daemon/wiring.rs - `run_with_config`

```rust
pub fn run_with_config(root: impl AsRef<Path>, config: DaemonConfig) -> io::Result<Infallible> {
    // ... setup directories ...

    // Creates watcher with canaries in subdirectories
    let verified_watcher =
        VerifiedWatcher::new(&root, &[agents_dir.clone(), submissions_dir.clone()])?;
    let (_fs_watcher, fs_rx) = verified_watcher.into_receiver(Duration::from_secs(5))?;

    // ... watcher is consumed here, fs_rx passed to io_loop ...
}
```

---

## After

### worker.rs - `wait_for_task`

```rust
pub fn wait_for_task(
    watcher: &mut VerifiedWatcher,  // PASSED IN
    pool_root: &Path,
    name: Option<&str>,
    timeout: Option<Duration>,
) -> io::Result<TaskAssignment> {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let uuid = Uuid::new_v4().to_string();
    let ready = ready_path(&agents_dir, &uuid);
    let task = task_path(&agents_dir, &uuid);

    let metadata = name.map_or_else(|| "{}".to_string(), |n| format!(r#"{{"name":"{n}"}}"#));
    fs::write(&ready, &metadata)?;

    // USES PASSED WATCHER
    watcher.wait_for(&task, timeout)?;

    let content = fs::read_to_string(&task)?;
    Ok(TaskAssignment { uuid, content })
}
```

### submit/file.rs - `submit_file_with_timeout`

```rust
pub fn submit_file_with_timeout(
    watcher: &mut VerifiedWatcher,  // PASSED IN
    root: impl AsRef<Path>,
    payload: &Payload,
    timeout: Duration,
) -> io::Result<Response> {
    let root = fs::canonicalize(root.as_ref())?;
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let submission_id = Uuid::new_v4().to_string();
    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let status_path = root.join(STATUS_FILE);

    // USES PASSED WATCHER
    watcher.wait_for(&status_path, Some(POOL_READY_TIMEOUT))?;
    atomic_write_str(&root, &request_path, &content)?;
    watcher.wait_for(&response_path, Some(timeout))?;
    // ... read response, cleanup
}
```

### submit/socket.rs - `submit`

```rust
pub fn submit(
    watcher: &mut VerifiedWatcher,  // PASSED IN
    root: impl AsRef<Path>,
    payload: &Payload,
) -> io::Result<Response> {
    let root = fs::canonicalize(root.as_ref())?;
    let status_path = root.join(STATUS_FILE);

    // USES PASSED WATCHER
    watcher.wait_for(&status_path, Some(POOL_READY_TIMEOUT))?;

    // ... socket connection and communication
}
```

### submit/mod.rs - `wait_for_pool_ready`

**DELETED** - Callers use `watcher.wait_for(&status_path, Some(timeout))` directly.

### agent_pool_cli/src/main.rs - `wait_for_status_file`

**DELETED** - Uses watcher instead.

### agent_pool_cli/src/main.rs - `Command::GetTask`

```rust
Command::GetTask { pool, name } => {
    let root = resolve_pool(&pool_root, &pool);

    // CREATE WATCHER AT CLI ENTRY POINT
    // Single canary at root - directories already exist (daemon created them)
    let mut watcher = match VerifiedWatcher::new(&root, &[root.clone()]) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to create watcher: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Wait for pool ready using watcher
    let status_path = root.join(STATUS_FILE);
    if let Err(e) = watcher.wait_for(&status_path, Some(Duration::from_secs(5))) {
        eprintln!("Daemon not ready: {e}");
        return ExitCode::FAILURE;
    }

    // Pass watcher to wait_for_task
    match wait_for_task(&mut watcher, &root, name.as_deref(), None) {
        Ok(assignment) => { ... }
        Err(e) => { ... }
    }
}
```

### agent_pool_cli/src/main.rs - `Command::SubmitTask`

```rust
Command::SubmitTask { pool, data, file, notify, timeout_secs } => {
    let root = resolve_pool(&pool_root, &pool);

    // CREATE WATCHER AT CLI ENTRY POINT
    let mut watcher = match VerifiedWatcher::new(&root, &[root.clone()]) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to create watcher: {e}");
            return ExitCode::FAILURE;
        }
    };

    let payload = match (data, file) { ... };

    let result = match (notify, timeout_secs) {
        (NotifyMethod::Socket, _) => submit(&mut watcher, &root, &payload),
        (NotifyMethod::File, Some(secs)) => {
            submit_file_with_timeout(&mut watcher, &root, &payload, Duration::from_secs(secs))
        }
        (NotifyMethod::File, None) => submit_file(&mut watcher, &root, &payload),
    };
    // ...
}
```

### daemon/wiring.rs - `run_with_config`

The daemon already creates its watcher at the entry point. The difference is it needs canaries in subdirectories because it creates `agents/` and `submissions/` after setting up the watcher (inotify race condition):

```rust
pub fn run_with_config(root: impl AsRef<Path>, config: DaemonConfig) -> io::Result<Infallible> {
    // ... setup ...

    // Create directories first
    fs::create_dir_all(&submissions_dir)?;
    fs::create_dir_all(&agents_dir)?;
    fs::create_dir_all(&scratch_dir)?;

    // Canaries in subdirs because we just created them - inotify race condition
    let verified_watcher =
        VerifiedWatcher::new(&root, &[agents_dir.clone(), submissions_dir.clone()])?;
    let (_fs_watcher, fs_rx) = verified_watcher.into_receiver(Duration::from_secs(5))?;

    // Pass fs_rx down to io_loop (watcher is consumed to get receiver)
    // ...
}
```

**Note:** The daemon's pattern is slightly different - it calls `into_receiver()` to get the raw channel for `select!`. The CLI keeps the `VerifiedWatcher` and uses `wait_for()` directly.

---

## Summary of Changes

| File | Before | After |
|------|--------|-------|
| `worker.rs` | Creates watcher | Takes `&mut VerifiedWatcher` |
| `submit/file.rs` | Creates watcher | Takes `&mut VerifiedWatcher` |
| `submit/socket.rs` | Creates watcher | Takes `&mut VerifiedWatcher` |
| `submit/mod.rs` | Polls with sleep | **Delete** `wait_for_pool_ready` |
| `agent_pool_cli` | Polls + calls funcs that create watchers | Creates watcher, passes down |
| `daemon/wiring.rs` | Creates watcher (already correct pattern) | No change needed |

## CLI vs Daemon Canary Difference

- **CLI**: `VerifiedWatcher::new(&root, &[root.clone()])` - single canary at root, directories already exist
- **Daemon**: `VerifiedWatcher::new(&root, &[agents_dir, submissions_dir])` - canaries in subdirs, directories just created
