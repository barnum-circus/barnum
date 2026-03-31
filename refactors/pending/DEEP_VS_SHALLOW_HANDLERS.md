# Deep vs Shallow Handlers

## The two kinds

Every Handle/Perform usage in Barnum falls into one of two categories based on what the handler does with the body's continuation:

| Kind | What the handler does | Body suspended? | Examples |
|------|----------------------|-----------------|----------|
| **Deep** | Always Resumes with a value. Function call semantics. | No | `bind` (VarRef access), future `provide`/`consume` |
| **Shallow** | May Resume, Discard, or RestartBody. Controls the body's execution. | Yes | `tryCatch`, `race`, `withTimeout`, `loop`, `scope` |

Deep handlers resemble function calls: the body Performs, a value comes back, the body continues. The handler always produces a value for the Perform site — it never discards or restarts the body.

Shallow handlers control the body's fate. They inspect the performed value and decide: resume the body, kill it and exit with a value, or restart it from scratch. The handler has authority over the continuation.

## Shallow handlers subsume deep

A shallow handler that always Resumes is semantically identical to a deep handler. The current engine implements all handlers as shallow — every Handle suspends the body, runs the handler DAG, inspects the HandlerOutput tag, and dispatches Resume/Discard/RestartBody.

For deep handlers, this suspension is unnecessary. The handler will always Resume. There's no decision to make. The body doesn't need to be frozen because nothing will ever discard or restart it.

## Shallow handlers can implement deep patterns

The loop pattern shows how shallow handlers implement "variable binding" behavior:

```
Handle(jumpEffect, RestartBody,
  Branch({
    Continue: body,   // run the body
    Break: identity(), // exit
  })
)
```

One handler, RestartBody + state + branch. The handler restarts the body with a tagged value, and the branch at the top dispatches. This is a shallow handler emulating iteration.

`bind` uses a different pattern — it Resumes with the stored value:

```
Handle(varEffect, Resume,
  body_that_performs_varEffect
)
```

The handler reads from state and Resumes. This is a deep handler: it always returns a value to the Perform site. The body is never discarded or restarted.

## The optimization: deep handlers don't suspend

For deep handlers, the execution flow is currently:

1. Body hits `Perform(effect_id)` with payload
2. Engine walks up the frame tree to find matching `Handle`
3. Engine **suspends** the body (sets `HandleStatus::Suspended(perform_parent)`)
4. Engine runs the handler DAG with `{ payload, state }`
5. Handler DAG completes with `HandlerOutput::Resume { value, state_update }`
6. Engine inspects the tag, sees Resume
7. Engine sets `HandleStatus::Free`
8. Engine delivers `value` back to `perform_parent`

Steps 3, 5–7 are unnecessary. The handler always Resumes. We know this statically — the handler DAG's output is always `Resume`. There's no branch to inspect.

The optimized flow:

1. Body hits `Perform(effect_id)` with payload
2. Engine walks up the frame tree to find matching `Handle`
3. Engine runs the handler DAG with `{ payload, state }` — **as a chain trampoline**
4. Handler DAG completes with a value
5. Engine delivers the value back to `perform_parent`

This is structurally identical to a Chain trampoline. Chain's `rest` is a statically known ActionId. A deep handler's "rest" is the handler DAG, found by walking up at runtime. But the mechanics are the same: remove a frame, advance the handler DAG, deliver the result when it completes.

### What we avoid

- **No suspension.** The Handle frame stays `Free` throughout. The body is never frozen.
- **No stash pressure.** The stash system exists because deliveries and effects can arrive while a Handle is suspended. Deep handlers never suspend, so their descendants never hit a suspended ancestor. No stashing needed for deep handler interactions.
- **No HandlerOutput inspection.** The handler DAG produces a raw value, not a tagged `Resume`/`Discard`/`RestartBody` wrapper. The engine delivers it directly. No deserialization of the wrapper, no match on the tag.

### What remains the same

