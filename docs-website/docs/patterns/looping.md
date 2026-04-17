# Looping

`loop` repeats a body until it produces a `Break` signal. The body receives two tokens — `recur` (restart the loop) and `done` (exit the loop) — and routes to one of them via `branch`.

## Type-check-and-fix loop

From [`demos/convert-folder-to-ts/handlers/type-check-fix.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/convert-folder-to-ts/handlers/type-check-fix.ts):

```ts
export const typeCheckFix = loop((recur) =>
  typeCheck.then(classifyErrors).branch({
    HasErrors: forEach(fix).drop().then(recur),
    Clean: drop,
  }),
);
```

1. `typeCheck` runs the TypeScript compiler.
2. `classifyErrors` returns `{ kind: "HasErrors", value: string[] }` or `{ kind: "Clean" }`.
3. On `HasErrors`: fix each error in parallel, then `recur` to type-check again.
4. On `Clean`: `drop` exits the loop.

## Loop with retry

From [`demos/retry-on-error/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/retry-on-error/run.ts):

```ts
loop((recur, done) =>
  tryCatch(
    (throwError) =>
      stepA.mapErr(drop).unwrapOr(done).drop()
        .then(withTimeout(constant(2_000), stepB.unwrapOr(throwError))
          .mapErr(constant("stepB: timed out"))
          .unwrapOr(throwError)
          .drop())
        .then(stepC.unwrapOr(throwError).drop()),
    logError.then(recur),
  ),
)
```

The outer `loop` provides `recur` and `done`. Inside, `tryCatch` catches errors from the pipeline. On error, `logError` runs and `recur` restarts the loop. On catastrophic failure (`stepA`), `done` exits immediately.

## Adversarial review loop

From [`demos/identify-and-address-refactors/handlers/refactor.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/identify-and-address-refactors/handlers/refactor.ts):

```ts
loop((recur) =>
  judgeRefactor.then(classifyJudgment).branch({
    NeedsWork: applyFeedback.drop()
      .then(params.pick("worktreePath").then(typeCheckFix))
      .drop().then(recur),
    Approved: drop,
  }),
)
```

A judge evaluates the refactor. If it `NeedsWork`, feedback is applied, type errors are fixed, and the loop restarts for another review. If `Approved`, the loop exits.

## How it works

Under the hood, `loop` compiles to a `RestartHandle` with a `Branch`. See [algebraic effect handlers](../architecture/algebraic-effect-handlers.md) for the full compilation.
