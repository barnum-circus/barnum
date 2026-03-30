# Option-returning extractors

## Motivation

`ExtractField` and `ExtractIndex` currently return `Value::Null` when the field/index is missing. This silently produces bad data. They should return `Option<T>` instead, making missing values explicit.

## Current behavior

```rust
// ExtractField — returns null if field missing
obj.get(field_name).cloned().unwrap_or(Value::Null)

// ExtractIndex — returns null if index out of bounds
arr.get(index).cloned().unwrap_or(Value::Null)
```

## Proposed behavior

Both should wrap the result as `Option<T>`:

- Field/index exists → `{ "kind": "Some", "value": <extracted> }`
- Field/index missing → `{ "kind": "None", "value": null }`

## Complications

`extractIndex` and `extractField` are used internally by combinators that construct their own tuples/objects and know the field/index is always present:

- **`withResource`**, **`augment`**, **`tap`** — use `extractIndex` to destructure `all()` tuples
- **`Option.map`**, **`Option.andThen`**, etc. — use `extractField("value")` to unwrap `Some` payloads

These internal uses would need to unwrap the Option, which requires either:

1. A separate non-option `extractFieldUnsafe` / `extractIndexUnsafe` for internal use
2. An `unwrap` builtin (requires a `panic` mechanism for the `None` case)
3. Keeping the current builtins for internal use and adding new `tryExtractField` / `tryExtractIndex` variants for user-facing code

Option 3 is probably cleanest — the current builtins stay as-is for engine internals where the TS type system guarantees presence, and new option-returning variants are added for dynamically-typed or user-facing scenarios.

## TS type changes

The current `extractField` signature enforces field presence at compile time:

```ts
function extractField<TObj, TField extends keyof TObj & string>(
  field: TField,
): TypedAction<TObj, TObj[TField]>
```

A new `tryExtractField` would accept any string key and return `Option<T>`:

```ts
function tryExtractField<TObj extends Record<string, unknown>, TField extends string>(
  field: TField,
): TypedAction<TObj, Option<TField extends keyof TObj ? TObj[TField] : unknown>>
```

Similarly for `tryExtractIndex`.
