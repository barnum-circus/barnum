# Postfix Methods

Every `TypedAction` has postfix methods (`.then()`, `.iterate()`, `.map()`, `.branch()`, etc.) that enable a fluent chaining API. These methods exist at the type level for the TypeScript compiler and as non-enumerable properties at runtime — invisible to `JSON.stringify()`.

The interesting part is how TypeScript's `this` parameter interacts with phantom types. Postfix methods use `this` in three distinct ways: as AST context (building `Chain(this, rest)` nodes), as a type constraint (gating availability based on the output type), and as a type source (reading `In` from the phantom fields to produce intersection types).

## Attachment: non-enumerable, shared closures

`typedAction()` attaches all methods via `Object.defineProperties`:

```ts
export function typedAction<In, Out, Refs extends string = never>(
  action: Action,
): TypedAction<In, Out, Refs> {
  if (!("then" in action)) {
    Object.defineProperties(action, {
      then: { value: thenMethod, configurable: true },
      iterate: { value: iterateMethod, configurable: true },
      map: { value: mapMethod, configurable: true },
      flatMap: { value: flatMapMethod, configurable: true },
      filter: { value: filterMethod, configurable: true },
      collect: { value: collectMethod, configurable: true },
      branch: { value: branchMethod, configurable: true },
      // ... more
    });
  }
  return action as TypedAction<In, Out, Refs>;
}
```

Properties are non-enumerable by default when created via `Object.defineProperties`. This means:

- `JSON.stringify()` skips them — the serialized AST is clean JSON.
- `toEqual()` in tests ignores them — two actions with the same structure are equal regardless of methods.
- `"then" in action` detects whether methods are already attached, preventing double-attachment.

The method implementations are module-level functions, not closures created per instance. Every `TypedAction` shares the same `thenMethod`, `iterateMethod`, `mapMethod`, etc. The `this` binding is provided by the call site.

## `this` as AST context

The simplest use of `this` is building `Chain(this, rest)` nodes. Most postfix methods follow this pattern:

```ts
function thenMethod(this: TypedAction, next: Action): TypedAction {
  return typedAction({ kind: "Chain", first: this, rest: next as Action });
}

function dropMethod(this: TypedAction): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Drop" } } },
  });
}
```

`a.then(b)` constructs `Chain(a, b)`. `a.drop()` constructs `Chain(a, Drop)`. The `this` parameter gives the method access to the Action object it was called on — the `first` half of the chain.

At the type level, `In` is preserved from `this` and `Out` comes from the chained action:

```ts
then<TNext>(next: Pipeable<Out, TNext>): TypedAction<In, TNext, Refs>
```

`In` flows through unchanged. `Out` becomes `TNext`. The method chains the type parameters exactly as `chain(a, b)` would.

## `this` as type constraint

Three methods use TypeScript's `this` parameter to restrict when the method is callable:

### Iterator methods — require `Iterator<T>` output

The Iterator methods (`.map()`, `.flatMap()`, `.filter()`, `.collect()`) use `this` constraints to restrict availability to `Iterator<T>` output:

```ts
map<TIn, TElement, TOut, TRefs extends string>(
  this: TypedAction<TIn, Iterator<TElement>, TRefs>,
  action: Pipeable<TElement, TOut>,
): TypedAction<TIn, Iterator<TOut>, TRefs>

flatMap<TIn, TElement, TOut, TRefs extends string>(
  this: TypedAction<TIn, Iterator<TElement>, TRefs>,
  action: Pipeable<TElement, TOut[]>,
): TypedAction<TIn, Iterator<TOut>, TRefs>

filter<TIn, TElement, TRefs extends string>(
  this: TypedAction<TIn, Iterator<TElement>, TRefs>,
  predicate: Pipeable<TElement, boolean>,
): TypedAction<TIn, Iterator<TElement>, TRefs>

collect<TIn, TElement, TRefs extends string>(
  this: TypedAction<TIn, Iterator<TElement>, TRefs>,
): TypedAction<TIn, TElement[], TRefs>
```

