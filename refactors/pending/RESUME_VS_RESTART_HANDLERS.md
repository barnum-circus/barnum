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

**After for ResumeHandle/ResumePerform:** The handler DAG runs at the Perform site, not at the Handle. The ResumeHandle frame is never suspended. State is initialized from the input value (same as current Handle). Combinators embed state into the input tuple alongside the pipeline input (bind: `All(...bindings, Identity)`, counter: `All(Constant(0), Identity)`). The body does `ExtractIndex(N)` to retrieve the pipeline input.

#### 6a. `advance` match arm for `FlatAction::ResumeHandle`

When the engine encounters a `ResumeHandle` action during `advance`:

```rust
FlatAction::ResumeHandle { resume_handler_id } => {
    let body = workflow_state.flat_config.resume_handle_body(action_id);
    let handler = workflow_state.flat_config.resume_handle_handler(action_id);

    // State is initialized from the input value. Body also receives
    // the input value. Combinators embed any "real" state (e.g.,
    // bound values for bind, counter value) into the input tuple
    // alongside the pipeline input.
    let frame_id = workflow_state.insert_frame(Frame {
        parent,
        kind: FrameKind::ResumeHandle(ResumeHandleFrame {
            resume_handler_id,
            body,
            handler,
            state: value.clone(),
        }),
    });

    advance(workflow_state, body, value, Some(ParentRef::ResumeHandle { frame_id }))?;
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

#### Invariant: RestartPerform must not fire as a non-terminal child of All/ForEach during advance

`teardown_body` runs during `advance` when a RestartPerform fires. If RestartPerform is a non-terminal child of All (e.g., `All(RestartPerform(e), Invoke(B))`), `teardown_body` removes the All frame while we're inside All's advance loop. Subsequent children would create frames with dangling parents.

Current combinators never produce this pattern. RestartPerform is always behind `Chain(Tag(...), RestartPerform(...))`, where the Tag is a builtin Invoke that must complete before RestartPerform fires. RestartPerform always fires from `complete` → `deliver` → Chain trampoline → `advance`, never from the initial `advance` of a multi-child combinator.

The `advance` arm for `FlatAction::RestartPerform` should include a `debug_assert` validating this invariant. The test `restart_perform_non_terminal_in_all` documents the scenario.

### Stash elimination

Neither handler kind suspends the Handle frame:

- **ResumePerform**: handler runs inline at the Perform site. Handle frame is uninvolved.
- **RestartPerform**: body is torn down immediately. Handle frame transitions directly from "running body" to "running handler."

`HandleStatus::Suspended` is deleted. `is_blocked_by_handle` is deleted. The stash (`stashed_items`, `sweep_stash`, `sweep_stash_once`, `StashedItem`, `SweepResult`, `StashOutcome`, `TryDeliverResult::Blocked`, `find_blocking_ancestor`, `AncestorCheck::Blocked`) is deleted entirely.

Completions for tasks that belonged to the torn-down body arrive at the engine with a stale `TaskId` — `task_to_frame.remove(&task_id)` returns `None` because `teardown_body` already removed the entry. `complete()` must handle this gracefully by returning `Ok(None)` instead of panicking (see STALE_TASK_COMPLETION.md).

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
| `bind` (readVar) | `Chain(ExtractIndex(1), Chain(ExtractIndex(n), Tag("Resume")))` | `All(Chain(ExtractIndex(1), ExtractIndex(n)), ExtractIndex(1))` — value = state[n], new_state = state (unchanged) |
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

---

## Phase 2: Implementation task list

> **Note on code style:** Architecture sections above use `self.` method style for readability. Actual code uses free functions with explicit `workflow_state` parameter, as established in the lib.rs split.

### Task 1: Remove `#[should_panic]` from target-behavior tests

**Goal:** Three tests become failing tests that define acceptance criteria.

**File:** `crates/barnum_engine/src/lib.rs`

Remove the `#[should_panic(expected = "...")]` attribute from:

```rust
// Before:
#[should_panic(expected = "resume handler should not cause stashing")]
fn resume_handler_does_not_block_sibling_completion() { ... }

#[should_panic(expected = "both handlers should dispatch concurrently")]
fn concurrent_resume_performs_not_serialized() { ... }

#[should_panic(expected = "throw should reach outer handler immediately")]
fn throw_proceeds_while_resume_handler_in_flight() { ... }

// After: remove the #[should_panic] line from each. Tests now fail.
```

**How to test:** `cargo test` — three tests fail. All others pass.

### Task 2: Add ResumeHandle/ResumePerform to all layers

**Goal:** Add the Resume types alongside existing Handle/Perform. Switch `bind` to emit the new types. Restart combinators still use old Handle/Perform.

#### 2.1: Add `ResumeHandlerId` to Rust AST

**File:** `crates/barnum_ast/src/lib.rs`

Add after `EffectId` (line 45):

```rust
// Before: only EffectId exists.

// After: add alongside EffectId (EffectId stays for now — deleted in Task 3).
/// Identifies a resume-style effect handler. Paired with [`ResumePerformAction`].
/// Separate type from [`RestartHandlerId`] prevents cross-matching at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResumeHandlerId(pub u16);

impl std::fmt::Display for ResumeHandlerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

#### 2.2: Add `ResumeHandleAction` and `ResumePerformAction` to Rust AST

**File:** `crates/barnum_ast/src/lib.rs`

Add new structs after `PerformAction` (line 163):

```rust
/// Resume-style effect handler. Handler runs inline at the Perform site.
/// Handler produces [value, new_state]. Engine delivers value to Perform's
/// parent and writes new_state back to the ResumeHandle frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeHandleAction {
    pub resume_handler_id: ResumeHandlerId,
    pub body: Box<Action>,
    pub handler: Box<Action>,
}

