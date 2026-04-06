---
image: /img/og/repertoire-hooks.png
---

# Side Effects

Run actions for side effects without changing the pipeline value. Use `tap` to log progress, write metrics, or perform cleanup while passing the original input through.

## Pattern

```ts
tap(sideEffect)
```

## Example

Log progress between pipeline steps:

```ts
export const logProgress = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.error(`[progress] Processing ${file}`);
  },
}, "logProgress");
```

```ts
runPipeline(
  listFiles.forEach(
    pipe(
      tap(logProgress),
      refactor,
      tap(logProgress),
      typeCheck,
    )
  ).drop(),
);
```

## Key points

- `tap` runs the action but discards its output, passing the original input through unchanged.
- Use `tap` for logging, metrics, notifications, or any side effect that shouldn't affect the pipeline value.
- `augment` is the related combinator that merges the action's output back into the input instead of discarding it.
