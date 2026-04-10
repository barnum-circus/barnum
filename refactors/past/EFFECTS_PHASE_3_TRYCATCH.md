# Phase 3: Error Handling (tryCatch + invokeWithThrow)

## Scope

TypeScript-only. The Rust engine already has Handle/Perform, bubble_effect, teardown_body, and the Discard continuation operation from Phase 1. This phase adds:

1. `tryCatch()` — TypeScript combinator (HOAS callback provides `throwError` token)
2. `invokeWithThrow()` — convenience combinator for handlers that return `Result<TOut, TError>`
3. A demo workflow demonstrating the retry-on-error pattern

No Rust changes. No changes to the engine's error propagation. This is purely for **type-level error handling** — handlers return `Result` values, the AST interprets them, and the effect system routes errors to recovery branches. If a handler panics or the runtime crashes, that's a separate mechanism handled by the existing error propagation path.

## Prerequisites

Phase 1 (Effect Substrate) complete. Phase 2 (Declare/bind) is not a prerequisite.

## tryCatch

### The effect

```
Effect: Throw
Payload: TError (the error data)
Handler behavior: Discard the continuation.
                  Run recovery branch with the error payload.
                  Handle frame exits with recovery's result.
```

The handler never resumes. The continuation is orphaned and torn down by the existing `teardown_body` from Phase 1.

### How it compiles

```ts
// User writes:
tryCatch(
  (throwError) => bodyUsingThrowError,
  recovery,
)

// Compiles to:
Handle(
  freshEffectId,
  recoveryHandler,  // handler DAG
  body,             // contains Perform(freshEffectId) at throw sites
)
```

The `throwError` token has type `TypedAction<TError, never>`. It takes the error payload and never returns — the continuation is discarded.

### Handler DAG

The handler DAG receives `{ payload, state }` from the engine. State is unused for tryCatch. The DAG extracts the error payload, runs recovery, and tags the result as Discard:

```ts
// Handler DAG (constructed by the tryCatch combinator, not user-written):
Chain(
  GetField("payload"),
  Chain(recovery, Tag("Discard")),
)
```

The Handle frame interprets `{ kind: "Discard", value }`: tears down the body subgraph and delivers `value` to its parent.

### TypeScript implementation

```ts
export function tryCatch<TIn, TOut, TError>(
  body: (throwError: TypedAction<TError, never>) => Pipeable<TIn, TOut>,
  recovery: Pipeable<TError, TOut>,
): TypedAction<TIn, TOut> {
  const effectId = nextEffectId++;
  const throwError = typedAction<TError, never>({ kind: "Perform", effect_id: effectId });
  const bodyAction = body(throwError) as Action;

  const handlerDag: Action = {
    kind: "Chain",
    first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", value: "payload" } } },
    rest: {
      kind: "Chain",
      first: recovery as Action,
      rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Discard" } } },
    },
  };

  return typedAction({
    kind: "Handle",
    effect_id: effectId,
    handler: handlerDag,
    body: bodyAction,
  });
}
```

### Nested tryCatch

Each tryCatch mints its own EffectId. Nested tryCatch gives precise targeting without re-throwing:

```ts
tryCatch((throwOuter) =>
  tryCatch((throwInner) =>
    pipe(
      riskyAction,
      branch({
        Recoverable: throwInner,  // caught by inner
        Fatal: throwOuter,        // skips inner, caught by outer
      }),
    ),
    innerRecovery,
  ),
  outerRecovery,
)
```

`throwOuter` is `Perform(effectId_7)`, `throwInner` is `Perform(effectId_8)`. Each Handle matches its own ID. Standard lexical scoping via `bubble_effect`.

## invokeWithThrow

Convenience combinator for handlers that return `Result<TOut, TError>`. Branches on the result: Ok passes through, Err throws via the provided throw token.

The handler remains oblivious to the effect system — it returns a `Result` value, and the AST translates it into control flow. This is the Free Monad / intent pattern.

### How it works

