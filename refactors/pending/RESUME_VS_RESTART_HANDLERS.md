# Resume and Restart Handlers

## Motivation

Every Handle/Perform usage falls into one of two categories:

| Kind | What happens | Handler input | Handler output | Examples |
|------|-------------|--------------|---------------|----------|
| **Resume** | Value delivered to Perform site. Body continues. | `[payload, state]` tuple | `[value, state]` tuple — value for perform site, state written back to ResumeHandle | `bind`, `counter` |
| **Restart** | Body torn down, re-entered with new input. | `[payload, state]` tuple | Raw value (new body input) | `loop`, `scope`/`jump`, `tryCatch`, `race` |

Each kind is unconditional. The engine knows what to do based on the Handle kind. There is no `Resume`/`RestartBody` tag dispatch. Both handler kinds receive `[payload, state]` as input. RestartHandle handlers produce a raw value. ResumeHandle handlers produce a `[value, state]` tuple (via `All`) — the engine destructures it, delivers `value` to the perform site, and writes `state` back to the ResumeHandle.

For RestartHandle, the "exit the Handle" path is a body behavior, not a handler behavior. The restart handler always restarts. The body has a Branch at the top that routes the restarted value: one arm runs the body; the other completes normally, exiting the Handle. This is how `loop`, `earlyReturn`, `tryCatch`, and `race` work.

The engine currently treats all handlers identically: suspend body, run handler DAG, deserialize a two-variant `HandlerOutput` (Resume/RestartBody), dispatch. Separating Resume and Restart into distinct Handle/Perform types lets the engine handle each directly — no deserialization, no tag dispatch.

## What changes

### 1. Replace EffectId with two separate ID types

**Before** (`barnum_ast/src/lib.rs:45`):

```rust
pub struct EffectId(pub u16);
```

**After:**

```rust
pub struct ResumeHandlerId(pub u16);
pub struct RestartHandlerId(pub u16);
```

Separate types, separate namespaces. A `ResumePerform` can only target a `ResumeHandlerId`. A `RestartPerform` can only target a `RestartHandlerId`. Cross-matching is a compile error.

### 2. Split HandleFrame into two frame kinds

**Before** (`frame.rs:110`):

```rust
pub struct HandleFrame {
    pub effect_id: EffectId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Value,
    pub status: HandleStatus,  // Free | Suspended(ParentRef)
}
```

**After:**

```rust
/// Function-call semantics. Handler value delivered to Perform site.
/// Never suspends.
pub struct ResumeHandleFrame {
    pub resume_handler_id: ResumeHandlerId,
    pub body: ActionId,
    pub handler: ActionId,
    /// Mutable state available to the handler. Set from the input tuple
    /// when the Handle advances; updated by each handler invocation's
    /// returned state value.
    pub state: Value,
}

/// Restart semantics. Body torn down, re-entered with handler value.
pub struct RestartHandleFrame {
    pub restart_handler_id: RestartHandlerId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Value,
}
```

Neither frame kind has `status` — neither suspends. `HandleStatus` is deleted.

### 3. Split FrameKind, ParentRef, and HandleSide

The frame tree infrastructure mirrors the split.

**FrameKind — before** (`frame.rs:69`):

```rust
pub enum FrameKind {
    Chain { rest: ActionId },
    All { results: Vec<Option<Value>> },
    ForEach { results: Vec<Option<Value>> },
    Handle(HandleFrame),
    Invoke { handler: HandlerId },
}
```

**FrameKind — after:**

```rust
pub enum FrameKind {
    Chain { rest: ActionId },
    All { results: Vec<Option<Value>> },
    ForEach { results: Vec<Option<Value>> },
    ResumeHandle(ResumeHandleFrame),
    RestartHandle(RestartHandleFrame),
    ResumePerform(ResumePerformFrame),
    Invoke { handler: HandlerId },
}
```

**ResumePerformFrame:**

```rust
/// Frame at the Perform site for a ResumeHandle. Runs the handler DAG
/// as a child. When the handler completes, intercepts the result to
/// apply state updates to the ResumeHandle's state, then delivers the
/// value to its parent.
pub struct ResumePerformFrame {
    /// The ResumeHandle frame this Perform targets.
    /// Used to apply state updates back to the ResumeHandle's state.
    pub handle_frame_id: FrameId,
}
```

