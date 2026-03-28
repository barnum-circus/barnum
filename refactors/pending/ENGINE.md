# Engine Design

The engine does one thing: advance a cursor through a flat action table.

Given a cursor `(ActionId, Value)`, the engine walks the action table, creating frames for structural combinators that track partial progress, until every active path is suspended at an Invoke node with a pending dispatch. When a handler completes, the engine receives the result, pops the Invoke frame, and advances the parent frame — which may produce more dispatches or complete the workflow.

The engine has no knowledge of scheduling, concurrency, or I/O. It is a pure state machine.

## Frames

A frame tracks the engine's execution state at one structural combinator. Frames form a tree. Leaves are Invoke frames (suspended, waiting for handler results). Interior frames are combinators tracking partial progress.

Every combinator follows one of two patterns:

- **Single-child**: has one active child. When the child completes, the frame either completes upward (Attempt, Chain) or re-enters the child (Loop on Continue). No indexing.
- **Fan-out**: has N children running concurrently. Each child fills a slot. When all slots are filled, the frame collects results and completes upward (Parallel, ForEach).

```rust
struct Frame {
    parent: Option<ParentRef>,
    kind: FrameKind,
}

enum ParentRef {
    /// Parent has one child. No index needed.
    /// Used by: Chain, Loop, Attempt.
    SingleChild { frame_id: FrameId },
    /// Parent has N children, this is child at `child_index`.
    /// Used by: Parallel, ForEach.
    IndexedChild { frame_id: FrameId, child_index: usize },
}

enum FrameKind {
    /// Sentinel: the workflow entry point. Only one exists per engine.
    /// When its child completes, the engine is done.
    Root,

    /// Leaf: handler dispatched, waiting for result.
    Invoke { task_id: TaskId },

    /// Sequential: run first child, then trampoline to rest.
    Chain { rest: ActionId },

    /// Collecting results from N parallel branches.
    Parallel { results: Vec<Option<Value>> },

    /// Collecting results from N array elements.
    ForEach { results: Vec<Option<Value>> },

    /// Fixed-point iteration.
    Loop { body: ActionId },

    /// Error materialization.
    Attempt,
}
```

**Branch and Step do not create frames.** They are immediate reductions — Branch reads `value["kind"]` and redirects to the matching case; Step follows the target ActionId. Both pass through the parent reference unchanged. No state to track, no frame needed.

**Chain does not mutate.** When its child completes, the Chain frame removes itself and trampolines to `rest` with the original parent. At most one Chain frame exists at a time per sequential chain.

## Engine state

```rust
struct Engine {
    flat_config: FlatConfig,
    frames: Slab<Frame>,
    task_to_frame: HashMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_task_id: u32,
    result: Option<EngineResult>,
}

struct Dispatch {
    task_id: TaskId,
    handler_id: HandlerId,
    value: Value,
}

enum EngineResult {
    Success(Value),
    Failure(String),
}
```

## advance

Walk the action table from `action_id`. Recurse through structural nodes until reaching Invoke leaves.

```rust
fn advance(&mut self, action_id: ActionId, value: Value, parent: ParentRef) {
    match self.flat_config.action(action_id) {
        FlatAction::Invoke { handler } => {
            let task_id = self.next_task_id();
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Invoke { task_id },
            });
            self.task_to_frame.insert(task_id, frame_id);
            self.pending_dispatches.push(Dispatch {
                task_id,
                handler_id: handler,
                value,
            });
        }

        FlatAction::Chain { rest } => {
            let first = self.flat_config.chain_first(action_id);
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Chain { rest },
            });
            self.advance(first, value, ParentRef::SingleChild { frame_id });
        }

        FlatAction::Parallel { count } => {
            let children: Vec<ActionId> =
                self.flat_config.parallel_children(action_id).collect();
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Parallel {
                    results: vec![None; count.0 as usize],
                },
            });
            for (i, child) in children.into_iter().enumerate() {
                self.advance(
                    child,
                    value.clone(),
                    ParentRef::IndexedChild { frame_id, child_index: i },
                );
            }
        }

        FlatAction::ForEach { body } => {
            let elements = match value {
                Value::Array(elements) => elements,
                other => panic!("ForEach expected array, got {other}"),
            };
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::ForEach {
                    results: vec![None; elements.len()],
                },
            });
            for (i, element) in elements.into_iter().enumerate() {
                self.advance(
                    body,
                    element,
                    ParentRef::IndexedChild { frame_id, child_index: i },
                );
            }
        }

        FlatAction::Branch { .. } => {
            let kind_str = value["kind"]
                .as_str()
                .expect("Branch input must have a string 'kind' field");
            let case_action_id = self
                .flat_config
                .branch_cases(action_id)
                .find(|(key, _)| key.as_str() == kind_str)
                .expect("no matching branch case")
                .1;
            self.advance(case_action_id, value, parent);
        }

        FlatAction::Loop { body } => {
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Loop { body },
            });
            self.advance(body, value, ParentRef::SingleChild { frame_id });
        }

        FlatAction::Attempt { child } => {
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Attempt,
            });
            self.advance(child, value, ParentRef::SingleChild { frame_id });
        }

        FlatAction::Step { target } => {
            self.advance(target, value, parent);
        }
    }
}
```

Branch and Step are tail calls — they redirect without allocating state.

## complete

A child resolved. Advance the parent.

