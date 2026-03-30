# Phase 1: Effect Substrate

## Goal

Build the structural routing mechanism for algebraic effects in the Rust scheduler. After this phase, a Perform can suspend execution, bubble up to a Handle, and be resumed or discarded. No semantic effects yet — just the plumbing.

## Prerequisites

None. This is the foundation.

## New AST nodes (tree AST, TypeScript)

```ts
export interface HandleAction {
  kind: "Handle";
  effect_id: number;  // EffectId (opaque u16, max 65535)
  handler: Action;    // the handler DAG to run when the effect fires
  body: Action;       // the body to execute within this Handle's scope
}

export interface PerformAction {
  kind: "Perform";
  effect_id: number;  // EffectId (opaque u16, must match some enclosing Handle)
}
```

Two new nodes. Each Handle intercepts exactly one effect. If you need to handle multiple effects, nest multiple Handles — that's composition.

There is no `ResumeAction`. Resumption, discarding, and body re-entry are the Handle frame's interpretation of the handler DAG's output.

### Continuation operations (Rust enum)

The handler DAG's output tells the Handle frame what to do with the suspended continuation. This is a closed Rust enum — these are the scheduler's internal protocol, not user-extensible.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationOp {
    /// Reconnect the continuation. Deliver value to the Perform's parent.
    /// Body continues from suspension point.
    /// Value type: what the body expects at the Perform site.
    Resume,
    /// Tear down the continuation. Deliver value to the Handle's parent.
    /// Handle exits.
    /// Value type: the Handle's output type.
    Discard,
    /// Tear down the continuation. Re-advance the body with the value.
    /// Handle stays alive.
    /// Value type: the body's input type.
    RestartBody,
}
```

The `value` field in each variant has a different semantic type:

| Op | Value delivered to | Value type |
|---|---|---|
| Resume | Perform's parent (body continues) | What the body expects at the suspension point |
| Discard | Handle's parent (Handle exits) | The Handle's output type |
| RestartBody | Body re-entry (fresh start) | The body's input type |

On the Rust side, all three are `serde_json::Value` — types are erased at serialization. On the TypeScript side, the handler DAG's output is a proper discriminated union with separate type parameters per variant:

```ts
type StateUpdate<TState> =
  | { kind: "Unchanged" }
  | { kind: "Updated"; value: TState };

type HandlerOutput<TResume, TDiscard, TRestart, TState> =
  | { kind: "Resume"; value: TResume; state_update: StateUpdate<TState> }
  | { kind: "Discard"; value: TDiscard }
  | { kind: "RestartBody"; value: TRestart; state_update: StateUpdate<TState> };
```

Discard has no `state_update` — the Handle frame is being torn down, so updating its state is meaningless. Only Resume and RestartBody carry state updates because the Handle frame survives those operations.

In practice, no handler produces all three variants. Each combinator narrows the union:

- **ReadVar handler**: `HandlerOutput<TValue, never, never, TState>` — only Resume, always `Unchanged`.
- **Throw handler**: `HandlerOutput<never, TRecoveryResult, never, TState>` — only Discard (no state_update).
- **Loop recur handler**: `HandlerOutput<never, never, TBodyInput, TState>` — only RestartBody, always `Unchanged`.
- **Loop done handler**: `HandlerOutput<never, TBreakValue, never, TState>` — only Discard (no state_update).
- **Retry handler**: `HandlerOutput<never, TError, TBodyInput, TState>` — RestartBody with `Updated(count - 1)`, or Discard when exhausted.
- **withState Put handler**: `HandlerOutput<null, never, never, TState>` — Resume with `Updated(new_value)`.

TypeScript enforces that each handler DAG only constructs the variants it should. A readVar handler that accidentally produces Discard is a type error.

Resume and Discard are the canonical two operations on continuations in the algebraic effects literature. In systems with generalized tail call optimization (Koka, Unison), RestartBody is unnecessary — loops are pure recursion and the runtime optimizes away the frame growth.

RestartBody exists because our scheduler is a cooperative slab-based state machine without generalized TCO. Without it, a loop modeled as pure recursion (handler DAG = recursive ref to Handle node) pushes a new Handle frame per iteration → OOM. RestartBody is a localized trampoline: the Handle frame tears down the old body frames and re-advances the body ActionId. O(1) memory, no complex tail-call analysis.

No handler ever uses both Resume and RestartBody. The partition is clean: ReadVar handlers always Resume. Throw handlers always Discard. Loop handlers use RestartBody (Continue) or Discard (Break).

### Effect routing: opaque gensym'd IDs

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EffectId(pub u16);
```

`u16`, not `u32`. 65535 effects per workflow is more than enough (each `declare`, `tryCatch`, `loop` invocation mints one or two). The smaller size is critical: `FlatAction::Handle { effect_id: EffectId, handler: ActionId }` = `u16 + u32` = 6 bytes, which keeps `FlatEntry` at 8 bytes (verified empirically). A `u32` EffectId would push Handle to 8 bytes payload and break the constraint.

Generated by the TypeScript builder's monotonic counter. The Rust engine compares integers. It never interprets what an effect means. No enum, no strings, no Rust changes for new effects.

## Generalized Handler State

The Handle frame carries opaque state. The Rust engine never interprets it — it's just `serde_json::Value`. All semantic meaning (variable bindings, retry counters, resource handles) lives in the handler DAGs that read and write the state. Handler DAGs are normal AST subgraphs — authored in the TypeScript builder layer, but compiled to the same action nodes as everything else and executed by the Rust engine. Most handler DAGs (readVar, loop control, tryCatch routing) are built entirely from builtins and never leave Rust.

### How it works

