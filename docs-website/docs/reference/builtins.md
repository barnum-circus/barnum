# Builtins

Barnum provides typed combinators for composing workflows. TypeScript tracks input and output types through the entire pipeline via phantom types.

Every combinator is either a **standalone function** (imported from `barnum`) or a **postfix method** on `TypedAction` (chained via `.method()`), or both. The tables below note availability.

## Control flow

### `pipe(...actions)`

Sequential composition. The output of each action becomes the input of the next.

```ts
function pipe<T1, T2, T3>(
  a: Pipeable<T1, T2>,
  b: Pipeable<T2, T3>,
): TypedAction<T1, T3>
```

Up to 10 actions. Zero arguments returns identity; one argument wraps the action.

**Postfix:** `.then(next)` chains a single action.

```ts
listFiles.then(forEach(processFile)).then(commit)
// equivalent to pipe(listFiles, forEach(processFile), commit)
```

---

### `all(...actions)`

Run multiple actions concurrently on the same input. Collects outputs as a tuple.

```ts
function all<T1, A, B, C>(
  a: Pipeable<T1, A>,
  b: Pipeable<T1, B>,
  c: Pipeable<T1, C>,
): TypedAction<T1, [A, B, C]>
```

Up to 10 actions. Zero arguments returns `TypedAction<any, []>`.

**Postfix:** No.

```ts
all(analyzeStyle, analyzeLogic, analyzeSecurity)
// Input: string → Output: [StyleReport, LogicReport, SecurityReport]
```

---

### `chain(first, rest)`

Binary chain. Equivalent to `pipe(first, rest)`.

```ts
function chain<T1, T2, T3>(
  first: Pipeable<T1, T2>,
  rest: Pipeable<T2, T3>,
): TypedAction<T1, T3>
```

**Postfix:** `.then(rest)` is the postfix equivalent.

---

### `forEach(action)`

Apply an action to each element of an array, concurrently.

```ts
function forEach<TIn, TOut>(
  action: Pipeable<TIn, TOut>,
): TypedAction<TIn[], TOut[]>
```

**Postfix:** Yes — `.forEach(action)` on an action that outputs an array.

```ts
listFiles.forEach(processFile)
// Input: string[] → Output: ProcessResult[]
```

---

### `branch(cases)`

Dispatch on a tagged union's `kind` field. Each variant maps to a handler that receives the unwrapped `value`.

```ts
function branch<TCases extends Record<string, Action>>(
  cases: TCases,
): TypedAction<BranchInput<TCases>, ExtractOutput<TCases[keyof TCases]>>
```

All case handlers must produce the same output type.

**Postfix:** Yes — `.branch(cases)` on an action that outputs a tagged union.

```ts
classify.branch({
  NeedsRefactor: refactor,
  Clean: drop,
})
```

---

### `loop(bodyFn)`

Iterative loop. The body receives `recur` (restart with new input) and `done` (break with value). The body must never complete normally — it always calls `recur` or `done`.

```ts
function loop<TBreak, TIn>(
  bodyFn: (
    recur: TypedAction<TIn, never>,
    done: TypedAction<TBreak, never>,
  ) => Pipeable<TIn, never>,
): TypedAction<TIn, TBreak>
```

**Postfix:** No.

```ts
loop((recur, done) =>
  pipe(typeCheck, classifyErrors).branch({
    HasErrors: pipe(fix, recur),
    Clean: done,
  })
)
```

---

### `tryCatch(body, recovery)`

Type-level error handling. The body receives a `throwError` token; firing it routes to the recovery arm. Both arms must return the same type.

```ts
function tryCatch<TIn, TOut, TError>(
  body: (throwError: TypedAction<TError, never>) => Pipeable<TIn, TOut>,
  recovery: Pipeable<TError, TOut>,
): TypedAction<TIn, TOut>
```

Handles type-level errors only (not exceptions/panics).

**Postfix:** No.

```ts
tryCatch(
  (throwError) => pipe(riskyStep, Result.unwrapOr(throwError)),
  fallbackStep,
)
```

---

### `race(...actions)`

Run actions concurrently. First to complete wins; others are cancelled.

```ts
function race<TIn, TOut>(
  ...actions: Pipeable<TIn, TOut>[]
): TypedAction<TIn, TOut>
```

All actions must have identical input and output types.

**Postfix:** No.

---

