# Phase 2: Variable Declarations (ReadVar Effect)

## Goal

Implement `declare` as the first real algebraic effect. This exercises the resume path: a Perform(ReadVar) fires, the Handle looks up the bound value, and immediately resumes the continuation with it. End-to-end validation that the Phase 1 substrate works for data flow.

## Prerequisites

Phase 1 (Effect Substrate) complete.

## The effect

```
Effect: ReadVar
Payload: DeclareId (string, e.g. "__declare_0")
Handler behavior: Look up the DeclareId in the Handle frame's stored bindings.
                  Resume the continuation with the bound value.
```

This is the simplest possible effect pattern: perform, handle, resume. No discarding, no re-entry, no side effects. Pure data injection.

## How declare compiles

### Single binding

```ts
// User writes:
declare({ x: computeX }, ({ x }) => body_using_x)

// TypeScript compiles to:
Chain(
  computeX,                              // evaluate binding eagerly
  Handle(
    { "ReadVar": handlerDAG },           // intercept ReadVar effects
    body_with_VarRefs_as_Performs         // body, VarRefs replaced by Perform(ReadVar)
  )
)
```

The handler DAG for ReadVar:

```ts
// Handler receives: { payload: "__declare_0" }
// It resolves the value and produces a Resume tagged output.
pipe(
  pick("payload"),
  resolveBinding,         // builtin: look up DeclareId → bound value
  tag("Resume"),          // produces { kind: "Resume", value: <bound_value> }
)
```

The Handle frame interprets `{ kind: "Resume", value }` and delivers the value to the suspended continuation.

`resolveBinding` is a builtin that maps a DeclareId to the stored value. The value is stored in the Handle frame's state at entry time (when the Chain delivers computeX's result to the Handle frame). This builtin accesses the Handle frame's internal state — it's the only part of this design that requires the Handle frame to hold domain-specific data (the bindings map). The alternative (passing state as a second field in the handler input) is discussed below.

### Object form (concurrent bindings)

```ts
// User writes:
declare({ a: exprA, b: exprB }, ({ a, b }) => body)

// Compiles to:
Chain(
  Parallel(exprA, exprB),      // evaluate bindings concurrently
  Handle(
    { "ReadVar": resolveAndResume },
    body                         // contains Perform(ReadVar("__0")) and Perform(ReadVar("__1"))
  )
)
```

