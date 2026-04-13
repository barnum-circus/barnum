# Timeout

`withTimeout` races a handler against a timer. If the handler finishes first, you get `Ok(result)`. If the timer fires first, you get `Err(void)`.

## Basic timeout

```ts
withTimeout(constant(5_000), longRunningStep)
```

The first argument is an action that produces the timeout duration in milliseconds. `constant(5_000)` is the simplest case — a fixed 5-second timeout.

The output is `Result<TOutput, void>`:
- `Ok(result)` if the handler completes in time
- `Err(void)` if the timeout fires

## Timeout with error conversion

From [`demos/retry-on-error/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/retry-on-error/run.ts):

```ts
withTimeout(constant(2_000), stepB.unwrapOr(throwError))
  .mapErr(constant("stepB: timed out"))
  .unwrapOr(throwError)
  .drop(),
```

1. `withTimeout(constant(2_000), ...)` — race `stepB` against a 2-second timer.
2. `.mapErr(constant("stepB: timed out"))` — replace the `Err(void)` with a descriptive string.
3. `.unwrapOr(throwError)` — on timeout, fire the enclosing `tryCatch`.

## Dynamic timeout

The timeout duration doesn't have to be constant. It can come from the pipeline:

```ts
pipe(
  getConfig,                           // { timeout: 10000, ... }
  withTimeout(
    extractField("timeout"),           // extract the timeout from the config
    extractField("payload").then(work) // extract the payload and process it
  ),
)
```

## How it works

`withTimeout` compiles to a `race` between the handler and a `sleep`. See [algebraic effect handlers](../architecture/algebraic-effect-handlers.md) for the compilation details.
