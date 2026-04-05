# Resume and Restart Handlers

## Motivation

Every Handle/Perform usage falls into one of two categories:

| Kind | What happens | Handler output | Examples |
|------|-------------|---------------|----------|
| **Resume** | Value delivered to Perform site. Body continues. | Raw value (for the body) | `bind`, future `provide`/`consume` |
| **Restart** | Body torn down, re-entered with new input. | Raw value (new body input) | `loop`, `scope`/`jump`, `tryCatch`, `race` |

Each kind is unconditional. The handler produces a raw value. The engine knows what to do based on the Handle kind. There is no enum, no tag dispatch, no envelope.

The "exit the Handle" path (currently `Discard`) is not a handler behavior — it's a body behavior. The handler always restarts. The body has a Branch at the top that routes the restarted value. One arm runs the body; the other arm completes normally, which exits the Handle. This is how `loop`, `earlyReturn`, `tryCatch`, and `race` already work (or can work with trivial recompilation for `tryCatch`/`race`).

The engine currently treats all handlers identically: suspend body, run handler DAG, deserialize a three-variant `HandlerOutput` (Resume/Discard/RestartBody), dispatch. The RestartBody and Discard paths do the same first step (tear down body) and differ only in what follows (re-enter vs exit). But the "exit" path is redundant — restarting into a Branch that takes the exit arm achieves the same result.

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
    pub state: Value,
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
- `ResumePerform` is a trampoline frame (like Chain). When its child completes, it removes itself and delivers to its parent.

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
    Discard { value },
    RestartBody { value, state_update },
}
```

**After:** Deleted entirely. Both handler kinds produce raw values.

- **ResumeHandleFrame**: deliver value to `perform_parent`.
- **RestartHandleFrame**: tear down body, re-enter with value.

No deserialization. No tag matching.

### 6. ResumePerform: inline handler execution at the Perform site

**Before** (`lib.rs:440`): `dispatch_to_handler` suspends the Handle and runs the handler DAG as a child of the Handle frame with `ParentRef::Handle { side: Handler }`.

**After for ResumePerform:** The handler DAG runs at the Perform site, not at the Handle. The ResumeHandle frame is never modified.

When the engine walks up and finds a matching `ResumeHandle`:

1. Create a `ResumePerform` frame at the Perform site (parent = `perform_parent`).
2. Look up handler ActionId and state from the ResumeHandle frame.
3. Advance the handler DAG as a child of the ResumePerform frame.

```rust
// Create ResumePerform frame for observability (like Invoke).
let perform_frame_id = self.frames.insert(Frame {
    parent: Some(perform_parent),
    kind: FrameKind::ResumePerform(ResumePerformFrame {
        resume_handler_id: resume_handle.resume_handler_id,
    }),
});

// Look up handler from the ResumeHandle frame.
let handler_action_id = resume_handle.handler;
let state = resume_handle.state.clone();
let handler_input = json!({ "payload": payload, "state": state });

// Handler DAG runs as child of the ResumePerform frame.
self.advance(handler_action_id, handler_input, Some(ParentRef::ResumePerform {
    frame_id: perform_frame_id,
}))?;
```

When the handler completes, it delivers to the ResumePerform frame. The frame removes itself and delivers the value to `perform_parent` (trampoline, same as Chain). The body continues. The ResumeHandle frame is uninvolved — no suspension, no Handler side, no `handle_handler_completion` path.

The ResumeHandle is a passive interceptor. The body runs through it, Performs look it up to find the handler and state, but execution stays in the body's frame subtree.

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

**Before** (`lib.rs:495`): Deserializes `HandlerOutput`, matches on Resume/Discard/RestartBody.

**After:** Deleted. The two handler kinds don't share a completion path:

- **ResumeHandle**: no handler completion — the handler DAG delivers directly to the ResumePerform frame's parent via normal `deliver`. The Handle frame is uninvolved.
- **RestartHandle**: handler completion is a single call to `restart_body` in the `deliver` match arm for `ParentRef::RestartHandle { side: Handler }`. No function needed.

### 9. Update handler DAGs to produce raw values

All handler DAGs drop their `Tag(...)` wrapping.

| Combinator | Before | After |
|-----------|--------|-------|
| `bind` | `Tag("Resume")` wrapper | Raw value |
| `tryCatch` | `Tag("Discard")` wrapper | Raw value (handler just extracts payload; recovery moves to body Branch) |
| `race` | `Tag("Discard")` wrapper | Raw value (handler just extracts payload) |
| `loop` | `Tag("RestartBody")` wrapper | Raw value (handler just extracts payload) |
| `scope`/`jump` | `Tag("RestartBody")` wrapper | Raw value (handler just extracts payload) |

### 10. Recompile tryCatch and race as restart+Branch

`tryCatch` and `race` currently use `Tag("Discard")` in the handler to exit the Handle directly. With only Resume and Restart, they use the same restart+Branch pattern as loop.

Note: Branch passes the full tagged value `{ kind, value }` to the matching arm. Arms that need the inner value must extract it with `ExtractField("value")`.

**tryCatch — before:**

```ts
Handle(effectId,
  body,
  Chain(ExtractField("payload"), Chain(recovery, Tag("Discard")))
)
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
```

`throwError` = `Chain(Tag("Break"), RestartPerform(restartHandlerId))`. Tags the error as Break, Performs. Handler extracts payload (`{ kind: "Break", value: error }`), engine restarts. Branch sees `kind: "Break"`, takes the Break arm. `ExtractField("value")` unwraps the error. Recovery runs. Body completes normally. Handle exits.

Initial entry: input is tagged Continue. Branch takes Continue arm. `ExtractField("value")` unwraps the input. Body runs.

**race — before:**

```ts
Handle(effectId,
  All(Chain(a, Perform(effectId)), Chain(b, Perform(effectId))),
  Chain(ExtractField("payload"), Tag("Discard"))
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

First branch to complete tags Break, Performs. Handler extracts payload, engine restarts (tearing down the All and its remaining branches). Branch takes Break arm, `ExtractField("value")` unwraps the winner's value, identity completes, Handle exits.

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

2. **Extract `restart_body` as a standalone method.** Currently `handle_handler_completion` handles Resume, Discard, and RestartBody. Extracting the RestartBody path into its own method prepares for the split where it becomes the sole `RestartHandle { side: Handler }` deliver path.

3. **Extract `teardown_children` as a standalone method.** Currently body teardown is interleaved in `handle_handler_completion`. Extracting it makes the "tear down immediately on RestartPerform" change trivial.

## Open questions

1. **State updates.** bind's state is set once and never updated (read-only). Restart handlers (loop) need state updates across iterations. With raw handler output values, state updates need a separate mechanism — either the handler DAG produces `{ value, state_update }` and the engine destructures, or state is always overwritten with the handler's raw value.

2. **Resume handler error semantics.** If a resume handler's DAG fails, the body is NOT suspended (the ResumePerform frame propagates the error to its parent in the body). The error flows through the body's frame tree to the Handle frame's parent. Same behavior as if the body itself failed.

3. **Stash deletion sequencing.** The stash can be deleted only after both handler kinds stop suspending. If RestartPerform's immediate teardown lands first (before ResumePerform's inline execution), the stash is still needed for resume handlers. Both changes should land together or RestartPerform's teardown should land second.
