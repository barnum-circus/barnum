# Resume and Restart Handlers

## Motivation

Every Handle/Perform usage falls into one of two categories:

| Kind | What happens | Handler output | Examples |
|------|-------------|---------------|----------|
| **Resume** | Value delivered to Perform site. Body continues. | Raw value (for the body) | `bind`, future `provide`/`consume` |
| **Restart** | Body torn down, re-entered with new input. | Raw value (new body input) | `loop`, `scope`/`jump`, `tryCatch`, `race` |

Each kind is unconditional. The handler produces a raw value. The engine knows what to do based on the Handle kind. There is no enum, no tag dispatch, no envelope.

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
    /// Write-once value captured when the Handle advances. Read by every
    /// handler invocation, never updated. Cannot be baked into the handler
    /// DAG because it's a runtime value (the Handle's input).
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
    Invoke { handler: HandlerId },
}
```

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

**After:** Deleted entirely. Both handler kinds produce raw values.

- **ResumeHandleFrame**: deliver value to `perform_parent`.
- **RestartHandleFrame**: tear down body, re-enter with value.

No deserialization. No tag matching.

### 6. ResumePerform: inline handler execution at the Perform site

**Before** (`lib.rs:440`): `dispatch_to_handler` suspends the Handle and runs the handler DAG as a child of the Handle frame with `ParentRef::Handle { side: Handler }`.

**After for ResumePerform:** The handler DAG runs at the Perform site, not at the Handle. The ResumeHandle frame is never modified — no suspension, no side enum.

#### 6a. `advance` match arm for `FlatAction::ResumePerform`

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

    // Create ResumePerform frame. Intercepts handler result to apply
    // state updates back to the ResumeHandle's captured_value.
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

#### 6b. `deliver` match arm for `ParentRef::ResumePerform`

When the handler completes, its result is `{ value, state_update }`:

```rust
ParentRef::ResumePerform { frame_id } => {
    let frame = self.frames.remove(frame_id).expect("frame exists");
    let FrameKind::ResumePerform(perform) = frame.kind else { unreachable!() };
    let parent = frame.parent.expect("ResumePerform always has a parent");

    // Deserialize handler result.
    let ResumeHandlerOutput { value, state_update } =
        serde_json::from_value(handler_result)?;

    // Apply state update to the ResumeHandle's captured_value.
    if let Some(new_value) = state_update {
        let handle = self.frames.get_mut(perform.handle_frame_id)
            .expect("ResumeHandle still alive");
        let FrameKind::ResumeHandle(ref mut handle) = handle.kind else {
            unreachable!()
        };
        handle.captured_value = new_value;
    }

    // Forward the value to the body at the original Perform site.
    self.deliver(parent, value)
}
```

#### 6c. Concurrent state updates

Multiple ResumePerforms can be in flight for the same ResumeHandle. Each reads `captured_value` at advance time and writes it back at deliver time. The engine is single-threaded and synchronous — `deliver` runs to completion before the next `complete` call. So there are no data races. But the ordering of state updates depends on the order handler completions arrive, which is determined by the external caller's `complete` call order. This is inherent to the model — the same ordering concern exists today with `state_update` on the current Handle.

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

### 9. Update handler DAGs to produce raw values

All handler DAGs drop their `Tag(...)` wrapping.

| Combinator | Before | After |
|-----------|--------|-------|
| `bind` | `Tag("Resume")` wrapper | Raw value |
| `tryCatch` | `Chain(ExtractField("payload"), Tag("RestartBody"))` | Raw value (just `ExtractField("payload")`) |
| `race` | `Chain(ExtractField("payload"), Tag("RestartBody"))` | Raw value (just `ExtractField("payload")`) |
| `loop` | `Tag("RestartBody")` wrapper | Raw value (just `ExtractField("payload")`) |
| `scope`/`jump` | `Tag("RestartBody")` wrapper | Raw value (just `ExtractField("payload")`) |

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
| `tryCatch` | RestartHandle | RestartPerform |
| `race` | RestartHandle | RestartPerform |
| `withTimeout` | RestartHandle (built on race) | RestartPerform |
| `loop` | RestartHandle | RestartPerform |
| `scope` / `jump` | RestartHandle | RestartPerform |

## Changes that can land independently on master

These don't require the full refactor. They simplify the current code and reduce the diff when the refactor lands.

1. **~~`HandleFrame::state: Option<Value>` → `Value`.~~** Done (already landed).

2. **Extract `restart_body` as a standalone method.** Currently `handle_handler_completion` handles Resume and RestartBody. Extracting the RestartBody path into its own method prepares for the split where it becomes the sole `RestartHandle { side: Handler }` deliver path.

3. **Extract `teardown_children` as a standalone method.** Currently body teardown is interleaved in `handle_handler_completion`. Extracting it makes the "tear down immediately on RestartPerform" change trivial.

4. **~~Extract an ancestor frame iterator.~~** Done (already landed). `Ancestors` iterator yields `(ParentRef, &Frame)` pairs. `find_blocking_ancestor` and `find_and_dispatch_handler` both use it.

## Open questions

1. **State updates for RestartHandle.** Restart handlers (loop) need state updates across iterations. With raw handler output values, state updates need a separate mechanism — either the handler DAG produces `{ value, state_update }` and the engine destructures, or state is always overwritten with the handler's raw value. ResumeHandle doesn't have this problem — `captured_value` is write-once-read-many.

2. **Resume handler error semantics.** If a resume handler's DAG fails, the error propagates through the handler's frames directly to `perform_parent` (since ResumePerform is transparent — no intermediate frame). The error flows through the body's frame tree to the Handle frame's parent. Same behavior as if the body itself failed.

3. **Stash deletion sequencing.** The stash can be deleted only after both handler kinds stop suspending. If RestartPerform's immediate teardown lands first (before ResumePerform's inline execution), the stash is still needed for resume handlers. Both changes should land together or RestartPerform's teardown should land second.
