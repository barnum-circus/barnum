# Resume, Restart, and Break Handlers

## Motivation

Every Handle/Perform usage falls into one of three categories based on what happens unconditionally when the handler completes:

| Kind | What happens | Handler output | Examples |
|------|-------------|---------------|----------|
| **Resume** | Value delivered to Perform site. Body continues. | Raw value (for the body) | `bind`, future `provide`/`consume` |
| **Restart** | Body torn down, re-entered with new input. | Raw value (new body input) | `loop`, `scope`/`jump` |
| **Break** | Body torn down, Handle exits. | Raw value (Handle's exit value) | `tryCatch`, `race` |

Each kind is unconditional. The handler produces a raw value. The engine knows what to do with it based on the Handle kind. There is no enum, no tag dispatch, no envelope.

The engine currently treats all handlers identically: suspend body, run handler DAG, deserialize a three-variant `HandlerOutput` (Resume/Discard/RestartBody), dispatch. Two of those three variants are dead code for any given combinator.

## What changes

### 1. Replace EffectId with three separate ID types

**Before** (`barnum_ast/src/lib.rs:45`):

```rust
pub struct EffectId(pub u16);
```

**After:**

```rust
pub struct ResumeHandlerId(pub u16);
pub struct RestartHandlerId(pub u16);
pub struct BreakHandlerId(pub u16);
```

Separate types, separate namespaces. A `ResumePerform` can only target a `ResumeHandlerId`, etc. Cross-matching is a compile error.

### 2. Split HandleFrame into three frame kinds

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

/// Break semantics. Body torn down, Handle exits with handler value.
pub struct BreakHandleFrame {
    pub break_handler_id: BreakHandlerId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Value,
    pub status: HandleStatus,
}
```

`ResumeHandleFrame` has no `status` — it never suspends. Restart and Break both suspend the body while the handler runs (the body is about to be torn down either way).

### 3. Split the AST nodes (Handle and Perform)

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

export interface BreakHandleAction {
  kind: "BreakHandle";
  break_handler_id: number;
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

export interface BreakPerformAction {
  kind: "BreakPerform";
  break_handler_id: number;
}
```

Same split in the Rust AST (`barnum_ast`).

### 4. Delete HandlerOutput

**Before** (`lib.rs:108`):

```rust
enum HandlerOutput {
    Resume { value, state_update },
    Discard { value },
    RestartBody { value, state_update },
}
```

**After:** Deleted. All three handler kinds produce raw values. The engine knows what to do based on the frame kind:

- **ResumeHandleFrame**: deliver value to `perform_parent`.
- **RestartHandleFrame**: tear down body, re-enter with value.
- **BreakHandleFrame**: tear down body, deliver value to Handle's parent.

No deserialization. No tag matching.

### 5. Change dispatch_to_handler

**Before** (`lib.rs:440`): Always suspends, always runs handler as child of the Handle frame on the Handler side.

**After:** Dispatch on frame kind.

- **ResumeHandleFrame**: Skip suspension. Run handler DAG. When handler completes, deliver value to `perform_parent`.
- **RestartHandleFrame**: Suspend body. Run handler DAG. When handler completes, tear down body, re-enter with value.
- **BreakHandleFrame**: Suspend body. Run handler DAG. When handler completes, tear down body, deliver value to Handle's parent.

### 6. Change handle_handler_completion

**Before** (`lib.rs:495`): Deserializes `HandlerOutput`, matches on Resume/Discard/RestartBody.

**After:** Dispatch on frame kind. Value is always raw.

- **ResumeHandleFrame**: deliver to `perform_parent`. Apply state update if applicable.
- **RestartHandleFrame**: `teardown_body` + `restart_body` with value. Apply state update.
- **BreakHandleFrame**: `teardown_body` + deliver value to Handle's parent.

### 7. Update handler DAGs to produce raw values

All handler DAGs drop their `Tag(...)` wrapping.

| Combinator | Before | After |
|-----------|--------|-------|
| `bind` | `Tag("Resume")` wrapper | Raw value |
| `tryCatch` | `Tag("Discard")` wrapper | Raw value |
| `race` | `Tag("Discard")` wrapper | Raw value |
| `loop` | `Tag("RestartBody")` wrapper | Raw value |
| `scope`/`jump` | `Tag("RestartBody")` wrapper | Raw value |

### 8. Loop body: break via normal completion

With RestartHandle being unconditional, a loop's "break" path is the body completing normally (not via Perform):

```ts
// loop body compiles to:
pipe(
  actualBody,   // produces LoopResult<TContinue, TBreak>
  branch({
    Continue: restartPerform(loopHandlerId),  // → handler restarts body
    Break: identity(),                         // → body completes, Handle exits
  })
)
```

The Continue branch Performs to the RestartHandle, which re-enters. The Break branch falls through to normal body completion, and the Handle delivers the value to its parent.

## Combinator-to-handle-kind mapping

| Combinator | Handle kind | Perform kind |
|-----------|-------------|-------------|
| `bind` / `bindInput` | ResumeHandle | ResumePerform |
| `tryCatch` | BreakHandle | BreakPerform |
| `race` | BreakHandle | BreakPerform |
| `withTimeout` | BreakHandle (built on race) | BreakPerform |
| `loop` | RestartHandle | RestartPerform |
| `scope` / `jump` | RestartHandle | RestartPerform |

## Open questions

1. **State updates.** bind's state is set once and never updated (read-only). Restart handlers (loop) need state updates across iterations. Break handlers (tryCatch) don't care — the Handle is exiting. The state update mechanism should probably differ per kind: read-only for Resume, handler DAG produces `{ value, state_update }` for Restart, ignored for Break.

2. **Resume handler error semantics.** If a resume handler's DAG fails, the body is NOT suspended. The error propagates through the Handle frame to its parent, same as if the body itself failed.

3. **Can Restart and Break share a frame implementation?** Both suspend the body and tear it down. They differ only in what happens after teardown (re-enter vs exit). Sharing implementation is fine; sharing a type is not — they're semantically different and the frame kind determines the unconditional behavior.