- **Effect bubbling.** The Perform still walks up the frame tree to find the matching Handle. This walk is the "dynamic dispatch" — the cost of finding the handler at runtime instead of statically.
- **Handle state.** Deep handlers can still have state (bind uses it). State updates happen when the handler DAG completes.
- **Nesting.** Deep handlers compose with other handlers (deep or shallow). A deep handler's body can contain other Handles.

## Engine representation

The Handle frame would carry a mode flag:

```rust
pub struct HandleFrame {
    pub effect_id: EffectId,
    pub mode: HandleMode,       // NEW
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Option<Value>,
    pub status: HandleStatus,   // Always Free for Deep
}

pub enum HandleMode {
    /// Handler always Resumes. Body never suspended. Function-call semantics.
    Deep,
    /// Handler may Resume, Discard, or RestartBody. Body suspended during handler execution.
    Shallow,
}
```

For `HandleMode::Deep`:
- `status` is always `Free` (could be removed for Deep, but keeping it uniform is simpler)
- The handler DAG produces a raw value, not a HandlerOutput envelope
- `dispatch_to_handler` skips suspension and runs the handler DAG as a chain-like trampoline
- `complete` for the handler side delivers the value directly to `perform_parent` without inspecting Resume/Discard/RestartBody

For `HandleMode::Shallow`:
- Current behavior. No changes.

## The trampoline analogy

Chain works like this:
1. The first child completes with a value
2. Chain frame removes itself
3. Engine advances `rest` with the value and the Chain's parent

A deep handler Perform works like this:
1. The Perform fires with a payload
2. Engine finds the matching Handle by walking up
3. Engine runs the handler DAG with `{ payload, state }` as a child of the Handle frame (handler side)
4. Handler DAG completes with a value
5. Engine delivers the value to the Perform's parent (back into the body)

The difference from Chain: step 2 is a runtime walk instead of a static ActionId. Everything else is the same mechanics. The Handle frame acts as a trampoline that routes the value through the handler DAG and back to the Perform site.

## TS surface

The mode is determined by the combinator, not by the user:

| Combinator | Mode | Why |
|-----------|------|-----|
| `bind` / `bindInput` | Deep | VarRef always Resumes with stored value |
| `tryCatch` | Shallow | Handler Discards on throw |
| `race` | Shallow | First Perform Discards the body |
| `withTimeout` | Shallow | Built on race |
| `loop` | Shallow | Handler RestartBodies on Continue |
| `scope` / `jump` | Shallow | Handler RestartBodies on jump |

The user never specifies the mode directly. Each combinator knows its own mode and emits the correct HandleMode in the AST.

If we ever expose raw `handle`/`perform` as a user-facing primitive, the mode would be explicit:

```ts
// Deep: handler is a function call, always resumes
handle.deep(effectId, handlerDag, body)

// Shallow: handler controls the continuation
handle.shallow(effectId, handlerDag, body)
```

## What this means for the handler DAG

### Deep handler DAG

The handler DAG produces a **raw value**. No `Tag("Resume")` wrapping. No `HandlerOutput` envelope.

```
// bind's handler DAG:
ExtractField("state") → ExtractIndex(n)
// produces: the nth bound variable's value
```

The engine takes this value and delivers it to the Perform's parent. Done.

### Shallow handler DAG

The handler DAG produces a **HandlerOutput envelope** — a tagged value with `Resume`, `Discard`, or `RestartBody` as the kind. The engine inspects the tag and dispatches accordingly.

```
// tryCatch's handler DAG:
pipe(recovery, Tag("Discard"))
// produces: { kind: "Discard", value: recoveryResult }
```

This is the current behavior for all handlers.

## Implementation plan

1. Add `HandleMode` enum to `barnum_ast` (Rust) and `HandleAction` (TS)
2. Add `mode` field to `HandleFrame` in the engine
3. Split `dispatch_to_handler` into deep and shallow paths:
   - Deep: run handler DAG as chain-like child, deliver raw result to perform_parent
   - Shallow: current behavior (suspend, run handler DAG, inspect HandlerOutput)
