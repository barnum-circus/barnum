# Engine Design

The engine does one thing: advance a cursor through a flat action table.

Given a cursor `(ActionId, Value)`, the engine walks the action table, creating frames for structural combinators that track partial progress, until every active path is suspended at an Invoke node with a pending dispatch. When a handler completes, the engine receives the result, pops the Invoke frame, and advances the parent frame — which may produce more dispatches or complete the workflow.

The engine has no knowledge of scheduling, concurrency, or I/O. It is a pure state machine.

## Frames

A frame tracks the engine's execution state at one structural combinator. Frames form a tree. Leaves are Invoke frames (suspended, waiting for handler results). Interior frames are combinators tracking partial progress.

Every combinator follows one of two patterns:

- **Single-child**: has one active child. When the child completes, the frame either completes upward (Attempt, Then) or re-enters the child (Loop on Continue). No indexing.
- **Fan-out**: has N children running concurrently. Each child fills a slot. When all slots are filled, the frame collects results and completes upward (Parallel, ForEach).

```rust
struct Frame {
    id: FrameId,
    parent: Option<ParentRef>,
    kind: FrameKind,
}

enum ParentRef {
    /// Parent has one child. No index needed.
    /// Used by: Then, Loop, Attempt.
    SingleChild { frame_id: FrameId },
    /// Parent has N children, this is child at `child_index`.
    /// Used by: Parallel, ForEach.
    IndexedChild { frame_id: FrameId, child_index: usize },
}

enum FrameKind {
    /// Leaf: handler dispatched, waiting for result.
    Invoke { task_id: TaskId },

    /// Sequential: run first child, then trampoline to rest.
    Then { rest: ActionId },

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

**Then does not mutate.** When its child completes, the Then frame removes itself and trampolines to `rest` with the original parent. At most one Then frame exists at a time per sequential chain.

## Engine state

```rust
struct Engine {
    flat: FlatConfig,
    frames: HashMap<FrameId, Frame>,
    task_to_frame: HashMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_frame_id: u32,
    next_task_id: u32,
    result: Option<EngineResult>,
}

struct Dispatch {
    task_id: TaskId,
    handler: HandlerKind,
    value: Value,
}

enum EngineResult {
    Success(Value),
    Failure(String),
}
```

## advance(action_id, value, parent)

Walk the action table from `action_id`. Recurse through structural nodes until reaching Invoke leaves.

```
advance(action_id, value, parent):
  match flat.action(action_id):

    Invoke { handler } =>
      create Invoke frame with parent
      queue dispatch(task_id, flat.handler(handler), value)
      register task_id → frame_id

    Then { rest } =>
      let first = flat.then_first(action_id)    // resolve child slot at action_id + 1
      create Then frame { rest } with parent
      advance(first, value, SingleChild(this_frame))

    Parallel { count } =>
      let children = flat.children(action_id)
      create Parallel frame (results = [None; count]) with parent
      for (i, child) in children:
        advance(child, value.clone(), IndexedChild(this_frame, i))

    ForEach { body } =>
      let elements = value as array
      create ForEach frame (results = [None; N]) with parent
      for (i, element) in elements:
        advance(body, element, IndexedChild(this_frame, i))

    Branch { count } =>
      let cases = flat.branch_cases(action_id)   // (KindDiscriminator, ActionId) pairs
      let kind = value["kind"]
      let case_id = cases.find(kind)
      advance(case_id, value, parent)       // no frame — pass through

    Loop { body } =>
      create Loop frame { body } with parent
      advance(body, value, SingleChild(this_frame))

    Attempt { child } =>
      create Attempt frame with parent
      advance(child, value, SingleChild(this_frame))

    Step { target } =>
      advance(target, value, parent)         // no frame — pass through
```

Branch and Step are tail calls — they redirect without allocating state.

## complete(parent_ref, value)

A child resolved. Advance the parent.

```
complete(parent_ref, value):
  if parent_ref is None:
    result = Success(value)
    return

  match parent_ref:
    SingleChild { frame_id } => complete_single(frame_id, value)
    IndexedChild { frame_id, child_index } => complete_indexed(frame_id, child_index, value)

complete_single(frame_id, value):
  let frame = frames[frame_id]
  match frame.kind:

    Then { rest } =>
      remove frame
      advance(rest, value, frame.parent)    // trampoline

    Loop { body } =>
      match value["kind"]:
        "Continue" => advance(body, value["value"], SingleChild(frame_id))
        "Break" =>
          remove frame
          complete(frame.parent, value["value"])

    Attempt =>
      remove frame
      complete(frame.parent, { kind: "Ok", value })

complete_indexed(frame_id, child_index, value):
  let frame = frames[frame_id]
  match frame.kind:

    Parallel { results } | ForEach { results } =>
      results[child_index] = Some(value)
      if results.iter().all(Option::is_some):
        let collected = results.drain(..).map(unwrap).collect()
        remove frame
        complete(frame.parent, Value::Array(collected))
```

## error(parent_ref, error)

A child failed.

```
error(parent_ref, error):
  if parent_ref is None:
    result = Failure(error)
    return

  let frame_id = parent_ref.frame_id()    // both variants have this
  let frame = frames[frame_id]

  match frame.kind:
    Attempt =>
      remove frame
      complete(frame.parent, { kind: "Err", error })

    Parallel | ForEach =>
      cancel all sibling Invoke frames (remove from frames + task_to_frame)
      remove frame
      error(frame.parent, error)

    _ =>
      remove frame
      error(frame.parent, error)
```

## on_task_completed(task_id, result)

```
on_task_completed(task_id, result):
  let frame_id = task_to_frame.remove(task_id)
  let frame = frames.remove(frame_id)
  match result:
    Success { value } => complete(frame.parent, value)
    Failure { error } => error(frame.parent, error)
```

## Public interface

```rust
impl Engine {
    fn new(flat: FlatConfig) -> Self;
    fn start(&mut self);                                         // advance(workflow, null, None)
    fn on_task_completed(&mut self, task_id: TaskId, result: TaskResult);
    fn take_pending_dispatches(&mut self) -> Vec<Dispatch>;      // always returns everything
    fn is_done(&self) -> bool;
    fn result(&self) -> Option<&EngineResult>;
}
```

## Testing strategy

```rust
fn single_invoke()                      // start → 1 dispatch
fn then_dispatches_first_only()         // then(a, b) → 1 dispatch (a)
fn then_trampolines_on_completion()     // complete a → dispatches b
fn nested_then_completes()              // then(a, then(b, c)) → full pipeline
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
fn error_propagates_through_then()
fn error_cancels_parallel_siblings()
fn nested_then_in_parallel()
```

Test helpers: `invoke(name)` builds an `Action::Invoke`. `success(v)` builds `TaskResult::Success`. `engine_from(config)` flattens and constructs.