/// Raise a resume-style effect. Targets the nearest enclosing
/// [`ResumeHandle`](Action::ResumeHandle) with matching `resume_handler_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePerformAction {
    pub resume_handler_id: ResumeHandlerId,
}
```

Add variants to the `Action` enum (line 62):

```rust
pub enum Action {
    // ... existing variants ...
    ResumeHandle(ResumeHandleAction),
    ResumePerform(ResumePerformAction),
}
```

#### 2.3: Add `FlatAction::ResumeHandle` and `FlatAction::ResumePerform`

**File:** `crates/barnum_ast/src/flat.rs`

Add to `FlatAction<T>` enum (after line 136):

```rust
/// Resume-style effect handler. 3-entry action (same layout as Handle):
/// this entry, body child slot at action_id + 1, handler child slot
/// at action_id + 2.
ResumeHandle {
    resume_handler_id: ResumeHandlerId,
},

/// Raise a resume-style effect. Single-entry (like Invoke/Perform).
ResumePerform {
    resume_handler_id: ResumeHandlerId,
},
```

Update `try_map_target` (line 146) — add passthrough arms:

```rust
FlatAction::ResumeHandle { resume_handler_id } => FlatAction::ResumeHandle { resume_handler_id },
FlatAction::ResumePerform { resume_handler_id } => FlatAction::ResumePerform { resume_handler_id },
```

**Complication:** Verify `FlatEntry<ActionId>` still fits in 8 bytes. The enum grows from 8 to 10 variants — discriminant still fits in 1 byte. `ResumeHandlerId(u16)` is the same size as `EffectId(u16)`. Static assert at line 615 will catch any regression.

#### 2.4: Update flatten for `ResumeHandle` and `ResumePerform`

**File:** `crates/barnum_ast/src/flat.rs`

In `flatten_action_at` (line 422), add arms after the `Action::Perform` arm:

```rust
Action::ResumeHandle(ResumeHandleAction {
    resume_handler_id,
    body,
    handler,
}) => {
    self.alloc(); // child slot for body (at action_id + 1)
    self.alloc(); // child slot for handler (at action_id + 2)
    self.fill_child_slot(*body, action_id + 1, workflow_root)?;
    self.fill_child_slot(*handler, action_id + 2, workflow_root)?;
    FlatAction::ResumeHandle { resume_handler_id }
}