1. **Frame initialization.** When the engine enters a Handle frame, it stores its input value as `state` in the frame. The same input value is also passed to the body. (If the state needs to differ from the body's input, the TypeScript compilation layer shapes the input via Chain/Parallel prefix — the engine doesn't know or care.)

2. **Handler input.** When a Perform fires and the handler DAG runs, the engine constructs a flat object and passes it to the handler DAG:
   ```json
   { "payload": <the_perform_value>, "state": <the_opaque_state> }
   ```
   The handler DAG uses standard, domain-ignorant builtins (`ExtractField`, `Pick`, etc.) to extract what it needs from the state.

3. **Handler output.** The handler DAG produces a tagged output. Resume and RestartBody carry an explicit `state_update`; Discard does not (the frame is about to be destroyed).

   Resume (state survives — Handle stays alive, body continues):
   ```json
   { "kind": "Resume", "value": <result>, "state_update": { "kind": "Unchanged" } }
   { "kind": "Resume", "value": <result>, "state_update": { "kind": "Updated", "value": <new_state> } }
   ```

   RestartBody (state survives — Handle stays alive, body restarts):
   ```json
   { "kind": "RestartBody", "value": <body_input>, "state_update": { "kind": "Unchanged" } }
   { "kind": "RestartBody", "value": <body_input>, "state_update": { "kind": "Updated", "value": <new_state> } }
   ```

   Discard (frame destroyed — no state_update):
   ```json
   { "kind": "Discard", "value": <exit_value> }
   ```

   The `state_update` field is a discriminated union. The Rust enum:
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub enum StateUpdate {
       Unchanged,
       Updated(Value),
   }
   ```

### Why this eliminates abstraction leaks

Under this architecture, there is no cache. There are no variables. There is only a delimited scope that holds an arbitrary state value, and an effect handler that reads and writes to that state.

- **`declare`** uses state as a read-only dictionary (the Reader monad). Handler extracts the requested variable from the state map, Resumes with it. State update is always `Unchanged`.
- **`withState`** uses state as a read/write cell (the State monad). Get handler Resumes with the current state (`Unchanged`). Put handler Resumes with null and produces `Updated(new_value)`.
- **`retry`** uses state to hold an integer counter. On each Throw, the handler produces `Updated(count - 1)` and RestartBody. When the counter reaches zero, it produces Discard with `Unchanged`.

The engine remains a pure structural router. It shuttles opaque JSON between frames and handler DAGs. All semantic meaning is constructed entirely out of context-free AST combinators in the TypeScript layer.

## New flat table entries (Rust)

```rust
pub enum FlatAction<T> {
    // ... existing variants (Invoke, Chain, Parallel, ForEach, Branch, Loop, Step) ...

    /// Install a scoped effect handler. Body is a child slot at `action_id + 1`
    /// (same pattern as Chain stores `first`).
    ///
    /// 2-entry action: the Handle entry itself, followed by one child slot
    /// for the body.
    Handle {
        effect_id: EffectId,
        handler: ActionId,
    },

    Perform {
        effect_id: EffectId,
    },
}
```

Handle is a 2-entry action (like Chain): the Handle entry itself, followed by one child slot for the body. The body child slot is either an inlined single-entry action or a `ChildRef` to a multi-entry subtree elsewhere. The handler is an `ActionId` pointing elsewhere in the table. This keeps Handle at two `u32` fields, preserving the 8-byte `FlatEntry` constraint.

Perform is a 1-entry action (like Invoke).

Both need to be added to `FlatAction::try_map_target` (trivial — they pass through unchanged like Invoke/Chain/etc.).

A `handle_body` accessor on `FlatConfig` resolves the child slot, mirroring `chain_first`:

```rust
impl FlatConfig {
    /// Returns the body `ActionId` for a Handle (resolves the child
    /// slot at `action_id + 1`).
    #[must_use]
    pub fn handle_body(&self, id: ActionId) -> ActionId {
        debug_assert!(matches!(self.action(id), FlatAction::Handle { .. }));
        self.resolve_child_slot(id + 1)
    }
}
```

## New frame kind (Rust scheduler)

The frame kind stores all three IDs (effect_id, handler, body) even though the flat table only stores two. The body `ActionId` is resolved from the child slot during `advance` and stored in the frame for RestartBody re-entry.

```rust
/// Named struct for the Handle frame's state. Lives inside
/// `FrameKind::Handle(HandleFrame)` — avoids anonymous struct
/// fields and lets methods take `&mut HandleFrame` directly.
#[derive(Debug)]
pub struct HandleFrame {
    pub effect_id: EffectId,
    pub handler: ActionId,
    /// Resolved from the child slot during advance. Stored here
    /// because RestartBody needs it to re-enter the body.
    pub body: ActionId,
    /// Opaque state. Initialized from the Handle's input value.
    /// Updated by handler DAG output's `state_update` field.
    pub state: Value,
    /// The suspended continuation, if a Perform has fired and the handler is running.
    /// None when the body is running normally.
    /// Some when the body is suspended and the handler is in flight.
    pub continuation: Option<ContinuationRoot>,
}

pub enum FrameKind {
    // ... existing variants (Chain, Parallel, ForEach, Loop) ...

    Handle(HandleFrame),
}

