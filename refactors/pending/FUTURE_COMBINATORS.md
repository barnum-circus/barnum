# Future combinators and builtin improvements

## extractIndex — tuple destructuring

Extract a single element from a JSON array by index. The array counterpart to `extractField`.

```ts
function extractIndex<TTuple extends unknown[], TIndex extends number>(
  index: TIndex,
): TypedAction<TTuple, TTuple[TIndex]>
```

Rust builtin: `ExtractIndex { value: usize }`. Implementation: `input.as_array()?[index].clone()`.

Primary motivation: enables tuple-based `withResource` redesign where `all(create, identity())` produces `[TResource, TIn]` and we need to extract elements by position.

## extractField should return Option, not panic

Currently, `ExtractField` returns `null` if the field is missing. This silently produces bad data. With option types, it should return `Option<T>`:

```ts
// Current: { key: T } → T (or null if missing — silent failure)
function extractField<K>(field: K): TypedAction<{ [k in K]: T }, T>

// Proposed: { key?: T } → Option<T>
function extractField<K>(field: K): TypedAction<Record<string, unknown>, Option<T>>
```

The caller must then handle the `None` case explicitly via `branch`. This makes missing fields visible instead of silently propagating nulls.

**Breaking change.** Existing uses of `extractField` would need `unwrapOr` or a `branch` to handle `None`. Consider adding `extractFieldUnsafe` for the current behavior, or a separate `tryExtractField` for the option-returning version.

## unwrap — extract value from Option or panic

```ts
function unwrap<T>(): TypedAction<Option<T>, T>
```

Desugars to `branch({ Some: extractField("value"), None: panic("unwrap on None") })`. Useful when the caller is certain the value exists and wants to fail fast rather than handle `None`.

Requires a `panic` builtin or mechanism for hard failure.

## mapOption — transform the value inside an Option

```ts
function mapOption<T, U>(
  action: TypedAction<T, U>,
): TypedAction<Option<T>, Option<U>>
```

Desugars to `branch({ Some: pipe(extractField("value"), action, some()), None: none() })`.

## collectSome — filter an array of Options to just the Some values

```ts
function collectSome<T>(): TypedAction<Option<T>[], T[]>
```

New Rust builtin. Iterates the array, extracts `value` from `Some` variants, discards `None`.

Used by `filterMap`: `pipe(forEach(action), collectSome())`.

## pick — extract multiple fields into a new object

```ts
function pick<TObj, K extends keyof TObj>(...keys: K[]): TypedAction<TObj, Pick<TObj, K>>
```

Useful for narrowing a wide context to the specific fields a handler needs, without a custom handler for data shaping.

## withResource redesign (requires extractIndex)

See RAII_RESOURCE_MANAGEMENT.md. The proposed tuple-based design:

```ts
withResource({
  create: TypedAction<TIn, TResource>,
  action: TypedAction<[TResource, TIn], TOut>,
  dispose: TypedAction<TResource, any>,
}): TypedAction<TIn, TOut>
```

Implementation with `extractIndex`:

```
TIn
→ all(create, identity())           → [TResource, TIn]
→ all(action, extractIndex(0))      → [TOut, TResource]
→ all(extractIndex(0), chain(extractIndex(1), dispose))  → [TOut, void]
→ extractIndex(0)                        → TOut
```

Dispose runs after action completes (sequential via chain). The overall combinator returns `TOut` (not `never` like the current version).