**ParentRef — before** (`frame.rs:16`):

```rust
pub enum ParentRef {
    Chain { frame_id: FrameId },
    All { frame_id: FrameId, child_index: usize },
    ForEach { frame_id: FrameId, child_index: usize },
    Handle { frame_id: FrameId, side: HandleSide },
}
```

**ParentRef — after:**

```rust
pub enum ParentRef {
    Chain { frame_id: FrameId },
    All { frame_id: FrameId, child_index: usize },
    ForEach { frame_id: FrameId, child_index: usize },
    ResumeHandle { frame_id: FrameId },
    RestartHandle { frame_id: FrameId, side: RestartHandleSide },
    ResumePerform { frame_id: FrameId },
}
```

Key differences:
- `ResumeHandle` has no `side` field — it only ever has body children. The handler runs at the Perform site, not at the Handle (see section 6).
- `ResumePerform` intercepts handler results to apply state updates to the ResumeHandle's `state`, then delivers the value to its parent.

**HandleSide — before** (`frame.rs:46`):

```rust
pub enum HandleSide {
    Body,
    Handler,
}
```

**HandleSide — after:** Replaced by `RestartHandleSide`. ResumeHandle doesn't need a side enum — it has no handler-side children.

```rust
pub enum RestartHandleSide {
    Body,
    Handler,
}
```

**deliver — before** (`lib.rs:759`):

```rust
ParentRef::Handle { frame_id, side } => match side {
    HandleSide::Body => { /* body completed, deliver to Handle's parent */ }
    HandleSide::Handler => { /* handler completed, deserialize HandlerOutput, dispatch */ }
}
```

**deliver — after:**

```rust
ParentRef::ResumeHandle { frame_id } => {
    // Body delivered. Remove the ResumeHandle frame, deliver to parent.
    let frame = self.frames.remove(frame_id).expect("frame exists");
    self.deliver(frame.parent, value)
}
ParentRef::RestartHandle { frame_id, side } => match side {
    RestartHandleSide::Body => {
        // Body delivered. Remove RestartHandle frame, deliver to parent.
        let frame = self.frames.remove(frame_id).expect("frame exists");
        self.deliver(frame.parent, value)
    }
    RestartHandleSide::Handler => {
        // Handler completed. Tear down body, re-enter with raw value.
        self.restart_body(frame_id, value)
    }
}
```

### 4. Split the AST nodes (Handle and Perform)

**Before** (`ast.ts:46`):

```ts
export interface HandleAction {
  kind: "Handle";
  effect_id: number;
  body: Action;
  handler: Action;
}

export interface PerformAction {
  kind: "Perform";
  effect_id: number;
}
```

**After:**

```ts
export interface ResumeHandleAction {
  kind: "ResumeHandle";
  resume_handler_id: number;
  body: Action;
  handler: Action;
}

export interface RestartHandleAction {
  kind: "RestartHandle";
  restart_handler_id: number;
  body: Action;
  handler: Action;
}

export interface ResumePerformAction {
  kind: "ResumePerform";
  resume_handler_id: number;
}

export interface RestartPerformAction {
  kind: "RestartPerform";
  restart_handler_id: number;
}
```

Same split in the Rust AST (`barnum_ast`).

### 5. Delete HandlerOutput

**Before** (`lib.rs:108`):

```rust
enum HandlerOutput {
    Resume { value, state_update },
    RestartBody { value, state_update },
}
```

**After:** The tagged enum is deleted. No `Resume`/`RestartBody` tag dispatch.

- **RestartHandle** handlers produce a raw value. No deserialization. The engine uses it directly as the new body input.
- **ResumeHandle** handlers produce a 2-tuple `[value, state]` via `All`. The engine destructures it: index 0 is delivered to the perform site, index 1 overwrites the ResumeHandle's `state`. This is simpler than the old `HandlerOutput` (positional tuple vs tagged enum with optional `StateUpdate` sub-enum) but is still deserialized.

```rust
// Handler result is a 2-element array [value, state].
let (value, state): (Value, Value) = serde_json::from_value(value)?;
```

### 6. ResumeHandle and ResumePerform: inline handler execution at the Perform site