```rust
fn complete(&mut self, parent_ref: ParentRef, value: Value) {
    match parent_ref {
        ParentRef::SingleChild { frame_id } => self.complete_single(frame_id, value),
        ParentRef::IndexedChild { frame_id, child_index } => {
            self.complete_indexed(frame_id, child_index, value);
        }
    }
}

fn complete_single(&mut self, frame_id: FrameId, value: Value) {
    // Each arm handles its own removal. Loop Continue keeps the frame alive.
    // Root terminates; all other arms propagate via frame.parent (always Some for non-Root).
    match self.frames[frame_id.0].kind {
        FrameKind::Root => {
            self.frames.remove(frame_id.0);
            self.result = Some(EngineResult::Success(value));
        }

        FrameKind::Chain { rest } => {
            let frame = self.frames.remove(frame_id.0);
            let parent = frame.parent.expect("non-root frame has parent");
            self.advance(rest, value, parent);
        }

        FrameKind::Loop { body } => {
            let kind = value["kind"].as_str().expect("Loop result must have 'kind'");
            let inner = value.get("value").cloned().unwrap_or(Value::Null);
            match kind {
                "Continue" => {
                    // Frame stays in place. Re-enter the body.
                    self.advance(body, inner, ParentRef::SingleChild { frame_id });
                }
                "Break" => {
                    let frame = self.frames.remove(frame_id.0);
                    let parent = frame.parent.expect("non-root frame has parent");
                    self.complete(parent, inner);
                }
                other => panic!("Loop result kind must be Continue or Break, got {other}"),
            }
        }

        FrameKind::Attempt => {
            let frame = self.frames.remove(frame_id.0);
            let parent = frame.parent.expect("non-root frame has parent");
            let wrapped = serde_json::json!({ "kind": "Ok", "value": value });
            self.complete(parent, wrapped);
        }

        other => panic!("complete_single called on {other:?}"),
    }
}

fn complete_indexed(&mut self, frame_id: FrameId, child_index: usize, value: Value) {
    let frame = self.frames.get_mut(frame_id.0).expect("frame not found");
    let results = match &mut frame.kind {
        FrameKind::Parallel { results } | FrameKind::ForEach { results } => results,
        other => panic!("complete_indexed called on {other:?}"),
    };

    results[child_index] = Some(value);

    if results.iter().all(Option::is_some) {
        let collected: Vec<Value> = results.drain(..).map(Option::unwrap).collect();
        let frame = self.frames.remove(frame_id.0);
        let parent = frame.parent.expect("non-root frame has parent");
        self.complete(parent, Value::Array(collected));
    }
}
```

## error

A child failed.

```rust
fn error(&mut self, parent_ref: ParentRef, error: String) {
    let frame_id = parent_ref.frame_id();
    let frame = self.frames.remove(frame_id.0);

    match frame.kind {
        FrameKind::Root => {
            self.result = Some(EngineResult::Failure(error));
        }

        FrameKind::Attempt => {
            let parent = frame.parent.expect("non-root frame has parent");
            let wrapped = serde_json::json!({ "kind": "Err", "error": error });
            self.complete(parent, wrapped);
        }

        FrameKind::Parallel { .. } | FrameKind::ForEach { .. } => {
            let parent = frame.parent.expect("non-root frame has parent");
            self.cancel_descendants(frame_id);
            self.error(parent, error);
        }

        _ => {
            let parent = frame.parent.expect("non-root frame has parent");
            self.error(parent, error);
        }
    }
}
```

## on_task_completed

```rust
fn on_task_completed(&mut self, task_id: TaskId, result: TaskResult) {
    let frame_id = self.task_to_frame.remove(&task_id).expect("unknown task");
    let frame = self.frames.remove(frame_id.0);
    let parent = frame.parent.expect("Invoke frame has parent");
    match result {
        TaskResult::Success { value } => self.complete(parent, value),
        TaskResult::Failure { error } => self.error(parent, error),
    }
}
```

## Public interface

```rust
impl Engine {
    fn new(flat_config: FlatConfig) -> Self;
    fn start(&mut self, input: Value);
    fn on_task_completed(&mut self, task_id: TaskId, result: TaskResult);
    fn take_pending_dispatches(&mut self) -> Vec<Dispatch>;
    fn is_done(&self) -> bool;
    fn result(&self) -> Option<&EngineResult>;
}
```

## Testing strategy

```rust
fn single_invoke()                      // start → 1 dispatch
fn chain_dispatches_first_only()         // chain(a, b) → 1 dispatch (a)
fn chain_trampolines_on_completion()     // complete a → dispatches b
fn nested_chain_completes()              // chain(a, chain(b, c)) → full pipeline
fn parallel_dispatches_all()            // parallel of 3 → 3 dispatches
fn parallel_collects_results()          // all complete → array result
fn foreach_dispatches_per_element()     // 3 elements → 3 dispatches
fn branch_dispatches_matching_case()    // only the selected case
fn loop_continue_re_dispatches()        // Continue → same handler again
fn loop_break_completes()              // Break → engine done with break value
fn attempt_wraps_success()              // Ok wrapper
fn attempt_catches_failure()            // Err wrapper, no propagation
fn step_follows_named()                 // enters the step's action
fn step_follows_root()                  // re-enters workflow
fn error_propagates_through_chain()
fn error_cancels_parallel_siblings()
fn nested_chain_in_parallel()
```

Test helpers: `invoke(name)` builds an `Action::Invoke`. `success(v)` builds `TaskResult::Success`. `engine_from(config)` flattens and constructs.