/// The suspended continuation: tracks where to deliver Resume values,
/// buffers async completions that arrive while suspended, and identifies
/// what to clean up on Discard/RestartBody.
#[derive(Debug)]
struct ContinuationRoot {
    /// Where the Perform would have delivered its result.
    /// On Resume, deliver the value here to continue the body.
    /// Not Option: a Perform with no parent has no Handle above it,
    /// so bubble_effect returns UnhandledEffect before we get here.
    perform_parent: ParentRef,
    /// External task completions that arrived while the body was suspended.
    /// Buffered in arrival order. On Resume, replayed sequentially after
    /// delivering the Resume value. On Discard/RestartBody, dropped
    /// (the frames they target are torn down).
    pending_deliveries: Vec<(ParentRef, Value)>,
}
```

One Handle, one effect, one handler, one continuation. All singular. At most one continuation exists at a time per Handle — the body subgraph is synchronously frozen at the Perform point, so no further Performs can fire from that execution path. However, other branches of a Parallel in the body may have in-flight external tasks that complete while the handler is running. These completions are buffered on the continuation (see "Async completions during suspension" below).

## advance for Handle and Perform

New match arms in `WorkflowState::advance`:

```rust
FlatAction::Handle { effect_id, handler } => {
    let body = self.flat_config.handle_body(action_id);
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::Handle(HandleFrame {
            effect_id,
            handler,
            body,
            state: value.clone(),       // input = initial state
            continuation: None,
        }),
    });
    self.advance(body, value, Some(ParentRef::SingleChild { frame_id }))?;
}

FlatAction::Perform { effect_id } => {
    self.bubble_effect(parent, effect_id, value)?;
}
```

Handle stores its input as state and passes the same input to its body. Perform is a leaf (like Invoke) — it doesn't create a frame.

`bubble_effect` returns `Result<(), AdvanceError>`. A new variant is needed:

```rust
pub enum AdvanceError {
    // ... existing variants ...

    /// A Perform fired but no enclosing Handle intercepts this effect.
    #[error("unhandled effect: {effect_id}")]
    UnhandledEffect {
        /// The effect ID that was not intercepted.
        effect_id: EffectId,
    },
}
```

## bubble_effect: the core traversal

When a Perform is evaluated, the scheduler walks up parent pointers looking for a Handle frame whose `effect_id` field matches.

```rust
fn bubble_effect(
    &mut self,
    starting_parent: Option<ParentRef>,
    effect_id: EffectId,
    payload: Value,
) -> Result<(), AdvanceError> {
    let mut current = starting_parent;

    while let Some(parent_ref) = current {
        let parent_id = parent_ref.frame_id();
        let parent = &self.frames[parent_id.0];

        if let FrameKind::Handle(handle_frame) = &parent.kind {
            if handle_frame.effect_id == effect_id {
                // starting_parent is guaranteed Some: if it were None, the while
                // loop would never have entered and we'd have returned UnhandledEffect.
                let perform_parent = starting_parent
                    .expect("Perform with no parent cannot reach a Handle");
                return self.dispatch_to_handler(parent_id, perform_parent, payload);
            }
        }

        current = parent.parent;
    }

    Err(AdvanceError::UnhandledEffect { effect_id })
}
```

Walk up, compare integers, stop on match. Chain, Parallel, Branch, ForEach are invisible — the traversal walks right past them. O(depth of frame tree).

## dispatch_to_handler: severing the body and starting the handler

```rust
fn dispatch_to_handler(
    &mut self,
    handle_frame_id: FrameId,
    perform_parent: ParentRef,
    payload: Value,
) -> Result<(), AdvanceError> {
    // 1. Store the continuation (the Perform's delivery point).
    //    bubble_effect guarantees this is a Handle frame.
    let FrameKind::Handle(handle_frame) = &mut self.frames[handle_frame_id.0].kind else {
        unreachable!("dispatch_to_handler called on non-Handle frame");
    };
    assert!(
        handle_frame.continuation.is_none(),
        "double Perform: Handle already has a suspended continuation"
    );
    handle_frame.continuation = Some(ContinuationRoot {
        perform_parent,
        pending_deliveries: Vec::new(),
    });
    let handler = handle_frame.handler;
    let state = handle_frame.state.clone();

    // 2. Construct handler input: { payload, state }.
    let handler_input = serde_json::json!({
        "payload": payload,
        "state": state,
    });

    // 3. Advance the handler DAG as a new child of the Handle frame.
    //    The body subgraph is naturally frozen (Perform point is stuck).
    //    The handler's frames use the Handle as parent (same as the body).
    //    This is safe because the body can't deliver to the Handle while
    //    the Perform is unresolved.
    self.advance(
        handler,
        handler_input,
        Some(ParentRef::SingleChild { frame_id: handle_frame_id }),
    )?;

    Ok(())
}
```

**Why we don't sever parent pointers.** The body's frame tree stays connected. We don't null out or reroute any `ParentRef`. The Perform point is synchronously stuck (it triggered bubble_effect instead of producing a Dispatch), so no further synchronous execution propagates from that point. The handler DAG runs as a new child of the Handle, using the same `ParentRef::SingleChild` relationship. When the handler completes and delivers to the Handle, the Handle knows it's the handler (because `continuation.is_some()`).

However, the body is NOT completely frozen — other branches of a Parallel may have in-flight external tasks. When those tasks complete, their results must be **buffered**, not delivered. See "Async completions during suspension" below.

**Double Perform (synchronous).** If two bare Performs for the same effect appear as immediate children of a Parallel (no async work between advance and Perform), the assert fires — the first Perform stores a continuation, and the second finds `continuation.is_some()`. This is a malformed AST. The TypeScript builder never produces it. Two Performs for the same effect in a Parallel always have async work (Invoke) before them, which means the second Perform can only fire after an external task completes — at which point it's intercepted by the buffering mechanism, not by `advance`.

## Async completions during suspension

When a Perform fires inside a Parallel branch, other branches may have external tasks still in flight. When those tasks complete, the engine must NOT deliver their results into the suspended body — that would advance the body (Chain trampolining, further actions executing) while the handler might Discard the entire subgraph.

The solution: **buffer at `complete_task`.** Before delivering, walk up from the target frame to check for a suspended Handle ancestor. If found, append the delivery to the continuation's `pending_deliveries`. If not, deliver normally.

The core primitive is `deliver_or_buffer`: check for a suspended Handle ancestor, buffer if found, deliver if not. Both `complete_task` and `resume_continuation`'s replay loop use this same function.

```rust
/// Deliver a value to a parent frame, unless the target is inside a
/// suspended body. If suspended, buffer the delivery on the innermost
/// suspended Handle's continuation. O(depth) per call.
fn deliver_or_buffer(
    &mut self,
    parent_ref: ParentRef,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    if let Some(handle_frame_id) = self.find_suspended_ancestor(parent_ref.frame_id()) {
        let FrameKind::Handle(handle_frame) = &mut self.frames[handle_frame_id.0].kind else {
            unreachable!();
        };
        let continuation = handle_frame.continuation.as_mut()
            .expect("suspended Handle must have continuation");
        continuation.pending_deliveries.push((parent_ref, value));
        Ok(None)
    } else {
        self.deliver(Some(parent_ref), value)
    }
}

