# Invoke Frames

## Motivation

The engine's execution state is split across two data structures: the frame arena (structural combinators) and `task_to_parent` (leaf tasks). Invoke actions are the only action type that doesn't create a frame. When an Invoke is dispatched, its parent pointer is stored directly in `task_to_parent: BTreeMap<TaskId, Option<ParentRef>>`, bypassing the frame tree entirely.

This means the frame tree is an incomplete picture of execution. While a task is in flight, it has no presence in the frame arena. Looking at the frames alone, you can't tell what's running. The `task_to_parent` map carries delivery semantics (`None` means "workflow done") that conceptually belong in the frame tree.

This also forces `teardown_body` to clean up two separate structures: it walks the frame tree to find descendant frames, then separately scans `task_to_parent` to remove entries whose parent points at a torn-down frame. With Invoke frames, teardown is a single frame-tree traversal.

## Current state

### `WorkflowState` (`lib.rs:227-234`)

```rust
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_parent: BTreeMap<TaskId, Option<ParentRef>>,
    pending_dispatches: Vec<Dispatch>,
    stashed_items: VecDeque<StashedItem>,
    next_task_id: u32,
}
```

`task_to_parent` maps each active task to its parent frame reference. `None` means the task is at the workflow root; completing it terminates the workflow.

### Invoke in `advance` (`lib.rs:825-833`)

```rust
FlatAction::Invoke { handler } => {
    let task_id = self.next_task_id();
    self.task_to_parent.insert(task_id, parent);
    self.pending_dispatches.push(Dispatch {
        task_id,
        handler_id: handler,
        value,
    });
}
```

No frame created. The parent pointer goes straight into the map.

### `complete` (`lib.rs:285-307`)

```rust
pub fn complete(&mut self, task_id: TaskId, value: Value) -> Result<Option<Value>, CompleteError> {
    let parent = self.task_to_parent.remove(&task_id).expect("unknown task");
    let result = match parent {
        Some(parent_ref) => match self.try_deliver(parent_ref, value)? { ... },
        None => Some(value),
    };
    ...
}
```

Reads the parent directly from the map. `None` parent means workflow termination.

### `teardown_body` (`lib.rs:619-640`)

```rust
fn teardown_body(&mut self, handle_frame_id: FrameId) {
    let to_remove: Vec<FrameId> = self.frames.iter()
        .filter_map(|(frame_id, _)| {
            if self.is_descendant_of_body(frame_id, handle_frame_id) {
                Some(frame_id)
            } else { None }
        })
        .collect();

    for frame_id in &to_remove {
        self.frames.remove(*frame_id);
    }

    // Separate cleanup pass for leaf tasks
    self.task_to_parent
        .retain(|_, parent| parent.is_none_or(|p| !to_remove.contains(&p.frame_id())));
}
```

Two-phase cleanup: first walk frames, then separately filter `task_to_parent`.

### `FrameKind` (`frame.rs:72-91`)

```rust
pub enum FrameKind {
    Chain { rest: ActionId },
    All { results: Vec<Option<Value>> },
    ForEach { results: Vec<Option<Value>> },
    Handle(HandleFrame),
}
```

No Invoke variant.

## Proposed changes

### 1. Add `FrameKind::Invoke`

```rust
pub enum FrameKind {
    Chain { rest: ActionId },
    All { results: Vec<Option<Value>> },
    ForEach { results: Vec<Option<Value>> },
    Handle(HandleFrame),
    Invoke,
}
```

The variant carries no data. It exists so the frame tree includes every active node. The `HandlerId` is already recorded in the `Dispatch`; the frame's only job is to hold the parent pointer and participate in frame-tree walks.

### 2. Replace `task_to_parent` with `task_to_frame`

```rust
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    stashed_items: VecDeque<StashedItem>,
    next_task_id: u32,
}
```

The map becomes `TaskId -> FrameId`. The parent pointer lives on the frame, where it belongs. No more `Option<ParentRef>` in the map.

### 3. Update `advance` for Invoke

```rust
FlatAction::Invoke { handler } => {
    let task_id = self.next_task_id();
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::Invoke,
    });
    self.task_to_frame.insert(task_id, frame_id);
    self.pending_dispatches.push(Dispatch {
        task_id,
        handler_id: handler,
        value,
    });
}
```

### 4. Update `complete`

```rust
pub fn complete(&mut self, task_id: TaskId, value: Value) -> Result<Option<Value>, CompleteError> {
    let frame_id = self.task_to_frame.remove(&task_id).expect("unknown task");
    let frame = self.frames.remove(frame_id).expect("invoke frame exists");
    let result = match frame.parent {
        Some(parent_ref) => match self.try_deliver(parent_ref, value)? {
            TryDeliverResult::Delivered(result) => result,
            TryDeliverResult::Blocked(value) => {
                self.stashed_items
                    .push_back(StashedItem::Delivery { parent_ref, value });
                None
            }
            TryDeliverResult::FrameGone => None,
        },
        None => Some(value),
    };
    if result.is_some() {
        return Ok(result);
    }
    self.sweep_stash()
}
```

One extra arena lookup and removal compared to today. The frame is removed immediately on completion, so Invoke frames are short-lived in the arena.

### 5. Simplify `teardown_body`

`teardown_body` currently does two passes: walk frames, then filter `task_to_parent`. With Invoke frames, `is_descendant_of_body` naturally finds Invoke frames during the frame walk. The `task_to_parent.retain(...)` line becomes a `task_to_frame.retain(...)` that checks whether the frame was removed:

```rust
fn teardown_body(&mut self, handle_frame_id: FrameId) {
    let to_remove: Vec<FrameId> = self.frames.iter()
        .filter_map(|(frame_id, _)| {
            if self.is_descendant_of_body(frame_id, handle_frame_id) {
                Some(frame_id)
            } else { None }
        })
        .collect();

    for frame_id in &to_remove {
        self.frames.remove(*frame_id);
    }

    self.task_to_frame.retain(|_, frame_id| !to_remove.contains(frame_id));
}
```

The `task_to_frame.retain` is simpler because it checks frame identity directly rather than extracting a `frame_id()` from a `ParentRef` variant. It also no longer needs the `is_none_or` guard, since there's no `Option` to handle.

### 6. Update `find_blocking_ancestor` and `stash` references

`find_blocking_ancestor` (`lib.rs:329-348`) walks parent chains from a `ParentRef`. It will now encounter Invoke frames during the walk. Since Invoke frames don't block anything, the walk passes through them by reading `frame.parent` and continuing. No change needed to the logic; Invoke frames are transparent to ancestor checks.

Stash items (`StashedItem::Delivery`) store a `ParentRef`, not a `TaskId`. These are unaffected because they reference the parent of the completed node, not the node itself.

### 7. Update snapshot tests

All completion snapshot tests will show Invoke frames in the arena. The snapshots become more informative: you can see every active task as a frame.

## Open questions

**Should `FrameKind::Invoke` store the `HandlerId`?** The handler is already in the `Dispatch` struct, so it's redundant for execution. Storing it would make the frame tree fully self-describing for observability (you could walk frames and see which handler is running). Costs one `u32` per Invoke frame.
