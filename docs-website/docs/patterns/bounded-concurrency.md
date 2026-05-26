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
// or Break when rest is empty.
export const advanceOrFinish = createHandler({
  inputValidator: batchStateSchema,
  handle: async ({ value: state }): Promise<LoopResult<BatchState, null>> => {
    if (state.rest.length === 0) {
      return { kind: "LoopResult.Break", value: null };
    }
    return {
      kind: "LoopResult.Continue",
      value: {
        batch: state.rest.slice(0, BATCH_SIZE),
        rest: state.rest.slice(BATCH_SIZE),
      },
    };
  },
}, "advanceOrFinish");
```

### Pipeline (fire-and-forget)

When each item handles its own side effects (writes to disk, sends a message, etc.) and you don't need the results back:

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
        Break: done,
      }),
    ),
  ),
);
```

After `.collect()`, `state` (the VarRef) discards the batch results and re-injects the captured batch state for `advanceOrFinish`.

### Pipeline (accumulating results)

When you need the combined results of all batches:

```ts
pipe(
  getItems,
  initBatches,
  loop<Result[], BatchState>((recur, done) =>
    bindInput<BatchState, never>((state) =>
      pipe(
        state.getField("batch").iterate().map(processItem).collect(),
        wrapInField("batchResults"),
        allObject({ state, batchResults: getField("batchResults") }),
        advanceOrFinish,  // appends batchResults to accumulated results
      ).branch({
        Continue: recur,
        Break: done,
      }),
    ),
  ),
);
```

In this variant, `advanceOrFinish` takes `{ state, batchResults }`, concatenates `batchResults` onto an accumulated array, and returns `Break` with the final results when `rest` is empty. The batch state grows to `{ batch, rest, results }`.

## How it works

1. `initBatches` splits the full list into `{ batch: first N, rest: remainder }`.
2. Each loop iteration:
   - `bindInput` captures the `BatchState` as a VarRef
   - `state.getField("batch").iterate().map().collect()` processes the current batch concurrently
   - The VarRef reaches back to the original state for `advanceOrFinish`
   - `advanceOrFinish` checks `rest` — if non-empty, slices the next batch and returns `Continue`; if empty, returns `Break`
3. `Continue` feeds the new `BatchState` back into the loop. `Break` terminates it.

The VarRef is essential — after `.collect()` produces batch results, you need to "reach back" to the original state to advance. Without `bindInput`, the state is lost after the map.

## Properties

- **Max N in flight.** Each loop iteration dispatches exactly `batch.length` (≤ N) items concurrently.
- **No tracking.** The framework handles concurrency within `.iterate().map()`. You only control the batch size.
- **Composable.** `processItem` can be any pipeline — retries, error handling, side effects all compose normally within the `.map()`.