**Before** (`lib.rs:440`): `dispatch_to_handler` suspends the Handle and runs the handler DAG as a child of the Handle frame with `ParentRef::Handle { side: Handler }`.

**After for ResumeHandle/ResumePerform:** The ResumeHandle splits its input tuple on advance. The handler DAG runs at the Perform site, not at the Handle. The ResumeHandle frame is never suspended.

#### 6a. `advance` match arm for `FlatAction::ResumeHandle`

When the engine encounters a `ResumeHandle` action during `advance`:

```rust
FlatAction::ResumeHandle { resume_handler_id } => {
    let body = self.flat_config.resume_handle_body(action_id);
    let handler = self.flat_config.resume_handle_handler(action_id);

    // Input is a 2-tuple [state, body_input].
    // Combinators construct this via All(state_expr, body_input_expr).
    let (state, body_input): (Value, Value) =
        serde_json::from_value(value)?;

    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::ResumeHandle(ResumeHandleFrame {
            resume_handler_id,
            body,
            handler,
            state,
        }),
    });

    // Body receives only body_input, not the full tuple.
    self.advance(body, body_input, Some(ParentRef::ResumeHandle { frame_id }))?;
}
```

#### 6b. `advance` match arm for `FlatAction::ResumePerform`

When the body hits a `ResumePerform`, the engine creates a `ResumePerformFrame` and advances the handler as its child.

```rust
FlatAction::ResumePerform { resume_handler_id } => {
    let Some(perform_parent) = parent else {
        return Err(AdvanceError::UnhandledEffect { ... });
    };

    // Walk up the frame tree to find the matching ResumeHandle.
    let (handle_frame_id, resume_handle) = self
        .ancestors(perform_parent)
        .find_map(|(edge, frame)| match &frame.kind {
            FrameKind::ResumeHandle(handle)
                if handle.resume_handler_id == resume_handler_id =>
            {
                Some((edge.frame_id(), handle))
            }
            _ => None,
        })
        .ok_or(AdvanceError::UnhandledEffect { ... })?;

    let handler_action_id = resume_handle.handler;
    let state = resume_handle.state.clone();
    let handler_input = json!([value, state]);

    // ResumePerformFrame intercepts the handler's result to apply
    // state updates, then delivers the value to its parent.
    let perform_frame_id = self.frames.insert(Frame {
        parent: Some(perform_parent),
        kind: FrameKind::ResumePerform(ResumePerformFrame {
            handle_frame_id,
        }),
    });

    // Handler receives [payload, state] — same shape as restart handlers.
    self.advance(handler_action_id, handler_input, Some(ParentRef::ResumePerform {
        frame_id: perform_frame_id,
    }))?;
}
```

Multiple concurrent ResumePerforms can be in flight for the same ResumeHandle — each creates its own frame with its own `perform_parent`. The ResumeHandle frame is not suspended.

#### 6c. `deliver` match arm for `ParentRef::ResumePerform`

When the handler completes, its result is a 2-tuple `[value, state]`:

```rust
ParentRef::ResumePerform { frame_id } => {
    let frame = self.frames.remove(frame_id).expect("frame exists");
    let FrameKind::ResumePerform(perform) = frame.kind else { unreachable!() };
    let parent = frame.parent.expect("ResumePerform always has a parent");

    // Deserialize handler result as [value, state] tuple.
    let (value, state): (Value, Value) =
        serde_json::from_value(value)?;

    // Always write state back. Handlers that don't mutate state pass
    // the current value through unchanged (idempotent write).
    let handle_frame = self.frames.get_mut(perform.handle_frame_id)
        .expect("ResumeHandle still alive");
    let FrameKind::ResumeHandle(ref mut resume_handle) = handle_frame.kind else {
        unreachable!()
    };
    resume_handle.state = state;

    // Deliver the value to parent.
    self.deliver(parent, value)
}
```

### 7. RestartPerform: tear down body immediately, run handler, re-enter

**Before** (`lib.rs:440`): Suspends the Handle (marking body as blocked), runs handler as child of Handle frame. Body frames stay alive during handler execution. Other completions for body-side frames are stashed. When handler completes, body is torn down and re-entered.

**After for RestartPerform:** The body is torn down immediately when the RestartPerform fires. No suspension. The handler becomes the sole child of the RestartHandle frame.