### `withTimeout(ms, body)`

Race an action against a timer. Returns `Result<TOut, void>`.

```ts
function withTimeout<TIn, TOut>(
  ms: Pipeable<TIn, number>,
  body: Pipeable<TIn, TOut>,
): TypedAction<TIn, Result<TOut, void>>
```

`Ok(value)` if body completes, `Err(void)` on timeout.

**Postfix:** No.

```ts
withTimeout(constant(30_000), slowStep)
```

---

### `earlyReturn(bodyFn)`

Create a scope with an early exit. The body receives an `earlyReturn` token. Output type is the union of normal completion and early return.

```ts
function earlyReturn<TEarlyReturn, TIn, TOut>(
  bodyFn: (
    earlyReturn: TypedAction<TEarlyReturn, never>,
  ) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TEarlyReturn | TOut>
```

**Postfix:** No.

---

### `recur(bodyFn)`

Restartable scope. The body receives a `restart` token that re-executes the body from the beginning with new input.

```ts
function recur<TIn, TOut>(
  bodyFn: (
    restart: TypedAction<TIn, never>,
  ) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut>
```

**Postfix:** No.

---

### `sleep()`

Delay for the number of milliseconds specified by the input. Cancellable during race teardown.

```ts
function sleep(): TypedAction<number, void>
```

**Postfix:** No.

---

### `bind(bindings, body)`

Bind concurrent values as typed references (`VarRef`). All bindings are evaluated concurrently; the body receives an array of typed references that can be dereferenced anywhere in the pipeline.

```ts
function bind<TBindings extends Action[], TOut>(
  bindings: [...TBindings],
  body: (vars: InferVarRefs<TBindings>) => BodyResult<TOut>,
): TypedAction<ExtractInput<TBindings[number]>, TOut>
```

**Postfix:** No.

```ts
bind([getConfig, getUser], ([configRef, userRef]) =>
  pipe(processWithConfig(configRef), notifyUser(userRef))
)
```

---

### `bindInput(body)`

Capture the pipeline input as a `VarRef` for later reference deeper in the pipeline.

```ts
function bindInput<TIn, TOut>(
  body: (input: VarRef<TIn>) => BodyResult<TOut>,
): TypedAction<TIn, TOut>
```

Sugar for `bind([identity], ([input]) => pipe(drop, body(input)))`.

**Postfix:** No.

---

### `withResource({ create, action, dispose })`

RAII-style resource management. Creates a resource, merges it with the input, runs an action, then disposes.

```ts
function withResource<
  TIn extends Record<string, unknown>,
  TResource extends Record<string, unknown>,
  TOut,
>(args: {
  create: Pipeable<TIn, TResource>,
  action: Pipeable<TResource & TIn, TOut>,
  dispose: Pipeable<TResource, unknown>,
}): TypedAction<TIn, TOut>
```

**Postfix:** No.

```ts
withResource({
  create: createWorktree,
  action: doWork,
  dispose: cleanupWorktree,
})
```

---

### `dropResult(action)`

Run an action for side effects, discard its output. Returns `never` (terminates the pipeline — typically used before `drop` or another action).

```ts
function dropResult<TInput, TOutput>(
  action: Pipeable<TInput, TOutput>,
): TypedAction<TInput, never>
```

**Postfix:** No.

---

## Data manipulation

### `constant(value)`

Produce a fixed value, ignoring the pipeline input.

```ts
function constant<TValue>(value: TValue): TypedAction<any, TValue>
```

**Postfix:** No.

```ts
constant("hello")
// Input: anything → Output: "hello"
```

---

### `identity`

Pass input through unchanged. A value, not a function.

```ts
const identity: TypedAction<any, any>
```

**Postfix:** No.

---

### `drop`

Discard the pipeline value. Produces `never`. A value, not a function.

```ts
const drop: TypedAction<any, never>
```

**Postfix:** Yes — `.drop()`.

```ts
sideEffect.drop()
// equivalent to pipe(sideEffect, drop)
```

---

### `tag(kind)`

Wrap the input as a tagged union member: `{ kind, value: input }`.

```ts
function tag<
  TDef extends Record<string, unknown>,
  TKind extends keyof TDef & string,
>(kind: TKind): TypedAction<TDef[TKind], TaggedUnion<TDef>>
```

**Postfix:** Yes — `.tag(kind)`.

