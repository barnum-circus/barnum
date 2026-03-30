# Loop with closure providing scoped recur/done

> **Convention**: All discriminated unions use `TaggedUnion<Def>` — every variant carries `{ kind: K; value: T; __def?: Def }`. All union constructors (`tag`, `recur`, `done`, `some`, `none`) require the full variant map so output carries `__def`. Branch auto-unwraps `value` — case handlers receive the payload directly.

## Problem

`recur()` and `done()` are top-level exports. They can be used anywhere in the AST, not just inside a `loop` body. This creates impossible states — using `recur()` outside a loop compiles fine in TypeScript but fails at runtime with a "no enclosing loop" error.

## Proposal

```ts
loop(({ recur, done }) =>
  pipe(
    typeCheck,
    classifyErrors,
    branch({
      HasErrors: pipe(forEach(fix), drop(), recur()),  // receives TypeError[] (auto-unwrapped)
      Clean: done(),  // receives void (auto-unwrapped)
    }),
  ),
)
```

The callback receives `recur` and `done` scoped to this specific loop instance. The closure is called once at AST construction time (not at runtime) to build the loop body.

## Type-level benefits

The scoped `recur` and `done` carry the loop's type parameters:

```ts
function loop<In, Out>(
  build: (ctx: {
    recur: () => TypedAction<In, LoopResult<In, Out>>;
    done: () => TypedAction<Out, LoopResult<In, Out>>;
  }) => TypedAction<In, LoopResult<In, Out>>,
): TypedAction<In, Out>
```

Note: the Continue type is `In` (the loop's input type), not `unknown`. The closure-based approach can provide a properly typed `recur` because the loop's `In` type is known at the point the closure is called. This is better than the current `LoopResult<unknown, Out>` workaround.

## Why the current approach uses `unknown` for Continue

With top-level `recur()`, TypeScript must infer `In` from the loop body's input type and `Out` from the Break value type simultaneously. Because `recur()` wraps whatever flows into it, a free `TContinue` generic would force TypeScript to unify it with `Out` from the Break branch — which fails (they're different types). `unknown` avoids the inference problem: `{ kind: "Continue"; value: unknown }` accepts anything.

With the closure approach, `In` is established by the loop's input (from context), and the closure's `recur` is pre-bound to `In`. No inference conflict.

## Runtime representation

The closure is called at construction time. At runtime, the AST node is still:

```json
{ "kind": "Loop", "body": { ... } }
```

The scoped `recur()` and `done()` produce the same AST nodes as the top-level versions. The difference is purely in the TypeScript type system — they enforce that `recur` and `done` are only used within their enclosing loop.

## Interaction with thunk builtins

If we also implement the thunk approach (combinators accept `TypedAction | () => TypedAction`), then `recur` and `done` inside the closure would be bare values (not function calls):

```ts
loop(({ recur, done }) =>
  pipe(
    branch({
      Continue: pipe(fix, recur),
      Break: done,
    }),
  ),
)
```

These are compatible — the closure provides the scoped values, and the thunk approach lets them be used without `()`.

## Backward compatibility

Keep the top-level `recur()` and `done()` exports. The closure-based `loop` is a new overload:

```ts
// Existing: loop body as a TypedAction
function loop<In, Out>(body: TypedAction<In, LoopResult<unknown, Out>>): TypedAction<In, Out>;

// New: loop body via closure
function loop<In, Out>(
  build: (ctx: { recur: ...; done: ... }) => TypedAction<In, LoopResult<In, Out>>,
): TypedAction<In, Out>;
```

TypeScript distinguishes the overloads by argument type (TypedAction vs function).

## Loop IDs and scoping

The scoped `recur` and `done` need to be tied to a specific loop instance. Each loop gets an ID assigned at construction time. The closure's `recur`/`done` close over that ID.

**ID stability for tests**: Use a deterministic counter (incrementing integer per config builder invocation). Since closures are called in source order at construction time, the same source always produces the same IDs. No randomness.

**Connection to continuations**: `recur()` and `done()` are effectively delimited continuations scoped to the loop. The closure makes the scoping explicit — they're first-class continuation constructors bound to a specific loop delimiter.

## Are these the right primitives?

### First-class continuations as the general mechanism

