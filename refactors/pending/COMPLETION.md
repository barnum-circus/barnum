# Completion

Implementation plan for the second engine milestone: task correlation, completion, and terminal results.

**Depends on:** ENGINE.md (design), FRAME_STORAGE_AND_ADVANCE.md (first milestone — frame storage, advance, pending dispatches)

**Scope:** `TaskId`, `task_to_parent`, `on_task_completed`, `complete`. This milestone takes the engine from "expand and dispatch" to "full advance/complete cycle."

**Note:** Since the advance milestone, `advance` is now a public method taking `Option<ParentRef>`, the `FrameKind::Root` sentinel has been removed, and `workflow_root()` returns the starting `ActionId`. Terminal state (workflow done) is detected when `complete` receives `parent: None`.

**Error handling:** Deferred. If a handler fails or the engine encounters an unexpected state, it panics. Error propagation, cancellation, and `Attempt`'s error-catching behavior are a separate design concern.

## What the first milestone left out

The advance milestone produces dispatches but has no way to consume results. Dispatches go out to the runtime but nothing comes back. The engine is a one-shot expansion machine.

This milestone closes the loop:

```
Dispatch goes out -> runtime executes handler -> result comes back
  -> on_task_completed(task_id, value)
    -> looks up parent from task_to_parent
    -> calls complete(parent, value)
      -> parent frame decides what to do
        -> may call advance() again (Chain trampoline, Loop re-enter)
          -> produces more dispatches
            -> cycle continues until a frame with parent: None completes
  -> on_task_completed returns Some(value) when workflow terminates
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

Monotonic `u32` counter. Assigned during advance when an Invoke action is reached. Returned in `Dispatch` so the runtime can correlate results. Used as a `HashMap` key in `task_to_parent`.

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

### Updated Engine

```rust
pub struct Engine {
    flat_config: FlatConfig,
    frames: Slab<Frame>,
    task_to_parent: HashMap<TaskId, Option<ParentRef>>,
    pending_dispatches: Vec<Dispatch>,
    next_task_id: u32,
}
```

Two new fields over the advance milestone:
- `task_to_parent`: maps pending TaskIds directly to the parent that should receive the result. Populated in advance (Invoke arm), consumed in on_task_completed.
- `next_task_id`: monotonic counter for TaskId allocation.

## Updated advance (Invoke arm only)

The only change to advance: the Invoke arm now allocates a TaskId, stores the parent mapping, and includes the TaskId in the dispatch. No frame is created — Invoke is not a structural frame.

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

## on_task_completed

The entry point from the runtime. A handler finished; deliver the result to the engine.

Returns `Some(value)` when the workflow terminates, `None` when it's still running.

```rust
pub fn on_task_completed(
    &mut self,
    task_id: TaskId,
    value: Value,
) -> Option<Value> {
    let parent = self.task_to_parent.remove(&task_id).expect("unknown task");
    self.complete(parent, value)
}
```

Look up the parent directly from `task_to_parent` — no frame to remove. The terminal result (if any) flows back through the return value.

## complete

A handler invocation finished successfully and produced a value. Now that value needs to go somewhere — it flows to the parent that was waiting for it. What happens next depends on what kind of parent was waiting:

- **No parent (`None`):** This was the top-level action. The workflow is done. Return the value as the terminal result.
- **Chain:** The value is the output of the chain's first child. Use it as input to advance the `rest` action. This is the "trampoline" — completion triggers more expansion.
- **Loop:** The value is the loop body's output. If it says `Continue`, re-advance the body with the new value (another iteration). If `Break`, the loop is done — complete the loop's parent with the break value.
- **Attempt:** The child succeeded. Wrap the value as `{ kind: "Ok", value }` and complete the attempt's parent. (First pass: wraps unconditionally. Proper Attempt semantics — structured error types, catching failures — are deferred.)
- **Parallel / ForEach:** The value is one child's result. Store it in the results slot at `child_index`. If all slots are filled, collect them into an array and complete the parent. If not, do nothing — more children are still in flight.

The key insight: `complete` either **terminates** (returns a terminal result), **mutates** a frame in place (Parallel/ForEach partial result), or **continues** by calling `advance` again (Chain/Loop). It never blocks — everything is synchronous state manipulation.

Each arm asserts the expected `ParentRef` variant. Chain/Loop/Attempt use `SingleChild`; Parallel/ForEach use `IndexedChild`.

For Parallel and ForEach, we **do not** remove the frame. We mutate it in place via `get_mut` and only remove it when all results are collected.

```rust
fn complete(
    &mut self,
    parent: Option<ParentRef>,
    value: Value,
) -> Option<Value> {
    let Some(parent_ref) = parent else {
        return Some(value);
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
                        _ => panic!(
                            "Loop body must return {{kind: \"Continue\"}} or {{kind: \"Break\"}}, got: {value}"
                        ),
                    }
                }
                // First pass: wrap in Ok unconditionally. Proper Attempt
                // semantics (structured error types, etc.) are deferred.
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
                FrameKind::Parallel { results }
                | FrameKind::ForEach { results } => {
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

## Updated public API

```rust
impl Engine {
    pub const fn new(flat_config: FlatConfig) -> Self;
    pub const fn workflow_root(&self) -> ActionId;
    pub fn advance(&mut self, action_id: ActionId, value: Value, parent: Option<ParentRef>) -> Result<(), AdvanceError>;
    pub fn on_task_completed(&mut self, task_id: TaskId, value: Value) -> Option<Value>;
    pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch>;
}
```

New in this milestone:
- `on_task_completed` — deliver a handler result, returns `Some(value)` when workflow terminates

Already public from advance milestone:
- `advance` — core primitive, expands an action into frames
- `workflow_root` — the starting action ID

## Tests

Tests use the full cycle: build config -> flatten -> Engine::new -> advance(workflow_root, input, None) -> take dispatches -> on_task_completed -> take more dispatches -> ... -> assert terminal result returned from on_task_completed.

### Completion tests

```rust
/// Chain(A, B): complete A -> dispatches B. Complete B -> workflow done.
#[test]
fn chain_trampolines_on_completion() {
    let mut engine = engine_from(chain(
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ));
    let root = engine.workflow_root();
    engine.advance(root, json!(null), None).unwrap();

    let d1 = engine.take_pending_dispatches();
    assert_eq!(d1.len(), 1); // A dispatched

    let result = engine.on_task_completed(d1[0].task_id, json!("a_result"));
    assert_eq!(result, None); // Not done yet

    let d2 = engine.take_pending_dispatches();
    assert_eq!(d2.len(), 1); // B dispatched
    assert_eq!(d2[0].value, json!("a_result")); // B receives A's output

    let result = engine.on_task_completed(d2[0].task_id, json!("b_result"));
    assert_eq!(result, Some(json!("b_result")));
}

/// Deep chain: Chain(A, Chain(B, C)) -> A -> B -> C -> done.
#[test]
fn nested_chain_completes() {
    let mut engine = engine_from(chain(
        invoke("./a.ts", "a"),
        chain(invoke("./b.ts", "b"), invoke("./c.ts", "c")),
    ));
    let root = engine.workflow_root();
    engine.advance(root, json!("input"), None).unwrap();

    // A
    let d = engine.take_pending_dispatches();
    assert_eq!(engine.on_task_completed(d[0].task_id, json!("a_out")), None);
    // B
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!("a_out"));
    assert_eq!(engine.on_task_completed(d[0].task_id, json!("b_out")), None);
    // C
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!("b_out"));
    assert_eq!(
        engine.on_task_completed(d[0].task_id, json!("c_out")),
        Some(json!("c_out")),
    );
}

/// Parallel(A, B): complete both -> workflow done with [a_result, b_result].
#[test]
fn parallel_collects_results() {
    let mut engine = engine_from(parallel(vec![
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ]));
    let root = engine.workflow_root();
    engine.advance(root, json!(null), None).unwrap();

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 2);

    // Complete in reverse order to verify index-based collection.
    assert_eq!(
        engine.on_task_completed(d[1].task_id, json!("b_result")),
        None, // Still waiting for A
    );
    assert_eq!(
        engine.on_task_completed(d[0].task_id, json!("a_result")),
        Some(json!(["a_result", "b_result"])),
    );
}

/// ForEach over [10, 20]: complete both -> [handler(10), handler(20)].
#[test]
fn foreach_collects_results() {
    let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
    let root = engine.workflow_root();
    engine.advance(root, json!([10, 20]), None).unwrap();

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 2);

    assert_eq!(engine.on_task_completed(d[0].task_id, json!("r10")), None);
    assert_eq!(
        engine.on_task_completed(d[1].task_id, json!("r20")),
        Some(json!(["r10", "r20"])),
    );
}

/// Loop: Continue re-dispatches, Break completes.
#[test]
fn loop_continue_and_break() {
    let mut engine = engine_from(loop_action(invoke("./handler.ts", "run")));
    let root = engine.workflow_root();
    engine.advance(root, json!(0), None).unwrap();

    // Iteration 1: handler returns Continue
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!(0));
    assert_eq!(
        engine.on_task_completed(d[0].task_id, json!({"kind": "Continue", "value": 1})),
        None,
    );

    // Iteration 2: handler returns Continue again
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!(1));
    assert_eq!(
        engine.on_task_completed(d[0].task_id, json!({"kind": "Continue", "value": 2})),
        None,
    );

    // Iteration 3: handler returns Break
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!(2));
    assert_eq!(
        engine.on_task_completed(d[0].task_id, json!({"kind": "Break", "value": "done"})),
        Some(json!("done")),
    );
}

/// Attempt wraps success in Ok.
#[test]
fn attempt_wraps_success() {
    let mut engine = engine_from(attempt(invoke("./handler.ts", "run")));
    let root = engine.workflow_root();
    engine.advance(root, json!("input"), None).unwrap();

    let d = engine.take_pending_dispatches();
    assert_eq!(
        engine.on_task_completed(d[0].task_id, json!("output")),
        Some(json!({"kind": "Ok", "value": "output"})),
    );
}
```
