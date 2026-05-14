# Sequential File Processing

Process a list of items one at a time, accumulating results — the workflow equivalent of `Array.reduce()` or a functional `fold`.

## Workflow

```ts
runPipeline(
  pipe(
    readFileList,
    initLoopState,
    loop<LoopResult, LoopState>((recur, done) =>
      pipe(
        processItem,
        advanceOrFinish,
      ).branch({
        Continue: recur,
        Done: done,
      })
    ),
    printFinalResults,
  ),
);
```

## This is a fold

If you've written `reduce` or `fold`, you already know this pattern:

```ts
// JavaScript reduce:
files.reduce((results, file) => [...results, process(file)], [])

// Barnum loop — same structure, different encoding:
// - init: readFileList → initLoopState (seed the accumulator)
// - step: processItem (apply f to current element + accumulator)
// - advance: advanceOrFinish (pop next element or return accumulator)
```

The `loop` combinator is Barnum's encoding of a left fold. The `LoopState` carries both the "remaining items" and the "accumulated results" — exactly like the accumulator in `reduce`. Each iteration processes one item, appends to results, and either recurs with the tail or breaks with the final value.

The key difference from `reduce`: each iteration is a full pipeline step. The "function" applied at each step can be an LLM call, a subprocess, a network request — anything a handler can do. The fold structure gives you sequential guarantees (each item sees the results of all prior items) without sacrificing the ability to do real work at each step.

## Stages

1. **Read file list** — load the list of items to process.
2. **Init loop state** — seed the accumulator: `{ current, rest, results: [] }`.
3. **Process item** — run the operation on `current`, append to `results`.
4. **Advance or finish** — if `rest` is non-empty, shift the next item into `current` and `Continue`. Otherwise `Done` with the final results.
5. **Print results** — consume the completed accumulator.

## Key points

- **Sequential by construction**: unlike `.iterate().map()` which fans out in parallel, `loop` processes one item at a time. Use this when order matters or when each step depends on prior results.
- **Accumulator pattern**: `LoopState` carries both the work queue and the running result. This is the standard left-fold shape — seed, step, done.
- **Tagged union for termination**: `advanceOrFinish` returns `Continue` or `Done`, which `branch` routes to `recur` or `done`. No boolean flags, no sentinel values — the type system enforces that every iteration either continues or terminates.
- **When to use parallel instead**: if items are independent (no item needs prior results), use `.iterate().map()` from the [codebase migration](./codebase-migration.md) pattern. Use this sequential fold when results accumulate or ordering constraints exist.