The Handle frame stores both bindings (keyed by gensym'd IDs). When either Perform(ReadVar) fires, the handler looks up the ID and resumes with the value.

### Array form (sequential, dependent bindings)

```ts
// User writes:
declare([
  { a: exprA },
  ({ a }) => ({ b: exprB_using_a }),
], ({ a, b }) => body)

// Compiles to nested Chain + Handle:
Chain(
  exprA,
  Handle(
    { "ReadVar": resolveAndResume_for_a },
    Chain(
      exprB_using_a,    // may contain Perform(ReadVar("__0")) to read a
      Handle(
        { "ReadVar": resolveAndResume_for_a_and_b },
        body
      )
    )
  )
)
```

Each Handle frame adds one binding. Inner Handle frames can intercept ReadVar for their own binding and re-perform for outer bindings (or the Handle can hold all bindings accumulated so far).

Alternative: each Handle holds only its own binding. If the ID doesn't match, the effect propagates to the outer Handle. This is simpler per-frame but requires walking the handler chain. Given that ReadVar resume is immediate (no async), the walk cost is negligible.

## The HOAS pattern

`declare` receives a callback. The callback gets opaque AST references (VarRefs). These are TypedAction nodes whose AST is `Perform(ReadVar("__declare_N"))`. TypeScript's lexical scoping ensures VarRefs can only be used within the callback body.

```ts
function declare<TIn, TBindings, TOut>(
  bindings: TBindings,
  body: (vars: VarRefs<TBindings>) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut> {
  const ids = generateIds(bindings);         // gensym: __declare_0, __declare_1, ...
  const varRefs = createVarRefs(ids);        // TypedAction nodes wrapping Perform(ReadVar(id))
  const bodyAst = body(varRefs);             // user builds the body using the opaque refs
  return compileToHandlePerform(bindings, ids, bodyAst);
}
```

The VarRef type: `TypedAction<never, T>`. Input is `never` because VarRefs don't consume pipeline input — they perform an effect that the handler resolves. Output is `T`, the bound value's type.

## Handle frame state for ReadVar

When the Handle frame is entered, it receives the binding values from the upstream Chain (either a single value or a Parallel tuple). It stores them in a local map:

```rust
// In FrameKind::Handle for ReadVar:
struct ReadVarState {
    bindings: HashMap<DeclareId, Value>,
}
```

### How the handler DAG accesses bindings

The handler DAG needs to resolve a DeclareId to a value. Two options:

**Option A: `resolveBinding` builtin (recommended).** The Handle frame stores bindings. `resolveBinding` is a new builtin kind that reads from the current Handle frame's state. The scheduler executes it by looking at the Handle frame that dispatched this handler invocation.

**Option B: Handle frame passes state to handler DAG.** The Handle node has an optional `state` field. When a handler is dispatched, the handler receives `{ payload, state }` instead of just `{ payload }`. The handler DAG can then look up the DeclareId in the state map using normal pipeline operations (no special builtin needed). This is more general but adds complexity to every Handle frame, even those that don't need state.

Recommendation: Option A for now. It's simpler, and ReadVar is the only effect that needs Handle-local state. If multiple effects need this pattern, migrate to Option B.

### Nested declares and propagation

When bubble_effect delivers a ReadVar(id) effect:
1. The handler DAG calls `resolveBinding` with the DeclareId.
2. If found: handler produces `{ kind: "Resume", value }`. Handle frame resumes the continuation.
3. If not found: the handler produces a re-perform signal (or the `resolveBinding` builtin triggers re-propagation to the outer Handle).

Step 3 handles nested declares where an inner body references an outer binding.

## What this replaces

| Current pattern | With declare |
|---|---|
| `augment(pipe(extract, transform))` | Bind the result, reference it later |
| `tap(sideEffect)` (to preserve context) | Side effect in body, context from VarRef |
| `withResource({ create, action, dispose })` | Bind the resource (dispose comes in Phase 5) |
| `pick("field1", "field2")` (to narrow for invariance) | Still needed — VarRef gives the full value, pick narrows it |

`augment` and `tap` become unnecessary for context threading. `pick` remains necessary for handler input narrowing (invariance at serialization boundaries).

## Test strategy

### TypeScript compilation tests

1. Single-binding declare produces correct Handle/Perform AST.
2. Object form with 2+ bindings produces Parallel + Handle.
3. Array form with dependent bindings produces nested Chain + Handle.
4. VarRef used in pipe produces Perform(ReadVar) in the right position.
5. VarRef type checking: binding type matches VarRef output type.

### Rust scheduler tests

1. **Simple declare**: Bind a constant, reference it once. Verify correct value.
2. **Multiple references**: Bind once, reference 3 times. Verify same value each time.
3. **Nested declares**: Inner and outer bindings, inner body references both.
4. **Declare inside ForEach**: Binding inside a forEach iteration. Each iteration gets its own Handle frame with the same bindings from the outer scope.
5. **Declare inside Parallel**: Two parallel branches, both referencing the same outer binding. Verify both get the correct value.
6. **Concurrent bindings (object form)**: Two bindings evaluated in parallel, both referenced in body.
7. **Sequential bindings (array form)**: Second binding references first. Verify correct evaluation order and value.

### Demo migration

Rewrite the `identify-and-address-refactors` demo to use `declare` instead of `tap`/`augment`/`pick` for context threading. The demo should be shorter and clearer. This is the acceptance test.

## Deliverables

1. `EffectType::ReadVar` variant
2. Handle frame ReadVar state (bindings map)
3. ReadVar handler logic in bubble_effect dispatch
4. `declare()` TypeScript function (object form + array form)
5. VarRef TypedAction construction
6. Flattener support for Handle/Perform nodes with ReadVar
7. Tests per above
8. Demo migration
