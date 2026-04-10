# Race + Timeout

## Scope

TypeScript-only. The Rust engine already has Handle/Perform, All (parallel execution), and teardown_body from Phase 1. This phase adds:

1. `race()` — TypeScript combinator (first branch to complete wins, losers cancelled)
2. `sleep()` — built-in handler (resolves after N milliseconds)
3. `withTimeout()` — sugar over `race(body, sleep)` that returns `Result<TOut, void>`
4. `invokeWithTimeout()` — convenience combinator combining `withTimeout` + `invokeWithThrow`
5. A demo workflow demonstrating timeout + retry patterns

No Rust changes. The engine's existing All + Handle + teardown is sufficient. If a sleep handler is still running when the other branch wins, the engine cancels it during teardown — this is standard Handle frame teardown behavior.

## Prerequisites

Phase 1 (Effect Substrate), Phase 3 (tryCatch — for invokeWithTimeout).

## race

### The problem

Run multiple actions concurrently. The first to complete wins. The losers are cancelled.

### How it compiles

```ts
// User writes:
race(actionA, actionB)

// Compiles to:
Handle(freshEffectId, raceHandler,
  All(
    Chain(actionA, Perform(freshEffectId)),
    Chain(actionB, Perform(freshEffectId)),
  ),
)
```

Each branch chains its result into a Perform. The first Perform to fire triggers the handler. The handler extracts the payload and produces Discard — the Handle frame tears down the body (including the un-completed All branch) and exits with the winner's result.

### Handler DAG

```ts
// Handler DAG: extract payload, tag as Discard
Chain(GetField("payload"), Tag("Discard"))
```

### TypeScript implementation

```ts
export function race<TIn, TOut>(
  ...actions: Pipeable<TIn, TOut>[]
): TypedAction<TIn, TOut> {
  const effectId = nextEffectId++;

  const raceHandler: Action = {
    kind: "Chain",
    first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", value: "payload" } } },
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Discard" } } },
  };

  const branches = actions.map((action) => ({
    kind: "Chain" as const,
    first: action as Action,
    rest: { kind: "Perform" as const, effect_id: effectId },
  }));

  return typedAction({
    kind: "Handle",
    effect_id: effectId,
    handler: raceHandler,
    body: { kind: "All", actions: branches },
  });
}
```

### Type safety

All branches must produce the same output type (since either could win):

```ts
function race<TIn, TOut>(...actions: Pipeable<TIn, TOut>[]): TypedAction<TIn, TOut>
```

## sleep

A built-in handler that resolves after a specified duration. This is a TypeScript handler (not a Rust-side Builtin) — it uses `setTimeout` to delay, then returns `void`.

```ts
// In libs/barnum/src/builtins.ts:
export const sleep = createHandlerWithConfig({
  stepConfigValidator: z.object({ ms: z.number() }),
  handle: async ({ stepConfig }) => {
    await new Promise((resolve) => setTimeout(resolve, stepConfig.ms));
  },
}, "sleep");
```

Usage: `sleep({ ms: 5000 })` → `TypedAction<never, void>` that resolves after 5 seconds.

Input is `never` — the timer doesn't consume pipeline data. The duration comes from the handler config (baked into the AST at build time).

When the engine cancels the sleep during race teardown, the worker subprocess is killed. The sleep never resolves. This is the standard cancellation mechanism.

## withTimeout

Sugar over `race(body, sleep)`. Returns `Result<TOut, void>` — Ok if the body completed, Err if the timeout fired. The Err payload is `void` because there's nothing meaningful to report beyond "it didn't finish."

```ts
export function withTimeout<TIn, TOut>(
  ms: number,
  body: Pipeable<TIn, TOut>,
): TypedAction<TIn, Result<TOut, void>> {
  return race(
    pipe(body, Result.ok()),     // body → tag as Ok
    pipe(sleep({ ms }), Result.err()),  // sleep → tag as Err
  );
}
```

Both branches produce `Result<TOut, void>`, satisfying race's homogeneous output requirement. The first to complete wins; the loser is cancelled.

### Usage

```ts
pipe(
  withTimeout(5000, longRunningAction),
).branch({
  Ok: processResult,
  Err: drop().then(constant("timed out")),
})
```

## invokeWithTimeout

Convenience combinator: run a handler with a timeout, throwing on timeout or handler error. Combines `withTimeout` + `invokeWithThrow`.

```ts
export function invokeWithTimeout<TIn, TOut, TError>(
  handler: Pipeable<TIn, Result<TOut, TError>>,
  ms: number,
  throwError: Pipeable<TError | void, never>,
): TypedAction<TIn, TOut> {
  // 1. withTimeout wraps the handler: Result<Result<TOut, TError>, void>
  // 2. Flatten: timeout Err(void) → throw; handler Err(TError) → throw; handler Ok(TOut) → pass through
  // Implementation: withTimeout(ms, handler) → branch Ok/Err
  //   Ok: the handler completed → Result<TOut, TError> → invokeWithThrow
  //   Err: timeout fired → void → throwError
  return typedAction(
    pipe(
      withTimeout(ms, handler),
      branch({
        Ok: invokeWithThrow(identity(), throwError),  // unwrap Result<TOut, TError>
        Err: throwError,  // timeout → throw void
      }),
    ) as Action,
  );
}
```

### Usage

```ts
tryCatch(
  (throwError) => invokeWithTimeout(riskyHandler, 5000, throwError),
  handleError,  // receives TError | void
)
```

## Demo: fallible action with timeout and retry

A workflow that runs a fallible action with a timeout, retrying on failure. The handler randomly: succeeds, takes too long (timeout), or returns an error.

```ts
// Pipeline:
//   loop(
//     tryCatch(
//       (throwError) => pipe(
//         invokeWithTimeout(unreliableAction, 5000, throwError),
//         done(),
//       ),
//       pipe(logError, recur()),
//     ),
//   )
```

The mock handler `unreliableAction` returns `Result<string, string>` and randomly:
1. Returns `Ok("success")` after a short delay
2. Sleeps longer than the timeout (gets cancelled)
3. Returns `Err("something went wrong")`

This demonstrates the full stack: race + sleep for timeouts, tryCatch for error handling, loop for retry, and the invokeWithTimeout convenience combinator.

## Test strategy

### TypeScript type tests

1. `race(a, b)` — both branches must have same output type.
2. `withTimeout(ms, body)` — returns `Result<TOut, void>`.
3. `invokeWithTimeout(handler, ms, throwError)` — extracts `TOut` from `Result<TOut, TError>`.

### TypeScript compilation tests

1. `race(a, b)` produces Handle(All(Chain(a, Perform), Chain(b, Perform))).
2. `withTimeout` produces race with Ok/Err tagging.
3. `sleep({ ms })` produces an Invoke with config.

### Rust engine tests (using existing substrate)

The race combinator produces Handle + All + Perform ASTs. Specific patterns to validate:

1. Two branches, first completes — second is cancelled.
2. Two branches, second completes — first is cancelled.
3. Losing branch has pending Invoke — task is cancelled during teardown.

## Deliverables

1. `race()` TypeScript function in `libs/barnum/src/race.ts`
2. `sleep()` handler in `libs/barnum/src/builtins.ts`
3. `withTimeout()` and `invokeWithTimeout()` (in `libs/barnum/src/race.ts` or `builtins.ts`)
4. Export from `libs/barnum/src/ast.ts` barrel
5. Type tests
6. Demo: `demos/timeout-retry/`