`recur()` and `done()` are delimited continuations: they tag a value to signal control flow to an enclosing delimiter (the loop). The key question is whether this mechanism should be general — not just for loops.

**Observation**: `done()` = "early return from the enclosing scope." This is the `?` operator. If a pipeline step produces `{ kind: "Break", value: x }`, the enclosing scope short-circuits with `x`. In a loop, that means "stop iterating." But the same mechanism could mean "skip the rest of this pipeline" in a non-loop context.

**Observation**: `recur()` = "restart the enclosing scope with new input." In a loop, that means "iterate again." But it could mean "retry" or "tail-call" in other contexts.

### Does `done` make sense outside a loop?

Yes. Consider:

```ts
pipe(
  validate,
  branch({
    Invalid: pipe(logError, done()),   // short-circuit, return error
    Valid: processData,                // continue pipeline
  }),
)
```

Without `done()` as early return, the only way to short-circuit is to structure the entire rest of the pipeline as the Valid branch, which gets deeply nested. This is exactly the problem `?` solves in Rust.

The primitive that enables this would be something like `scope`:

```ts
scope(({ exit }) =>
  pipe(
    validate,
    branch({
      Invalid: pipe(logError, exit()),
      Valid: processData,
    }),
  ),
)
```

`loop` is then: `scope` + "repeat on `recur`, return on `exit`."

### Is `loop` the right name?

`loop` is accurate for what it does. But if the underlying primitive is `scope` with continuations, then `loop` is sugar for `scope` + implicit recur-on-Continue. The question is whether to expose `scope` as the primitive and build `loop` on top, or keep `loop` as the primitive and add `scope` later.

**Argument for `loop` as primitive**: It's the common case. Most uses of scoped continuations are loops. Adding `scope` as a separate primitive later doesn't break anything.

**Argument for `scope` as primitive**: `loop` has slightly different semantics than `scope` — in `loop`, the body is called repeatedly; in `scope`, it's called once with an escape hatch. Making `loop = scope + recur` is clean. But it means `loop`'s body must explicitly recur on every path that should iterate, or else the loop exits. That's actually how our current `loop` works (the body must produce Continue or Break), so there's no behavioral difference.

### Should `workflowBuilder` be a function providing combinators?

```ts
workflow(({ pipe, loop, branch, forEach }) =>
  pipe(
    constant({ folder: "/path" }),
    listFiles,
    forEach(processFile),
  ),
)
```

**Pros**:
- No imports needed — all combinators come from the closure
- Deterministic call ordering (JS execution order = source order), making IDs stable
- Natural scoping: `loop` provides `recur`/`done`, `workflow` provides `self` for self-recursion

**Cons**:
- Breaks tree-shaking (all combinators are always provided)
- Unusual API pattern — most TS libraries use imports
- Deeply nested closures if combinators also take closures (loop inside workflow)

**Middle ground**: Keep imports for stateless combinators (`pipe`, `branch`, `forEach`, `constant`, etc.) and use closures only for scoping (`loop`, `scope`, `workflow` for self-ref).

### Proposed primitive hierarchy

1. **`scope`** — the fundamental delimiting construct. Provides `exit()` (early return).
2. **`loop`** — `scope` + implicit restart on Continue. Provides `recur()` and `done()`.
3. **`workflowBuilder`** — top-level scope. Provides `self` for self-recursion.
4. **`registerSteps`** — provides `stepRef` for mutual recursion.

Each level provides scoped continuations appropriate to its control flow pattern. The underlying mechanism is the same: tag + dispatch by enclosing delimiter.

### Rust engine implications

Currently the engine has `Loop` as an AST node that handles `Continue`/`Break` tags. If we add `scope`, we'd need a `Scope` node that handles `Break` tags (for early return) but not `Continue`. This is a small addition — it's basically `Loop` minus the restart logic.

## Implementation

1. Add the overload signature to `loop` in `ast.ts`
2. At runtime, check if the argument is a function. If so, create scoped `recur`/`done` (just call `tag("Continue")` and `tag("Break")`), call the builder, and return the loop node.
3. No changes to the Rust engine — the AST shape is identical.
4. Consider adding `scope` as a primitive alongside or before `loop` closure support.
