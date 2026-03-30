# Phase 2: Variable Declarations (ReadVar Effect)

## Goal

Implement `declare` as the first real algebraic effect. This exercises the resume path: a Perform fires, the Handle reads from its opaque state and Resumes with the value. End-to-end validation that the Phase 1 substrate works for data flow.

## Prerequisites

Phase 1 (Effect Substrate) complete.

## Surface syntax

The canonical form is an array of bindings followed by a body callback:

```ts
declare([exprA, exprB], ([a, b]) => body)
```

Each array element is either:
- **An action** (Pipeable) — evaluated with the declare's pipeline input
- **A function** `([prev_vars...]) => action` — receives an array of all previously-bound values as VarRefs, returns an action

The body callback receives an array of all VarRefs, one per binding. VarRefs are destructured positionally.

### Examples

**Concurrent bindings (no dependencies):**

```ts
declare([fetchUser, fetchConfig], ([user, config]) =>
  pipe(user, processWithConfig(config))
)
```

Both `fetchUser` and `fetchConfig` run concurrently. The body receives VarRefs for both.

**Sequential binding (depends on previous):**

```ts
declare([
  fetchUser,
  ([user]) => fetchReposForUser(user),
], ([user, repos]) =>
  pipe(repos, forEach(processRepo))
)
```

`fetchUser` runs first. Then `fetchReposForUser` receives a VarRef for user and runs. The body receives VarRefs for both.

**Mixed concurrent and sequential:**

```ts
declare([
  fetchUser,
  fetchConfig,
  ([user, config]) => deriveSettings(user, config),
], ([user, config, settings]) =>
  body
)
```