```rust
// Tear down the body immediately. All body frames and in-flight tasks
// are removed from the arena. No suspension, no stash needed.
self.teardown_children(handle_frame_id);

// Look up handler ActionId and state.
let handler_action_id = restart_handle.handler;
let state = restart_handle.state.clone();
let handler_input = json!([value, state]);

// Advance handler DAG as child of the RestartHandle frame (Handler side).
self.advance(handler_action_id, handler_input, Some(ParentRef::RestartHandle {
    frame_id: handle_frame_id,
    side: RestartHandleSide::Handler,
}))?;
```

When the handler completes, `deliver` hits `ParentRef::RestartHandle { side: Handler }`, which calls `restart_body(frame_id, value)` — re-enter the body with the raw value. No teardown at this point (body was already torn down when the RestartPerform fired).

### Stash elimination

Neither handler kind suspends the Handle frame:

- **ResumePerform**: handler runs inline at the Perform site. Handle frame is uninvolved.
- **RestartPerform**: body is torn down immediately. Handle frame transitions directly from "running body" to "running handler."

`HandleStatus::Suspended` is deleted. `is_blocked_by_handle` is deleted. The stash (`stashed_items`, `sweep_stash`, `sweep_stash_once`, `StashedItem`, `SweepResult`, `StashOutcome`, `TryDeliverResult::Blocked`, `find_blocking_ancestor`, `AncestorCheck::Blocked`) is deleted entirely.

Completions for tasks that belonged to the torn-down body arrive at the engine with a stale `FrameId` (the generational index rejects them). The existing `FrameGone` handling covers this — no new code needed.

This is a significant simplification. The stash was the most complex part of the engine (the sweep loop, blocked ancestor detection, re-entrant stash processing). Removing it cuts a substantial amount of code and eliminates an entire class of ordering bugs.

### 8. Delete handle_handler_completion

**Before** (`lib.rs:495`): Deserializes `HandlerOutput`, matches on Resume/RestartBody.

**After:** Deleted. The two handler kinds don't share a completion path:

- **ResumeHandle**: no handler completion at the Handle frame. The handler DAG delivers to the ResumePerformFrame, which applies state updates and delivers the value to its parent. The ResumeHandle frame is uninvolved.
- **RestartHandle**: handler completion is a single call to `restart_body` in the `deliver` match arm for `ParentRef::RestartHandle { side: Handler }`. No function needed.

### 9. Update handler DAGs

All handler DAGs drop their `Tag("Resume")`/`Tag("RestartBody")` wrapping.

**RestartHandle handlers** receive `[payload, state]` and produce a raw value (the new body input). No wrapping.

| Combinator | Before | After |
|-----------|--------|-------|
| `tryCatch` | `Chain(ExtractField("payload"), Tag("RestartBody"))` | `ExtractIndex(0)` |
| `race` | `Chain(ExtractField("payload"), Tag("RestartBody"))` | `ExtractIndex(0)` |
| `loop` | `Tag("RestartBody")` wrapper | `ExtractIndex(0)` |
| `scope`/`jump` | `Tag("RestartBody")` wrapper | `ExtractIndex(0)` |

**ResumeHandle handlers** receive `[payload, state]` and produce `[value, new_state]`. Handlers that don't mutate state pass state through unchanged.

| Combinator | Before | After |
|-----------|--------|-------|
| `bind` (readVar) | `ExtractField("state") → ExtractIndex(n) → Tag("Resume")` | `All(ExtractIndex(1), ExtractIndex(1))` — payload (index 0) unused; value = state, new_state = state (pass-through) |
| `counter` | N/A | TypeScript Invoke: `([payload, state]) => [state, state + 1]` — value = current count, new_state = incremented. Concurrent calls may race on state (lost update). |

The `All` node constructs the `[value, state]` tuple that the engine destructures (by convention, index 0 = value, index 1 = new state).

### 10. Rename Handle/Perform to RestartHandle/RestartPerform in tryCatch and race

`tryCatch` and `race` already use the restart+Branch pattern (Discard was eliminated as a pre-refactor). The remaining change is replacing `Handle`/`Perform` with `RestartHandle`/`RestartPerform`, and dropping `Tag("RestartBody")` from the handler DAG since the engine knows RestartHandle always restarts.

