# Bounded Concurrency

`.iterate().map()` dispatches all elements concurrently. When the list is large and each element is expensive (LLM calls, network requests), you need bounded concurrency — process at most N items at a time.

## The pattern

A `{ batch: T[], rest: T[] }` structure holds the current batch (up to N items) and the remaining work. A loop processes one batch concurrently, then advances to the next.

### Data shape

```ts
const BATCH_SIZE = 5;

const batchStateSchema = z.object({
  batch: z.array(itemSchema),
  rest: z.array(itemSchema),
});

type BatchState = z.infer<typeof batchStateSchema>;
```

### Handlers

```ts
// Initialize the first batch from a raw array.
export const initBatches = createHandler({
  inputValidator: z.array(itemSchema),
  outputValidator: batchStateSchema,
  handle: async ({ value: items }): Promise<BatchState> => ({
    batch: items.slice(0, BATCH_SIZE),
    rest: items.slice(BATCH_SIZE),
  }),
}, "initBatches");

// Advance to the next batch. Returns Continue with a new batch state,
// or Done when rest is empty.
export const advanceOrFinish = createHandler({
  inputValidator: batchStateSchema,
  outputValidator: taggedUnionSchema("Advance", {
    Continue: batchStateSchema,
    Done: z.null(),
  }),
  handle: async ({ value: state }): Promise<Advance> => {
    if (state.rest.length === 0) {
      return { kind: "Advance.Done", value: null };
    }
    return {
      kind: "Advance.Continue",
      value: {
        batch: state.rest.slice(0, BATCH_SIZE),
        rest: state.rest.slice(BATCH_SIZE),
      },
    };
  },
}, "advanceOrFinish");
```

### Pipeline

```ts
pipe(
  getItems,
  initBatches,
  loop<null, BatchState>((recur, done) =>
    bindInput<BatchState, never>((state) =>
      pipe(
        state.getField("batch").iterate().map(processItem).collect(),
        state,
        advanceOrFinish,
      ).branch({
        Continue: recur,
        Done: done,
      }),
    ),
  ),
);
```

## How it works

1. `initBatches` splits the full list into `{ batch: first N, rest: remainder }`.
2. Each loop iteration:
   - `bindInput` captures the `BatchState` as a VarRef
   - `state.getField("batch").iterate().map().collect()` processes the current batch concurrently
   - `state` (the VarRef) discards the collect result and re-injects the captured batch state
   - `advanceOrFinish` checks `rest` — if non-empty, slices the next batch and returns `Continue`; if empty, returns `Done`
3. `Continue` feeds the new `BatchState` back into the loop. `Done` terminates it.

The VarRef is essential here — after `.collect()` produces the batch results, you need to "reach back" to the original state to advance. Without `bindInput`, the state is lost after the map.

## Properties

- **Max N in flight.** Each loop iteration dispatches exactly `batch.length` (≤ N) items concurrently.
- **No tracking.** The framework handles concurrency within `.iterate().map()`. You only control the batch size.
- **Composable.** `processItem` can be any pipeline — retries, error handling, side effects all compose normally within the `.map()`.