`fetchUser` and `fetchConfig` run concurrently (neither is a function). `deriveSettings` runs after both complete (it's a function that receives previous VarRefs).

## How declare compiles

The TS macro splits the array into groups:

1. **Leading non-function items** form a concurrent group (evaluated in Parallel).
2. **Each function item** forms a sequential step (evaluated after all previous bindings).

### Compilation: concurrent group

```ts
declare([exprA, exprB], ([a, b]) => body)

// Compiles to:
Chain(
  Parallel(exprA, exprB, Identity),    // [valA, valB, pipeline_input]
  Handle(effectId, readVarHandler,
    Chain(ExtractIndex(2), body)        // body receives pipeline_input
  )
)
```

`Parallel(exprA, exprB, Identity)` evaluates both bindings concurrently AND preserves the original pipeline input (via Identity). The result is a tuple `[valA, valB, pipeline_input]`.

The Handle stores this tuple as opaque state. The body receives the pipeline input (extracted via `ExtractIndex(2)`).

When a VarRef fires (Perform), the handler DAG receives `{ payload: 0, state: [valA, valB, pipeline_input] }`. It extracts `state[payload]` and Resumes with it.

### Compilation: sequential binding

```ts
declare([
  exprA,
  ([a]) => exprB_using_a,
], ([a, b]) => body)

// Compiles to:
Chain(
  Parallel(exprA, Identity),           // [valA, pipeline_input]
  Handle(effectId_group0, readVarHandler,
    Chain(
      // Evaluate exprB_using_a. It may Perform to read `a`.
      exprB_using_a,                   // produces valB
      // Now we have valB as pipeline value.
      // Wrap: Parallel(Identity, Constant(???)) to get [valA, valB, pipeline_input]
      // Actually: re-enter a new Handle with accumulated state.
      Handle(effectId_group1, readVarHandler,
        Chain(ExtractIndex(2), body)
      )
    )
  )
)
```

Each sequential step adds a nested Handle. The inner Handle's state accumulates all bindings so far. This is standard lexical scoping: each `let` binding opens a new scope.

### The readVar handler DAG

The handler DAG is identical for every declare — it's a pure structural operation:

```ts
// Receives: { payload: <index>, state: [val0, val1, ..., pipeline_input] }
// Returns:  { kind: "Resume", value: state[payload] }
pipe(
  parallel(pick("payload"), pick("state")),  // [index, state_tuple]
  extractDynamic(),                           // state_tuple[index]
  tag("Resume"),
)
```

This uses standard builtins. No special `resolveBinding` builtin. The engine is domain-ignorant — it just routes `{ payload, state }` to the handler DAG and acts on the tagged output.

**Note:** `extractDynamic` (dynamic index extraction) is a new builtin: given `[index, array]`, return `array[index]`. This is the only new builtin needed. It's a pure data operation — the engine doesn't know it's resolving a variable.

## VarRef: generic over the bound type

`VarRef<TValue>` wraps `TypedAction<never, TValue>`. Input is `never` because VarRefs don't consume pipeline input — they perform an effect. Output is `TValue`, the concrete type of the bound value.

Because `declare` is a generic function call, TypeScript infers `TValue` from each binding expression:

```ts
declare(
  [computeName, computeCount],
  // TypeScript infers: [VarRef<string>, VarRef<number>]
  ([name, count]) => pipe(
    name,                 // produces string
    appendCount(count),   // count produces number
  ),
)
```

Each VarRef carries the concrete type of its binding. No manual annotations, no `unknown` casts. The HOAS callback is what makes this work.

### Implementation sketch

```ts
type VarRef<TValue> = TypedAction<never, TValue>;

function declare<TIn, TBindings extends (Pipeable<TIn, any> | ((vars: VarRef<any>[]) => Pipeable<TIn, any>))[], TOut>(
  bindings: [...TBindings],
  body: (vars: InferVarRefs<TBindings>) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut> {
  const effectId = generateUniqueId();
  const varRefs = bindings.map((_, i) => createVarRef(effectId, i));
  const bodyAst = body(varRefs as any);
  return compileToHandlePerform(bindings, effectId, bodyAst);
}
```

The key type: `InferVarRefs<TBindings>` maps each binding to `VarRef<OutputOf<binding>>`. TypeScript resolves `OutputOf` from each binding expression's output type.

## The HOAS pattern

Each `declare` invocation gensyms a fresh `EffectId`. The VarRefs are `Perform(effectId)` nodes with the binding index as payload. TypeScript's lexical scoping ensures VarRefs can only be used within the callback body.

Per the HOAS pattern established in the roadmap:
1. Gensym a fresh `EffectId`
2. Create a Handle keyed on that ID
3. Provide `Perform(thatId)` wrappers to the callback as opaque `Pipeable` nodes

For nested declares, each has its own `EffectId`. Inner VarRefs are caught by the inner Handle. Outer VarRefs bubble past the inner Handle (wrong effect) and are caught by the outer Handle.

## What this replaces

| Current pattern | With declare |
|---|---|
| `augment(pipe(extract, transform))` | Bind the result, reference it later via VarRef |
| `tap(sideEffect)` (to preserve context) | Side effect in body, context from VarRef |
| `withResource({ create, action, dispose })` | Bind the resource (dispose comes in Phase 5) |
| `pick("field1", "field2")` (to narrow for invariance) | Still needed — VarRef gives the full value, pick narrows it |

`augment` and `tap` become unnecessary for context threading. `pick` remains necessary for handler input narrowing (invariance at serialization boundaries).

## Test strategy

### TypeScript compilation tests

1. Single-binding declare produces correct Chain + Handle AST.
2. Concurrent bindings (array of non-functions) produce Parallel + Handle.
3. Sequential binding (function in array) produces nested Chain + Handle.
4. Mixed concurrent/sequential produces correct grouping.
5. VarRef type checking: binding type matches VarRef output type.

### Rust scheduler tests

1. **Simple declare**: Handle with state = `[42]`. Perform fires with payload 0. Handler extracts state[0], Resumes. Verify body receives 42.
2. **Multiple bindings**: Handle with state = `[1, 2, 3]`. Three Performs (payload 0, 1, 2) in a Chain. Verify each returns correct value.
3. **Nested declares**: Inner Handle has state = `["inner"]`, outer has state = `["outer"]`. Inner Perform caught by inner Handle. Perform with outer's effectId bubbles past inner, caught by outer.
4. **Concurrent bindings**: Parallel evaluates two expressions. Handle receives tuple. Verify correct variable resolution from tuple.
5. **Sequential binding**: Chain evaluates first binding, then second (which Performs to read first). Verify correct evaluation order.
6. **Declare inside ForEach**: Each iteration enters its own Handle frame. Verify isolation.
7. **Declare inside Parallel**: Two branches reference the same outer declare. Verify both get correct values.

### Demo migration

Rewrite the `identify-and-address-refactors` demo to use `declare` instead of `tap`/`augment`/`pick` for context threading. The demo should be shorter and clearer.

## New builtin: ExtractDynamic

```rust
pub enum BuiltinKind {
    // ... existing variants ...

    /// Dynamic index extraction: given [index, array], return array[index].
    /// Used by readVar handler DAGs to extract variables from state tuples.
    ExtractDynamic,
}
```

Input: `[index: number, array: any[]]`. Output: `array[index]`.

This is the only new builtin. It's a pure data operation — no knowledge of variables or scoping.

## Deliverables

1. `declare()` TypeScript function (array form with concurrent/sequential grouping)
2. VarRef TypedAction construction (Perform with index payload)
3. readVar handler DAG (using ExtractDynamic builtin)
4. `ExtractDynamic` builtin kind (Rust + TypeScript)
5. Flattener/engine support for ExtractDynamic (inline evaluation, no subprocess)
6. Tests per above
7. Demo migration
