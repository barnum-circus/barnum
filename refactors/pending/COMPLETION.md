# Completion and Error Handling

Implementation plan for the second engine milestone: task correlation, completion, error propagation, and terminal results.

**Depends on:** ENGINE.md (design), FRAME_STORAGE_AND_ADVANCE.md (first milestone — frame storage, advance, pending dispatches)

**Scope:** `TaskId`, `task_to_frame`, `on_task_completed`, `complete`, `error`. This milestone takes the engine from "expand and dispatch" to "full advance/complete cycle."

**Note:** Since the advance milestone, `advance` is now a public method taking `Option<ParentRef>`, the `FrameKind::Root` sentinel has been removed, and `start()` is convenience sugar for `advance(workflow_root, input, None)`. Terminal state (workflow done) is detected when `complete` or `error` receives `parent: None`.

## What the first milestone left out

The advance milestone produces dispatches but has no way to consume results. Dispatches go out to the runtime but nothing comes back. The engine is a one-shot expansion machine.

This milestone closes the loop:

```
Dispatch goes out → runtime executes handler → result comes back
  → on_task_completed(task_id, Ok(value) | Err(error))
    → finds the Invoke frame
    → calls complete(parent, value) or error(parent, error_string)
      → parent frame decides what to do
        → may call advance() again (Chain trampoline, Loop re-enter)
          → produces more dispatches
            → cycle continues until a frame with parent: None completes or errors
  → on_task_completed returns Some(result) when workflow terminates
```

## New types

### TaskId

```rust
u32_newtype!(
    /// Identifies a pending handler invocation. Assigned by the engine,
    /// returned to the engine in `on_task_completed`.
    pub TaskId
);
```

Monotonic `u32` counter. Assigned during advance when an Invoke frame is created. Returned in `Dispatch` so the runtime can correlate results. Used as a `HashMap` key in `task_to_frame`.

### Updated Dispatch

```rust
#[derive(Debug)]
pub struct Dispatch {
    pub task_id: TaskId,
    pub handler_id: HandlerId,
    pub value: Value,
}
```

Now includes `task_id` so the runtime can send results back keyed by task.

### Updated FrameKind::Invoke

```rust
FrameKind::Invoke { task_id: TaskId },
```

The Invoke frame now stores its TaskId. When `on_task_completed` arrives, the engine looks up the frame by TaskId, extracts the parent, removes the frame, and calls complete or error.

### Updated Engine

```rust
pub struct Engine {
    flat_config: FlatConfig,
    frames: Slab<Frame>,
    task_to_frame: HashMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_task_id: u32,
}
```

Two new fields over the advance milestone:
- `task_to_frame`: maps pending TaskIds to Invoke FrameIds. Populated in advance (Invoke arm), consumed in on_task_completed.
- `next_task_id`: monotonic counter for TaskId allocation.

No stored result. Terminal results are returned directly from `on_task_completed`.

## Updated advance (Invoke arm only)

The only change to advance: the Invoke arm now allocates a TaskId, stores the mapping, and includes the TaskId in the dispatch. Note: `advance` takes `parent: Option<ParentRef>` and stores it directly (no wrapping in `Some`).

```rust
FlatAction::Invoke { handler } => {
    let task_id = self.next_task_id();
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::Invoke { task_id },
    });
    self.task_to_frame.insert(task_id, frame_id);
    self.pending_dispatches.push(Dispatch {
        task_id,
        handler_id: handler,
        value,
    });
}
```

## on_task_completed

The entry point from the runtime. A handler finished; deliver the result to the engine.

Returns `Some(result)` when the workflow terminates, `None` when it's still running.

```rust
pub fn on_task_completed(
    &mut self,
    task_id: TaskId,
    task_result: Result<Value, String>,
) -> Option<Result<Value, String>> {
    let frame_id = self.task_to_frame.remove(&task_id).expect("unknown task");
    let frame = self.frames.remove(frame_id.0);
    match task_result {
        Ok(value) => self.complete(frame.parent, value),
        Err(error) => self.error(frame.parent, error),
    }
}
```

Remove the Invoke frame and its task mapping. Pass `frame.parent` (which is `Option<ParentRef>`) directly to complete or error. The terminal result (if any) flows back through the return value.

## complete

In the advance milestone, complete existed as a private no-op for empty ForEach/Parallel. Now it's the core of the engine's execution model.

