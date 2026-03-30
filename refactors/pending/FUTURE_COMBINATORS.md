# Future combinators and builtin improvements

## Status

Most items from the original doc have been implemented. This doc tracks what remains.

### Implemented

- **extractIndex** — `ExtractIndex { value: usize }` builtin. Used internally by `withResource`.
- **pick** — `Pick { value: string[] }` builtin. Prefix `pick()` and postfix `.pick()`.
- **collectSome** — `CollectSome` builtin, exposed as `Option.collect<T>()`.
- **mapOption** — exposed as `Option.map(action)` and postfix `.mapOption(action)`.
- **withResource** — implemented with merge-based design (not tuple-based). `all(create, identity()) → merge → action → all(action, identity()) → dispose → extractIndex(0)`.
- **Option namespace** — full set: `some`, `none`, `map`, `andThen`, `unwrapOr`, `flatten`, `filter`, `collect`, `isSome`, `isNone`.
- **Postfix operators** — `.then()`, `.forEach()`, `.branch()`, `.flatten()`, `.drop()`, `.tag()`, `.get()`, `.augment()`, `.pick()`, `.mapOption()`.

### Remaining

#### extractField should return Option, not panic

Currently, `ExtractField` returns `null` if the field is missing. This silently produces bad data. With option types now available, it should return `Option<T>`:

```ts
// Current: { key: T } → T (or null if missing — silent failure)
function extractField<K>(field: K): TypedAction<{ [k in K]: T }, T>

// Proposed: { key?: T } → Option<T>
function tryGet<K>(field: K): TypedAction<Record<string, unknown>, Option<T>>
```

The current `extractField` (and its postfix `.get()`) assume the field exists — the TS type system enforces this at build time via `TField extends keyof Out & string`. Runtime missing fields are a JSON deserialization concern, not a combinator concern.

**Open question:** Is this worth doing? The TS type system already prevents accessing missing fields at compile time. The only scenario where a field is missing at runtime is malformed handler output. A `tryGet` variant that returns `Option<T>` could be useful for dynamically-typed data, but `extractField`/`.get()` should stay non-optional for typed pipelines.

#### unwrap — extract value from Option or panic

```ts
function unwrap<T>(): TypedAction<Option<T>, T>
```

Would desugar to `Option.unwrapOr(panic("unwrap on None"))`. Requires a `panic` builtin or mechanism for hard failure. Low priority — `Option.unwrapOr(constant(default))` covers the common case, and branching handles the rest.
