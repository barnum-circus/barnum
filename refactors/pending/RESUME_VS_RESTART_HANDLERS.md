# Resume vs Restart Handlers

## Motivation

Every Handle/Perform usage falls into one of two categories:

| Kind | Behavior | Examples |
|------|----------|----------|
| **Resume** | Handler returns a value to the Perform site. Body continues. | `bind`, future `provide`/`consume` |
| **Restart** | Handler tears down the body. Re-enters (Continue) or exits (Break). | `tryCatch`, `race`, `loop`, `scope` |

The engine currently treats all handlers as restart: it suspends the body, runs the handler DAG, deserializes a three-variant `HandlerOutput`, and dispatches. For resume handlers, suspension and the three-way dispatch are dead code paths by construction.

## What changes

### 1. Split HandleFrame into two frame kinds

**Before** (`frame.rs:110`):

```rust
pub struct HandleFrame {
    pub effect_id: EffectId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Option<Value>,
    pub status: HandleStatus,  // Free | Suspended(ParentRef)
}
```

**After:**

```rust
/// Function-call semantics. Always resumes. Never suspends.
pub struct ResumeHandleFrame {
    pub effect_id: EffectId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Option<Value>,
    // No status field -- always Free by construction.
}

/// Control-flow semantics. Tears down body, then Continue or Break.
pub struct RestartHandleFrame {
    pub effect_id: EffectId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Option<Value>,
    pub status: HandleStatus,
}
```

Two frame kinds, not a mode flag. `ResumeHandleFrame` has no `status` because it never suspends.

### 2. Split the AST node

**Before** (`ast.ts:46`):

```ts
export interface HandleAction {
  kind: "Handle";
  effect_id: number;
  body: Action;
  handler: Action;
}
```

**After:**

```ts
export interface ResumeHandleAction {
  kind: "ResumeHandle";
  effect_id: number;
  body: Action;
  handler: Action;
}

export interface RestartHandleAction {
  kind: "RestartHandle";
  effect_id: number;
  body: Action;
  handler: Action;
}
```

Same split in the Rust AST (`barnum_ast`).

### 3. Collapse HandlerOutput from three variants to two paths

**Before** (`lib.rs:108`):

```rust
enum HandlerOutput {
    Resume { value, state_update },
    Discard { value },
    RestartBody { value, state_update },
}
```

Three variants, but no combinator mixes them. `bind` always produces Resume. `tryCatch`/`race` always produce Discard. `loop`/`scope` always produce RestartBody.

**After:**

- **ResumeHandle handlers** produce a raw value. No envelope. The engine delivers it directly to the Perform's parent.
- **RestartHandle handlers** produce a `LoopResult` (Continue/Break). The engine tears down the body, then re-enters (Continue) or exits (Break).

```rust
/// Only used by RestartHandle handlers.
enum RestartHandlerOutput {
    Continue { value, state_update },
    Break { value },
}
```

Resume = direct delivery. Discard = Break. RestartBody = Continue. The three-variant `HandlerOutput` is deleted.

### 4. Change dispatch_to_handler

**Before** (`lib.rs:440`): Always suspends, always runs handler as child of the Handle frame on the Handler side.

**After:** Dispatch on frame kind.

- **ResumeHandleFrame**: Skip suspension. Run handler DAG as a child of the Handle frame. When handler completes, deliver value directly to `perform_parent`. No `HandlerOutput` deserialization.
- **RestartHandleFrame**: Current behavior (suspend, run handler DAG, deserialize `RestartHandlerOutput`, dispatch Continue/Break).

### 5. Change handle_handler_completion

**Before** (`lib.rs:495`): Deserializes `HandlerOutput`, matches on Resume/Discard/RestartBody.

**After:** Dispatch on frame kind.

- **ResumeHandleFrame**: Value is raw. Deliver to `perform_parent` (stored on the handler-side `ParentRef` or alongside the handler child). Apply state update if applicable.
- **RestartHandleFrame**: Deserialize `RestartHandlerOutput`. Continue = `restart_body`. Break = `discard_continuation`.

### 6. Update bind's handler DAG

**Before** (`bind.ts`): Handler DAG wraps output in `Tag("Resume")` to produce `{ kind: "Resume", value, state_update: { kind: "Unchanged" } }`.

**After:** Handler DAG produces a raw value. `ExtractField("state") -> ExtractIndex(n)`. No Tag wrapping.

### 7. Update restart combinator handler DAGs

`tryCatch`, `race`, `loop`, `scope` change their Tag from `"Discard"`/`"RestartBody"` to `"Break"`/`"Continue"` respectively.

| Combinator | Before | After |
|-----------|--------|-------|
| `bind` | `Tag("Resume")` | Raw value (no tag) |
| `tryCatch` | `Tag("Discard")` | `Tag("Break")` |
| `race` | `Tag("Discard")` | `Tag("Break")` |
| `loop` | `Tag("RestartBody")` | `Tag("Continue")` |
| `scope`/`jump` | `Tag("RestartBody")` | `Tag("Continue")` |

## Combinator-to-handle-kind mapping

Each combinator statically knows its kind:

| Combinator | Emits |
|-----------|-------|
| `bind` / `bindInput` | `ResumeHandle` |
| `tryCatch` | `RestartHandle` |
| `race` | `RestartHandle` |
| `withTimeout` | `RestartHandle` (built on race) |
| `loop` | `RestartHandle` |
| `scope` / `jump` | `RestartHandle` |

Users never pick the mode. If raw `handle`/`perform` is exposed, the mode would be explicit: `handle.resume(...)` vs `handle.restart(...)`.

## Open questions

1. **State updates for resume handlers.** bind's state is set once and never updated. If all resume handlers have read-only state, the handler DAG doesn't need to produce a state update at all. If future resume handlers need mutable state, the handler DAG would produce `{ value, state_update }` and the engine destructures.

2. **Resume handler error semantics.** If a resume handler's DAG fails, the body is NOT suspended (unlike restart handlers). The error propagates through the Handle frame to its parent, same as if the body itself failed. This should work naturally since the handler DAG is a child of the Handle frame.