Action::ResumePerform(ResumePerformAction { resume_handler_id }) => {
    FlatAction::ResumePerform { resume_handler_id }
}
```

In `fill_child_slot` (line 513), add `Action::ResumeHandle { .. }` to the multi-entry match arm alongside `Action::Handle { .. }`:

```rust
Action::Chain { .. }
| Action::All { .. }
| Action::Branch { .. }
| Action::Handle { .. }
| Action::ResumeHandle { .. } => {
    // multi-entry: flatten elsewhere, write ChildRef
```

**Complication:** `ResumePerform` is single-entry (like `Perform`), so it falls through to the `single_entry` arm in `fill_child_slot`. No change needed there.

#### 2.5: Add `FlatConfig` accessors for `ResumeHandle`

**File:** `crates/barnum_ast/src/flat.rs`

Add after `handle_handler` (line 337):

```rust
/// Returns the body `ActionId` for a ResumeHandle (resolves child
/// slot at `action_id + 1`).
#[must_use]
pub fn resume_handle_body(&self, id: ActionId) -> ActionId {
    debug_assert!(matches!(self.action(id), FlatAction::ResumeHandle { .. }));
    self.resolve_child_slot(id + 1)
}

/// Returns the handler `ActionId` for a ResumeHandle (resolves child
/// slot at `action_id + 2`).
#[must_use]
pub fn resume_handle_handler(&self, id: ActionId) -> ActionId {
    debug_assert!(matches!(self.action(id), FlatAction::ResumeHandle { .. }));
    self.resolve_child_slot(id + 2)
}
```

#### 2.6: Add engine frame types

**File:** `crates/barnum_engine/src/frame.rs`

Add new imports at top:

```rust
use barnum_ast::ResumeHandlerId;
```

Add new structs (after `HandleFrame`, line 122):

```rust
/// Resume-style effect handler frame. Handler runs at the Perform site,
/// not here. Multiple concurrent ResumePerforms can target this frame.
/// Never suspended.
#[derive(Debug)]
pub struct ResumeHandleFrame {
    /// Which resume effect type this handler intercepts.
    pub resume_handler_id: ResumeHandlerId,
    /// The body action (for reference — not used for restart).
    pub body: ActionId,
    /// The handler DAG to invoke when the effect fires.
    pub handler: ActionId,
    /// Mutable state. Updated by each handler invocation's returned
    /// state value.
    pub state: Value,
}

/// Frame at the Perform site for a ResumeHandle. Runs the handler DAG
/// as a child. When the handler completes, intercepts the result to
/// apply state updates to the ResumeHandle's state, then delivers the
/// value to its parent.
#[derive(Debug)]
pub struct ResumePerformFrame {
    /// The ResumeHandle frame this Perform targets.
    pub handle_frame_id: FrameId,
}
```

Add to `FrameKind` enum:

```rust
pub enum FrameKind {
    // ... existing variants ...
    ResumeHandle(ResumeHandleFrame),
    ResumePerform(ResumePerformFrame),
}
```

Add to `ParentRef` enum:

```rust
pub enum ParentRef {
    // ... existing variants ...
    /// Parent is a ResumeHandle frame — body child only (no handler side).
    ResumeHandle {
        frame_id: FrameId,
    },
    /// Parent is a ResumePerform frame — handler child.
    ResumePerform {
        frame_id: FrameId,
    },
}
```

Update `ParentRef::frame_id()` to include the new variants:

```rust
pub const fn frame_id(self) -> FrameId {
    match self {
        Self::Chain { frame_id }
        | Self::All { frame_id, .. }
        | Self::ForEach { frame_id, .. }
        | Self::Handle { frame_id, .. }
        | Self::ResumeHandle { frame_id }
        | Self::ResumePerform { frame_id } => frame_id,
    }
}
```

#### 2.7: Add advance arms for ResumeHandle and ResumePerform

**File:** `crates/barnum_engine/src/advance.rs`

Add import:

```rust
use super::frame::{ResumeHandleFrame, ResumePerformFrame};
use barnum_ast::ResumeHandlerId;
```

Add after `FlatAction::Perform` arm (line 183):

```rust
FlatAction::ResumeHandle { resume_handler_id } => {
    let body = workflow_state.flat_config.resume_handle_body(action_id);
    let handler = workflow_state.flat_config.resume_handle_handler(action_id);
    let frame_id = workflow_state.insert_frame(Frame {
        parent,
        kind: FrameKind::ResumeHandle(ResumeHandleFrame {
            resume_handler_id,
            body,
            handler,
            state: value.clone(),
        }),
    });
    advance(
        workflow_state,
        body,
        value,
        Some(ParentRef::ResumeHandle { frame_id }),
    )?;
}

FlatAction::ResumePerform { resume_handler_id } => {
    let parent = parent.ok_or(AdvanceError::UnhandledResumeEffect {
        resume_handler_id,
    })?;

    // Walk ancestors to find the matching ResumeHandle.
    let (handle_frame_id, handler_action_id, state) =
        super::ancestors::ancestors(&workflow_state.frames, parent)
            .find_map(|(edge, frame)| {
                if let FrameKind::ResumeHandle(handle) = &frame.kind
                    && handle.resume_handler_id == resume_handler_id
                {
                    Some((edge.frame_id(), handle.handler, handle.state.clone()))
                } else {
                    None
                }
            })
            .ok_or(AdvanceError::UnhandledResumeEffect {
                resume_handler_id,
            })?;

    let handler_input = serde_json::json!([value, state]);

    let perform_frame_id = workflow_state.insert_frame(Frame {
        parent: Some(parent),
        kind: FrameKind::ResumePerform(ResumePerformFrame {
            handle_frame_id,
        }),
    });

    advance(
        workflow_state,
        handler_action_id,
        handler_input,
        Some(ParentRef::ResumePerform {
            frame_id: perform_frame_id,
        }),
    )?;
}
```

Add new error variant to `AdvanceError` in `lib.rs`:

```rust
#[error("unhandled resume effect: {resume_handler_id}")]
UnhandledResumeEffect {
    resume_handler_id: ResumeHandlerId,
},
```

#### 2.8: Add deliver arms for ResumeHandle and ResumePerform

**File:** `crates/barnum_engine/src/complete.rs`

In `deliver` function (line 163), add arms after the `ParentRef::Handle` arm:

```rust
ParentRef::ResumeHandle { frame_id } => {
    // Body completed. Remove ResumeHandle frame, deliver to parent.
    let frame = workflow_state
        .frames
        .remove(frame_id)
        .expect("ResumeHandle frame exists");
    deliver(workflow_state, frame.parent, value)
}

ParentRef::ResumePerform { frame_id } => {
    // Handler completed. Destructure [value, state].
    let frame = workflow_state
        .frames
        .remove(frame_id)
        .expect("ResumePerform frame exists");
    let FrameKind::ResumePerform(perform) = frame.kind else {
        unreachable!("ResumePerform ParentRef points to non-ResumePerform frame");
    };
    let parent = frame
        .parent
        .expect("ResumePerform always has a parent");

    let (resume_value, new_state): (Value, Value) =
        serde_json::from_value(value)?;

    // Write state back to the ResumeHandle frame.
    let handle_frame = workflow_state
        .frames
        .get_mut(perform.handle_frame_id)
        .expect("ResumeHandle still alive");
    let FrameKind::ResumeHandle(ref mut resume_handle) = handle_frame.kind
    else {
        unreachable!("handle_frame_id points to non-ResumeHandle");
    };
    resume_handle.state = new_state;

    deliver(workflow_state, Some(parent), resume_value)
}
```

**Complication:** `deliver` currently returns `Result<Option<Value>, CompleteError>`. The `serde_json::from_value` in the ResumePerform arm produces `serde_json::Error`, which converts to `CompleteError::InvalidHandlerOutput` via the existing `#[from]` attribute on `CompleteError`.

#### 2.9: Update TypeScript effect-id.ts

**File:** `libs/barnum/src/effect-id.ts`

```typescript
// Before:
export type EffectId = number & { readonly __brand: unique symbol };
let nextEffectId = 0;
export function allocateEffectId(): EffectId {
  return nextEffectId++ as EffectId;
}

// After: add ResumeHandlerId alongside EffectId (EffectId stays for now).
export type EffectId = number & { readonly __brand: unique symbol };
export type ResumeHandlerId = number & { readonly __resumeHandlerBrand: unique symbol };

let nextEffectId = 0;
export function allocateEffectId(): EffectId {
  return nextEffectId++ as EffectId;
}
export function allocateResumeHandlerId(): ResumeHandlerId {
  return nextEffectId++ as ResumeHandlerId;
}
```

Both share the same counter — IDs are globally unique.

#### 2.10: Add ResumeHandle/ResumePerform to TypeScript AST

**File:** `libs/barnum/src/ast.ts`

Add new interfaces after `PerformAction` (around line 55):

```typescript
export interface ResumeHandleAction {
  kind: "ResumeHandle";
  resume_handler_id: number;
  body: Action;
  handler: Action;
}

export interface ResumePerformAction {
  kind: "ResumePerform";
  resume_handler_id: number;
}
```

Add to the `Action` type union:

```typescript
// Before:
export type Action = InvokeAction | ChainAction | ... | HandleAction | PerformAction;

// After:
export type Action = InvokeAction | ChainAction | ... | HandleAction | PerformAction
  | ResumeHandleAction | ResumePerformAction;
```

**Complication:** Any exhaustive `kind` switches over `Action` in TS must add the new cases. Search for uses of `action.kind` or `switch` on action kind.

#### 2.11: Update bind.ts to emit ResumeHandle/ResumePerform

**File:** `libs/barnum/src/bind.ts`

Update imports:

```typescript
// Before:
import { allocateEffectId, type EffectId } from "./effect-id.js";

// After:
import { allocateResumeHandlerId, type ResumeHandlerId } from "./effect-id.js";
```

Update `createVarRef`:

```typescript
// Before:
function createVarRef<TValue>(effectId: EffectId): VarRef<TValue> {
  return typedAction({ kind: "Perform", effect_id: effectId });
}

// After:
function createVarRef<TValue>(resumeHandlerId: ResumeHandlerId): VarRef<TValue> {
  return typedAction({ kind: "ResumePerform", resume_handler_id: resumeHandlerId });
}
```

Update `readVar` — handler now produces `[value, new_state]` via `All`:

```typescript
// Before:
function readVar(n: number): Action {
  return {
    kind: "Chain",
    first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: 1 } } },
    rest: {
      kind: "Chain",
      first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: n } } },
      rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Resume" } } },
    },
  };
}

// After:
function readVar(n: number): Action {
  return {
    kind: "All",
    actions: [
      {
        kind: "Chain",
        first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: 1 } } },
        rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: n } } },
      },
      { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: 1 } } },
    ],
  };
}
```

Handler receives `[payload, state]`. First All branch: `ExtractIndex(1)` → state, `ExtractIndex(n)` → state[n] = value. Second All branch: `ExtractIndex(1)` → state = new_state (unchanged). Result: `[state[n], state]`.

Update `bind` function — replace `Handle`/`Perform` with `ResumeHandle`/`ResumePerform`:

```typescript
// Before (line 108):
const effectIds = bindings.map(() => allocateEffectId());

// After:
const resumeHandlerIds = bindings.map(() => allocateResumeHandlerId());

// Before (line 111):
const varRefs = effectIds.map((id) => createVarRef(id));

// After:
const varRefs = resumeHandlerIds.map((id) => createVarRef(id));

// Before (line 124-131):
for (let i = effectIds.length - 1; i >= 0; i--) {
  inner = {
    kind: "Handle",
    effect_id: effectIds[i],
    handler: readVar(i),
    body: inner,
  };
}

// After:
for (let i = resumeHandlerIds.length - 1; i >= 0; i--) {
  inner = {
    kind: "ResumeHandle",
    resume_handler_id: resumeHandlerIds[i],
    handler: readVar(i),
    body: inner,
  };
}
```

**Complication:** `bindInput` calls `bind`, so it inherits the changes automatically.

#### 2.12: Update engine tests

**File:** `crates/barnum_engine/src/lib.rs`

Add test helper constructors:

```rust
fn resume_handle(resume_handler_id: u16, handler: Action, body: Action) -> Action {
    Action::ResumeHandle(ResumeHandleAction {
        resume_handler_id: ResumeHandlerId(resume_handler_id),
        body: Box::new(body),
        handler: Box::new(handler),
    })
}

fn resume_perform(resume_handler_id: u16) -> Action {
    Action::ResumePerform(ResumePerformAction {
        resume_handler_id: ResumeHandlerId(resume_handler_id),
    })
}
```

Update `read_var` test helper:

```rust
// Before:
fn read_var(n: u64) -> Action {
    chain(
        invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
        chain(
            invoke_builtin(BuiltinKind::ExtractIndex { value: json!(n) }),
            invoke_builtin(BuiltinKind::Tag { value: json!("Resume") }),
        ),
    )
}

// After:
fn read_var(n: u64) -> Action {
    parallel(vec![
        chain(
            invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
            invoke_builtin(BuiltinKind::ExtractIndex { value: json!(n) }),
        ),
        invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
    ])
}
```

Update all bind tests (tests named `bind_*`) to use `resume_handle`/`resume_perform` instead of `handle`/`perform`. The test logic stays the same — only the constructors change. The handler output changes from `{"kind": "Resume", "value": V}` to `[V, state]` (raw tuple).

Example — `bind_single_binding_single_read`:

```rust
// Before: handle(e0, read_var(0), chain(perform(e0), invoke("./echo.ts", "echo")))
// After:
resume_handle(e0, read_var(0), chain(resume_perform(e0), invoke("./echo.ts", "echo")))
```

Update `resume_with_state` test — handler now returns `[value, state]` instead of `{"kind": "Resume", "value": V}`:

```rust
// Before: handler DAG is Chain(ExtractIndex(1), Tag("Resume"))
// After: handler DAG is All(ExtractIndex(1), ExtractIndex(1))
//   — value = state, new_state = state
```

Update `state_update_persists` test — handler returns `[value, new_state]`:

```rust
// Before:
json!({"kind": "Resume", "value": "v1", "state_update": {"kind": "Updated", "value": "new_state"}})

// After:
json!(["v1", "new_state"])
```

And the second handler verification:

```rust
// Before: handler receives ["mid_out", "new_state"]
// After: same — handler receives [payload, state] where state = "new_state"
// Before: handler returns {"kind": "Resume", "value": "v2"}
// After: handler returns ["v2", "new_state"]  (state pass-through)
```

Update `multi_step_handler_chain` test — handler return changes:

```rust
// Before: json!({"kind": "Resume", "value": "final"})
// After: json!(["final", "input"])  // state = Handle input = "input"
```

**How to test:** All existing bind tests pass with new constructors. Three `#[should_panic]` tests still fail (they depend on Tasks 3-4).

### Task 3: Convert Handle/Perform to RestartHandle/RestartPerform

**Goal:** Rename old `Handle`/`Perform` to `RestartHandle`/`RestartPerform`. Implement immediate body teardown for `RestartPerform`. Update restart combinators. Delete old types.

#### 3.1: Add `RestartHandlerId` and restart action types to Rust AST

**File:** `crates/barnum_ast/src/lib.rs`

Add alongside `ResumeHandlerId`:

```rust
/// Identifies a restart-style effect handler. Paired with [`RestartPerformAction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RestartHandlerId(pub u16);

impl std::fmt::Display for RestartHandlerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

Add action structs:

```rust
/// Restart-style effect handler. Body torn down on Perform, re-entered
/// with handler's output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartHandleAction {
    pub restart_handler_id: RestartHandlerId,
    pub body: Box<Action>,
    pub handler: Box<Action>,
}

/// Raise a restart-style effect. Targets the nearest enclosing
/// [`RestartHandle`](Action::RestartHandle) with matching `restart_handler_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartPerformAction {
    pub restart_handler_id: RestartHandlerId,
}
```

Add `Action::RestartHandle(RestartHandleAction)` and `Action::RestartPerform(RestartPerformAction)` to the enum. Delete `Action::Handle(HandleAction)`, `Action::Perform(PerformAction)`, `HandleAction`, `PerformAction`, and `EffectId`.

#### 3.2: Update flat representation

**File:** `crates/barnum_ast/src/flat.rs`

Replace `FlatAction::Handle` and `FlatAction::Perform` with:

```rust
RestartHandle {
    restart_handler_id: RestartHandlerId,
},
RestartPerform {
    restart_handler_id: RestartHandlerId,
},
```

Update `try_map_target` — replace Handle/Perform passthrough arms with RestartHandle/RestartPerform.

Update `flatten_action_at` — replace `Action::Handle` arm with `Action::RestartHandle`:

```rust
Action::RestartHandle(RestartHandleAction {
    restart_handler_id,
    body,
    handler,
}) => {
    self.alloc(); // body child slot
    self.alloc(); // handler child slot
    self.fill_child_slot(*body, action_id + 1, workflow_root)?;
    self.fill_child_slot(*handler, action_id + 2, workflow_root)?;
    FlatAction::RestartHandle { restart_handler_id }
}

Action::RestartPerform(RestartPerformAction { restart_handler_id }) => {
    FlatAction::RestartPerform { restart_handler_id }
}
```

Replace `Action::Handle { .. }` with `Action::RestartHandle { .. }` in `fill_child_slot`'s multi-entry match arm.

Rename accessors `handle_body` → `restart_handle_body`, `handle_handler` → `restart_handle_handler`. Update their `debug_assert!` to match `FlatAction::RestartHandle`.

Remove imports of `HandleAction`, `PerformAction`, `EffectId`. Add imports of `RestartHandleAction`, `RestartPerformAction`, `RestartHandlerId`.

#### 3.3: Add engine frame types for restart

**File:** `crates/barnum_engine/src/frame.rs`

```rust
/// Which side of a RestartHandle frame a child belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartHandleSide {
    Body,
    Handler,
}