`complete` takes `Option<ParentRef>`. When `parent` is `None`, the top-level action has completed and the workflow is done — return the result. Otherwise, look up the parent frame and dispatch on its kind.

Each arm asserts the expected `ParentRef` variant. Chain/Loop/Attempt use `SingleChild`; Parallel/ForEach use `IndexedChild`.

For Parallel and ForEach, we **do not** remove the frame. We mutate it in place via `get_mut` and only remove it when all results are collected.

```rust
fn complete(
    &mut self,
    parent: Option<ParentRef>,
    value: Value,
) -> Option<Result<Value, String>> {
    let Some(parent_ref) = parent else {
        return Some(Ok(value));
    };

    let frame_id = parent_ref.frame_id();

    match parent_ref {
        ParentRef::SingleChild { .. } => {
            let frame = self.frames.remove(frame_id.0);
            match frame.kind {
                FrameKind::Chain { rest } => {
                    self.advance(rest, value, frame.parent).unwrap();
                    None
                }
                FrameKind::Loop { body } => {
                    // value must be { kind: "Continue", value } or { kind: "Break", value }
                    match value["kind"].as_str() {
                        Some("Continue") => {
                            self.advance(body, value["value"].clone(), frame.parent).unwrap();
                            None
                        }
                        Some("Break") => {
                            self.complete(frame.parent, value["value"].clone())
                        }
                        _ => {
                            // Handler returned garbage — treat as workflow error.
                            let msg = format!(
                                "Loop body must return {{kind: \"Continue\"}} or {{kind: \"Break\"}}, got: {value}"
                            );
                            self.error(frame.parent, msg)
                        }
                    }
                }
                // First pass: wrap in Ok unconditionally. Proper Attempt
                // semantics (structured error types, etc.) are deferred —
                // see DEFERRED_FEATURES.md.
                FrameKind::Attempt => {
                    let wrapped = serde_json::json!({ "kind": "Ok", "value": value });
                    self.complete(frame.parent, wrapped)
                }
                _ => unreachable!(
                    "SingleChild parent must be Chain, Loop, or Attempt, got {:?}",
                    frame.kind
                ),
            }
        }
        ParentRef::IndexedChild { child_index, .. } => {
            let frame = self.frames.get_mut(frame_id.0)
                .expect("parent frame exists");
            match &mut frame.kind {
                FrameKind::Parallel { results } => {
                    results[child_index] = Some(value);
                    if results.iter().all(Option::is_some) {
                        let collected: Vec<Value> =
                            results.iter_mut().map(|r| r.take().unwrap()).collect();
                        let parent = frame.parent;
                        self.frames.remove(frame_id.0);
                        self.complete(parent, Value::Array(collected))
                    } else {
                        None
                    }
                }
                FrameKind::ForEach { results } => {
                    results[child_index] = Some(value);
                    if results.iter().all(Option::is_some) {
                        let collected: Vec<Value> =
                            results.iter_mut().map(|r| r.take().unwrap()).collect();
                        let parent = frame.parent;
                        self.frames.remove(frame_id.0);
                        self.complete(parent, Value::Array(collected))
                    } else {
                        None
                    }
                }
                _ => unreachable!(
                    "IndexedChild parent must be Parallel or ForEach, got {:?}",
                    frame.kind
                ),
            }
        }
    }
}
```

## error

A child failed. Walk up the frame tree until an Attempt catches the error or a frame with `parent: None` terminates.

```rust
fn error(
    &mut self,
    parent: Option<ParentRef>,
    error: String,
) -> Option<Result<Value, String>> {
    let Some(parent_ref) = parent else {
        return Some(Err(error));
    };
    let frame_id = parent_ref.frame_id();
    let frame = self.frames.remove(frame_id.0);

    match frame.kind {
        FrameKind::Attempt => {
            let wrapped = serde_json::json!({ "kind": "Err", "error": error });
            self.complete(frame.parent, wrapped)
        }

        FrameKind::Parallel { .. } | FrameKind::ForEach { .. } => {
            self.cancel_descendants(frame_id);
            self.error(frame.parent, error)
        }

        _ => {
            self.error(frame.parent, error)
        }
    }
}
```

### Error propagation path