```ts
tag<{ NeedsRefactor: FileInfo; Clean: FileInfo }, "NeedsRefactor">("NeedsRefactor")
// Input: FileInfo → Output: TaggedUnion<{ NeedsRefactor: FileInfo; Clean: FileInfo }>
```

---

### `merge()`

Merge a tuple of objects into a single object via intersection.

```ts
function merge<
  TObjects extends Record<string, unknown>[],
>(): TypedAction<TObjects, UnionToIntersection<TObjects[number]>>
```

**Postfix:** Yes — `.merge()`.

```ts
all(getUser, getSettings).merge()
// Output: User & Settings
```

---

### `flatten()`

Flatten a nested array one level.

```ts
function flatten<TElement>(): TypedAction<TElement[][], TElement[]>
```

**Postfix:** Yes — `.flatten()`.

---

### `getField(field)` / `.getField(field)`

Extract a single field from an object.

```ts
function getField<
  TObj extends Record<string, unknown>,
  TField extends keyof TObj & string,
>(field: TField): TypedAction<TObj, TObj[TField]>
```

**Postfix:** Yes — `.getField(field)`.

```ts
getUserProfile.getField("email")
// equivalent to pipe(getUserProfile, getField("email"))
```

---

### `getIndex(index)`

Extract a single element from a tuple by index.

```ts
function getIndex<TTuple extends unknown[], TIndex extends number>(
  index: TIndex,
): TypedAction<TTuple, TTuple[TIndex]>
```

**Postfix:** No.

---

### `pick(...keys)`

Select named fields from an object.

```ts
function pick<
  TObj extends Record<string, unknown>,
  TKeys extends (keyof TObj & string)[],
>(...keys: TKeys): TypedAction<TObj, Pick<TObj, TKeys[number]>>
```

**Postfix:** Yes — `.pick(...keys)`.

```ts
getUserProfile.pick("name", "email")
// equivalent to pipe(getUserProfile, pick("name", "email"))
```

---

### `range(start, end)`

Produce an integer array `[start, start+1, ..., end-1]`. Computed at AST build time (emits a `constant` node).

```ts
function range(start: number, end: number): TypedAction<any, number[]>
```

**Postfix:** No.

---

### `augment(action)`

Run an action, then merge its output back into the original input.

```ts
function augment<
  TInput extends Record<string, unknown>,
  TOutput extends Record<string, unknown>,
>(action: Pipeable<TInput, TOutput>): TypedAction<TInput, TInput & TOutput>
```

**Postfix:** Yes — `.augment()` (no arguments; wraps the preceding action).

```ts
augment(computeHash)
// Input: { file: string } → Output: { file: string, hash: string }
```

---

### `tap(action)`

Run an action for side effects, then pass the original input through unchanged.

```ts
function tap<TInput extends Record<string, unknown>>(
  action: Pipeable<TInput, any>,
): TypedAction<TInput, TInput>
```

**Postfix:** No.

```ts
tap(logToFile)
// Input: T → Output: T (logToFile runs but output is discarded)
```

---

## `Option<T>`

`Option<T>` is a tagged union: `TaggedUnion<{ Some: T; None: void }>`.

All combinators desugar to `branch` + builtins at the AST level.

| Combinator | Type | Description |
|---|---|---|
| `Option.some()` | `T → Option<T>` | Wrap as Some |
| `Option.none()` | `void → Option<T>` | Produce None |
| `Option.map(action)` | `Option<T> → Option<U>` | Transform Some value |
| `Option.andThen(action)` | `Option<T> → Option<U>` | Monadic bind (flatMap) |
| `Option.unwrapOr(default)` | `Option<T> → T` | Extract Some or compute default |
| `Option.flatten()` | `Option<Option<T>> → Option<T>` | Unwrap nested Option |
| `Option.filter(predicate)` | `Option<T> → Option<T>` | Keep if predicate returns Some |
| `Option.collect()` | `Option<T>[] → T[]` | Collect Some values, discard Nones |
| `Option.isSome()` | `Option<T> → boolean` | Test for Some |
| `Option.isNone()` | `Option<T> → boolean` | Test for None |

**Postfix:** `.mapOption(action)` transforms the Some value of an `Option` output.

---

## `Result<TValue, TError>`

`Result<TValue, TError>` is a tagged union: `TaggedUnion<{ Ok: TValue; Err: TError }>`.