/// Restart-style effect handler frame. Body torn down when effect fires.
/// Handler runs as child on Handler side.
#[derive(Debug)]
pub struct RestartHandleFrame {
    pub restart_handler_id: RestartHandlerId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Value,
}
```

Update `FrameKind`:

```rust
// Before:
Handle(HandleFrame),

// After: replace with
RestartHandle(RestartHandleFrame),
```

Update `ParentRef`:

```rust
// Before:
Handle { frame_id: FrameId, side: HandleSide },

// After: replace with
RestartHandle { frame_id: FrameId, side: RestartHandleSide },
```

Delete `HandleFrame`, `HandleStatus`, `HandleSide`.

Update `ParentRef::frame_id()` — replace `Self::Handle { frame_id, .. }` with `Self::RestartHandle { frame_id, .. }`.

#### 3.4: Update engine advance for RestartHandle/RestartPerform

**File:** `crates/barnum_engine/src/advance.rs`

Replace `FlatAction::Handle` arm with:

```rust
FlatAction::RestartHandle { restart_handler_id } => {
    let body = workflow_state.flat_config.restart_handle_body(action_id);
    let handler = workflow_state.flat_config.restart_handle_handler(action_id);
    let frame_id = workflow_state.insert_frame(Frame {
        parent,
        kind: FrameKind::RestartHandle(RestartHandleFrame {
            restart_handler_id,
            body,
            handler,
            state: value.clone(),
        }),
    });
    advance(
        workflow_state,
        body,
        value,
        Some(ParentRef::RestartHandle {
            frame_id,
            side: RestartHandleSide::Body,
        }),
    )?;
}
```

Replace `FlatAction::Perform` arm with:

```rust
FlatAction::RestartPerform { restart_handler_id } => {
    let parent = parent.ok_or(AdvanceError::UnhandledRestartEffect {
        restart_handler_id,
    })?;

    // Walk ancestors to find the matching RestartHandle.
    let handle_frame_id =
        super::ancestors::ancestors(&workflow_state.frames, parent)
            .find_map(|(edge, frame)| {
                if let FrameKind::RestartHandle(handle) = &frame.kind
                    && handle.restart_handler_id == restart_handler_id
                {
                    Some(edge.frame_id())
                } else {
                    None
                }
            })
            .ok_or(AdvanceError::UnhandledRestartEffect {
                restart_handler_id,
            })?;

    // Tear down body immediately.
    super::effects::teardown_body(
        &mut workflow_state.frames,
        &mut workflow_state.task_to_frame,
        handle_frame_id,
    );

    // Look up handler and state.
    let handle_frame = workflow_state
        .frames
        .get(handle_frame_id)
        .expect("RestartHandle frame exists");
    let FrameKind::RestartHandle(ref handle) = handle_frame.kind else {
        unreachable!();
    };
    let handler_action_id = handle.handler;
    let state = handle.state.clone();
    let handler_input = serde_json::json!([value, state]);

    advance(
        workflow_state,
        handler_action_id,
        handler_input,
        Some(ParentRef::RestartHandle {
            frame_id: handle_frame_id,
            side: RestartHandleSide::Handler,
        }),
    )?;
}
```

Add new error variant:

```rust
#[error("unhandled restart effect: {restart_handler_id}")]
UnhandledRestartEffect {
    restart_handler_id: RestartHandlerId,
},
```

Delete old `AdvanceError::UnhandledEffect`.

**Complication:** `teardown_body` uses `is_descendant_of_body` which checks for `ParentRef::Handle { side: HandleSide::Body, .. }`. Update to check `ParentRef::RestartHandle { side: RestartHandleSide::Body, .. }`.

#### 3.5: Update engine deliver for RestartHandle

**File:** `crates/barnum_engine/src/complete.rs`

Replace `ParentRef::Handle` arm with:

```rust
ParentRef::RestartHandle { frame_id, side } => match side {
    RestartHandleSide::Body => {
        // Body completed. Remove RestartHandle frame, deliver to parent.
        let frame = workflow_state
            .frames
            .remove(frame_id)
            .expect("RestartHandle frame exists");
        deliver(workflow_state, frame.parent, value)
    }
    RestartHandleSide::Handler => {
        // Handler completed. Value is the new body input.
        // Extract payload from [payload, state].
        // (The handler DAG is ExtractIndex(0), so value IS the payload.)
        super::effects::restart_body(workflow_state, frame_id, value)
    }
}
```

**Complication:** `restart_body` currently returns `Result<Option<Value>, CompleteError>`. It tears down body (already done in advance for immediate teardown), re-advances body. Since body was already torn down in `advance`, `restart_body` in `deliver` just needs to re-advance. Update `restart_body` to skip the teardown step if body is already torn down (the frame tree is empty below the handle).

Actually, on reflection: `restart_body` currently calls `teardown_body` then re-advances. For the restart handler completion path in `deliver`, the body was already torn down during `advance` when the `RestartPerform` fired. `restart_body` would call `teardown_body` again — but it's a no-op because there are no body descendants left (they were already removed). Then it re-advances the body. This is safe and correct, just slightly wasteful. No change needed.

#### 3.6: Handle stale task completions

**File:** `crates/barnum_engine/src/complete.rs`

When `RestartPerform` tears down the body during advance, it removes task_to_frame entries for torn-down Invoke frames. If those tasks later try to complete, `task_to_frame.remove(&task_id)` returns `None`.

Change `complete()`:

```rust
// Before:
let frame_id = workflow_state
    .task_to_frame
    .remove(&task_id)
    .expect("unknown task");