The `this: TypedAction<TIn, Iterator<TElement>, TRefs>` constraint means these methods only compile when the output type is `Iterator<T>`. TypeScript infers `TElement` from the Iterator's element type:

```ts
listFiles                        // TypedAction<void, string[]>
  .iterate()                     // TypedAction<void, Iterator<string>>
  .map(processFile)              // ✓ — TElement = string, Out = Iterator<ProcessResult>
  .collect()                     // TypedAction<void, ProcessResult[]>
```

### `.iterate()` — requires array, Option, or Result output

`.iterate()` has three overloads gated by `this` constraints:

```ts
iterate<TIn, TElement>(this: TypedAction<TIn, TElement[]>): TypedAction<TIn, Iterator<TElement>>
iterate<TIn, TElement>(this: TypedAction<TIn, Option<TElement>>): TypedAction<TIn, Iterator<TElement>>
iterate<TIn, TElement, TError>(this: TypedAction<TIn, Result<TElement, TError>>): TypedAction<TIn, Iterator<TElement>>
```

It bridges from array/Option/Result into the Iterator world via `branchFamily` dispatch.

### `forEach` (low-level) — requires array output

`forEach` is the low-level AST primitive that Iterator methods build on. It still exists as a postfix method, but prefer `.iterate().map(f).collect()` for user-facing code:

```ts
forEach<TIn, TElement, TNext, TRefs extends string>(
  this: TypedAction<TIn, TElement[], TRefs>,
  action: Pipeable<TElement, TNext>,
): TypedAction<TIn, TNext[], TRefs>
```

The `this: TypedAction<TIn, TElement[], TRefs>` constraint means `.forEach()` only compiles when the output type is an array. TypeScript infers `TElement` from the array element type.

The constraint also **re-binds `In`** as `TIn`. The `this` parameter's `TIn` replaces the outer `In` — TypeScript unifies them during overload resolution, extracting the phantom type from the concrete instance.

### `mapOption` — requires Option output

```ts
mapOption<TIn, T, U, TRefs extends string>(
  this: TypedAction<TIn, Option<T>, TRefs>,
  action: Pipeable<T, U>,
): TypedAction<TIn, Option<U>, TRefs>
```

Only callable when `Out` is `Option<T>`. TypeScript unifies the phantom `__phantom_out` with `() => Option<T>` to infer `T`, making it available as the action's input type.

### `mapErr` and `unwrapOr` — requires Result output

```ts
mapErr<TIn, TValue, TError, TErrorOut>(
  this: TypedAction<TIn, Result<TValue, TError>, any>,
  action: Pipeable<TError, TErrorOut>,
): TypedAction<TIn, Result<TValue, TErrorOut>, Refs>
```

```ts
unwrapOr<TIn, TValue, TError>(
  this: TypedAction<TIn, Result<TValue, TError>, any>,
  defaultAction: CaseHandler<TError, TValue>,
): TypedAction<TIn, TValue, Refs>
```

Both constrain `Out` to `Result<TValue, TError>` and extract `TValue` and `TError` from the phantom fields.

The `Refs` position uses `any` instead of a type parameter. Without this, when `Refs = never` (the common case), TypeScript falls back to the constraint bound `string`, which breaks unification. Using `any` suppresses that fallback. The return type reads `Refs` from the enclosing `TypedAction` type directly (not from the `this` constraint), preserving the original refs tracking.

## `this` as type source: `.augment()`

`.augment()` is the method that most directly exploits phantom type access through `this`:

```ts
// Type signature
augment(): TypedAction<In, In & Out, Refs>
```

The return type is `In & Out` — an intersection of the action's input and output. This requires knowing `In`, which is only available from the `TypedAction`'s phantom fields.

The implementation constructs `All(this, Identity) → Merge`:

```ts
function augmentMethod(this: TypedAction): TypedAction {
  return typedAction({
    kind: "Chain",
    first: {
      kind: "All",
      actions: [this as Action, { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Identity" } } }],
    },
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Merge" } } },
  });
}
```