Error walks up frame-by-frame:
1. **`parent: None`**: terminal failure. Returns `Some(Err(error))`.
2. **Attempt**: catches the error. Wraps as `{ kind: "Err", error }` and completes the parent normally. Error stops propagating.
3. **Parallel/ForEach**: cancel all other in-flight children (they're now irrelevant), then propagate the error upward. Short-circuit — one failure fails the whole fan-out.
4. **Chain/Loop/Invoke**: transparent — just propagate upward.

### cancel_descendants

When a Parallel or ForEach frame errors, its surviving children (other Invoke frames still pending) must be cancelled. The engine walks the subtree rooted at the fan-out frame and:
- Removes all descendant frames from the slab
- Removes all descendant TaskIds from `task_to_frame`
- Does NOT remove dispatches already in `pending_dispatches` — those are already queued for the runtime. The runtime will send results for cancelled tasks; `on_task_completed` must handle "unknown task" gracefully (the frame was already removed).

```rust
fn cancel_descendants(&mut self, frame_id: FrameId) {
    // Walk all frames in the slab, find those whose ancestor chain
    // includes frame_id, remove them and their task_to_frame entries.
    //
    // Simple approach: iterate slab, check parent chains.
    // O(n) where n = number of frames. Fine for now.
    let to_remove: Vec<usize> = self.frames.iter()
        .filter(|(_, frame)| self.is_descendant_of(frame, frame_id))
        .map(|(key, _)| key)
        .collect();

    for key in to_remove {
        let frame = self.frames.remove(key);
        if let FrameKind::Invoke { task_id } = frame.kind {
            self.task_to_frame.remove(&task_id);
        }
    }
}
```

**Open question:** Should `on_task_completed` panic or silently ignore unknown TaskIds? If cancelled tasks' results arrive after cancellation, the TaskId won't be in `task_to_frame`. Panicking is correct for debugging (unknown TaskId = bug); ignoring is correct for production (cancelled tasks' results are expected noise). Start with panic, add a flag or match later.

## Updated public API

```rust
impl Engine {
    pub const fn new(flat_config: FlatConfig) -> Self;
    pub const fn workflow_root(&self) -> ActionId;
    pub fn advance(&mut self, action_id: ActionId, value: Value, parent: Option<ParentRef>) -> Result<(), AdvanceError>;
    pub fn start(&mut self, input: Value) -> Result<(), AdvanceError>; // sugar for advance(workflow_root, input, None)
    pub fn on_task_completed(&mut self, task_id: TaskId, result: Result<Value, String>) -> Option<Result<Value, String>>;
    pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch>;
    pub fn handler(&self, id: HandlerId) -> &HandlerKind;
}
```

New in this milestone:
- `on_task_completed` — deliver a handler result, returns `Some(result)` when workflow terminates

Already public from advance milestone:
- `advance` — core primitive, expands an action into frames
- `workflow_root` — the starting action ID
- `start` — convenience sugar over `advance`

## Tests

Tests use the full cycle: build config → flatten → Engine::new → start → take dispatches → on_task_completed → take more dispatches → ... → assert terminal result returned from on_task_completed.

### Completion tests

```rust
/// Chain(A, B): complete A → dispatches B. Complete B → workflow done.
#[test]
fn chain_trampolines_on_completion() {
    let mut engine = engine_from(chain(
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ));
    engine.start(json!(null)).unwrap();

    let d1 = engine.take_pending_dispatches();
    assert_eq!(d1.len(), 1); // A dispatched

    let result = engine.on_task_completed(d1[0].task_id, Ok(json!("a_result")));
    assert_eq!(result, None); // Not done yet

    let d2 = engine.take_pending_dispatches();
    assert_eq!(d2.len(), 1); // B dispatched
    assert_eq!(d2[0].value, json!("a_result")); // B receives A's output

    let result = engine.on_task_completed(d2[0].task_id, Ok(json!("b_result")));
    assert_eq!(result, Some(Ok(json!("b_result"))));
}

/// Deep chain: Chain(A, Chain(B, C)) → A → B → C → done.
#[test]
fn nested_chain_completes() {
    let mut engine = engine_from(chain(
        invoke("./a.ts", "a"),
        chain(invoke("./b.ts", "b"), invoke("./c.ts", "c")),
    ));
    engine.start(json!("input")).unwrap();

    // A
    let d = engine.take_pending_dispatches();
    assert_eq!(engine.on_task_completed(d[0].task_id, Ok(json!("a_out"))), None);
    // B
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!("a_out"));
    assert_eq!(engine.on_task_completed(d[0].task_id, Ok(json!("b_out"))), None);
    // C
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!("b_out"));
    assert_eq!(
        engine.on_task_completed(d[0].task_id, Ok(json!("c_out"))),
        Some(Ok(json!("c_out"))),
    );
}

/// Parallel(A, B): complete both → workflow done with [a_result, b_result].
#[test]
fn parallel_collects_results() {
    let mut engine = engine_from(parallel(vec![
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ]));
    engine.start(json!(null)).unwrap();

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 2);

    // Complete in reverse order to verify index-based collection.
    assert_eq!(
        engine.on_task_completed(d[1].task_id, Ok(json!("b_result"))),
        None, // Still waiting for A
    );
    assert_eq!(
        engine.on_task_completed(d[0].task_id, Ok(json!("a_result"))),
        Some(Ok(json!(["a_result", "b_result"]))),
    );
}

/// ForEach over [10, 20]: complete both → [handler(10), handler(20)].
#[test]
fn foreach_collects_results() {
    let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
    engine.start(json!([10, 20])).unwrap();

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 2);

    assert_eq!(engine.on_task_completed(d[0].task_id, Ok(json!("r10"))), None);
    assert_eq!(
        engine.on_task_completed(d[1].task_id, Ok(json!("r20"))),
        Some(Ok(json!(["r10", "r20"]))),
    );
}

/// Loop: Continue re-dispatches, Break completes.
#[test]
fn loop_continue_and_break() {
    let mut engine = engine_from(loop_action(invoke("./handler.ts", "run")));
    engine.start(json!(0)).unwrap();

    // Iteration 1: handler returns Continue
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!(0));
    assert_eq!(
        engine.on_task_completed(d[0].task_id, Ok(json!({"kind": "Continue", "value": 1}))),
        None,
    );

    // Iteration 2: handler returns Continue again
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!(1));
    assert_eq!(
        engine.on_task_completed(d[0].task_id, Ok(json!({"kind": "Continue", "value": 2}))),
        None,
    );

    // Iteration 3: handler returns Break
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!(2));
    assert_eq!(
        engine.on_task_completed(d[0].task_id, Ok(json!({"kind": "Break", "value": "done"}))),
        Some(Ok(json!("done"))),
    );
}

/// Attempt wraps success in Ok.
#[test]
fn attempt_wraps_success() {
    let mut engine = engine_from(attempt(invoke("./handler.ts", "run")));
    engine.start(json!("input")).unwrap();

    let d = engine.take_pending_dispatches();
    assert_eq!(
        engine.on_task_completed(d[0].task_id, Ok(json!("output"))),
        Some(Ok(json!({"kind": "Ok", "value": "output"}))),
    );
}

/// Attempt catches failure as Err.
#[test]
fn attempt_catches_failure() {
    let mut engine = engine_from(attempt(invoke("./handler.ts", "run")));
    engine.start(json!("input")).unwrap();

    let d = engine.take_pending_dispatches();
    let result = engine.on_task_completed(
        d[0].task_id,
        Err("handler crashed".to_string()),
    );
    // Success, not failure — Attempt caught the error.
    assert_eq!(
        result,
        Some(Ok(json!({"kind": "Err", "error": "handler crashed"}))),
    );
}

/// Error propagates through Chain to top.
#[test]
fn error_propagates_through_chain() {
    let mut engine = engine_from(chain(
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ));
    engine.start(json!(null)).unwrap();

    let d = engine.take_pending_dispatches();
    assert_eq!(
        engine.on_task_completed(d[0].task_id, Err("a failed".to_string())),
        Some(Err("a failed".to_string())),
    );
}

/// Error in one Parallel child fails the whole Parallel.
#[test]
fn error_in_parallel_child() {
    let mut engine = engine_from(parallel(vec![
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ]));
    engine.start(json!(null)).unwrap();

    let d = engine.take_pending_dispatches();
    assert_eq!(
        engine.on_task_completed(d[0].task_id, Err("a failed".to_string())),
        Some(Err("a failed".to_string())),
    );
}
```

### Not tested here

- cancel_descendants correctness (requires inspecting internal frame state)
- Cancelled task results arriving after cancellation (on_task_completed with unknown TaskId)
- Deeply nested error propagation (Attempt inside Parallel inside Chain)
- Step(Root) re-entry patterns