// After:
let Some(frame_id) = workflow_state.task_to_frame.remove(&task_id) else {
    // Task belonged to a torn-down subtree (e.g., RestartPerform
    // tore down the body while this task was in flight).
    return Ok(None);
};
```

This is the key change that allows immediate body teardown to work. Without it, stale task completions would panic.

#### 3.7: Update TypeScript — add RestartHandlerId, rename Handle/Perform

**File:** `libs/barnum/src/effect-id.ts`

```typescript
// Add:
export type RestartHandlerId = number & { readonly __restartHandlerBrand: unique symbol };

export function allocateRestartHandlerId(): RestartHandlerId {
  return nextEffectId++ as RestartHandlerId;
}
```

Delete `EffectId` and `allocateEffectId` (no longer used).

**File:** `libs/barnum/src/ast.ts`

Add new interfaces:

```typescript
export interface RestartHandleAction {
  kind: "RestartHandle";
  restart_handler_id: number;
  body: Action;
  handler: Action;
}

export interface RestartPerformAction {
  kind: "RestartPerform";
  restart_handler_id: number;
}
```

Add to `Action` union. Delete `HandleAction` and `PerformAction`.

Update `buildRestartBranchAction`:

```typescript
// Before:
export function buildRestartBranchAction(effectId: EffectId, continueArm: Action, breakArm: Action): Action {
  return {
    kind: "Chain",
    first: TAG_CONTINUE,
    rest: {
      kind: "Handle",
      effect_id: effectId,
      body: { ... },
      handler: RESTART_BODY_HANDLER,
    },
  };
}