`Result<TOut, TError>` is a `TaggedUnion<{ Ok: TOut; Err: TError }>`. Branch auto-unwraps `value`, so the Ok case receives `TOut` directly and the Err case receives `TError` directly.

```ts
export function invokeWithThrow<TIn, TOut, TError>(
  handler: Pipeable<TIn, Result<TOut, TError>>,
  throwError: Pipeable<TError, never>,
): TypedAction<TIn, TOut> {
  // handler → branch({ Ok: identity (pass through), Err: throwError })
  return typedAction({
    kind: "Chain",
    first: handler as Action,
    rest: {
      kind: "Branch",
      cases: unwrapBranchCases({
        Ok: identity() as Action,
        Err: throwError as Action,
      }),
    },
  });
}
```

### Usage

```ts
tryCatch(
  (throwError) => pipe(
    invokeWithThrow(riskyHandler, throwError),
    processResult,
  ),
  handleError,
)
```

The throw token is always passed explicitly — same pattern as Rust's `Result` propagation. If a utility function needs to throw, its signature declares the throw token parameter.

## Handler execution failure

tryCatch handles **type-level errors only** — values returned by handlers via the `Result` type. If a handler panics, throws a JavaScript exception, or the runtime crashes, the existing error propagation path handles it. tryCatch does not catch those. This is analogous to Rust's `Result` vs `panic!` distinction.

## tryCatch + bind interaction

```ts
bind([acquireResource], ([resource]) =>
  tryCatch(
    (throwError) => pipe(resource, useResource, riskyStep(throwError)),
    pipe(resource, cleanupPartialWork, reportError),
  ),
)
```

The bind Handle is the outer scope. The tryCatch Handle is inner. If riskyStep throws:
1. `bubble_effect` walks up from the Perform site to the tryCatch Handle.
2. The tryCatch Handle discards the continuation and runs recovery.
3. Recovery can reference `resource` via the bind Handle (still active above).
4. The ReadVar and Throw effects compose without interference — different Handle frames.

## Demo: retry-on-error pipeline

A workflow that runs multiple fallible steps inside a loop. Each step returns a `Result`. On any error, the catch handler logs the error and recurs. On success, done.

```ts
// Pipeline:
//   loop(
//     tryCatch(
//       (throwError) => pipe(
//         invokeWithThrow(stepA, throwError),
//         invokeWithThrow(stepB, throwError),
//         invokeWithThrow(stepC, throwError),
//         done(),
//       ),
//       pipe(logError, recur()),
//     ),
//   )
```

The demo handlers are simple mocks: they randomly succeed or return an error. This isolates the tryCatch + invokeWithThrow pattern from domain-specific complexity.

## Test strategy

### TypeScript type tests

1. `tryCatch(body, recovery)` — recovery input type matches the throw payload type.
2. `tryCatch` output type matches both body and recovery output types.
3. `invokeWithThrow(handler, throwToken)` — extracts `TOut` from `Result<TOut, TError>`.
4. `throwError` token has type `TypedAction<TError, never>`.
5. Nested tryCatch — each throwError token has independent TError.

### TypeScript compilation tests

1. `tryCatch(body, recovery)` produces correct Handle/Perform AST.
2. Handler DAG: `GetField("payload") → recovery → Tag("Discard")`.
3. `invokeWithThrow` produces correct Chain/Branch AST.

### Rust engine tests (using existing substrate)

These are pre-existing from Phase 1. The tryCatch combinator produces Handle/Perform ASTs that the engine already handles. Specific patterns to validate:

1. Body throws → recovery runs → produces a value.
2. Body completes normally → recovery never runs.
3. Throw inside Chain → rest doesn't execute.
4. Throw inside All → other branches cancelled during teardown.
5. Nested tryCatch → inner throw caught by inner handler.

## Deliverables

1. `tryCatch()` TypeScript function in `libs/barnum/src/try-catch.ts`
2. `invokeWithThrow()` convenience combinator (same file)
3. Export from `libs/barnum/src/ast.ts` barrel
4. Type tests in `libs/barnum/tests/types.test.ts`
5. Compilation tests (AST structure)
6. Demo: `demos/retry-on-error/`