All combinators desugar to `branch` + builtins at the AST level.

| Combinator | Type | Description |
|---|---|---|
| `Result.ok()` | `TValue → Result<TValue, TError>` | Wrap as Ok |
| `Result.err()` | `TError → Result<TValue, TError>` | Wrap as Err |
| `Result.map(action)` | `Result<V, E> → Result<U, E>` | Transform Ok value |
| `Result.mapErr(action)` | `Result<V, E> → Result<V, E2>` | Transform Err value |
| `Result.andThen(action)` | `Result<V, E> → Result<U, E>` | Monadic bind on Ok |
| `Result.or(fallback)` | `Result<V, E> → Result<V, E2>` | Fallback on Err |
| `Result.and(other)` | `Result<V, E> → Result<U, E>` | Replace Ok with other |
| `Result.unwrapOr(default)` | `Result<V, E> → V` | Extract Ok or compute default |
| `Result.flatten()` | `Result<Result<V, E>, E> → Result<V, E>` | Unwrap nested Result |
| `Result.toOption()` | `Result<V, E> → Option<V>` | Ok→Some, Err→None |
| `Result.toOptionErr()` | `Result<V, E> → Option<E>` | Err→Some, Ok→None |
| `Result.transpose()` | `Result<Option<V>, E> → Option<Result<V, E>>` | Swap Result/Option nesting |
| `Result.isOk()` | `Result<V, E> → boolean` | Test for Ok |
| `Result.isErr()` | `Result<V, E> → boolean` | Test for Err |

**Postfix:** `.mapErr(action)` transforms the Err value; `.unwrapOr(default)` extracts Ok or applies default to Err.

---

## Handler definition

### `createHandler(definition, exportName?)`

Create a typed handler from an async function with optional Zod validators.

```ts
function createHandler<TValue, TOutput>(
  definition: {
    inputValidator?: z.ZodType<TValue>;
    outputValidator?: z.ZodType<TOutput>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): TypedAction<TValue, TOutput>
```

The returned action serializes to an `Invoke` node. At runtime, the Rust scheduler spawns a TypeScript worker subprocess that calls `handle`.

```ts
export const processFile = createHandler({
  inputValidator: z.string(),
  outputValidator: z.object({ status: z.string() }),
  handle: async ({ value: filePath }) => {
    // ...
    return { status: "done" };
  },
}, "processFile");
```

---

### `createHandlerWithConfig(definition, exportName?)`

Like `createHandler`, but also accepts step-level configuration.

```ts
function createHandlerWithConfig<TValue, TOutput, TStepConfig>(
  definition: {
    inputValidator?: z.ZodType<TValue>;
    outputValidator?: z.ZodType<TOutput>;
    stepConfigValidator?: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): TypedAction<TValue, TOutput>
```

---

## Workflow execution

### `runPipeline(pipeline, input?)`

Run a pipeline to completion. Optionally provide an input value.

```ts
async function runPipeline(
  pipeline: Action,
  input?: unknown,
): Promise<void>
```

This is the main entry point. It serializes the pipeline AST to JSON, resolves the Rust binary, and spawns `barnum run --config <json>`.

```ts
await runPipeline(
  pipe(listFiles, forEach(processFile), commit),
);
```

---

## Postfix method summary

These methods are available on any `TypedAction` via dot-chaining:

| Method | Standalone equivalent | Notes |
|---|---|---|
| `.then(next)` | `chain(a, next)` | |
| `.forEach(action)` | `chain(a, forEach(action))` | Requires array output |
| `.branch(cases)` | `chain(a, branch(cases))` | Requires tagged union output |
| `.drop()` | `chain(a, drop)` | |
| `.tag(kind)` | `chain(a, tag(kind))` | |
| `.merge()` | `chain(a, merge())` | Requires tuple-of-objects output |
| `.flatten()` | `chain(a, flatten())` | Requires nested array output |
| `.getField(field)` | `chain(a, getField(field))` | |
| `.pick(...keys)` | `chain(a, pick(...keys))` | |
| `.augment()` | `augment(a)` | Merges output back into input |
| `.mapOption(action)` | `chain(a, Option.map(action))` | Requires `Option` output |
| `.mapErr(action)` | `chain(a, Result.mapErr(action))` | Requires `Result` output |
| `.unwrapOr(default)` | `chain(a, Result.unwrapOr(default))` | Requires `Result` output |