fn complete_task(
    &mut self,
    task_id: TaskId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let parent_ref = self.task_to_parent.remove(&task_id);
    match parent_ref {
        None => {
            // Task was already cancelled (frame torn down by Discard/RestartBody).
            // Silently drop the result.
            Ok(None)
        }
        Some(parent_ref) => self.deliver_or_buffer(parent_ref, value),
    }
}

/// Walk up from `frame_id` looking for an ancestor Handle frame
/// that has a suspended continuation. Returns the first one found
/// (innermost suspended Handle).
fn find_suspended_ancestor(&self, frame_id: FrameId) -> Option<FrameId> {
    let mut current = self.frames.get(frame_id.0)?.parent;
    while let Some(parent_ref) = current {
        let parent_id = parent_ref.frame_id();
        if let FrameKind::Handle(handle_frame) = &self.frames[parent_id.0].kind {
            if handle_frame.continuation.is_some() {
                return Some(parent_id);
            }
        }
        current = self.frames.get(parent_id.0)?.parent;
    }
    None
}
```

**Why `deliver_or_buffer` is needed in both `complete_task` and replay.** `complete_task` is the entry point for external async completions — the obvious case. But during `resume_continuation`'s replay loop, a replayed delivery can trigger a Perform on a *different* Handle (e.g., inner), causing that Handle to suspend. Subsequent replayed deliveries may target frames inside that newly-suspended body. Without the `find_suspended_ancestor` check during replay, those deliveries would advance the frozen body.

**Delivery migration across Handles.** A delivery originally buffered on Handle A can migrate to Handle B during replay. When A resumes and replays its buffer, `deliver_or_buffer` discovers that the target is now inside B's suspended body and buffers it there. No explicit transfer logic — `find_suspended_ancestor` finds the right Handle each time.

**Why handler completions aren't buffered.** The handler's Invoke has `parent = SingleChild { frame_id: handle_frame_id }`. `find_suspended_ancestor` walks up from the Handle frame's parent (whatever is above the Handle). The Handle itself is not checked — only ancestors above it. So handler task completions deliver normally to the Handle frame, triggering `handle_handler_completion`.

**Cancelled tasks.** When `teardown_body` runs (on Discard or RestartBody), it removes body frames from the slab and removes their `task_to_parent` entries. If those tasks later complete, `complete_task` finds no entry and silently drops the result. No explicit cancel signal is sent to the external driver — tasks run to completion, and their results are ignored.

### Concrete example

```
Handle(effect, handler,
  Parallel(
    Chain(Invoke(taskA), Perform(effect)),
    Chain(Invoke(taskB), Perform(effect)),
  )
)
```

1. Advance: Parallel dispatches both branches. Chain A and Chain B created. Invoke A and B dispatch external tasks.
2. Task A completes. `complete_task` → `find_suspended_ancestor(chain_a)` → no suspended Handle → deliver normally. Chain A trampolines → advance Perform → `bubble_effect` → `dispatch_to_handler`. Continuation stored. Handler starts.
3. Task B completes. `complete_task` → `find_suspended_ancestor(chain_b)` → walks up to Parallel → walks up to Handle → Handle has `continuation.is_some()` → **buffer** `(chain_b_parent, taskB_result)` on continuation.
4. Handler completes with Resume(V):
   - `resume_continuation` delivers V to `perform_parent` (Parallel slot for branch A via Chain A's old parent). Chain A was removed by trampoline, so the parent is `ParallelChild { parallel_id, 0 }`. Parallel stores V in slot 0. Not all slots full → returns.
   - Replay: deliver `(chain_b_parent, taskB_result)`. Chain B receives it, trampolines to Perform(effect). Perform fires `bubble_effect`. Handle has `continuation: None` (we cleared it). `dispatch_to_handler` runs normally. Second handler invocation starts.
   - Second handler completes with Resume(W). Deliver W to Parallel slot 1 (via branch B's path). Both slots full. Parallel joins and delivers to Handle. Handle exits.
5. **OR** handler completes with Discard(E):
   - `discard_continuation` drops `pending_deliveries` (taskB's buffered result is gone).
   - `teardown_body` removes Parallel, Chain B, and their `task_to_parent` entries.
   - Handle exits with E.

## Handler completion (deliver to Handle frame)

The `deliver` method needs a new match arm for Handle in the SingleChild case:

```rust
ParentRef::SingleChild { .. } => {
    // Check if this is a handler completion (not a body completion).
    let is_handler_completion = matches!(
        &self.frames[frame_id.0].kind,
        FrameKind::Handle(HandleFrame { continuation: Some(_), .. })
    );

    if is_handler_completion {
        return self.handle_handler_completion(frame_id, value);
    }

    // Normal single-child completion path (body completed, or Chain/Loop).
    let frame = self.frames.remove(frame_id.0);
    match frame.kind {
        FrameKind::Chain { rest } => {
            self.advance(rest, value, frame.parent)?;
            Ok(None)
        }
        FrameKind::Loop { body } => {
            // ... existing Loop logic ...
        }
        FrameKind::Handle(_) => {
            // Body completed normally. No continuation to clean up.
            // Handle exits, deliver value to parent.
            self.deliver(frame.parent, value)
        }
        _ => unreachable!(
            "SingleChild parent must be Chain, Loop, or Handle, got {:?}",
            frame.kind
        ),
    }
}
```

### handle_handler_completion

```rust
fn handle_handler_completion(
    &mut self,
    handle_frame_id: FrameId,
    output: Value,
) -> Result<Option<Value>, CompleteError> {
    // 1. Parse continuation operation from handler output.
    let kind_str = output["kind"].as_str()
        .ok_or_else(|| CompleteError::InvalidHandlerOutput {
            value: output.clone(),
        })?;
    let value = output["value"].clone();

    // 2. Take the continuation from the frame.
    let FrameKind::Handle(handle_frame) = &mut self.frames[handle_frame_id.0].kind else {
        unreachable!("handle_handler_completion called on non-Handle frame");
    };
    let continuation = handle_frame.continuation.take()
        .expect("handler completion requires stored continuation");

    // 3. Dispatch on the continuation operation.
    //    Resume and RestartBody carry state_update. Discard does not
    //    (the frame is about to be torn down).
    match kind_str {
        "Resume" => {
            self.apply_state_update(handle_frame_id, &output)?;
            self.resume_continuation(handle_frame_id, continuation, value)
        }
        "Discard" => {
            self.discard_continuation(handle_frame_id, continuation, value)
        }
        "RestartBody" => {
            self.apply_state_update(handle_frame_id, &output)?;
            self.restart_body(handle_frame_id, continuation, value)
        }
        _ => Err(CompleteError::InvalidHandlerOutput { value: output }),
    }
}

