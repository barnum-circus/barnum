# Resume and Restart Handlers

## Motivation

Every Handle/Perform usage falls into one of two categories:

| Kind | What happens | Handler output | Examples |
|------|-------------|---------------|----------|
| **Resume** | Value delivered to Perform site. Body continues. | `[value, state]` tuple — value for perform site, state overwrites `captured_value` | `bind`, future `allocateId` |
| **Restart** | Body torn down, re-entered with new input. | Raw value (new body input) | `loop`, `scope`/`jump`, `tryCatch`, `race` |

Each kind is unconditional. The engine knows what to do based on the Handle kind. There is no `Resume`/`RestartBody` tag dispatch. RestartHandle handlers produce a raw value. ResumeHandle handlers produce a `[value, state]` tuple (via `All`) — the engine destructures it, delivers `value` to the perform site, and writes `state` to `captured_value`.

The "exit the Handle" path is a body behavior, not a handler behavior. The handler always restarts. The body has a Branch at the top that routes the restarted value: one arm runs the body; the other completes normally, exiting the Handle. This is how `loop`, `earlyReturn`, `tryCatch`, and `race` work.

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
    /// returned `state` field. Cannot be baked into the handler DAG
    /// because it's a runtime value.
    pub captured_value: Value,
}

/// Restart semantics. Body torn down, re-entered with handler value.
pub struct RestartHandleFrame {
    pub restart_handler_id: RestartHandlerId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Value,
    pub status: HandleStatus,
}
```

`ResumeHandleFrame` has no `status` — it never suspends.

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
/// apply state updates to the ResumeHandle's captured_value, then
/// forwards the value to perform_parent.
pub struct ResumePerformFrame {
    /// The ResumeHandle frame this Perform targets.
    /// Used to apply state updates back to the ResumeHandle's captured_value.
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
- `ResumePerform` intercepts handler results to apply state updates to the ResumeHandle's `captured_value`, then forwards the value to the body.

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
    // Body completed normally. Remove the ResumeHandle frame,
    // deliver to its parent.
    let frame = self.frames.remove(frame_id).expect("frame exists");
    self.deliver(frame.parent, value)
}
ParentRef::RestartHandle { frame_id, side } => match side {
    RestartHandleSide::Body => {
        // Body completed normally. Remove RestartHandle frame,
        // deliver to its parent.
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
- **ResumeHandle** handlers produce a 2-tuple `[value, state]` via `All`. The engine destructures it: index 0 is delivered to the perform site, index 1 overwrites `captured_value`. This is simpler than the old `HandlerOutput` (positional tuple vs tagged enum with optional `StateUpdate` sub-enum) but is still deserialized.

```rust
// Handler result is a 2-element array [value, state].
let (value, state): (Value, Value) = serde_json::from_value(handler_result)?;
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

    // Input is a 2-tuple [captured_value, body_input].
    // Combinators construct this via All(state_expr, body_input_expr).
    let (captured_value, body_input): (Value, Value) =
        serde_json::from_value(value)?;

    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::ResumeHandle(ResumeHandleFrame {
            resume_handler_id,
            body,
            handler,
            captured_value,
        }),
    });

    // Body receives only body_input, not the full tuple.
    self.advance(body, body_input, Some(ParentRef::ResumeHandle { frame_id }))?;
}
```

#### 6b. `advance` match arm for `FlatAction::ResumePerform`

When the engine encounters a `ResumePerform` action during `advance`:

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

    // Look up handler and captured value from the ResumeHandle.
    let handler_action_id = resume_handle.handler;
    let captured_value = resume_handle.captured_value.clone();
    let handler_input = json!({ "payload": value, "state": captured_value });

    // Create ResumePerform frame. Intercepts handler result to write
    // state back to the ResumeHandle's captured_value.
    let perform_frame_id = self.frames.insert(Frame {
        parent: Some(perform_parent),
        kind: FrameKind::ResumePerform(ResumePerformFrame {
            handle_frame_id,
        }),
    });

    // Handler DAG runs as child of the ResumePerform frame.
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
        serde_json::from_value(handler_result)?;

    // Always write state to captured_value. Handlers that don't mutate
    // state pass the current state through unchanged (idempotent write).
    let handle_frame = self.frames.get_mut(perform.handle_frame_id)
        .expect("ResumeHandle still alive");
    let FrameKind::ResumeHandle(ref mut resume_handle) = handle_frame.kind else {
        unreachable!()
    };
    resume_handle.captured_value = state;

    // Forward the value to the body at the original Perform site.
    self.deliver(parent, value)
}
```

#### 6d. Concurrent state updates

Multiple ResumePerforms can be in flight for the same ResumeHandle. Each reads `captured_value` at advance time (when constructing `handler_input`) and writes it back at deliver time (when the handler returns `{ value, state }`). The engine is single-threaded and synchronous — `deliver` runs to completion before the next `complete` call. So there are no data races. But the ordering of state updates depends on the order handler completions arrive, which is determined by the external caller's `complete` call order. This is inherent to the model — the same ordering concern exists today with `state_update` on the current Handle.