4. Split `complete` handler-side path:
   - Deep: deliver value directly, apply state update (if any)
   - Shallow: current behavior (deserialize HandlerOutput, dispatch)
5. Update `bind` to emit `HandleMode::Deep`
6. Verify all existing shallow handlers (tryCatch, race, loop) still work
7. Remove `Tag("Resume")` wrapping from bind's handler DAG

## Deep handlers as a general call mechanism

Since deep handlers are function calls — walk up, find the handler, get a value back — they share the same mechanics as other things function calls do in traditional languages.

### RAII / resource management

In C++ and Rust, RAII ties resource cleanup to scope exit. A destructor runs when the stack frame is popped, regardless of whether the function returned normally or unwound via exception/panic.

A deep handler Handle frame is a scope with a lifetime. When the body completes (normally or via a shallow handler's Discard above it), the Handle frame is torn down. If deep handlers had a **cleanup action** that runs on frame teardown, you'd get RAII:

```ts
withResource(
  (resource) => pipe(
    resource.get(),   // deep Perform: reads the resource value
    doWork,
  ),
  { create: acquireDb, dispose: releaseDb }
)
```

The Handle frame would:
1. Run `create` to acquire the resource, store in state
2. Run the body — `resource.get()` is a deep Perform that reads from state
3. On body completion OR on body teardown (Discard from an outer handler), run `dispose`

Step 3 is the RAII guarantee: cleanup runs regardless of exit path. The current `withResource` combinator (in builtins.ts) desugars to a chain of All + Merge + extractIndex, which doesn't handle the teardown-on-Discard case. A deep handler with a cleanup action would handle it naturally because the Handle frame's teardown hook fires whenever the frame is removed.

This would require a small engine addition: an optional `on_teardown: ActionId` on HandleFrame that the engine advances (with the state as input) during `teardown_body` or when the Handle frame itself is removed. The cleanup action runs as a "finally" block.

### Provide/Consume (dynamic scope)

Deep handlers ARE Provide/Consume. `bind` provides values; VarRef Performs consume them. A general `provide(name, value, body)` is a deep handler where the handler DAG returns the provided value on every Perform.

The connection: dynamic scope in traditional languages is implemented as a stack walk — `consume("x")` walks the call stack looking for the nearest binding of `x`. Deep handler Perform does the same thing — it walks the frame tree looking for the matching Handle. The mechanics are identical.

### Capabilities / tokens

A deep handler that returns a capability token is an **effect-scoped capability**. The token is only valid within the Handle's body — Performing outside the scope hits `UnhandledEffect`. This is how `tryCatch`'s `throwError` token works (though that's shallow). A deep handler version would be: "here's a logger/db/auth token, use it freely within this scope, it's cleaned up when the scope exits."

## Open questions

1. **State updates for deep handlers.** Deep handlers can have state (bind uses it). But the current state update mechanism is part of the HandlerOutput envelope (`Resume { state_update }`). For deep handlers, we need a different mechanism — either the handler DAG produces a `{ value, state_update }` tuple that the engine destructures, or state is read-only for deep handlers (which is fine for bind, where state is set once and never updated).

2. **Can the mode be inferred?** Given a handler DAG, can we statically determine whether it always Resumes? If the DAG always ends with `Tag("Resume")`, it's deep. If it has branches that might produce `Tag("Discard")` or `Tag("RestartBody")`, it's shallow. This would let us auto-optimize without user annotation. But it requires static analysis of the handler DAG at flatten time, which is doable but adds complexity. Explicit mode is simpler and more predictable.

3. **Deep handler error semantics.** What happens if a deep handler's DAG fails (e.g., a TypeScript handler inside the DAG throws)? For shallow handlers, the body is already suspended, so the engine can propagate the error upward. For deep handlers, the body is NOT suspended — it's still "running" (from the frame tree's perspective). The handler failure needs to propagate through the Handle frame and up to the Handle's parent, same as if the body itself had failed. This should work naturally — the handler DAG is a child of the Handle frame, so errors propagate upward through the Handle.