fn apply_state_update(
    &mut self,
    handle_frame_id: FrameId,
    output: &Value,
) -> Result<(), CompleteError> {
    let state_update_kind = output["state_update"]["kind"].as_str()
        .ok_or_else(|| CompleteError::InvalidHandlerOutput {
            value: output.clone(),
        })?;
    match state_update_kind {
        "Unchanged" => Ok(()),
        "Updated" => {
            let new_state = output["state_update"]["value"].clone();
            let FrameKind::Handle(handle_frame) = &mut self.frames[handle_frame_id.0].kind else {
                unreachable!("apply_state_update called on non-Handle frame");
            };
            handle_frame.state = new_state;
            Ok(())
        }
        _ => Err(CompleteError::InvalidHandlerOutput { value: output.clone() }),
    }
}
```

New `CompleteError` variant:

```rust
pub enum CompleteError {
    // ... existing variants ...

    /// Handler DAG output is not a valid `{ kind, value }` object.
    #[error("invalid handler output: {value}")]
    InvalidHandlerOutput {
        /// The invalid output value.
        value: Value,
    },
}
```

## Resume

Reconnect the continuation. Deliver the Resume value, then replay any buffered async completions. Each replay uses `deliver_or_buffer` so that deliveries targeting a newly-suspended inner Handle are buffered there, not delivered into the frozen body.

```rust
fn resume_continuation(
    &mut self,
    handle_frame_id: FrameId,
    continuation: ContinuationRoot,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    // 1. Deliver the Resume value to where the Perform would have delivered.
    //    This wakes up the Chain/Parallel/etc. that was waiting for the Perform's result.
    let result = self.deliver(Some(continuation.perform_parent), value)?;
    if result.is_some() {
        return Ok(result); // workflow completed
    }

    // 2. Replay buffered async completions in arrival order.
    //    Use deliver_or_buffer for each: a previous replay may have caused
    //    a Perform on a different Handle, suspending part of the tree.
    for (parent_ref, value) in continuation.pending_deliveries {
        let result = self.deliver_or_buffer(parent_ref, value)?;
        if result.is_some() {
            return Ok(result); // workflow completed
        }
    }

    Ok(None)
}
```

The body subgraph is still intact (we never severed it). Delivering to `perform_parent` is identical to what would have happened if the Perform had been a normal Invoke that completed.

**Delivery migration.** During replay, `deliver_or_buffer` may route a delivery to a *different* Handle's continuation. Consider middle/inner Handles with a Parallel body:

1. Middle resumes. Replay P2's delivery → `deliver_or_buffer` → no suspended ancestor → deliver normally → Chain trampolines → Perform(inner) → inner suspends.
2. Replay P3's delivery → `deliver_or_buffer` → `find_suspended_ancestor` finds inner → buffers P3 on inner's continuation.
3. Inner's handler completes → `resume_continuation` on inner → replays P3 → `deliver_or_buffer` → no suspended ancestor → deliver normally.

P3 migrated from middle's buffer to inner's buffer automatically. No explicit transfer logic.

**Re-suspension on the same Handle.** If a replayed delivery triggers a Perform on THIS Handle (middle), `deliver_or_buffer` finds middle (since the remaining deliveries target frames inside middle's body, and middle just re-suspended). They get buffered on middle's new continuation. The second handler eventually completes and replays them.

## Discard

Tear down the continuation (body subgraph). Deliver the value to the Handle's parent. Handle exits.

```rust
fn discard_continuation(
    &mut self,
    handle_frame_id: FrameId,
    continuation: ContinuationRoot,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    // 1. Teardown the body subgraph.
    self.teardown_body(handle_frame_id);

    // 2. Handle exits, deliver value to parent.
    let parent = self.frames[handle_frame_id.0].parent;
    self.frames.remove(handle_frame_id.0);
    Ok(self.deliver(parent, value)?)
}
```

## RestartBody

Tear down the continuation (body subgraph). Re-advance the body from scratch with the new value. Handle stays alive.

```rust
fn restart_body(
    &mut self,
    handle_frame_id: FrameId,
    continuation: ContinuationRoot,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    // 1. Teardown the old body subgraph.
    self.teardown_body(handle_frame_id);

    // 2. Re-advance the body with the new value.
    let FrameKind::Handle(handle_frame) = &self.frames[handle_frame_id.0].kind else {
        unreachable!("restart_body called on non-Handle frame");
    };
    let body = handle_frame.body;
    self.advance(body, value, Some(ParentRef::SingleChild { frame_id: handle_frame_id }))?;
    Ok(None)
}
```

## teardown_body: cleaning up the body subgraph

Removes all frames that are descendants of the Handle (in the body subgraph, not in the handler) and removes their `task_to_parent` entries. In-flight external tasks are not explicitly cancelled — they run to completion, and when they do, `complete_task` finds no `task_to_parent` entry and silently drops the result. The `pending_deliveries` buffer on the ContinuationRoot was already taken by the caller (Discard/RestartBody) and is dropped.

```rust
fn teardown_body(&mut self, handle_frame_id: FrameId) {
    // Collect frame IDs to remove: all frames whose parent chain leads
    // to the Handle frame (without passing through it from above).
    let body_frames: Vec<FrameId> = self.frames
        .iter()
        .filter_map(|(id, frame)| {
            if self.is_descendant_of(FrameId(id), handle_frame_id) {
                Some(FrameId(id))
            } else {
                None
            }
        })
        .collect();

    // Remove body frames from the slab.
    for frame_id in &body_frames {
        self.frames.remove(frame_id.0);
    }

    // Remove task_to_parent entries for body tasks.
    // In-flight tasks will complete later; complete_task silently drops
    // results for unknown task_ids.
    let body_frame_set: std::collections::HashSet<FrameId> = body_frames.into_iter().collect();
    self.task_to_parent.retain(|_, parent| {
        match parent {
            Some(parent_ref) => !body_frame_set.contains(&parent_ref.frame_id()),
            None => true,
        }
    });
}