For read-only handlers (like bind), the handler returns the current state unchanged, so concurrent invocations are trivially safe.

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
let handler_input = json!({ "payload": payload, "state": state });

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

- **ResumeHandle**: no handler completion — the handler DAG delivers directly to the ResumePerform frame's parent via normal `deliver`. The Handle frame is uninvolved.
- **RestartHandle**: handler completion is a single call to `restart_body` in the `deliver` match arm for `ParentRef::RestartHandle { side: Handler }`. No function needed.

### 9. Update handler DAGs

All handler DAGs drop their `Tag("Resume")`/`Tag("RestartBody")` wrapping.

**RestartHandle handlers** produce a raw value (the new body input). No wrapping.

| Combinator | Before | After |
|-----------|--------|-------|
| `tryCatch` | `Chain(ExtractField("payload"), Tag("RestartBody"))` | `ExtractField("payload")` |
| `race` | `Chain(ExtractField("payload"), Tag("RestartBody"))` | `ExtractField("payload")` |
| `loop` | `Tag("RestartBody")` wrapper | `ExtractField("payload")` |
| `scope`/`jump` | `Tag("RestartBody")` wrapper | `ExtractField("payload")` |

**ResumeHandle handlers** produce `{ value, state }`. The handler receives `{ payload, state }` and must return both the value for the perform site and the full next state. Handlers that don't mutate state pass it through unchanged.

| Combinator | Before | After |
|-----------|--------|-------|
| `bind` (readVar) | `ExtractField("state") → ExtractIndex(n) → Tag("Resume")` | `All(Chain(ExtractField("state"), ExtractIndex(n)), ExtractField("state"))` — value = `state[n]`, state = pass-through |
| `allocateId` | N/A | TypeScript Invoke: `({ state }) => [state, state + 1]` — value = current count, state = incremented |

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

Every restart handler's DAG is now: extract the payload from `{ payload, state }`. That's it. The handler doesn't decide what to do — the engine always restarts, and the body's Branch routes the value.

```ts
const EXTRACT_PAYLOAD_HANDLER: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "payload" } },
};
```

One handler DAG for all restart combinators.

## Combinator-to-handle-kind mapping

| Combinator | Handle kind | Perform kind |
|-----------|-------------|-------------|
| `bind` / `bindInput` | ResumeHandle | ResumePerform |
| `allocateId` (hypothetical) | ResumeHandle | ResumePerform |
| `tryCatch` | RestartHandle | RestartPerform |
| `race` | RestartHandle | RestartPerform |
| `withTimeout` | RestartHandle (built on race) | RestartPerform |
| `loop` | RestartHandle | RestartPerform |
| `scope` / `jump` | RestartHandle | RestartPerform |

## Concrete examples

### bind — read-only captured_value

User writes:
```ts
bind([fetchUser, fetchPosts], ([user, posts]) =>
  pipe(renderPage(user, posts), deployToS3)
)
```

Compiled form (single binding for clarity):
```
bind([fetchUser], ([user]) => body)

→ Chain(
    All(fetchUser, Identity),             ← [userVal, pipelineInput] = input tuple
    ResumeHandle(e0,
      body,                               ← body receives pipelineInput (index 1)
      readVarHandler                      ← returns { value: state, state: state }
    )
  )
```

Engine execution:
1. `All(fetchUser, Identity)` runs concurrently → `[userVal, pipelineInput]`
2. ResumeHandle(e0) receives `[userVal, pipelineInput]`. Engine splits: `captured_value = userVal`, body input = `pipelineInput`.
3. Body runs with `pipelineInput`. When it hits VarRef `user` (= `ResumePerform(e0)`):
   - Engine walks ancestors, finds ResumeHandle(e0)
   - Creates ResumePerformFrame
   - Runs handler with `{ payload: <body value>, state: userVal }`
   - Handler: `All(ExtractField("state"), ExtractField("state"))` → `[userVal, userVal]`
   - Engine destructures: value = `userVal`, writes state = `userVal` (unchanged)
   - Delivers `userVal` to perform site
4. Body continues with `userVal`

For N bindings, the outer `All(...bindings, Identity)` produces a flat N+1 tuple. bind restructures this into nested pairs so each ResumeHandle peels off one binding:

```
All(b0, b1, Identity)                      → [v0, v1, input]
All(ExtractIndex(0), All(ExtractIndex(1), ExtractIndex(2)))
                                           → [v0, [v1, input]]
ResumeHandle(e0,                           ← captured_value = v0, body gets [v1, input]
  ResumeHandle(e1,                         ← captured_value = v1, body gets input
    body,                                  ← receives pipelineInput directly
    handler1: All(ExtractField("state"), ExtractField("state"))
  ),
  handler0: All(ExtractField("state"), ExtractField("state"))
)
```

