# Stale Task Completion

## Motivation

`teardown_body` removes both frames and `task_to_frame` entries for in-flight tasks. When those tasks complete later (they were already spawned as tokio tasks), `complete()` panics:

```rust
// complete.rs:32
let frame_id = workflow_state
    .task_to_frame
    .remove(&task_id)
    .expect("unknown task");
```

The `completing_torn_down_task_is_noop` test documents this bug with `#[should_panic(expected = "unknown task")]`.

## When it happens

The retry-on-error demo triggers this. The sequence:

1. Body has `All(Chain(A, break_perform(e)), B)`.
2. Both A and B are dispatched as tokio tasks.
3. A completes. `break_perform` fires. Handler restarts the body.
4. `teardown_body` removes B's frame and `task_to_frame` entry.
5. Body re-enters. New tasks dispatched.
6. B's tokio task completes and sends its result through the channel.
7. Event loop calls `complete(b_task_id, value)`.
8. `task_to_frame.remove(&b_task_id)` returns `None`. Panic.

## Why `FrameGone` doesn't catch this

`FrameGone` handles a different code path. It catches cases where a stashed `ParentRef` points to a frame that was removed from the arena (the generational index rejects stale references). The stale task problem happens earlier: `task_to_frame.remove()` fails before we ever reach the arena or ancestor check.

`teardown_body` removes both the frame (from the arena) and the `task_to_frame` entry (from the BTreeMap). So when the stale task arrives, the BTreeMap lookup fails immediately.

## Current state

- `complete.rs:32`: `task_to_frame.remove(&task_id).expect("unknown task")`
- `effects.rs:248`: `task_to_frame.retain(|_, frame_id| !to_remove.contains(frame_id))`

## Fix

### Engine: graceful handling of unknown tasks

**File:** `crates/barnum_engine/src/complete.rs`

```rust
// Before:
let frame_id = workflow_state
    .task_to_frame
    .remove(&task_id)
    .expect("unknown task");

// After:
let Some(frame_id) = workflow_state.task_to_frame.remove(&task_id) else {
    return Ok(None);
};
```

Update doc comments on `complete()` in both `complete.rs` and `lib.rs`: remove the `# Panics` section, update the `Ok(None)` documentation to mention torn-down tasks.

### Event loop: defense in depth

**File:** `crates/barnum_event_loop/src/lib.rs`

Add `is_task_pending` to `WorkflowState`:

```rust
pub fn is_task_pending(&self, task_id: TaskId) -> bool {
    self.task_to_frame.contains_key(&task_id)
}
```

Check before calling `complete()`:

```rust
let (task_id, result) = scheduler.recv().await.expect("...");

if !workflow_state.is_task_pending(task_id) {
    continue;
}

let value = result?;
if let Some(terminal_value) = workflow_state.complete(task_id, value)? {
    return Ok(terminal_value);
}
```

The engine-level fix is sufficient for correctness. The event loop check avoids unnecessary error propagation (the `result?` would surface handler errors for tasks we're about to discard).

### Test update

Remove `#[should_panic(expected = "unknown task")]` from `completing_torn_down_task_is_noop`. The test asserts `engine.complete(b_task_id, json!("b_out")).unwrap()` returns `None`.