/// Check if `frame_id` is a descendant of `ancestor_id` (child, grandchild, etc.).
fn is_descendant_of(&self, frame_id: FrameId, ancestor_id: FrameId) -> bool {
    let mut current = self.frames.get(frame_id.0)
        .and_then(|f| f.parent);
    while let Some(parent_ref) = current {
        let parent_id = parent_ref.frame_id();
        if parent_id == ancestor_id {
            return true;
        }
        current = self.frames.get(parent_id.0)
            .and_then(|f| f.parent);
    }
    false
}
```

This is O(n × depth) where n is the number of frames in the slab. Acceptable for Phase 1. If profiling shows this is hot, we can add child-pointer tracking or generation counters later.

## Flattener changes

### Tree AST additions

Add `Handle` and `Perform` to the `Action` enum in `barnum_ast/src/lib.rs`:

```rust
pub enum Action {
    // ... existing variants ...
    Handle(HandleAction),
    Perform(PerformAction),
}

pub struct HandleAction {
    pub effect_id: EffectId,
    pub handler: Box<Action>,
    pub body: Box<Action>,
}

pub struct PerformAction {
    pub effect_id: EffectId,
}
```

### flatten_action_at additions

New match arms in `UnresolvedFlatConfig::flatten_action_at`:

```rust
Action::Handle(HandleAction { effect_id, handler, body }) => {
    // Child slot for body (same pattern as Chain allocates a slot for first).
    self.alloc();
    let handler_id = self.flatten_action(*handler, workflow_root)?;
    self.fill_child_slot(*body, action_id + 1, workflow_root)?;
    FlatAction::Handle {
        effect_id,
        handler: handler_id,
    }
}