Each handler reads from its own `captured_value` (a single binding value, not the full tuple). No ExtractIndex in handlers. No ExtractIndex in body.

### allocateId — mutable captured_value (hypothetical)

A counter that returns an increasing number each time it's called. Each invocation is a single ResumePerform — the handler atomically reads the current count and increments the state.

User writes:
```ts
allocateId((nextId) =>
  pipe(
    fetchItems,
    forEach(
      pipe(
        nextId,             // → 0, 1, 2, ... (each call gets the next number)
        processWithId,
      ),
    ),
  ),
)
```

Compiled form:
```
Chain(
  All(Constant(0), Identity),             ← [0, pipelineInput] = input tuple
  ResumeHandle(counterEffectId,
    body,                                  ← body receives pipelineInput
    incrementHandler                       ← TypeScript Invoke: returns [state, state + 1]
  )
)
```

`nextId` compiles to `ResumePerform(counterEffectId)` — a leaf action, like a VarRef.

The handler is a TypeScript Invoke that receives `{ payload, state }` and returns `[state, state + 1]`. This is atomic — one Perform, one handler invocation, one state write. No window for interleaving.

Engine execution:
1. `All(Constant(0), Identity)` → `[0, pipelineInput]`
2. ResumeHandle splits: `captured_value = 0`, body gets `pipelineInput`
3. Body runs. First iteration hits `nextId` (= `ResumePerform(counterEffectId)`):
   - Engine finds ResumeHandle, reads `captured_value = 0`
   - Creates ResumePerformFrame, runs handler with `{ payload: null, state: 0 }`
   - Handler returns `[0, 1]` — value = 0 (delivered to body), state = 1 (written to captured_value)
4. Second iteration hits `nextId`:
   - `captured_value = 1` → handler returns `[1, 2]` → body gets 1, state becomes 2
5. And so on.

**Concurrency note:** If multiple iterations hit `nextId` concurrently (e.g., inside forEach which advances children in parallel), the handler invocations interleave through the dispatch → complete cycle. Two handlers could read the same `captured_value` before either writes back — a lost-update race. This is a known limitation. A future mutex combinator built from async TypeScript handlers (a lock-acquire Invoke that suspends until no other handler is in flight) would solve this in user land. The engine substrate supports it — the handler DAG can contain arbitrary async Invoke nodes that delay handler completion. We don't have the primitives yet, but the model doesn't preclude them.

## Changes that can land independently on master

These don't require the full refactor. They simplify the current code and reduce the diff when the refactor lands.

1. **~~`HandleFrame::state: Option<Value>` → `Value`.~~** Done (already landed).

2. **Extract `restart_body` as a standalone method.** Currently `handle_handler_completion` handles Resume and RestartBody. Extracting the RestartBody path into its own method prepares for the split where it becomes the sole `RestartHandle { side: Handler }` deliver path.

3. **Extract `teardown_children` as a standalone method.** Currently body teardown is interleaved in `handle_handler_completion`. Extracting it makes the "tear down immediately on RestartPerform" change trivial.

4. **~~Extract an ancestor frame iterator.~~** Done (already landed). `Ancestors` iterator yields `(ParentRef, &Frame)` pairs. `find_blocking_ancestor` and `find_and_dispatch_handler` both use it.

## Open questions

1. **State updates for RestartHandle.** Restart handlers (loop) need state updates across iterations. With raw handler output values, state updates need a separate mechanism — either the handler DAG produces `{ value, state }` (like ResumeHandle) and the engine destructures, or state is always overwritten with the handler's raw value. Currently restart handlers receive `{ payload, state }` and the `StateUpdate` enum handles this; we need to decide what replaces it.

2. **Resume handler error semantics.** If a resume handler's DAG fails, the error propagates through the ResumePerform frame to `perform_parent`. The error flows through the body's frame tree to the ResumeHandle frame's parent. Same behavior as if the body itself failed.

3. **Stash deletion sequencing.** The stash can be deleted only after both handler kinds stop suspending. If RestartPerform's immediate teardown lands first (before ResumePerform's inline execution), the stash is still needed for resume handlers. Both changes should land together or RestartPerform's teardown should land second.

4. **ResumeHandle handler output convention.** The handler returns a 2-tuple `[value, state]` constructed via `All` (for builtin handlers) or directly (for TypeScript handlers). The engine destructures by index (0 = value for perform site, 1 = new captured_value). This is a convention, not enforced by the type system at the AST level.

5. **Mutable-state concurrency control.** Concurrent ResumePerforms on the same ResumeHandle can race on `captured_value` — both read the same state before either writes back. This is solvable in user land: a future mutex combinator would include an async TypeScript Invoke in the handler DAG that suspends (returns a pending Promise) until no other handler is in flight for that ResumeHandle. The engine substrate supports this — handler DAGs can contain arbitrary async Invoke nodes. Not needed now; bind (the only current resume handler) is read-only.