// After:
export function buildRestartBranchAction(restartHandlerId: RestartHandlerId, continueArm: Action, breakArm: Action): Action {
  return {
    kind: "Chain",
    first: TAG_CONTINUE,
    rest: {
      kind: "RestartHandle",
      restart_handler_id: restartHandlerId,
      body: {
        kind: "Branch",
        cases: unwrapBranchCases({
          Continue: continueArm,
          Break: breakArm,
        }),
      },
      handler: EXTRACT_PAYLOAD_HANDLER,
    },
  };
}
```

Replace `RESTART_BODY_HANDLER` with:

```typescript
// Before:
const RESTART_BODY_HANDLER: Action = {
  kind: "Chain",
  first: EXTRACT_PAYLOAD,
  rest: TAG_RESTART_BODY,
};

// After:
const EXTRACT_PAYLOAD_HANDLER: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: 0 } },
};
```

Delete `TAG_RESTART_BODY`, `RESTART_BODY_HANDLER`, `EXTRACT_PAYLOAD` (replaced by `EXTRACT_PAYLOAD_HANDLER`).

#### 3.8: Update restart combinators

**File:** `libs/barnum/src/try-catch.ts`

```typescript
// Before:
import { allocateEffectId } from "./effect-id.js";
const effectId = allocateEffectId();
const throwError = typedAction<TError, never>({
  kind: "Chain",
  first: TAG_BREAK,
  rest: { kind: "Perform", effect_id: effectId },
});
return typedAction(buildRestartBranchAction(effectId, bodyAction, recovery as Action));