Action::Perform(PerformAction { effect_id }) => {
    FlatAction::Perform { effect_id }
}
```

Handle is a 2-entry action (like Chain): one entry for itself, one child slot for the body. The handler is flattened elsewhere and stored as an `ActionId` in the variant. Perform is a 1-entry leaf.

### fill_child_slot addition

Handle is a multi-entry action (2 entries). Add to the multi-entry branch:

```rust
fn fill_child_slot(&mut self, action: Action, ...) {
    match action {
        Action::Chain { .. } | Action::Parallel { .. } | Action::Branch { .. }
        | Action::Handle { .. } => {
            // Multi-entry: flatten elsewhere, write ChildRef.
            let action_id = self.flatten_action(action, workflow_root)?;
            self.entries[slot.0 as usize] = Some(FlatEntry::ChildRef { action: action_id });
        }
        single_entry => { ... }
    }
}
```

### try_map_target addition

```rust
FlatAction::Handle { effect_id, handler } => FlatAction::Handle { effect_id, handler },
FlatAction::Perform { effect_id } => FlatAction::Perform { effect_id },
```

Both pass through unchanged (no Step target to resolve).

## Interaction with existing frame kinds

### Chain

No changes. If a child suspends (via bubble_effect walking past the Chain frame), the Chain frame sits dormant. When the continuation is resumed and the child eventually delivers a value, the Chain frame's existing `deliver` logic advances to the rest.

### Parallel

No changes. If child A suspends, A's slot in the Parallel frame's results vector remains empty. When the continuation is resumed and A delivers, the Parallel frame's existing logic checks if all slots are full and joins if so. This is structurally identical to waiting for an external Invoke to complete.

### Branch

No changes. Same pattern as Chain.

### ForEach

No changes. Same pattern as Parallel — each iteration is a slot, suspended iterations are empty slots.

### Loop (before Phase 4 migration)

During Phase 1, Loop still exists as a separate frame kind. It doesn't interact with Handle/Perform. After Phase 4, Loop is replaced by two nested Handles (see Phase 4).

## Test strategy

Tests use synthetic effects — trivial handler DAGs that exercise the substrate without semantic meaning. Each handler DAG is built from existing AST nodes (Invoke with builtins, Chain, Branch).

### Constructing synthetic effects in tests

```rust
// Helper: create a handler DAG that always Resumes with a fixed value.
// Handler receives { payload, state } but ignores both.
fn always_resume_handler(value: Value) -> Action {
    Action::Invoke(InvokeAction {
        handler: HandlerKind::Builtin(BuiltinHandler {
            builtin: BuiltinKind::Constant {
                value: json!({
                    "kind": "Resume",
                    "value": value,
                    "state_update": { "kind": "Unchanged" },
                }),
            },
        }),
    })
}

// Helper: handler that Resumes with the payload (echo) and Unchanged state.
// Produces { kind: "Resume", value: <payload>, state_update: { kind: "Unchanged" } }
fn echo_resume_handler() -> Action {
    // pipe(pick("payload"), augment with state_update, tag("Resume"))
    // In practice, a chain of builtins that extracts payload, wraps it with
    // the Unchanged state_update, and tags as Resume.
    chain(
        invoke_builtin(BuiltinKind::ExtractField { value: json!("payload") }),
        // ... wrap with state_update: Unchanged ...
        invoke_builtin(BuiltinKind::Tag { value: json!("Resume") }),
    )
}

// Helper: handler that always Discards with a fixed value.
fn always_discard_handler(value: Value) -> Action {
    Action::Invoke(InvokeAction {
        handler: HandlerKind::Builtin(BuiltinHandler {
            builtin: BuiltinKind::Constant {
                value: json!({ "kind": "Discard", "value": value }),
            },
        }),
    })
}

// Helper: handler that always RestartBody with the payload and Unchanged state.
fn always_restart_body_handler() -> Action {
    chain(
        invoke_builtin(BuiltinKind::ExtractField { value: json!("payload") }),
        // ... wrap with state_update: Unchanged ...
        invoke_builtin(BuiltinKind::Tag { value: json!("RestartBody") }),
    )
}

