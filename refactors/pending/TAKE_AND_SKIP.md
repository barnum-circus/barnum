# Take and Skip

A single `Slice` builtin for array slicing, exposed as `Iterator.take(n)` and `Iterator.skip(n)` postfix methods.

---

## Motivation

`take(n)` and `skip(n)` are fundamental slicing operations. They appear in the API surface audit as proposed (medium priority) and in ITERATOR_METHODS.md under "Limiting & Slicing." Both are trivial array slice operations on our eager iterator model — no laziness, no short-circuiting, just `input[..n]` and `input[n..]`.

Both operations are `input[start..end]` with different defaults. One builtin handles both.

---

## Current state

No slicing builtins exist. The closest operations are `SplitFirst` and `SplitLast`, which decompose arrays into head/tail or init/last pairs. There's no way to take/skip an arbitrary number of elements without composing a loop over `splitFirst` — which works but generates an unnecessarily complex AST for a trivial slice.

---

## Design

### New Rust builtin

One new `BuiltinKind` variant: `Slice`. Operates on arrays with `start` and `end` indices, both clamped to array length.

- `take(n)` → `Slice { start: 0, end: Some(n) }`
- `skip(n)` → `Slice { start: n, end: None }`

`end: None` means "to the end of the array."

```rust
BuiltinKind::Slice { start, end } => {
    let Value::Array(items) = input else {
        return Err(BuiltinError::TypeMismatch {
            builtin: "Slice",
            expected: "array",
            actual: input.clone(),
        });
    };
    let len = items.len();
    let s = start.min(len);
    let e = end.map_or(len, |n| n.min(len));
    // If start >= end after clamping, return empty array
    if s >= e {
        Ok(Value::Array(vec![]))
    } else {
        Ok(Value::Array(items[s..e].to_vec()))
    }
}
```

### AST definition

**Rust** (`crates/barnum_ast/src/lib.rs`, in `BuiltinKind` enum):

```rust
/// Slice an array from `start` to `end`. Both clamped to array length.
/// `end: None` means "to the end of the array."
Slice {
    /// Start index (inclusive). Clamped to array length.
    start: usize,
    /// End index (exclusive). `None` means end of array. Clamped to array length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end: Option<usize>,
},
```

**TypeScript** (`libs/barnum/src/ast.ts`, in `BuiltinKind` type):

```typescript
| { kind: "Slice"; start: number; end?: number }
```

### TypeScript builtin constructors

**`libs/barnum/src/builtins/array.ts`:**

```typescript
export function slice<TElement>(
  start: number,
  end?: number,
): TypedAction<TElement[], TElement[]> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: end !== undefined
        ? { kind: "Slice", start, end }
        : { kind: "Slice", start },
    },
  });
}
```

`take` and `skip` are thin wrappers:

```typescript
export function take<TElement>(n: number): TypedAction<TElement[], TElement[]> {
  return slice(0, n);
}

export function skip<TElement>(n: number): TypedAction<TElement[], TElement[]> {
  return slice(n);
}
```

Re-export all three from `builtins/index.ts`.

### Iterator namespace methods

**`libs/barnum/src/iterator.ts`:**

```typescript
/** First n elements. `Iterator<T> → Iterator<T>` */
take<TElement>(n: number): TypedAction<IteratorT<TElement>, IteratorT<TElement>> {
  return chain(
    Iterator.collect<TElement>(),
    chain(toAction(take<TElement>(n)), Iterator.fromArray<TElement>()),
  );
},

/** Drop first n elements. `Iterator<T> → Iterator<T>` */
skip<TElement>(n: number): TypedAction<IteratorT<TElement>, IteratorT<TElement>> {
  return chain(
    Iterator.collect<TElement>(),
    chain(toAction(skip<TElement>(n)), Iterator.fromArray<TElement>()),
  );
},
```

Pattern: `collect → builtin → fromArray`. Same as `map` (unwrap, transform, re-wrap). Both delegate to `Slice` through the `take`/`skip` array builtin wrappers.

### Postfix methods

**`libs/barnum/src/ast.ts`:**

```typescript
function takeMethod(this: TypedAction, n: number): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.take(n)));
}

function skipMethod(this: TypedAction, n: number): TypedAction {
  return chain(toAction(this), toAction(IteratorNs.skip(n)));
}
```

Register in `typedAction`'s property descriptor block:

```typescript
take: { value: takeMethod, configurable: true },
skip: { value: skipMethod, configurable: true },
```

No `branchFamily` dispatch — take/skip only make sense on Iterator, not Option/Result. Direct delegation to `Iterator.take(n)` / `Iterator.skip(n)`.

### Type declarations

Add to the `TypedAction` interface in `ast.ts`:

```typescript
take(n: number): TypedAction;
skip(n: number): TypedAction;
```

---

## Edge cases

All handled by clamping `start` and `end` to array length, and returning `[]` when `start >= end`.

| Builtin | Equivalent | Result |
|---------|-----------|--------|
| `Slice { start: 0, end: Some(0) }` | `take(0)` | `[]` |
| `Slice { start: 0, end: Some(n) }` where `n >= len` | `take(big)` | full array |
| `Slice { start: n, end: None }` where `n >= len` | `skip(big)` | `[]` |
| `Slice { start: 3, end: Some(1) }` | start past end | `[]` |
| any slice on `[]` | — | `[]` |

