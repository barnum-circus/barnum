# Deferred Child Dispatch for Parallel and ForEach

## Motivation

Parallel and ForEach both advance their children synchronously in a for loop during `advance`. Each child's `self.advance()` call executes inline, recursing into the child's action tree. Without effects, every child eventually hits an Invoke, dispatches an external task, and returns. The synchronous loop works because no child advance has side effects on sibling children.

With Handle/Perform (effects substrate), a child can be a bare Perform or a Chain ending in Perform. If multiple children Perform the same effect, the first dispatches the handler and all subsequent ones pile up in the stash. For a 1000-element ForEach where each iteration Performs, that's 999 stashed effects. Each stashed effect is retried during `sweep_stash` after the handler completes, walks the parent chain, dispatches, handler completes, next retry. O(n^2) in the number of effects.

Deferred dispatch eliminates the quadratic behavior. Parallel and ForEach enqueue their children as pending work items instead of advancing them inline. A pump loop at the top-level entry points drains the queue one item at a time. Between each child advance, effects and completions interleave naturally — if child 0 dispatches a handler, child 1 is still in the queue when the handler completes, not already stashed.

Secondary benefit: flattens the call stack for nested fan-out. Parallel containing Parallel currently recurses. With the queue, inner children go to the back — breadth-first instead of depth-first. Same final result, shallower stack.

This is a prerequisite for the effects substrate. It should land before Handle/Perform so the stash mechanism doesn't have to handle bulk stashing from fan-out nodes.

## Blocks

Effects Phase 1 Substrate (`EFFECTS_PHASE_1_SUBSTRATE.md`).

## Current state

### Parallel

**File:** `crates/barnum_engine/src/lib.rs`, lines 304-331

```rust
FlatAction::Parallel { count } => {
    if count.0 == 0 {
        self.deliver(parent, Value::Array(vec![]))
            .expect("vacuous empty-parallel completion should not fail");
        return Ok(());
    }
    #[allow(clippy::needless_collect)]
    let children: Vec<ActionId> =
        self.flat_config.parallel_children(action_id).collect();
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::Parallel {
            results: vec![None; count.0 as usize],
        },
    });
    for (i, child) in children.into_iter().enumerate() {
        self.advance(
            child,
            value.clone(),
            Some(ParentRef::IndexedChild {
                frame_id,
                child_index: i,
            }),
        )?;
    }
}
```

Each child is advanced inline. The advance call recurses into the child's action tree. If the child is `Chain(Invoke(handler), ...)`, advance creates a Chain frame, advances the Invoke, dispatches a task, and returns. The loop continues to the next child.

### ForEach

**File:** `crates/barnum_engine/src/lib.rs`, lines 334-362

```rust
FlatAction::ForEach { body } => {
    let elements = match value {
        Value::Array(elements) => elements,
        other => {
            return Err(AdvanceError::ForEachExpectedArray { value: other });
        }
    };
    if elements.is_empty() {
        self.deliver(parent, Value::Array(vec![]))
            .expect("vacuous empty-foreach completion should not fail");
        return Ok(());
    }
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::ForEach {
            results: vec![None; elements.len()],
        },
    });
    for (i, element) in elements.into_iter().enumerate() {
        self.advance(
            body,
            element,
            Some(ParentRef::IndexedChild {
                frame_id,
                child_index: i,
            }),
        )?;
    }
}
```

Same pattern: each element's advance is inline.

### Completion path

**File:** `crates/barnum_engine/src/lib.rs`, lines 188-215

Both use the same completion logic via IndexedChild:

```rust
ParentRef::IndexedChild { child_index, .. } => {
    let frame = self.frames.get_mut(frame_id.0).expect("parent frame exists");
    match &mut frame.kind {
        FrameKind::Parallel { results } | FrameKind::ForEach { results } => {
            results[child_index] = Some(value);
            if results.iter().all(Option::is_some) {
                let collected: Vec<Value> =
                    results.iter_mut().map(|r| r.take().unwrap()).collect();
                let parent = frame.parent;
                self.frames.remove(frame_id.0);
                self.deliver(parent, Value::Array(collected))
            } else {
                Ok(None)
            }
        }
        // ...
    }
}
```

Store result at index, check if all slots filled, collect and deliver when done. Order-preserving.

## Proposed change

### Pending advance queue

Add a queue to WorkflowState for deferred advance calls:

```rust
struct PendingAdvance {
    action_id: ActionId,
    value: Value,
    parent: Option<ParentRef>,
}

pub struct WorkflowState {
    // existing fields...
    pending_advances: VecDeque<PendingAdvance>,
}
```

VecDeque for FIFO ordering — children are processed in the order they were enqueued.

### Parallel and ForEach enqueue instead of advancing

```rust
FlatAction::Parallel { count } => {
    if count.0 == 0 {
        self.deliver(parent, Value::Array(vec![]))
            .expect("vacuous empty-parallel completion should not fail");
        return Ok(());
    }
    let children: Vec<ActionId> =
        self.flat_config.parallel_children(action_id).collect();
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::Parallel {
            results: vec![None; count.0 as usize],
        },
    });
    for (i, child) in children.into_iter().enumerate() {
        self.pending_advances.push_back(PendingAdvance {
            action_id: child,
            value: value.clone(),
            parent: Some(ParentRef::IndexedChild {
                frame_id,
                child_index: i,
            }),
        });
    }
}

FlatAction::ForEach { body } => {
    let elements = match value {
        Value::Array(elements) => elements,
        other => {
            return Err(AdvanceError::ForEachExpectedArray { value: other });
        }
    };
    if elements.is_empty() {
        self.deliver(parent, Value::Array(vec![]))
            .expect("vacuous empty-foreach completion should not fail");
        return Ok(());
    }
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::ForEach {
            results: vec![None; elements.len()],
        },
    });
    for (i, element) in elements.into_iter().enumerate() {
        self.pending_advances.push_back(PendingAdvance {
            action_id: body,
            value: element,
            parent: Some(ParentRef::IndexedChild {
                frame_id,
                child_index: i,
            }),
        });
    }
}
```