// Helper: handler that reads state and Resumes with it, Unchanged state.
// pipe(pick("state"), wrap with Unchanged, tag("Resume"))
fn resume_with_state_handler() -> Action {
    chain(
        invoke_builtin(BuiltinKind::ExtractField { value: json!("state") }),
        // ... wrap with state_update: Unchanged ...
        invoke_builtin(BuiltinKind::Tag { value: json!("Resume") }),
    )
}
```

All handlers are built from Constant, ExtractField, and Tag builtins — no TypeScript handlers needed. Tests run entirely in Rust.

### Unit tests (Rust)

1. **Perform without Handle**: Advance a bare `Perform(effectId)` with no enclosing Handle. Verify `AdvanceError::UnhandledEffect`.

2. **Resume: body completes after Resume**:
   ```
   Handle(effect, always_resume_handler(42),
     Chain(Perform(effect), Invoke(echo))
   )
   ```
   Advance → Perform fires → handler dispatched → handler completes with `{ kind: "Resume", value: 42 }` → Chain trampolines to Invoke(echo) with value 42 → echo dispatched → complete → workflow done.

3. **Discard: Handle exits with value**:
   ```
   Handle(effect, always_discard_handler("error"),
     Chain(Perform(effect), Invoke(should_not_run))
   )
   ```
   Advance → Perform fires → handler returns Discard → continuation torn down → Handle delivers "error" to parent → workflow done. Invoke(should_not_run) is never dispatched.

4. **RestartBody: re-enters body with value**:
   ```
   Handle(effect_done, always_discard_handler(???),
     Handle(effect_recur, always_restart_body_handler(),
       Chain(Invoke(counter), branch_on_count_then_perform)
     )
   )
   ```
   This tests the loop pattern: inner Handle catches recur (RestartBody), outer Handle catches done (Discard). The TypeScript counter handler increments and decides whether to recur or break. Verify multiple re-entries and final exit.

5. **Nested Handle: correct routing**:
   ```
   Handle(effect_outer, handler_outer,
     Handle(effect_inner, handler_inner,
       Perform(effect_outer)    // skips inner Handle, caught by outer
     )
   )
   ```
   Verify that Perform(effect_outer) bubbles past the inner Handle (wrong effect) and is caught by the outer Handle.

6. **Handle + Perform in Chain**: Perform is the first half of a Chain. After Resume with value V, the Chain's rest receives V. Verify correct trampolining.

7. **Handle + Perform in Parallel**: One parallel branch Performs, the other completes normally. After Resume, the Parallel joins both results.

8. **Discard cleans up slab**: After Discard, verify that all body frames are removed from the slab and `task_to_parent` entries for body tasks are removed.

9. **Multiple Performs (sequential)**: Body Performs twice in a Chain. First Perform is Resumed, then second Perform fires and is Resumed. Verify both resume correctly.

10. **RestartBody loop pattern**: Handler produces RestartBody 3 times, then Discard. Verify 3 re-entries and final exit. Verify slab doesn't grow (old body frames cleaned up each iteration).

11. **Perform across ForEach**: An action inside ForEach Performs. Verify the iteration suspends and resumes correctly. Other iterations are unaffected.

12. **Resume with state**: Handler DAG reads state and Resumes with it (the `resume_with_state_handler`). Verify the body receives the state value at the Perform point.

13. **StateUpdate::Updated updates state**: Handler produces `{ kind: "Resume", value: V, state_update: { kind: "Updated", value: S } }`. Body Performs again. Second handler invocation receives state = S (not the original state). Verify state was updated.

14. **Handle body completes without Performing**: Body runs to completion without any Perform. Handle exits normally, delivers body result to parent. No continuation to clean up.

15. **Async completion during suspension (buffered, then resumed)**:
    ```
    Handle(effect, echo_resume_handler(),
      Parallel(
        Chain(Invoke(taskA), Perform(effect)),
        Invoke(taskB),
      )
    )
    ```
    Advance → both tasks dispatched. Complete taskA → Chain trampolines → Perform fires → handler starts. Complete taskB → `find_suspended_ancestor` finds the Handle → buffered on continuation. Handler completes with Resume → deliver to Parallel slot 0 → replay buffered delivery → Parallel slot 1 filled → Parallel joins → Handle exits. Verify final output contains both results.

16. **Async completion during suspension (buffered, then discarded)**:
    Same tree as test 15, but handler produces Discard. Complete taskA → Perform → handler starts. Complete taskB → buffered. Handler completes with Discard → `discard_continuation` drops pending_deliveries → `teardown_body` removes frames and task entries → Handle exits with discard value. Verify Parallel frame and Chain B are removed from slab. Verify buffered delivery is dropped.

17. **Cancelled task completion (arrives after teardown)**: Same tree, but taskB completes AFTER discard has torn down the body. `complete_task` finds no `task_to_parent` entry → silently ignored. Verify no panic.

18. **Re-suspension during replay**: Two parallel branches, both with `Chain(Invoke, Perform(effect))`. TaskA completes → Perform A fires → handler starts. TaskB completes → buffered. Handler Resumes → deliver to branch A's Perform parent → replay buffered delivery → Chain B trampolines → Perform B fires → **new continuation stored**. Verify the second handler receives a fresh dispatch. Second handler Resumes → body completes normally.

19. **Multiple buffered deliveries**: Three parallel branches with Invokes. Branch A Performs. Branches B and C complete while handler runs. Both buffered. On Resume, both replayed in arrival order. Verify FIFO ordering.

20. **Delivery migration across nested Handles**:
    ```
    Handle(middle, handler_middle,
      Handle(inner, handler_inner,
        Parallel(
          Chain(Invoke(P1), Perform(middle)),
          Chain(Invoke(P2), Perform(inner)),
          Chain(Invoke(P3), Invoke(echo)),
        )
      )
    )
    ```
    P1 completes → Perform(middle) → middle suspends. P2 and P3 complete → both buffered on middle's continuation. Middle resumes → replay P2 → Chain trampolines → Perform(inner) → inner suspends. Replay P3 → `deliver_or_buffer` finds inner is suspended → **P3 migrates to inner's buffer**. Inner resumes → replays P3 → deliver normally → echo dispatched → Parallel joins → workflow completes. Verify P3 was delivered exactly once and the final result includes all three branches.

### Integration tests

1. **End-to-end with TypeScript handler**: The handler DAG includes an Invoke that calls a TypeScript function. The function's result feeds into a tagged output. The Handle frame interprets it and resumes the continuation.
2. **Nested handlers with TypeScript**: Multiple Handle blocks, TypeScript handlers at different levels.

## Deliverables

1. `EffectId(u16)` type (in `barnum_ast` or `barnum_engine`)
2. `HandleAction` and `PerformAction` in the tree AST (`barnum_ast/src/lib.rs`)
3. `FlatAction::Handle` and `FlatAction::Perform` in the flat table (`barnum_ast/src/flat.rs`)
4. Flattener support: `flatten_action_at` match arms, `fill_child_slot` update, `try_map_target` passthrough
5. `FrameKind::Handle` in the scheduler (`barnum_engine/src/frame.rs`)
6. `ContinuationRoot` struct
7. `bubble_effect` traversal
8. `dispatch_to_handler` (sever, construct `{ payload, state }`, advance handler)
9. `handle_handler_completion` (parse `{ kind, value, state_update }`, dispatch on ContinuationOp)
10. `resume_continuation`, `discard_continuation`, `restart_body`
11. `teardown_body` + `is_descendant_of`
12. `AdvanceError::UnhandledEffect`, `CompleteError::InvalidHandlerOutput`
13. Updated `deliver` for Handle (body-completion and handler-completion paths)
14. `FlatConfig::handle_body` accessor (resolves child slot at `action_id + 1`)
15. Tests per above

## What this phase does NOT include

- No semantic effects (ReadVar, Throw, LoopControl). Those come in Phases 2-4.
- No TypeScript surface API changes. Declare, tryCatch, loop rewriting come later.
- No RAII / Bracket. That's Phase 5.
- No durable suspension. That's Phase 6.

This phase builds infrastructure only. The test suite validates the mechanism using synthetic effects (handler DAGs built from existing builtins that produce each of the three tagged outputs).

## Note on cycle hazards

The frame tree is always acyclic. StepAction (and RestartBody) create new frames per invocation — they never create pointers back to existing frames. `teardown_body` is a scan of the slab filtered by ancestry, not a graph traversal. No cycle detection is needed.
