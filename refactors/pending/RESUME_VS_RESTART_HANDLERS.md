# Resume vs Restart Handlers

## Motivation

Every Handle/Perform usage falls into one of two categories:

| Kind | Behavior | Examples |
|------|----------|----------|
| **Resume** | Handler returns a value to the Perform site. Body continues. | `bind`, future `provide`/`consume` |
| **Restart** | Handler tears down the body. Re-enters (Continue) or exits (Break). | `tryCatch`, `race`, `loop`, `scope` |

The engine currently treats all handlers as restart: it suspends the body, runs the handler DAG, deserializes a three-variant `HandlerOutput`, and dispatches. For resume handlers, suspension and the three-way dispatch are dead code paths by construction.

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
/// Function-call semantics. Always resumes. Never suspends.
pub struct ResumeHandleFrame {
    pub resume_handler_id: ResumeHandlerId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Value,
    // No status field -- always Free by construction.
}

/// Control-flow semantics. Tears down body, then Continue or Break.
pub struct RestartHandleFrame {
    pub restart_handler_id: RestartHandlerId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Value,
    pub status: HandleStatus,
}
```

Two frame kinds, not a mode flag. `ResumeHandleFrame` has no `status` because it never suspends.

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

export interface ResumePerformAction {
  kind: "ResumePerform";
  resume_handler_id: number;
}

export interface RestartPerformAction {
  kind: "RestartPerform";
  restart_handler_id: number;
}
```

Same split in the Rust AST (`barnum_ast`). A `ResumePerform` can only match a `ResumeHandle`. A `RestartPerform` can only match a `RestartHandle`. The engine's frame-tree walk checks only the matching frame kind.

### 4. Collapse HandlerOutput from three variants to two paths

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

### 5. Change dispatch_to_handler

**Before** (`lib.rs:440`): Always suspends, always runs handler as child of the Handle frame on the Handler side.

**After:** Dispatch on frame kind.

- **ResumeHandleFrame**: Skip suspension. Run handler DAG as a child of the Handle frame. When handler completes, deliver value directly to `perform_parent`. No `HandlerOutput` deserialization.
- **RestartHandleFrame**: Current behavior (suspend, run handler DAG, deserialize `RestartHandlerOutput`, dispatch Continue/Break).

### 6. Change handle_handler_completion

**Before** (`lib.rs:495`): Deserializes `HandlerOutput`, matches on Resume/Discard/RestartBody.

**After:** Dispatch on frame kind.

- **ResumeHandleFrame**: Value is raw. Deliver to `perform_parent` (stored on the handler-side `ParentRef` or alongside the handler child). Apply state update if applicable.
- **RestartHandleFrame**: Deserialize `RestartHandlerOutput`. Continue = `restart_body`. Break = `discard_continuation`.

### 7. Update bind's handler DAG

**Before** (`bind.ts`): Handler DAG wraps output in `Tag("Resume")` to produce `{ kind: "Resume", value, state_update: { kind: "Unchanged" } }`.

**After:** Handler DAG produces a raw value. `ExtractField("state") -> ExtractIndex(n)`. No Tag wrapping.

### 8. Update restart combinator handler DAGs

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