At runtime: `All` runs the sub-pipeline and `Identity` in parallel on the same input. `Identity` passes the input through unchanged. `Merge` combines `[Out, In]` into `In & Out`.

The standalone `augment(action)` function in `builtins.ts` does the same thing at the AST level, but the postfix form is more natural in pipelines:

```ts
// Standalone: augment wraps the action
augment(pick("file").then(computeHash))

// Postfix: augment follows the sub-pipeline
pick("file").then(computeHash).augment()
```

Both produce identical AST nodes.

## CaseHandler: covariant output for throw tokens

`unwrapOr` accepts `CaseHandler<TError, TValue>` rather than `Pipeable<TError, TValue>` for its `defaultAction`:

```ts
type CaseHandler<TIn, TOut, TRefs extends string = never> = Action & {
  __phantom_in?: (input: TIn) => void;  // contravariant
  __phantom_out?: () => TOut;            // covariant only — no __phantom_out_check
};
```

`CaseHandler` omits the contravariant `__phantom_out_check` that `TypedAction` has. This makes the output **covariant only** instead of invariant:

```ts
// With invariant output (TypedAction):
//   TypedAction<string, never> is NOT assignable to TypedAction<string, number>
//   because (output: number) => void is not assignable to (output: never) => void

// With covariant output (CaseHandler):
//   CaseHandler<string, never> IS assignable to CaseHandler<string, number>
//   because () => never is assignable to () => number
```

This matters for throw tokens. `throwError` from `tryCatch` has type `TypedAction<TError, never>` — it fires an effect and never returns. With covariant output, `never` (the bottom type) is assignable to any `TValue`, so throw tokens work as `unwrapOr` defaults:

```ts
tryCatch(
  (throwError) =>
    riskyStep.unwrapOr(throwError),  // throwError: TypedAction<Error, never>
                                     // CaseHandler<Error, string> ← ✓ never <: string
  recovery,
)
```

## Type transformations

Each postfix method transforms the type signature in a specific way:

| Method | `In` | `Out` | Notes |
|--------|------|-------|-------|
| `.then(b)` | preserved | becomes `b`'s output | |
| `.iterate()` | preserved | `T[]` → `Iterator<T>`, `Option<T>` → `Iterator<T>`, `Result<T,E>` → `Iterator<T>` | `this` constraint gates on array/Option/Result |
| `.map(b)` | preserved | `Iterator<T>` → `Iterator<U>` (also Option, Result) | `this` constraint extracts element type |
| `.flatMap(b)` | preserved | `Iterator<T>` → `Iterator<U>` | `b` returns any IntoIterator type |
| `.filter(pred)` | preserved | `Iterator<T>` → `Iterator<T>` | pred returns boolean |
| `.collect()` | preserved | `Iterator<T>` → `T[]` | unwraps Iterator |
| `.forEach(b)` | preserved | `TElement[]` → `TNext[]` | low-level; prefer `.iterate().map(b).collect()` |
| `.branch(cases)` | preserved | union of case outputs | |
| `.drop()` | preserved | `never` | |
| `.tag(kind)` | preserved | `TaggedUnion<TDef>` | |
| `.getField(field)` | preserved | `Out[TField]` | |
| `.pick(...keys)` | preserved | `Pick<Out, TKeys>` | |
| `.flatten()` | preserved | `T[][]` → `T[]` | conditional type |
| `.merge()` | preserved | `MergeTuple<Out>` | |
| `.augment()` | preserved | `In & Out` | reads `In` from phantom fields |
| `.mapOption(b)` | re-bound via `this` | `Option<T>` → `Option<U>` | `this` constraint extracts `T` |
| `.mapErr(b)` | re-bound via `this` | `Result<V, E>` → `Result<V, E2>` | `this` constraint extracts `V`, `E` |
| `.unwrapOr(b)` | re-bound via `this` | `Result<V, E>` → `V` | covariant output via `CaseHandler` |

Every method preserves `In` (or re-binds it identically via `this` constraints). `Out` is always transformed. `Refs` is always preserved.