// After:
import { allocateRestartHandlerId } from "./effect-id.js";
const restartHandlerId = allocateRestartHandlerId();
const throwError = typedAction<TError, never>({
  kind: "Chain",
  first: TAG_BREAK,
  rest: { kind: "RestartPerform", restart_handler_id: restartHandlerId },
});
return typedAction(buildRestartBranchAction(restartHandlerId, bodyAction, recovery as Action));
```

**File:** `libs/barnum/src/race.ts`

```typescript
// Before:
import { allocateEffectId, type EffectId } from "./effect-id.js";
const effectId = allocateEffectId();
function breakPerform(effectId: EffectId): Action {
  return {
    kind: "Chain",
    first: TAG_BREAK,
    rest: { kind: "Perform", effect_id: effectId },
  };
}

// After:
import { allocateRestartHandlerId, type RestartHandlerId } from "./effect-id.js";
const restartHandlerId = allocateRestartHandlerId();
function breakPerform(restartHandlerId: RestartHandlerId): Action {
  return {
    kind: "Chain",
    first: TAG_BREAK,
    rest: { kind: "RestartPerform", restart_handler_id: restartHandlerId },
  };
}
```

**File:** `libs/barnum/src/ast.ts` — `recur`, `earlyReturn`, `loop`

Same pattern: `allocateEffectId()` → `allocateRestartHandlerId()`, `Perform` → `RestartPerform`, `Handle` → `RestartHandle`.

For `recur` (line 800):

```typescript
// Before:
const effectId = allocateEffectId();
const restartAction = typedAction({ kind: "Perform", effect_id: effectId });
return typedAction({ kind: "Handle", effect_id: effectId, body, handler: RESTART_BODY_HANDLER });

// After:
const restartHandlerId = allocateRestartHandlerId();
const restartAction = typedAction({ kind: "RestartPerform", restart_handler_id: restartHandlerId });
return typedAction({
  kind: "RestartHandle",
  restart_handler_id: restartHandlerId,
  body,
  handler: EXTRACT_PAYLOAD_HANDLER,
});
```

For `loop` (line 893):

```typescript
// Before:
const effectId = allocateEffectId();
const perform: Action = { kind: "Perform", effect_id: effectId };
const recurAction = typedAction({ kind: "Chain", first: TAG_CONTINUE, rest: perform });
const doneAction = typedAction({ kind: "Chain", first: TAG_BREAK, rest: perform });
return typedAction(buildRestartBranchAction(effectId, body, IDENTITY));

// After:
const restartHandlerId = allocateRestartHandlerId();
const perform: Action = { kind: "RestartPerform", restart_handler_id: restartHandlerId };
const recurAction = typedAction({ kind: "Chain", first: TAG_CONTINUE, rest: perform });
const doneAction = typedAction({ kind: "Chain", first: TAG_BREAK, rest: perform });
return typedAction(buildRestartBranchAction(restartHandlerId, body, IDENTITY));
```

For `earlyReturn` (line 836):

```typescript
// Before:
const effectId = allocateEffectId();
const earlyReturnAction = typedAction({ kind: "Chain", first: TAG_BREAK, rest: { kind: "Perform", effect_id: effectId } });
return typedAction(buildRestartBranchAction(effectId, body, IDENTITY));

// After:
const restartHandlerId = allocateRestartHandlerId();
const earlyReturnAction = typedAction({
  kind: "Chain",
  first: TAG_BREAK,
  rest: { kind: "RestartPerform", restart_handler_id: restartHandlerId },
});
return typedAction(buildRestartBranchAction(restartHandlerId, body, IDENTITY));
```

#### 3.9: Update engine tests for restart

**File:** `crates/barnum_engine/src/lib.rs`

Update test helpers:

```rust
// Before:
fn handle(effect_id: u16, handler: Action, body: Action) -> Action { ... }
fn perform(effect_id: u16) -> Action { ... }
fn restart_body_handler() -> Action { chain(extract_index(0), tag_builtin("RestartBody")) }
fn restart_branch(...) -> Action { ... }
fn break_perform(effect_id: u16) -> Action { chain(tag_builtin("Break"), perform(effect_id)) }

// After:
fn restart_handle(restart_handler_id: u16, handler: Action, body: Action) -> Action {
    Action::RestartHandle(RestartHandleAction {
        restart_handler_id: RestartHandlerId(restart_handler_id),
        body: Box::new(body),
        handler: Box::new(handler),
    })
}
fn restart_perform(restart_handler_id: u16) -> Action {
    Action::RestartPerform(RestartPerformAction {
        restart_handler_id: RestartHandlerId(restart_handler_id),
    })
}
fn extract_payload_handler() -> Action {
    extract_index(0)
}
fn restart_branch(restart_handler_id: u16, continue_arm: Action, break_arm: Action) -> Action {
    chain(
        tag_builtin("Continue"),
        restart_handle(
            restart_handler_id,
            extract_payload_handler(),
            branch(vec![
                ("Continue", chain(extract_field("value"), continue_arm)),
                ("Break", chain(extract_field("value"), break_arm)),
            ]),
        ),
    )
}
fn break_restart_perform(restart_handler_id: u16) -> Action {
    chain(tag_builtin("Break"), restart_perform(restart_handler_id))
}
```

Update restart tests to use new helpers. The test logic is identical — only the constructor names change.

Update tests that used old `handle`/`perform` for restart patterns:
- `restart_branch_break_skips_rest_of_chain`
- `restart_branch_break_cleans_up_frames_and_tasks`
- `restart_branch_multiple_then_break`
- `teardown_cleans_up_concurrent_tasks`

Update the three `#[should_panic]` tests to use `resume_handle`/`resume_perform` for the inner (resume) handle and `restart_handle`/`restart_perform` for the outer (restart) handle in `throw_proceeds_while_resume_handler_in_flight`.