---

## Tasks

### Task 1: Rust builtin

**1.1: AST variant** — `crates/barnum_ast/src/lib.rs`

Add `Slice { start: usize, end: Option<usize> }` to `BuiltinKind`.

**1.2: Builtin execution** — `crates/barnum_builtins/src/lib.rs`

Add match arm for `Slice` as shown in the Design section.

**1.3: Rust tests** — `crates/barnum_builtins/src/lib.rs`

```rust
#[tokio::test]
async fn slice_with_end() {
    let input = json!([1, 2, 3, 4, 5]);
    let result = execute_builtin(
        &BuiltinKind::Slice { start: 0, end: Some(3) },
        &input,
    ).await;
    assert_eq!(result.unwrap(), json!([1, 2, 3]));
}

#[tokio::test]
async fn slice_without_end() {
    let input = json!([1, 2, 3, 4, 5]);
    let result = execute_builtin(
        &BuiltinKind::Slice { start: 2, end: None },
        &input,
    ).await;
    assert_eq!(result.unwrap(), json!([3, 4, 5]));
}

#[tokio::test]
async fn slice_middle() {
    let input = json!([1, 2, 3, 4, 5]);
    let result = execute_builtin(
        &BuiltinKind::Slice { start: 1, end: Some(4) },
        &input,
    ).await;
    assert_eq!(result.unwrap(), json!([2, 3, 4]));
}

#[tokio::test]
async fn slice_clamps_end_to_array_length() {
    let input = json!([1, 2]);
    let result = execute_builtin(
        &BuiltinKind::Slice { start: 0, end: Some(10) },
        &input,
    ).await;
    assert_eq!(result.unwrap(), json!([1, 2]));
}

#[tokio::test]
async fn slice_clamps_start_to_array_length() {
    let input = json!([1, 2]);
    let result = execute_builtin(
        &BuiltinKind::Slice { start: 10, end: None },
        &input,
    ).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn slice_start_at_zero_end_at_zero() {
    let input = json!([1, 2, 3]);
    let result = execute_builtin(
        &BuiltinKind::Slice { start: 0, end: Some(0) },
        &input,
    ).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn slice_start_past_end_returns_empty() {
    let input = json!([1, 2, 3]);
    let result = execute_builtin(
        &BuiltinKind::Slice { start: 3, end: Some(1) },
        &input,
    ).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn slice_empty_array() {
    let result = execute_builtin(
        &BuiltinKind::Slice { start: 0, end: Some(3) },
        &json!([]),
    ).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn slice_rejects_non_array() {
    let result = execute_builtin(
        &BuiltinKind::Slice { start: 0, end: Some(1) },
        &json!("not array"),
    ).await;
    assert!(result.is_err());
}
```

### Task 2: TypeScript layer

**2.1: BuiltinKind type** — `libs/barnum/src/ast.ts`

Add `| { kind: "Slice"; start: number; end?: number }` to the `BuiltinKind` union.

**2.2: Builtin constructors** — `libs/barnum/src/builtins/array.ts`

Add `slice`, `take`, and `skip` functions as shown in the Design section.

**2.3: Re-export** — `libs/barnum/src/builtins/index.ts`

Add `slice`, `take`, and `skip` to the barrel export.

**2.4: Iterator methods** — `libs/barnum/src/iterator.ts`

Add `take` and `skip` to the `Iterator` namespace object. Import `take` and `skip` from `./builtins/index.js` (aliased to avoid name collision with the Iterator methods: `import { take as takeBuiltin, skip as skipBuiltin } from "./builtins/index.js"`).

**2.5: Postfix methods** — `libs/barnum/src/ast.ts`

Add `takeMethod`, `skipMethod` functions. Register in `typedAction` property descriptors. Add `take(n)` and `skip(n)` to the `TypedAction` interface.

**2.6: Public export** — `libs/barnum/src/pipeline.ts` (or wherever the public barrel is)

Export `Iterator.take` and `Iterator.skip` are already accessible through the `Iterator` namespace. The `take`/`skip` array builtins should also be exported if we want them usable on raw arrays.

### Task 3: Execution tests

E2E tests through `runPipeline` exercising the full path (TypeScript AST → Rust execution → result).

```typescript
test("Iterator.take returns first n elements", async () => {
  const result = await runPipeline(
    pipe(constant([1, 2, 3, 4, 5]), identity<number[]>().iterate().take(3).collect()),
  );
  expect(result).toEqual([1, 2, 3]);
});

test("Iterator.skip drops first n elements", async () => {
  const result = await runPipeline(
    pipe(constant([1, 2, 3, 4, 5]), identity<number[]>().iterate().skip(2).collect()),
  );
  expect(result).toEqual([3, 4, 5]);
});

test("take and skip compose to partition", async () => {
  const result = await runPipeline(
    pipe(
      constant([1, 2, 3, 4, 5]),
      bindInput<number[]>(arr => all(
        arr.iterate().take(2).collect(),
        arr.iterate().skip(2).collect(),
      )),
    ),
  );
  expect(result).toEqual([[1, 2], [3, 4, 5]]);
});
```
