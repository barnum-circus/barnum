# Error Handling

`tryCatch` wraps a body in an error boundary. If the body calls `throwError`, execution is halted and the recovery handler runs instead.

## Basic tryCatch

```ts
tryCatch(
  (throwError) => pipe(
    riskyStep.unwrapOr(throwError).drop(),
    anotherStep,
  ),
  recovery,
)
```

If `riskyStep` returns `Err`, `.unwrapOr(throwError)` fires the error boundary. The body is torn down and `recovery` runs with the error value.

## tryCatch with retry

From [`demos/retry-on-error/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/retry-on-error/run.ts):

```ts
loop((recur, done) =>
  tryCatch(
    (throwError) =>
      pipe(
        stepA.mapErr(drop).unwrapOr(done).drop(),
        withTimeout(constant(2_000), stepB.unwrapOr(throwError))
          .mapErr(constant("stepB: timed out"))
          .unwrapOr(throwError)
          .drop(),
        stepC.unwrapOr(throwError).drop(),
      ),
    logError.then(recur),
  ),
)
```

Three error strategies in one pipeline:

- **`stepA`**: catastrophic — `.unwrapOr(done)` exits the entire loop, bypassing the catch block.
- **`stepB`**: retryable — `.unwrapOr(throwError)` fires the catch block. Also wrapped in `withTimeout`, so a timeout is converted to an error via `.mapErr(constant("stepB: timed out")).unwrapOr(throwError)`.
- **`stepC`**: retryable — same pattern as `stepB` without the timeout.

The catch handler `logError.then(recur)` logs the error, then `recur` restarts the `loop`.

## How it works

`tryCatch` compiles to the same `RestartHandle + Branch` substrate as `loop`. See [algebraic effect handlers](../architecture/algebraic-effect-handlers.md) for the compilation details.
