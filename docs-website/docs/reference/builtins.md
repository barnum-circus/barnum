# Builtins

Barnum provides a set of built-in combinators for composing workflows. Everything is fully typed — TypeScript tracks input and output types through the entire pipeline.

## Composition

### `pipe(...actions)`

Sequence actions left-to-right. The output of each action becomes the input of the next.

```ts
pipe(listFiles, forEach(processFile).drop(), commit)
```

### `all(...actions)`

Run multiple actions concurrently on the same input. Collects outputs as a tuple.

```ts
all(analyzeStyle, analyzeLogic, analyzeSecurity)
// Input: string → Output: [StyleReport, LogicReport, SecurityReport]
```

### `chain(first, rest)`

Explicit two-action chain. Equivalent to `pipe(first, rest)`.

## Control flow

### `forEach(action)`

Apply an action to each element of an array, in parallel.

```ts
listFiles.forEach(processFile)
// Input: string[] → runs processFile on each string concurrently
```

### `branch(cases)`

Dispatch on a tagged union's `kind` field. Each variant maps to a different action.

```ts
classify.branch({
  NeedsRefactor: refactor,
  Clean: drop,
})
```

### `loop(bodyFn)`

Iterative loop. The body receives `recur` (restart the loop) and `done` (break out). Returns whatever `done` produces.

```ts
loop((recur, done) =>
  pipe(typeCheck, classifyErrors).branch({
    HasErrors: pipe(fix, recur),
    Clean: done,
  })
)
```

### `tryCatch(body, { catch: recovery })`

Error handling. If `body` throws, route to `recovery` instead.

```ts
tryCatch(riskyStep, {
  catch: fallbackStep,
})
```

### `race(...actions)`

Run multiple actions concurrently on the same input. The first to complete wins; others are cancelled.

### `withTimeout(ms, body)`

Race an action against a timeout. Returns `Result<TOut, void>` — `Ok` if the action completes, `Err` if it times out.

```ts
withTimeout(30_000, slowStep)
```

### `earlyReturn(bodyFn)`

Create a scope with an early exit. The body receives an `earlyReturn` token that can be used to break out.

### `recur(bodyFn)`

Create a restartable scope. The body receives a `restart` token.

## Data transformation

### `constant(value)`

Produce a fixed value, ignoring the pipeline input.

```ts
constant("hello")
// Input: anything → Output: "hello"
```

### `identity`

Pass input through unchanged.

### `drop`

Discard the pipeline value. Produces `never` (terminates the pipeline).

### `tag(kind)`

Wrap the input as a tagged union member: `{ kind, value: input }`.

```ts
tag("NeedsRefactor")
// Input: FileInfo → Output: { kind: "NeedsRefactor", value: FileInfo }
```

### `merge()`

Merge a tuple of objects into a single object.

```ts
all(getUser, getSettings).merge()
// Output: User & Settings
```

### `flatten()`

Flatten a nested array one level.

### `extractField(field)` / `.get(field)`

Extract a single field from an object.

### `extractIndex(index)`

Extract a single element from a tuple by index.

### `pick(...keys)`

Select named fields from an object.

### `range(start, end)`

Produce an integer array `[start, start+1, ..., end-1]`.

### `sleep()`

Delay for the specified number of milliseconds (input: number, output: void).

## Side effects

### `augment(action)`

Run an action, then merge its output back into the original input.

```ts
augment(computeHash)
// Input: { file: string } → Output: { file: string, hash: string }
```

### `tap(action)`

Run an action for side effects, then pass the original input through unchanged.

```ts
tap(logToFile)
```

### `dropResult(action)`

Run an action but discard its output, keeping the original input.

## Resource management

### `withResource(create, action, dispose)`

RAII-style resource management. Creates a resource, runs an action with it, then disposes.

```ts
withResource(createWorktree, doWork, cleanupWorktree)
```

## Variable binding

### `bind(bindings, body)`

Bind concurrent values as typed references (`VarRef`). Useful when multiple parts of a pipeline need to reference the same computed value.

```ts
bind([getConfig, getUser], ([configRef, userRef], input) =>
  pipe(processWithConfig(configRef), notifyUser(userRef))
)
```

### `bindInput(body)`

Bind the pipeline input as a `VarRef` for later reference deeper in the pipeline.

## Option and Result

Barnum includes `Option<T>` and `Result<TValue, TError>` tagged unions with full combinator suites.

### Option combinators

`Option.some()`, `Option.none()`, `Option.map(action)`, `Option.andThen(action)`, `Option.unwrapOr(default)`, `Option.flatten()`, `Option.filter(predicate)`, `Option.collect()`, `Option.isSome()`, `Option.isNone()`

### Result combinators

`Result.ok()`, `Result.err()`, `Result.map(action)`, `Result.mapErr(action)`, `Result.andThen(action)`, `Result.or(fallback)`, `Result.unwrapOr(default)`, `Result.flatten()`, `Result.toOption()`, `Result.isOk()`, `Result.isErr()`

## Handler definition

### `createHandler(definition, exportName?)`

Create a typed handler from an async function with optional Zod validators.

```ts
export const processFile = createHandler({
  inputValidator: z.string(),
  outputValidator: z.object({ status: z.string() }),
  handle: async ({ value: file }) => {
    // ...
    return { status: "done" };
  },
}, "processFile");
```

### `createHandlerWithConfig(definition, exportName?)`

Like `createHandler`, but also accepts a step-level config object for parameterizing handler behavior.

## Workflow execution

### `runPipeline(pipeline, input?)`

Run a pipeline to completion. Optionally provide an input value, which is prepended as a `constant` node.

```ts
runPipeline(
  listFiles.forEach(processFile).drop(),
);
```
