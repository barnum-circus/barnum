# Take and Skip

New `Take` and `Skip` builtins for array slicing, exposed as `Iterator.take(n)` and `Iterator.skip(n)` postfix methods.

---

## Motivation

`take(n)` and `skip(n)` are fundamental slicing operations. They appear in the API surface audit as proposed (medium priority) and in ITERATOR_METHODS.md under "Limiting & Slicing." Both are trivial array slice operations on our eager iterator model — no laziness, no short-circuiting, just `input[..n]` and `input[n..]`.

---

## Current state

No slicing builtins exist. The closest operations are `SplitFirst` and `SplitLast`, which decompose arrays into head/tail or init/last pairs. There's no way to take/skip an arbitrary number of elements without composing a loop over `splitFirst` — which works but generates an unnecessarily complex AST for a trivial slice.

---

## Design

### New Rust builtins

Two new `BuiltinKind` variants. Both operate on arrays and clamp `n` to the array length (no panics on out-of-bounds).

**`Take`** — `T[] → T[]`

```rust
BuiltinKind::Take { n } => {
    let Value::Array(items) = input else {
        return Err(BuiltinError::TypeMismatch {
            builtin: "Take",
            expected: "array",
            actual: input.clone(),
        });
    };
    let end = n.min(items.len());
    Ok(Value::Array(items[..end].to_vec()))
}
```

**`Skip`** — `T[] → T[]`

```rust
BuiltinKind::Skip { n } => {
    let Value::Array(items) = input else {
        return Err(BuiltinError::TypeMismatch {
            builtin: "Skip",
            expected: "array",
            actual: input.clone(),
        });
    };
    let start = n.min(items.len());
    Ok(Value::Array(items[start..].to_vec()))
}
```

### AST definitions

**Rust** (`crates/barnum_ast/src/lib.rs`, in `BuiltinKind` enum):

```rust
/// First `n` elements of an array. Clamps to array length.
Take {
    /// Number of elements to take.
    n: usize,
},
/// Drop the first `n` elements of an array. Clamps to array length.
Skip {
    /// Number of elements to skip.
    n: usize,
},
```

**TypeScript** (`libs/barnum/src/ast.ts`, in `BuiltinKind` type):

```typescript
| { kind: "Take"; n: number }
| { kind: "Skip"; n: number }
```

### TypeScript builtin constructors

**`libs/barnum/src/builtins/array.ts`:**

```typescript
export function take<TElement>(n: number): TypedAction<TElement[], TElement[]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Take", n } },
  });
}

export function skip<TElement>(n: number): TypedAction<TElement[], TElement[]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Skip", n } },
  });
}
```

Re-export from `builtins/index.ts`.

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

Pattern: `collect → builtin → fromArray`. Same as `map` (unwrap, transform, re-wrap).

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

| Input | `take(n)` | `skip(n)` |
|-------|-----------|-----------|
| `n = 0` | `[]` | full array |
| `n >= length` | full array | `[]` |
| empty array | `[]` | `[]` |

All handled by the `min` clamp — no special-casing needed.

---

## Tasks

### Task 1: Rust builtins

**1.1: AST variants** — `crates/barnum_ast/src/lib.rs`

Add `Take { n: usize }` and `Skip { n: usize }` to `BuiltinKind`.

**1.2: Builtin execution** — `crates/barnum_builtins/src/lib.rs`

Add match arms for `Take` and `Skip` as shown in the Design section.

**1.3: Rust tests** — `crates/barnum_builtins/src/lib.rs`

```rust
#[tokio::test]
async fn take_returns_first_n_elements() {
    let input = json!([1, 2, 3, 4, 5]);
    let result = execute_builtin(&BuiltinKind::Take { n: 3 }, &input).await;
    assert_eq!(result.unwrap(), json!([1, 2, 3]));
}

#[tokio::test]
async fn take_clamps_to_array_length() {
    let input = json!([1, 2]);
    let result = execute_builtin(&BuiltinKind::Take { n: 10 }, &input).await;
    assert_eq!(result.unwrap(), json!([1, 2]));
}

#[tokio::test]
async fn take_zero_returns_empty() {
    let input = json!([1, 2, 3]);
    let result = execute_builtin(&BuiltinKind::Take { n: 0 }, &input).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn take_empty_array() {
    let result = execute_builtin(&BuiltinKind::Take { n: 3 }, &json!([])).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn take_rejects_non_array() {
    let result = execute_builtin(&BuiltinKind::Take { n: 1 }, &json!("not array")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn skip_drops_first_n_elements() {
    let input = json!([1, 2, 3, 4, 5]);
    let result = execute_builtin(&BuiltinKind::Skip { n: 2 }, &input).await;
    assert_eq!(result.unwrap(), json!([3, 4, 5]));
}

#[tokio::test]
async fn skip_clamps_to_array_length() {
    let input = json!([1, 2]);
    let result = execute_builtin(&BuiltinKind::Skip { n: 10 }, &input).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn skip_zero_returns_full_array() {
    let input = json!([1, 2, 3]);
    let result = execute_builtin(&BuiltinKind::Skip { n: 0 }, &input).await;
    assert_eq!(result.unwrap(), json!([1, 2, 3]));
}

#[tokio::test]
async fn skip_empty_array() {
    let result = execute_builtin(&BuiltinKind::Skip { n: 3 }, &json!([])).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn skip_rejects_non_array() {
    let result = execute_builtin(&BuiltinKind::Skip { n: 1 }, &json!("not array")).await;
    assert!(result.is_err());
}
```

### Task 2: TypeScript layer

**2.1: BuiltinKind type** — `libs/barnum/src/ast.ts`

Add `| { kind: "Take"; n: number }` and `| { kind: "Skip"; n: number }` to the `BuiltinKind` union.

**2.2: Builtin constructors** — `libs/barnum/src/builtins/array.ts`

Add `take` and `skip` functions as shown in the Design section.

**2.3: Re-export** — `libs/barnum/src/builtins/index.ts`

Add `take` and `skip` to the barrel export.

**2.4: Iterator methods** — `libs/barnum/src/iterator.ts`

Add `take` and `skip` to the `Iterator` namespace object. Import `take` and `skip` from `./builtins/index.js` (aliased to avoid name collision with Iterator methods: `import { take as takeBuiltin, skip as skipBuiltin } from "./builtins/index.js"`).

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