Delete tests that tested stash behavior with old Handle/Perform:
- `stash_delivery_during_suspension` — stash no longer exists
- `concurrent_performs_serialized` — resume performs are concurrent, not serialized
- `two_effects_different_handles` — this tested stash/suspension interaction

Replace with equivalents that verify the new non-suspending behavior.

**How to test:** `cargo test` — all tests pass except the three `#[should_panic]`-removed tests from Task 1.

### Task 4: Delete stash infrastructure

**Goal:** Remove all stash-related types, functions, and fields. The engine is purely non-suspending.

#### 4.1: Delete types from lib.rs

**File:** `crates/barnum_engine/src/lib.rs`

Delete:
- `HandlerOutput` enum (lines 111-130)
- `StateUpdate` enum (lines 132-144)
- `StashedItem` enum (lines 150-169)
- `AncestorCheck` enum (lines 171-180)
- `TryDeliverResult` enum (lines 182-192)
- `StashOutcome` enum (lines 194-202)
- `SweepResult` enum (lines 204-213)
- `stashed_items: VecDeque<StashedItem>` field from `WorkflowState` (line 231)
- `VecDeque` import (line 13)
- `serde::Deserialize` import (line 21)
- Remove `stashed_items: VecDeque::new()` from `WorkflowState::new()`

#### 4.2: Delete functions from effects.rs

**File:** `crates/barnum_engine/src/effects.rs`

Delete everything except `restart_body`, `teardown_body`, and `is_descendant_of_body`. These are still needed by RestartPerform/RestartHandle.

Specifically delete:
- `bubble_effect`
- `find_and_dispatch_handler`
- `dispatch_to_handler`
- `handle_handler_completion`
- `apply_state_update`
- `resume_continuation`

Update `is_descendant_of_body` to check `ParentRef::RestartHandle { side: RestartHandleSide::Body, .. }` instead of `ParentRef::Handle { side: HandleSide::Body, .. }`.

#### 4.3: Delete stash code from complete.rs

**File:** `crates/barnum_engine/src/complete.rs`

Delete:
- `sweep_stash` function
- `sweep_stash_once` function
- `try_deliver` function

Simplify `complete`:

```rust
// Before:
pub fn complete(workflow_state, task_id, value) -> Result<Option<Value>, CompleteError> {
    let frame_id = workflow_state.task_to_frame.remove(&task_id).expect("unknown task");
    ...
    if result.is_some() { return Ok(result); }
    sweep_stash(workflow_state)
}

// After:
pub fn complete(workflow_state, task_id, value) -> Result<Option<Value>, CompleteError> {
    let Some(frame_id) = workflow_state.task_to_frame.remove(&task_id) else {
        return Ok(None); // Task belonged to a torn-down subtree.
    };
    let Some(frame) = workflow_state.frames.remove(frame_id) else {
        return Ok(None); // Frame was torn down (stale generational index).
    };
    debug_assert!(
        matches!(frame.kind, FrameKind::Invoke { .. }),
        "task_to_frame pointed at non-Invoke frame: {:?}",
        frame.kind,
    );
    match frame.parent {
        Some(parent_ref) => deliver(workflow_state, Some(parent_ref), value),
        None => Ok(Some(value)),
    }
}
```

No stash sweep. No try_deliver. Direct delivery.

#### 4.4: Delete ancestor blocking code

**File:** `crates/barnum_engine/src/ancestors.rs`

Delete:
- `find_blocking_ancestor` function
- `is_blocked_by_handle` function
- `AncestorCheck` import (from lib.rs — already deleted in 4.1)

Keep:
- `Ancestors` struct and `Iterator` impl
- `ancestors()` function — still used by ResumePerform and RestartPerform to find matching handles

#### 4.5: Delete obsolete builtins

**File:** `crates/barnum_ast/src/lib.rs`

Delete from `BuiltinKind`:

```rust
// Delete these variants:
TagResume,
TagRestartBody,
```

**File:** `crates/barnum_builtins/src/lib.rs` (or wherever builtins are executed)

Remove the match arms for `TagResume` and `TagRestartBody`.

#### 4.6: Clean up tests

**File:** `crates/barnum_engine/src/lib.rs`

Delete test helpers that are no longer needed:
- `always_resume_handler` — old-style handler that returned `{"kind": "Resume", "value": V}`
- `echo_resume_handler` — same
- `garbage_output_handler` — tested HandlerOutput deserialization
- `missing_fields_handler` — same

Delete tests that tested old Handle/Perform mechanics:
- `garbage_handler_output_errors` — HandlerOutput is deleted
- `missing_fields_handler_output_errors` — same

Delete tests that tested stash behavior (if not already deleted in Task 3):
- `stash_delivery_during_suspension`
- `concurrent_performs_serialized`
- `two_effects_different_handles`

The three target-behavior tests should now pass:
- `resume_handler_does_not_block_sibling_completion`
- `concurrent_resume_performs_not_serialized`
- `throw_proceeds_while_resume_handler_in_flight`

**How to test:** `cargo test` — all tests pass. Zero warnings.
