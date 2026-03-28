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
    frames: HashMap<FrameId, Frame>,
    task_to_frame: HashMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_frame_id: u32,
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
fn advance(&mut self, action_id: ActionId, value: Value, parent: Option<ParentRef>) {
    match self.flat_config.action(action_id) {
        FlatAction::Invoke { handler } => {
            let task_id = self.next_task_id();
            let frame_id = self.alloc_frame(Frame {
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

        FlatAction::Chain { rest } => {
            let first = self.flat_config.chain_first(action_id);
            let frame_id = self.alloc_frame(Frame {
                parent,
                kind: FrameKind::Chain { rest },
            });
            self.advance(first, value, Some(ParentRef::SingleChild { frame_id }));
        }

        FlatAction::Parallel { count } => {
            let children: Vec<ActionId> =
                self.flat_config.parallel_children(action_id).collect();
            let frame_id = self.alloc_frame(Frame {
                parent,
                kind: FrameKind::Parallel {
                    results: vec![None; count.0 as usize],
                },
            });
            for (i, child) in children.into_iter().enumerate() {
                self.advance(
                    child,
                    value.clone(),
                    Some(ParentRef::IndexedChild { frame_id, child_index: i }),
                );
            }
        }

        FlatAction::ForEach { body } => {
            let elements = match value {
                Value::Array(elements) => elements,
                other => panic!("ForEach expected array, got {other}"),
            };
            let frame_id = self.alloc_frame(Frame {
                parent,
                kind: FrameKind::ForEach {
                    results: vec![None; elements.len()],
                },
            });
            for (i, element) in elements.into_iter().enumerate() {
                self.advance(
                    body,
                    element,
                    Some(ParentRef::IndexedChild { frame_id, child_index: i }),
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
            let frame_id = self.alloc_frame(Frame {
                parent,
                kind: FrameKind::Loop { body },
            });
            self.advance(body, value, Some(ParentRef::SingleChild { frame_id }));
        }

        FlatAction::Attempt { child } => {
            let frame_id = self.alloc_frame(Frame {
                parent,
                kind: FrameKind::Attempt,
            });
            self.advance(child, value, Some(ParentRef::SingleChild { frame_id }));
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
fn complete(&mut self, parent: Option<ParentRef>, value: Value) {
    let Some(parent_ref) = parent else {
        self.result = Some(EngineResult::Success(value));
        return;
    };

    match parent_ref {
        ParentRef::SingleChild { frame_id } => self.complete_single(frame_id, value),
        ParentRef::IndexedChild { frame_id, child_index } => {
            self.complete_indexed(frame_id, child_index, value);
        }
    }
}

fn complete_single(&mut self, frame_id: FrameId, value: Value) {
    let frame = self.frames.remove(&frame_id).expect("frame not found");
    match frame.kind {
        FrameKind::Chain { rest } => {
            // Trampoline: remove this frame, advance rest with original parent.
            self.advance(rest, value, frame.parent);
        }

        FrameKind::Loop { body } => {
            let kind = value["kind"].as_str().expect("Loop result must have 'kind'");
            let inner = value.get("value").cloned().unwrap_or(Value::Null);
            match kind {
                "Continue" => {
                    // Re-insert the frame and re-enter the body.
                    self.frames.insert(frame_id, Frame {
                        parent: frame.parent,
                        kind: FrameKind::Loop { body },
                    });
                    self.advance(body, inner, Some(ParentRef::SingleChild { frame_id }));
                }
                "Break" => {
                    self.complete(frame.parent, inner);
                }
                other => panic!("Loop result kind must be Continue or Break, got {other}"),
            }
        }

        FrameKind::Attempt => {
            let wrapped = serde_json::json!({ "kind": "Ok", "value": value });
            self.complete(frame.parent, wrapped);
        }

        other => panic!("complete_single called on {other:?}"),
    }
}

fn complete_indexed(&mut self, frame_id: FrameId, child_index: usize, value: Value) {
    let frame = self.frames.get_mut(&frame_id).expect("frame not found");
    let results = match &mut frame.kind {
        FrameKind::Parallel { results } | FrameKind::ForEach { results } => results,
        other => panic!("complete_indexed called on {other:?}"),
    };

    results[child_index] = Some(value);

    if results.iter().all(Option::is_some) {
        let collected: Vec<Value> = results.drain(..).map(Option::unwrap).collect();
        let frame = self.frames.remove(&frame_id).expect("frame not found");
        self.complete(frame.parent, Value::Array(collected));
    }
}
```

## error

A child failed.

```rust
fn error(&mut self, parent: Option<ParentRef>, error: String) {
    let Some(parent_ref) = parent else {
        self.result = Some(EngineResult::Failure(error));
        return;
    };

    let frame_id = parent_ref.frame_id();
    let frame = self.frames.remove(&frame_id).expect("frame not found");

    match frame.kind {
        FrameKind::Attempt => {
            let wrapped = serde_json::json!({ "kind": "Err", "error": error });
            self.complete(frame.parent, wrapped);
        }

        FrameKind::Parallel { .. } | FrameKind::ForEach { .. } => {
            self.cancel_descendants(frame_id);
            self.error(frame.parent, error);
        }

        _ => {
            self.error(frame.parent, error);
        }
    }
}
```

## on_task_completed

```rust
fn on_task_completed(&mut self, task_id: TaskId, result: TaskResult) {
    let frame_id = self.task_to_frame.remove(&task_id).expect("unknown task");
    let frame = self.frames.remove(&frame_id).expect("frame not found");
    match result {
        TaskResult::Success { value } => self.complete(frame.parent, value),
        TaskResult::Failure { error } => self.error(frame.parent, error),
    }
}
```

## Public interface

```rust
impl Engine {
    fn new(flat_config: FlatConfig) -> Self;
    fn start(&mut self);
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