**tryCatch — before:**

```ts
Chain(Tag("Continue"),
  Handle(effectId,
    Branch({
      Continue: Chain(ExtractField("value"), body),
      Break: Chain(ExtractField("value"), recovery),
    }),
    Chain(ExtractField("payload"), Tag("RestartBody"))
  )
)
// throwError = Chain(Tag("Break"), Perform(effectId))
```

**tryCatch — after:**

```ts
Chain(Tag("Continue"),
  RestartHandle(restartHandlerId,
    Branch({
      Continue: Chain(ExtractField("value"), body),
      Break: Chain(ExtractField("value"), recovery),
    }),
    ExtractPayloadHandler
  )
)
// throwError = Chain(Tag("Break"), RestartPerform(restartHandlerId))
```

**race — before:**

```ts
Chain(Tag("Continue"),
  Handle(effectId,
    Branch({
      Continue: Chain(ExtractField("value"),
        All(
          Chain(a, Chain(Tag("Break"), Perform(effectId))),
          Chain(b, Chain(Tag("Break"), Perform(effectId))),
        )
      ),
      Break: Chain(ExtractField("value"), identity()),
    }),
    Chain(ExtractField("payload"), Tag("RestartBody"))
  )
)
```

**race — after:**

```ts
Chain(Tag("Continue"),
  RestartHandle(restartHandlerId,
    Branch({
      Continue: Chain(ExtractField("value"),
        All(
          Chain(a, Chain(Tag("Break"), RestartPerform(restartHandlerId))),
          Chain(b, Chain(Tag("Break"), RestartPerform(restartHandlerId))),
        )
      ),
      Break: Chain(ExtractField("value"), identity()),
    }),
    ExtractPayloadHandler
  )
)
```

Structurally identical. The only differences: `Handle` → `RestartHandle`, `Perform` → `RestartPerform`, `Chain(ExtractField("payload"), Tag("RestartBody"))` → `ExtractPayloadHandler`.

### 11. All restart handlers share the same handler DAG

Every restart handler's DAG is now: extract the payload (index 0) from `[payload, state]`. That's it. The handler doesn't decide what to do — the engine always restarts, and the body's Branch routes the value.

```ts
const EXTRACT_PAYLOAD_HANDLER: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: 0 } },
};
```

One handler DAG for all restart combinators.

## Combinator-to-handle-kind mapping

| Combinator | Handle kind | Perform kind |
|-----------|-------------|-------------|
| `bind` / `bindInput` | ResumeHandle | ResumePerform |
| `counter` | ResumeHandle | ResumePerform |
| `tryCatch` | RestartHandle | RestartPerform |
| `race` | RestartHandle | RestartPerform |
| `withTimeout` | RestartHandle (built on race) | RestartPerform |
| `loop` | RestartHandle | RestartPerform |
| `scope` / `jump` | RestartHandle | RestartPerform |

## Changes that can land independently on master

These don't require the full refactor. They simplify the current code and reduce the diff when the refactor lands.

1. **~~`HandleFrame::state: Option<Value>` → `Value`.~~** Done (already landed).

2. **~~Extract `restart_body` as a standalone function.~~** Done (landed in lib.rs split). `restart_body` is a free function in `effects.rs`.

3. **~~Extract `teardown_body` as a standalone function.~~** Done (landed in lib.rs split). `teardown_body` is a free function in `effects.rs`.

4. **~~Extract an ancestor frame iterator.~~** Done (already landed). `Ancestors` iterator yields `(ParentRef, &Frame)` pairs. `find_blocking_ancestor` and `find_and_dispatch_handler` both use it.

## Implementation order

1. **Remove `#[should_panic]` from target-behavior tests.** Three tests in `barnum_engine/src/lib.rs` already describe the target behavior:
   - `resume_handler_does_not_block_sibling_completion`
   - `concurrent_resume_performs_not_serialized`
   - `throw_proceeds_while_resume_handler_in_flight`

   Remove `#[should_panic]`. These become failing tests that define acceptance criteria.

2. Implement the ResumeHandle/ResumePerform changes (sections 1-6).
3. Implement the RestartHandle/RestartPerform changes (sections 7-10).
4. Delete stash infrastructure.
5. All three tests pass.
