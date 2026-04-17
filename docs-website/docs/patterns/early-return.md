# Early Return

`earlyReturn` creates a scope that can be exited early. The body receives an `exit` token — calling it immediately returns a value from the scope, skipping any remaining steps.

## Basic early return

```ts
earlyReturn((exit) =>
  validate.branch({
    Invalid: exit,             // return early with the error
    Valid: processAndContinue,  // normal path
  }),
)
```

If `validate` returns `Invalid`, `exit` fires and the scope returns immediately with the error value. `processAndContinue` never runs.

## Early return vs. tryCatch

Both can exit a scope early, but they serve different purposes:

- **`tryCatch`** is for errors — the catch handler transforms or recovers from the failure.
- **`earlyReturn`** is for short-circuiting — the exit value is the final result, with no recovery step.

```ts
// tryCatch: catch block handles the error
tryCatch(
  (throwError) => body,
  recovery, // ← runs on error
)

// earlyReturn: exit value IS the result
earlyReturn((exit) => body)
// ← no recovery, exit value passes through
```

## How it works

`earlyReturn` compiles to the same `RestartHandle + Branch` substrate as `loop` and `tryCatch`. The Break arm runs `Identity` — the exit value passes through unchanged. See [algebraic effect handlers](../architecture/algebraic-effect-handlers.md) for the compilation details.
