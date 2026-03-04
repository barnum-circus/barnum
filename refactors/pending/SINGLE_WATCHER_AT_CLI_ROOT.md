# Single Watcher Created at CLI Root

**Depends on:** `WAIT_FOR_POOL_READY_WATCHER.md`

## Motivation

Various functions create their own `VerifiedWatcher` internally. This is wasteful. Create one at the CLI entry point and pass it down.

## Changes

### 1. Update function signatures to take `&mut VerifiedWatcher`

```rust
pub fn wait_for_pool_ready(watcher: &mut VerifiedWatcher, root: &Path, timeout: Duration) -> io::Result<()>;
pub fn wait_for_task(watcher: &mut VerifiedWatcher, pool_root: &Path, name: Option<&str>) -> io::Result<TaskAssignment>;
pub fn submit_file(watcher: &mut VerifiedWatcher, root: &Path, payload: &Payload) -> io::Result<Response>;
```

### 2. Create watcher at CLI entry points

```rust
// agent_pool CLI - GetTask command
// Single canary at root is sufficient - directories already exist
let mut watcher = VerifiedWatcher::new(&root, &[root.clone()])?;
wait_for_pool_ready(&mut watcher, &root, Duration::from_secs(5))?;
let assignment = wait_for_task(&mut watcher, &root, name.as_deref())?;

// gsd CLI - Run command
let mut watcher = VerifiedWatcher::new(&pool_path, &[pool_path.clone()])?;
// Pass &mut watcher to runner
```

### 3. Daemon needs canaries in subdirectories

The daemon creates `agents/` and `submissions/` after setting up the watcher. Due to the inotify race condition, it needs canaries in those directories:

```rust
// Daemon startup
let agents_dir = pool_root.join(AGENTS_DIR);
let submissions_dir = pool_root.join(SUBMISSIONS_DIR);
let watcher = VerifiedWatcher::new(&pool_root, &[agents_dir, submissions_dir])?;
```

### 4. Remove internal watcher creation from library functions

Functions no longer create watchers - they receive them.
