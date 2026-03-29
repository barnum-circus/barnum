# Loop with closure providing scoped recur/done

## Problem

`recur()` and `done()` are top-level exports. They can be used anywhere in the AST, not just inside a `loop` body. This creates impossible states — using `recur()` outside a loop compiles fine in TypeScript but fails at runtime with a "no enclosing loop" error.

## Proposal

```ts
loop(({ recur, done }) =>
  pipe(
    typeCheck,
    classifyErrors,
    branch({
      HasErrors: pipe(forEach(fix), drop(), recur()),
      Clean: done(),
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
    recur: () => TypedAction<In, { kind: "Continue"; value: In }>;
    done: () => TypedAction<Out, { kind: "Break"; value: Out }>;
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

## Implementation

1. Add the overload signature to `loop` in `ast.ts`
2. At runtime, check if the argument is a function. If so, create scoped `recur`/`done` (just call `tag("Continue")` and `tag("Break")`), call the builder, and return the loop node.
3. No changes to the Rust engine — the AST shape is identical.