The advance call returns without advancing any children. The children sit in the queue.

### Pump loop

A pump function drains the pending advance queue. Called at the top-level entry points (workflow start and task completion):

```rust
fn pump(&mut self) -> Result<Option<Value>, AdvanceError> {
    while let Some(pending) = self.pending_advances.pop_front() {
        self.advance(pending.action_id, pending.value, pending.parent)?;
        // advance may enqueue more children (nested Parallel/ForEach)
    }
    Ok(None)
}
```

The top-level entry points call pump after their initial work:

```rust
/// Called by the external driver to start the workflow.
pub fn start(&mut self, input: Value) -> Result<Option<Value>, AdvanceError> {
    self.advance(self.root_action, input, None)?;
    self.pump()
}

/// Called by the external driver when an external task completes.
pub fn complete_task(&mut self, task_id: TaskId, value: Value) -> Result<Option<Value>, CompleteError> {
    let parent_ref = self.task_to_parent.remove(&task_id);
    match parent_ref {
        None => Ok(None),
        Some(parent_ref) => {
            let result = self.deliver(Some(parent_ref), value)?;
            if result.is_some() {
                return Ok(result);
            }
            // deliver may have trampolined into a Parallel/ForEach
            // that enqueued children.
            self.pump().map_err(CompleteError::from)
        }
    }
}
```

### What does NOT change

Chain still advances its `first` child inline via `self.advance()`. Chain is sequential — only one active path at a time, no fan-out. Inlining the advance is correct and produces the familiar trampoline behavior (Chain creates frame, advances first, first eventually completes, Chain trampolines to rest).

The same applies to all other single-child nodes: Loop, Branch (after dispatching on the key). These advance their child inline.

Only Parallel and ForEach — the fan-out nodes — switch to enqueuing.

### Ordering

Children are enqueued in index order (child 0, 1, 2, ...) and processed FIFO. This matches the current behavior where child 0 is advanced first, then child 1, etc. Observable behavior is identical: all children eventually dispatch tasks, tasks complete in arbitrary order, results are collected by index.

For nested fan-out (Parallel containing another Parallel), the outer Parallel enqueues its children. When pump processes child 0, that child's advance may itself be a Parallel that enqueues more children. Those go to the back of the queue. The outer Parallel's remaining children are processed first (breadth-first), then inner children. This is different from the current depth-first order but produces the same final result — all Invokes are dispatched, all tasks are in flight.

### Interaction with effects (future)

After Handle/Perform lands, a Parallel child could be a bare Perform. With deferred dispatch:
1. Pump processes child 0 -> advance -> Perform -> bubble_effect -> handler dispatched.
2. Pump processes child 1 -> advance -> Perform -> bubble_effect -> Handle is busy -> stashed.
3. Pump drains remaining children -> all stashed.
4. Return to external driver. Handler task completes -> complete_task -> Resume -> sweep_stash -> process stashed effects one at a time.

Without deferred dispatch, all children are advanced in the same synchronous loop, producing identical stashing. The difference: with deferred dispatch, each child advance is a separate pump iteration. Stashing is explicit and traceable, and the pump loop is the single scheduling point for all fan-out work.

The real win is avoiding the O(n^2) stash processing. With the synchronous loop, N children all Perform in one call — N-1 items stashed. After each handler completion, sweep_stash retries all stashed items, each walking the parent chain. With deferred dispatch, only the items already processed are stashed; the rest haven't advanced yet. The pump drains them one at a time after each handler completion, producing O(n) total work.

## Test strategy

All existing tests pass unchanged. The refactor is behavior-preserving — children are still advanced in the same order, completions still collect by index, results are still order-preserving arrays.

New tests:

1. **Parallel with nested Parallel**: outer Parallel has 2 children, each is a Parallel with 2 children. Verify all 4 leaf tasks are dispatched and the final result is `[[a, b], [c, d]]` (nested arrays).

2. **ForEach with nested ForEach**: `forEach(forEach(invoke(echo)))` on `[[1,2],[3,4]]`. Verify result is `[[1,2],[3,4]]`.

3. **Parallel where one child completes synchronously**: one child is `constant(42)` (builtin, completes during advance), another is `Invoke(external)`. Verify the synchronous completion fills its slot immediately, and the Parallel waits for the external task.

4. **Empty pending queue**: Chain(Invoke(A), Invoke(B)) — no fan-out, pending queue stays empty. Verify normal trampoline behavior.

5. **Large ForEach**: ForEach over 1000-element array. Each element hits an Invoke. Verify all 1000 tasks are dispatched and results are collected in order.

## Deliverables

1. `PendingAdvance` struct
2. `pending_advances: VecDeque<PendingAdvance>` on WorkflowState
3. `pump()` method
4. Parallel advance: enqueue children instead of inline advance
5. ForEach advance: enqueue iterations instead of inline advance
6. Top-level entry points (`start`, `complete_task`) call pump after initial work
7. Tests per above
