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

The frame tree infrastructure mirrors the split. These are the enums that the engine dispatches on at every `deliver` and `advance`.

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
    ResumeHandle { frame_id: FrameId, side: ResumeHandleSide },
    RestartHandle { frame_id: FrameId, side: RestartHandleSide },
}
```

**HandleSide — before** (`frame.rs:46`):

```rust
pub enum HandleSide {
    Body,
    Handler,
}
```

**HandleSide — after:** Two separate enums. `ResumeHandleSide` may not need a Handler variant at all if the resume handler DAG is inlined into `dispatch_to_handler` rather than spawned as a child frame. But if the handler DAG runs as a child:

```rust
pub enum ResumeHandleSide {
    Body,
    Handler,
}

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
ParentRef::ResumeHandle { frame_id, side } => match side {
    ResumeHandleSide::Body => { /* body completed, deliver to Handle's parent */ }
    ResumeHandleSide::Handler => { /* handler completed, deliver raw value to perform_parent */ }
}
ParentRef::RestartHandle { frame_id, side } => match side {
    RestartHandleSide::Body => { /* body completed, deliver to Handle's parent */ }
    RestartHandleSide::Handler => { /* handler completed, teardown body, re-enter with raw value */ }
}
```

No `HandlerOutput` deserialization in either arm. The `deliver` match determines behavior from the `ParentRef` variant, not from the handler's return value.

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

### 6. Change dispatch_to_handler

**Before** (`lib.rs:440`): Always suspends, always runs handler as child of the Handle frame on the Handler side.

**After:** Dispatch on frame kind.

- **ResumeHandleFrame**: Skip suspension. Run handler DAG. When handler completes, deliver value to `perform_parent`.
- **RestartHandleFrame**: Suspend body. Run handler DAG. When handler completes, tear down body, re-enter with value.

### 7. Change handle_handler_completion

**Before** (`lib.rs:495`): Deserializes `HandlerOutput`, matches on Resume/Discard/RestartBody.

**After:** Dispatch on frame kind. Value is always raw.

- **ResumeHandleFrame**: deliver to `perform_parent`. Apply state update if applicable.
- **RestartHandleFrame**: `teardown_body` + `restart_body` with value. Apply state update.

### 8. Update handler DAGs to produce raw values

All handler DAGs drop their `Tag(...)` wrapping.

| Combinator | Before | After |
|-----------|--------|-------|
| `bind` | `Tag("Resume")` wrapper | Raw value |
| `tryCatch` | `Tag("Discard")` wrapper | Raw value (handler just extracts payload, runs recovery) |
| `race` | `Tag("Discard")` wrapper | Raw value (handler just extracts payload) |
| `loop` | `Tag("RestartBody")` wrapper | Raw value (handler just extracts payload) |
| `scope`/`jump` | `Tag("RestartBody")` wrapper | Raw value (handler just extracts payload) |

### 9. Recompile tryCatch and race as restart+Branch

`tryCatch` and `race` currently use `Tag("Discard")` in the handler to exit the Handle directly. With only Resume and Restart, they use the same restart+Branch pattern as loop.

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
      Continue: body,       // first entry: run the body
      Break: recovery,      // throw → restart → Branch takes Break → recovery runs → body completes
    }),
    ExtractPayloadHandler   // handler extracts payload, engine restarts unconditionally
  )
)
```

`throwError` = `Chain(Tag("Break"), RestartPerform(restartHandlerId))`. Tags the error as Break, Performs. Handler extracts payload (the tagged value), engine restarts. Branch sees `{ kind: "Break", value: error }`, takes the Break arm, runs recovery. Body completes normally with recovery output. Handle exits.

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
      Continue: All(                           // first entry: run all branches
        Chain(a, Chain(Tag("Break"), RestartPerform(restartHandlerId))),
        Chain(b, Chain(Tag("Break"), RestartPerform(restartHandlerId))),
      ),
      Break: identity(),                       // winner → restart → exits
    }),
    ExtractPayloadHandler
  )
)
```

First branch to complete tags Break, Performs. Handler extracts payload, engine restarts (tearing down the All and its remaining branches). Branch sees Break, identity completes, Handle exits with winner's value.

### 10. All restart handlers share the same handler DAG

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

## Open questions

1. **State updates.** bind's state is set once and never updated (read-only). Restart handlers (loop) may need state updates across iterations. The handler DAG currently produces `{ payload, state }` input and the engine applies state updates from the handler output. With raw values, state updates need a separate mechanism — either the handler DAG produces `{ value, state_update }` and the engine destructures, or state is always overwritten with the handler's raw value (which is the restart input and becomes the new state).

2. **Resume handler error semantics.** If a resume handler's DAG fails, the body is NOT suspended. The error propagates through the Handle frame to its parent, same as if the body itself failed.
