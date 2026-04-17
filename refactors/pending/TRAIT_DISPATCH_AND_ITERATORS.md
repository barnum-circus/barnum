# Trait Dispatch and Iterators

## Context

Dynamic dispatch in barnum uses **prefix-based dispatch** via the `ExtractPrefix` builtin and `matchPrefix` combinator. Tagged union values carry namespaced kind strings (`"Option.Some"`, `"Result.Ok"`). `ExtractPrefix` splits on `'.'` to restructure the value so `branch()` can dispatch on the family first, then the variant. No runtime dispatch tables — the AST encodes the dispatch.

Currently, transformation methods like `.map()` and `.andThen()` are postfix methods on TypedAction that use `matchPrefix` to dispatch across Option and Result.

**This doc introduces Iterator as the sole transformation interface.** Transformation methods (map, filter, find, etc.) live only on Iterator. Option and Result gain `.iterate()` to enter Iterator, and methods like `.first()`, `.collect()`, `.collectResult()` to exit. `.map()` and `.andThen()` are removed from Option/Result — to transform, enter Iterator first.

---

## Design: Iterator as the sole transformation interface

Currently, `.map()` and `.andThen()` are shared postfix methods that dispatch via `matchPrefix` across Option and Result:

```ts
// CURRENT — shared dispatch across families
function mapMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({
      Ok: chain(toAction(action), toAction(Result.ok)),
      Err: Result.err,
    }),
    Option: branch({
      Some: chain(toAction(action), toAction(Option.some)),
      None: Option.none,
    }),
  })));
}
```

**This refactor removes `.map()`, `.andThen()`, and `.forEach()` from Option/Result/arrays.** Iterator becomes the only interface for element-wise transformation. To transform, enter Iterator first:

```ts
// BEFORE: option.map(validate)
// AFTER:
option.iterate().map(validate).first()

// BEFORE: result.map(transform)
// AFTER:
result.iterate().map(transform).first().unwrapOr(defaultValue)

// BEFORE: array.forEach(process)
// AFTER:
array.then(Iter.wrap()).map(process).collect()
```

**Why:** One place for transformations eliminates multi-family dispatch complexity. `matchPrefix` is still used for `.unwrapOr()`, `.unwrap()`, `.transpose()`, and `.iterate()` itself — methods that need to know which family they're operating on. But map/andThen don't need family dispatch — they only operate on Iterator.

### Postfix methods by family

**Option postfix methods** (via direct branch, no `matchPrefix`):
- `.unwrapOr(default)` — exit Option
- `.unwrap()` — exit Option (panic on None)
- `.isSome()` / `.isNone()` — query
- `.filter(pred)` — `Option<T> → Option<T>` (inherent to Option, pred: `T → bool`)
- `.transpose()` — `Option<Result<T,E>> → Result<Option<T>,E>`
- `.iterate()` — enter Iterator

**Result postfix methods** (via direct branch, no `matchPrefix`):
- `.unwrapOr(default)` — exit Result
- `.unwrap()` — exit Result (panic on Err)
- `.mapErr(f)` — transform error variant
- `.or(fallback)` — recover from Err
- `.and(other)` — chain on Ok
- `.isOk()` / `.isErr()` — query
- `.toOption()` / `.toOptionErr()` — convert
- `.transpose()` — `Result<Option<T>,E> → Option<Result<T,E>>`
- `.iterate()` — enter Iterator

**Shared postfix methods (using `matchPrefix`):**
- `.unwrapOr(default)`, `.unwrap()`, `.transpose()`, `.iterate()`

**Removed from Option/Result** (now Iterator-only):
- `.map()`, `.andThen()`, `.forEach()`

**Removed entirely:**
- `Option.collect()` — the `forEach(pred).then(Option.collect())` pattern is replaced by `Iterator.filter(pred)`. The `CollectSome` builtin is no longer needed.

---

## Iterator<T> — a wrapper type with its own prefix

### What is Iterator<T>?

`Iterator<T>` is a tagged union wrapper — like Option and Result — with namespaced kind: `"Iterator.Iterator"`. `.iterate()` converts Option/Result into an Iterator (arrays use `.then(Iter.wrap())`). Iterator is the single place where transformation methods live.

### Runtime representation — tagged wrapper

```ts
type IteratorDef<T> = { Iterator: T[] };
type Iterator<T> = TaggedUnion<"Iterator", IteratorDef<T>>;
// Runtime: { kind: "Iterator.Iterator", value: [1, 2, 3] }
```

This means:
- `Iter.wrap()` wraps the array: `[1, 2, 3]` → `{ kind: "Iterator.Iterator", value: [1, 2, 3] }`
- Iterator methods operate on `.value` (the inner array), then re-wrap
- `.collect()` unwraps: `{ kind: "Iterator.Iterator", value: [1, 2, 3] }` → `[1, 2, 3]`

**Note on single-variant representation:** `"Iterator.Iterator"` is redundant — the prefix is the only thing that matters for dispatch, and there's no second variant to distinguish. The `.Iterator` suffix exists solely to fit the `TaggedUnion<Name, Def>` pattern. An alternative is a simpler wrapper (e.g., just `{ kind: "Iterator", value: T[] }`) that doesn't go through the tagged union machinery. But consistency with Option/Result has value — it means `branch()` works on it, and the Rust engine treats it uniformly.

Why tagged wrapper over phantom brand:
- Consistent with every other barnum type (Option, Result, all TaggedUnion)
- `.branch()` works on it (you can pattern-match on `{ kind: "Iterator.Iterator" }`)
- The Rust engine can recognize and optimize it
- Handlers that receive an Iterator see a proper `{ kind, value }` object, not a bare array that happens to be branded

The wrap/unwrap overhead is real but small — it's a Rust builtin (WrapInField/GetField), not a subprocess call.

### IntoIterator — conversion to Iterator

| Self type | Conversion | Runtime behavior |
|-----------|------------|------------------|
| `Option<T>` | `.iterate()` postfix or `Iter.fromOption()` | Branch: Some → `[value]`, None → `[]`, then wrap |
| `Result<T, E>` | `.iterate()` postfix or `Iter.fromResult()` | Branch: Ok → `[value]`, Err → `[]`, then wrap |
| `T[]` | `.then(Iter.wrap())` | Wrap in `{ kind: "Iterator.Iterator", value: array }` |

`.iterate()` is a postfix method that uses `matchPrefix` for Option/Result. Arrays have no `kind` field, so `extractPrefix` would fail — arrays use `.then(Iter.wrap())` instead.

### Implementation

`.iterate()` postfix uses `matchPrefix` for Option/Result. Arrays use `Iter.wrap()`:

```ts
// option.iterate() / result.iterate() — dispatches via matchPrefix
// Some(value) / Ok(value) → Iterator([value])
// None / Err(_)           → Iterator([])

// array.then(Iter.wrap()) — no dispatch needed, just wrap
// [1, 2, 3] → { kind: "Iterator.Iterator", value: [1, 2, 3] }
```

---

## Iterator methods

All Iterator methods unwrap `{ kind: "Iterator.Iterator", value: T[] }` → operate on `T[]` → re-wrap (for methods that stay in Iterator) or exit (for methods that produce Option, Result, or plain values). The pattern is: `getField("value")` → array operation → `Iter.wrap()`.

### Phase 1 — implement now (used in demos)

| Method | Rust equivalent | Signature | Implementation | Notes |
|--------|----------------|-----------|----------------|-------|
| `.map(f)` | `Iterator::map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → rewrap | Per-element transform |
| `.andThen(f)` | `Iterator::flat_map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → unwrap each inner Iterator → concat → rewrap | `f: T → Iterator<U>`. Monadic bind for Iterator. |
| `.filter(pred)` | `Iterator::filter` | `Iterator<T> → Iterator<T>` | New `Filter` builtin | pred: `T → bool`. New Rust builtin. Needed for demos. |
| `.collect()` | `Iterator::collect` | `Iterator<T> → T[]` | Unwrap (getField("value")) | Exit Iterator |

All Phase 1 methods exist in both forms: postfix (`.map(f)`) and standalone (`Iter.map(f)`). Standalone forms are `TypedAction` values composable into pipelines.

### Future — add when needed

| Method | Rust equivalent | Signature | Implementation | Notes |
|--------|----------------|-----------|----------------|-------|
| `.first()` | `Iterator::next` | `Iterator<T> → Option<T>` | Independent impl, not built on splitFirst | Exit Iterator, enter Option |
| `.find(pred)` | `Iterator::find` | `Iterator<T> → Option<T>` | `filter(pred).first()` or dedicated builtin | Exits Iterator, enters Option. Not short-circuiting |
| `.collectResult()` | `collect::<Result<Vec,E>>` | `Iterator<Result<T,E>> → Result<T[],E>` | Unwrap → fold with short-circuit on Err | Exit Iterator, enter Result |
| `.collectOption()` | `collect::<Option<Vec>>` | `Iterator<Option<T>> → Option<T[]>` | Unwrap → fold with short-circuit on None | Exit Iterator, enter Option |
| `.last()` | `Iterator::last` | `Iterator<T> → Option<T>` | Unwrap → splitLast → Option wrap | Exit Iterator, enter Option |
| `.count()` | `Iterator::count` | `Iterator<T> → number` | Unwrap → Arr.length | Needs builtin |
| `.any(pred)` | `Iterator::any` | `Iterator<T> → boolean` | `find(pred).isSome()` | Not short-circuiting |
| `.all(pred)` | `Iterator::all` | `Iterator<T> → boolean` | Needs design | Name collision with `all()` combinator |

### Needs new builtins

| Method | Rust equivalent | Signature | Notes |
|--------|----------------|-----------|-------|
| `.filter(pred)` | `Iterator::filter` | `Iterator<T> → Iterator<T>` | `Filter` builtin. pred: `T → bool`. Phase 1. |
| `.take(n)` | `Iterator::take` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.skip(n)` | `Iterator::skip` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.reverse()` | `Iterator::rev` | `Iterator<T> → Iterator<T>` | Always available on our eager arrays |
| `.join(sep)` | `slice::join` | `Iterator<string> → string` | Not on Iterator trait in Rust. Include for ergonomics |
| `.zip(other)` | `Iterator::zip` | Needs design | |
| `.chain(other)` | `Iterator::chain` | `Iterator<T> → Iterator<T>` | No naming collision — barnum's `chain()` is internal, users see `.then()` |
| `.nth(n)` | `Iterator::nth` | `Iterator<T> → Option<T>` | Trivial: unwrap → getIndex → Option wrap |

### Family transitions

| Method | Returns | Next family |
|--------|---------|-------------|
| `.map(f)` | `Iterator<U>` | Iterator (stay) |
| `.andThen(f)` | `Iterator<U>` | Iterator (stay) |
| `.filter(pred)` | `Iterator<T>` | Iterator (stay) |
| `.collect()` | `T[]` | none (exit) |
| `.collectResult()` | `Result<T[],E>` | Result |
| `.collectOption()` | `Option<T[]>` | Option |
| `.first()` | `Option<T>` | Option |
| `.last()` | `Option<T>` | Option |
| `.find(pred)` | `Option<T>` | Option |
| `.nth(n)` | `Option<T>` | Option |
| `.count()` | `number` | none (exit) |
| `.any(pred)` | `boolean` | none (exit) |
| `.all(pred)` | `boolean` | none (exit) |

---

## Example chains

```ts
// Option → Iterator → transform → exit to Option
option.iterate()                             // Iterator<string>
  .map(validate)                             // Iterator<ValidResult>
  .first()                                   // Option<ValidResult>

// Result → unwrap → Iterator → transform
result                                       // Result<User[], Error>
  .unwrapOr(constant([]))                    // User[]
  .then(Iter.wrap())                         // Iterator<User>
  .map(getName)                              // Iterator<string>
  .collect()                                 // string[]

// Array → Iterator → map → collect
users                                        // User[]
  .then(Iter.wrap())                         // Iterator<User>
  .map(getName)                              // Iterator<string>
  .collect()                                 // string[]

// andThen (monadic bind): Iterator<File> → Iterator<Refactor>
files                                        // File[]
  .then(Iter.wrap())                         // Iterator<File>
  .andThen(analyze)                          // Iterator<Refactor>  (f returns Iterator<Refactor>)
  .collect()                                 // Refactor[]
```

---

## What Iterator adds

1. **Single transformation interface.** `.map()`, `.andThen()`, `.filter()`, `.find()` — all transformation methods live on Iterator. Option/Result have `.iterate()` to enter, and `.first()`, `.collect()`, etc. to exit.

2. **Typed collect.** `.collectResult()` and `.collectOption()` provide type-directed collection — the Rust `collect::<Result<Vec<T>,E>>()` pattern.

3. **Simpler dispatch.** `.iterate()` dispatches via `matchPrefix` to convert Option/Result into Iterator. But transformation methods (map, filter, etc.) only operate on Iterator — no multi-family dispatch needed.

4. **Iterator-only methods.** find, first, last, count, take, skip, etc. — these only make sense on sequences and don't exist on Option/Result.

---

## Open questions

1. ~~**Naming**~~ **Decided:** `.iterate()`.

2. ~~**Array → Iterator**~~ **Decided:** No postfix `.iterate()` on arrays — `defineProperties` attaches one function that uses `matchPrefix`, which fails on arrays (no `kind` field). Arrays enter Iterator via `.then(Iter.wrap())`. Postfix `.iterate()` handles Option and Result only.

3. ~~**`filter` predicate type**~~ **Decided:** `T → bool`. New `Filter` Rust builtin. Consistent with Rust's `Iterator::filter`.

4. ~~**Short-circuit semantics**~~ **Not an issue now:** All Phase 1 methods (map, andThen, filter, collect) are inherently non-short-circuiting. Short-circuit matters for find/any/all — those are future phases.

5. **`collect` destination types**: Rust's `Iterator::collect` is generic over the destination type via `FromIterator`. Barnum uses separate named methods:
   - `.collect()` → `T[]` (default, like `Vec`)
   - `.collectResult()`: `Iterator<Result<T, E>> → Result<T[], E>` — if all Ok, returns `Result.Ok(values[])`; on first Err, returns that `Result.Err`. This works because TypedAction carries the type info: the TypeScript types constrain `.collectResult()` to only be callable when the inner type is `Result<T, E>`. At runtime, it unwraps the Iterator, iterates the array branching each element on Ok/Err, and short-circuits on first Err. This is the Rust `Iterator<Result<T,E>>::collect::<Result<Vec<T>,E>>()` equivalent.
   - `.collectOption()`: `Iterator<Option<T>> → Option<T[]>` — if all Some, returns `Option.Some(values[])`; on first None, returns `Option.None`. Same pattern.

6. **`.forEach()` removal**: `.forEach()` is removed as a postfix method on arrays. Arrays use `.then(Iter.wrap()).map(f).collect()` for element-wise transforms. The `ForEach` AST node remains — it's the internal mechanism that Iterator's `.map()` compiles to. See "ForEach AST node" section below.

7. ~~**`chain` naming collision**~~ **Not an issue:** barnum's `chain()` is an internal combinator; users see `.then()`. Iterator can use `.chain()` for concatenation without ambiguity.

---

## ForEach AST node

`ForEach` is a fundamental AST node — `{ kind: "ForEach", action: Action }` applies an action to every element of an array. It's how the Rust engine does element-wise operations. Currently it's exposed as:

1. **Standalone combinator:** `forEach(action)` — `TypedAction<T[], U[]>`. Used internally by Iterator's `.map()`.
2. **Postfix method:** `array.forEach(f)` — sugar for `chain(array, forEach(f))`.

With Iterator as the sole transformation interface, the **postfix method is removed**. Users write `array.then(Iter.wrap()).map(f).collect()` instead of `array.forEach(f)`.

The **standalone combinator stays** — it's the implementation mechanism for Iterator's `.map()`:

```ts
// Iterator.map internally:
chain(getField("value"), forEach(action), Iter.wrap())
```

The `ForEach` AST node itself is unchanged. It's an implementation detail, not user-facing API.

---

## Demo migration plan

Each demo that uses `forEach` (postfix or standalone) needs to migrate to Iterator. Specific changes:

### `identify-and-address-refactors/run.ts`

```ts
// BEFORE (line 54): each file produces Refactor[], need to concat results
forEach(analyze).flatten(),

// AFTER: andThen (monadic bind) — f returns Iterator, results are concatenated
constant({ folder: srcDir }).then(listTargetFiles)
  .then(Iter.wrap())
  .andThen(analyze)

// BEFORE (line 57): filter — assess each, collect Somes
forEach(assessWorthiness).then(Option.collect()),

// AFTER: filter with bool predicate
  .filter(assessWorthiness)

// BEFORE (line 60-66): map with resource
forEach(withResource({ ... })),

// AFTER:
  .map(withResource({ ... }))
  .collect(),
```

Full pipeline becomes:
```ts
constant({ folder: srcDir })
  .then(listTargetFiles)
  .then(Iter.wrap())                            // T[] → Iterator<T>
  .andThen(analyze)                             // each file → Iterator<Refactor>, concatenated
  .filter(assessWorthiness)                     // keep only worthwhile (bool predicate)
  .map(withResource({
    create: createBranchWorktree,
    action: implementAndReview,
    dispose: deleteWorktree,
  }))
  .collect()                                    // Iterator<T> → T[]
```

### `convert-folder-to-ts/run.ts`

```ts
// BEFORE (line 26):
listFiles.forEach(migrate({ to: "Typescript" })).drop(),

// AFTER:
listFiles.then(Iter.wrap()).map(migrate({ to: "Typescript" })).drop(),
```

### `simple-workflow/run.ts`

```ts
// BEFORE (line 18):
listFiles.forEach(pipe(implementRefactor, typeCheckFiles, fixTypeErrors, commitChanges, createPullRequest)),

// AFTER:
listFiles.then(Iter.wrap()).map(
  implementRefactor.then(typeCheckFiles).then(fixTypeErrors).then(commitChanges).then(createPullRequest)
).collect(),
```

### `babysit-prs/run.ts`

```ts
// BEFORE (lines 44-56):
forEach(bindInput<number>((prNumber) => prNumber.then(checkPR).branch({...}))),
Option.collect<number>(),

// AFTER: postfix — filter replaces forEach + Option.collect
  .iterate()
  .filter(bindInput<number>((prNumber) => prNumber.then(checkPR).branch({...})))
  .collect(),
```

### `*/handlers/type-check-fix.ts` (both demos)

```ts
// BEFORE (line 148):
HasErrors: forEach(fix).drop().then(recur),

// AFTER: postfix chain
HasErrors: Iter.wrap<TypeError>().then(Iter.map(fix)).drop().then(recur),
```

---

## Implementation tasks

Per `refactors/PROCESS.md`, every task follows test-first: failing test → implement → remove failure marker (3 commits).

### What needs new builtins vs what composes from existing primitives

**No new builtins needed for Phase 1 methods except `filter`.**

| Method | Needs builtin? | Implementation |
|--------|---------------|----------------|
| `Iter.wrap()` | No | `tag("Iterator", "Iterator")` — reuses existing `tag` |
| `Iter.collect()` | No | `getField("value")` — reuses existing `getField` |
| `Iter.map(f)` | No | `getField("value")` → `forEach(f)` → `tag("Iterator", "Iterator")` |
| `Iter.andThen(f)` | No | `getField("value")` → `forEach(chain(f, getField("value")))` → `flatten()` → `tag("Iterator", "Iterator")` |
| `Iter.filter(pred)` | **Yes** | `getField("value")` → `forEach(all(identity(), pred))` → **`CollectWhere`** → `tag("Iterator", "Iterator")` |
| `Iter.fromOption()` | No | `branch` → wrap in array → `tag("Iterator", "Iterator")` |
| `Iter.fromResult()` | No | `branch` → wrap in array → `tag("Iterator", "Iterator")` |
| `.iterate()` postfix | No | `matchPrefix` → `branch` → wrap in array → `tag("Iterator", "Iterator")` (Option/Result only, no array overload) |

**`filter` requires one new builtin: `CollectWhere`.** It can't be composed from existing primitives because there's no way to branch on a boolean (only on tagged union `kind` fields). The compositional approach avoids a new AST node by splitting filter into two steps:
1. `forEach(all(identity(), pred))` — produces `[T, boolean][]` using existing nodes
2. `CollectWhere` builtin — keeps elements where the boolean (index 1) is `true`, returning values (index 0)

This keeps the scheduler unchanged. `CollectWhere` is a pure data transformation like `CollectSome`.

---

### Phase 1: Iterator foundation

#### Task 1: Add `CollectWhere` builtin (Rust)

**Goal:** New Rust builtin for filter's second step. Input: `[[value, bool], ...]`. Output: `[value, ...]` for `true` elements.

##### 1.1: Add variant to `BuiltinKind`

**File:** `crates/barnum_ast/src/lib.rs` (after `CollectSome`, line ~266)

```rust
// Before:
CollectSome,

// After:
CollectSome,
/// Filter an array of `[value, bool]` pairs, keeping values where bool is `true`.
///
/// Input: array of two-element arrays `[value, predicate_result]`.
/// Output: array of `value` entries where `predicate_result` was `true`.
CollectWhere,
```

##### 1.2: Add execution match arm

**File:** `crates/barnum_builtins/src/lib.rs` (after `CollectSome` arm, line ~198)

```rust
BuiltinKind::CollectWhere => {
    let Value::Array(pairs) = input else {
        return Err(BuiltinError::TypeMismatch {
            builtin: "CollectWhere",
            expected: "array",
            actual: input.clone(),
        });
    };
    let mut collected = Vec::new();
    for pair in pairs {
        let Value::Array(items) = pair else {
            return Err(BuiltinError::TypeMismatch {
                builtin: "CollectWhere",
                expected: "[value, bool] pair",
                actual: pair.clone(),
            });
        };
        if items.len() >= 2 && items[1] == Value::Bool(true) {
            collected.push(items[0].clone());
        }
    }
    Ok(Value::Array(collected))
}
```

##### 1.3: Add Rust tests

**File:** `crates/barnum_builtins/src/lib.rs` (in `mod tests`)

```rust
#[tokio::test]
async fn collect_where_keeps_true_elements() {
    let input = json!([[1, true], [2, false], [3, true]]);
    let result = execute_builtin(&BuiltinKind::CollectWhere, &input).await;
    assert_eq!(result.unwrap(), json!([1, 3]));
}

#[tokio::test]
async fn collect_where_all_false() {
    let input = json!([[1, false], [2, false]]);
    let result = execute_builtin(&BuiltinKind::CollectWhere, &input).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn collect_where_empty() {
    let result = execute_builtin(&BuiltinKind::CollectWhere, &json!([])).await;
    assert_eq!(result.unwrap(), json!([]));
}

#[tokio::test]
async fn collect_where_rejects_non_array() {
    let result = execute_builtin(&BuiltinKind::CollectWhere, &json!("bad")).await;
    assert!(result.is_err());
}
```

##### 1.4: Add to TypeScript `BuiltinKind` type

**File:** `libs/barnum/src/ast.ts` (line ~128, after `ExtractPrefix`)

```ts
// Before:
| { kind: "ExtractPrefix" };

// After:
| { kind: "ExtractPrefix" }
| { kind: "CollectWhere" };
```

##### 1.5: Add TypeScript standalone function

**File:** `libs/barnum/src/builtins/array.ts` (after `range`)

```ts
/**
 * Filter an array of [value, bool] pairs, keeping values where bool is true.
 * Used internally by Iter.filter — not user-facing.
 */
export function collectWhere<TElement>(): TypedAction<[TElement, boolean][], TElement[]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "CollectWhere" } },
  });
}
```

**File:** `libs/barnum/src/builtins/index.ts` — add export:

```ts
export { getIndex, flatten, splitFirst, splitLast, range, collectWhere } from "./array.js";
```

---

#### Task 2: Add `Iterator` types and `Iter` namespace (TypeScript)

**Goal:** Define `IteratorDef<T>`, `Iterator<T>` type aliases and the `Iter` namespace with standalone combinators.

##### 2.1: Add types to `ast.ts`

**File:** `libs/barnum/src/ast.ts` (after `ResultDef`/`Result`, line ~466)

```ts
// After existing Result types:
export type IteratorDef<TElement> = { Iterator: TElement[] };
export type Iterator<TElement> = TaggedUnion<"Iterator", IteratorDef<TElement>>;
```

**Complication:** `Iterator` shadows the global `Iterator` interface. This is fine — barnum's `Iterator` is a type alias only used in type annotations.

##### 2.2: Create `iterator.ts`

**File:** `libs/barnum/src/iterator.ts` (new file)

This follows the same pattern as `option.ts` / `result.ts` — a namespace object with standalone combinators.

```ts
import {
  type Iterator as IteratorT,
  type IteratorDef,
  type Option as OptionT,
  type Result as ResultT,
  type Pipeable,
  type TypedAction,
  toAction,
  typedAction,
  forEach,
  branch,
} from "./ast.js";
import { chain } from "./chain.js";
import { all } from "./all.js";
import {
  constant,
  drop,
  getField,
  identity,
  tag,
  flatten,
  collectWhere,
  wrapInField,
} from "./builtins/index.js";
import { Option } from "./option.js";

// ---------------------------------------------------------------------------
// Iter namespace — combinators for Iterator<T>
// ---------------------------------------------------------------------------

/**
 * Iter namespace. All combinators produce TypedAction AST nodes that
 * compose from existing builtins — no Iterator-specific AST nodes.
 * The only new builtin is CollectWhere (used by filter).
 */
export const Iter = {
  /** Wrap an array as Iterator. `T[] → Iterator<T>` */
  wrap<TElement>(): TypedAction<TElement[], IteratorT<TElement>> {
    return tag<"Iterator", IteratorDef<TElement>, "Iterator">("Iterator", "Iterator");
  },

  /**
   * Unwrap Iterator to array. `Iterator<T> → T[]`
   *
   * Standalone form of the `.collect()` postfix method.
   */
  collect<TElement>(): TypedAction<IteratorT<TElement>, TElement[]> {
    return getField("value") as TypedAction<IteratorT<TElement>, TElement[]>;
  },

  /**
   * Transform each element. `Iterator<T> → Iterator<U>`
   *
   * Implementation: unwrap → forEach(f) → rewrap
   */
  map<TIn, TOut>(
    action: Pipeable<TIn, TOut>,
  ): TypedAction<IteratorT<TIn>, IteratorT<TOut>> {
    return chain(
      toAction(getField("value")),
      chain(
        toAction(forEach(action)),
        toAction(tag("Iterator", "Iterator")),
      ),
    ) as TypedAction<IteratorT<TIn>, IteratorT<TOut>>;
  },

  /**
   * Flat-map. `f: T → Iterator<U>`, results concatenated. `Iterator<T> → Iterator<U>`
   *
   * Implementation: unwrap → forEach(chain(f, getField("value"))) → flatten → rewrap
   */
  andThen<TIn, TOut>(
    action: Pipeable<TIn, IteratorT<TOut>>,
  ): TypedAction<IteratorT<TIn>, IteratorT<TOut>> {
    return chain(
      toAction(getField("value")),
      chain(
        toAction(forEach(chain(toAction(action), toAction(getField("value"))))),
        chain(
          toAction(flatten()),
          toAction(tag("Iterator", "Iterator")),
        ),
      ),
    ) as TypedAction<IteratorT<TIn>, IteratorT<TOut>>;
  },

  /**
   * Filter elements by bool predicate. `Iterator<T> → Iterator<T>`
   *
   * Implementation: unwrap → forEach(all(identity, pred)) → collectWhere → rewrap
   *
   * `forEach(all(identity(), pred))` produces `[T, boolean][]`.
   * `collectWhere()` keeps elements where the boolean is `true`.
   */
  filter<TElement>(
    predicate: Pipeable<TElement, boolean>,
  ): TypedAction<IteratorT<TElement>, IteratorT<TElement>> {
    return chain(
      toAction(getField("value")),
      chain(
        toAction(forEach(all(identity(), predicate))),
        chain(
          toAction(collectWhere()),
          toAction(tag("Iterator", "Iterator")),
        ),
      ),
    ) as TypedAction<IteratorT<TElement>, IteratorT<TElement>>;
  },

  /**
   * Convert Option to Iterator. `Option<T> → Iterator<T>`
   *
   * Some(x) → Iterator([x]), None → Iterator([])
   * Uses direct branch (no matchPrefix — knows the family).
   */
  fromOption<TElement>(): TypedAction<OptionT<TElement>, IteratorT<TElement>> {
    return branch({
      Some: chain(
        toAction(all(identity())),
        toAction(tag("Iterator", "Iterator")),
      ),
      None: chain(
        toAction(constant([])),
        toAction(tag("Iterator", "Iterator")),
      ),
    }) as TypedAction<OptionT<TElement>, IteratorT<TElement>>;
  },

  /**
   * Convert Result to Iterator. `Result<T, E> → Iterator<T>`
   *
   * Ok(x) → Iterator([x]), Err(_) → Iterator([])
   * Uses direct branch (no matchPrefix — knows the family).
   */
  fromResult<TElement, TError = unknown>(): TypedAction<ResultT<TElement, TError>, IteratorT<TElement>> {
    return branch({
      Ok: chain(
        toAction(all(identity())),
        toAction(tag("Iterator", "Iterator")),
      ),
      Err: chain(
        toAction(constant([])),
        toAction(tag("Iterator", "Iterator")),
      ),
    }) as TypedAction<ResultT<TElement, TError>, IteratorT<TElement>>;
  },
} as const;
```

**Notes:**
- `all(identity())` wraps a single value into `[value]` — a one-element tuple. This is how `Some(x)` / `Ok(x)` becomes `[x]` before being wrapped as an Iterator.
- `Iter.wrap()` is a function (not a value) per `TAGGED_UNION_CONSTRUCTORS.md` — preserves `TElement` type info.
- No `Iter.iterate()` — can't express `Option<T> | Result<T, E>` input with invariant input types. Use `Iter.fromOption()` / `Iter.fromResult()` for standalone, or the postfix `.iterate()` which dispatches via `matchPrefix` with proper type overloads.

##### 2.3: Export from `index.ts`

**File:** `libs/barnum/src/index.ts`

```ts
// Before:
export { Option, first, last } from "./option.js";
export { Result } from "./result.js";

// After:
export { Option, first, last } from "./option.js";
export { Result } from "./result.js";
export { Iter } from "./iterator.js";

// Add to existing type re-exports:
export type Iterator<TElement> = TaggedUnion<"Iterator", IteratorDef<TElement>>;
```

Also add `IteratorDef` to the type-only import at the top of `index.ts`:

```ts
// Before:
import type { TaggedUnion, OptionDef, ResultDef } from "./ast.js";

// After:
import type { TaggedUnion, OptionDef, ResultDef, IteratorDef } from "./ast.js";
```

---

#### Task 3: Add `.iterate()` postfix method (TypeScript)

**Goal:** Postfix `.iterate()` that converts Option/Result into Iterator. Arrays use `.then(Iter.wrap())`.

##### 3.1: Add type signature to `TypedAction`

**File:** `libs/barnum/src/ast.ts` (in the `TypedAction` type, after `.collect`)

Two overloads — one for Option, one for Result. No array overload (arrays use `.then(Iter.wrap())`):

```ts
/** Convert to Iterator. Option<T> → Iterator<T>, Result<T,E> → Iterator<T>. */
iterate<TIn, TElement>(
  this: TypedAction<TIn, Option<TElement>>,
): TypedAction<TIn, Iterator<TElement>>;
iterate<TIn, TElement, TError>(
  this: TypedAction<TIn, Result<TElement, TError>>,
): TypedAction<TIn, Iterator<TElement>>;
```

`Iterator` type is added in Task 2.1. `Option` and `Result` are already defined in `ast.ts`.

##### 3.2: Add method implementation

**File:** `libs/barnum/src/ast.ts` (after `collectMethod`, line ~687)

```ts
function iterateMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Option: branch({
      Some: chain(toAction(all(identity())), toAction(tag("Iterator", "Iterator"))),
      None: chain(toAction(constant([])), toAction(tag("Iterator", "Iterator"))),
    }),
    Result: branch({
      Ok: chain(toAction(all(identity())), toAction(tag("Iterator", "Iterator"))),
      Err: chain(toAction(constant([])), toAction(tag("Iterator", "Iterator"))),
    }),
  })));
}
```

The method builds the `matchPrefix` AST inline (same pattern as `mapMethod`, `unwrapOrMethod`).

**No array overload.** `defineProperties` attaches one function — it builds a `matchPrefix` AST that dispatches on Option/Result. Arrays have no `kind` field, so `extractPrefix` would fail. Arrays enter Iterator via `.then(Iter.wrap())` instead. Minor ergonomic difference, architecturally clean.

Import `all` from builtins (already imported as part of existing builtins import) and import `constant`, `tag` (already imported).

##### 3.3: Register in `typedAction()`

**File:** `libs/barnum/src/ast.ts` (in `Object.defineProperties`, line ~744)

```ts
// After existing properties:
iterate: { value: iterateMethod, configurable: true },
```

---

#### Task 4: Add Iterator postfix methods (TypeScript)

**Goal:** `.map()`, `.andThen()`, `.filter()`, `.collect()` as postfix methods on `TypedAction` when the output is `Iterator<T>`.

##### 4.1: Add type signatures to `TypedAction`

**File:** `libs/barnum/src/ast.ts` (in the `TypedAction` type)

Add new overloads to existing `.map()` and `.andThen()` — they already have Option and Result overloads:

```ts
// Existing map overloads:
map<TIn, T, U>(
  this: TypedAction<TIn, Option<T>>,
  action: Pipeable<T, U>,
): TypedAction<TIn, Option<U>>;
map<TIn, TValue, TOut, TError>(
  this: TypedAction<TIn, Result<TValue, TError>>,
  action: Pipeable<TValue, TOut>,
): TypedAction<TIn, Result<TOut, TError>>;

// NEW — Iterator overload:
map<TIn, TElement, TOut>(
  this: TypedAction<TIn, Iterator<TElement>>,
  action: Pipeable<TElement, TOut>,
): TypedAction<TIn, Iterator<TOut>>;
```

```ts
// Existing andThen overloads (Option, Result)... then:

// NEW — Iterator overload:
andThen<TIn, TElement, TOut>(
  this: TypedAction<TIn, Iterator<TElement>>,
  action: Pipeable<TElement, Iterator<TOut>>,
): TypedAction<TIn, Iterator<TOut>>;
```

New methods (Iterator-only, no existing overloads):

```ts
/** Filter elements by bool predicate. Iterator only. */
filter<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
  predicate: Pipeable<TElement, boolean>,
): TypedAction<TIn, Iterator<TElement>>;

/** Unwrap Iterator to array. `Iterator<T> → T[]` */
collect<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
): TypedAction<TIn, TElement[]>;
```

**Complication:** The existing `.filter()` overload is for Option (`Option<T> → Option<T>`). The new Iterator `.filter()` overload has a different predicate type (`T → boolean` vs `T → Option<T>`). TypeScript should disambiguate by the `this` type. The existing `.collect()` is also for Option (`Option<T>[] → T[]`). Again, the `this` type disambiguates — Option's collect takes `Option<T>[]`, Iterator's collect takes `Iterator<T>`.

##### 4.2: Add method implementations

**File:** `libs/barnum/src/ast.ts`

The existing `mapMethod`, `andThenMethod`, and `filterMethod` dispatch via `matchPrefix` for Option/Result. For Iterator, the dispatch must also handle the `Iterator` prefix. Extend the `matchPrefix` calls:

```ts
// Before (mapMethod, line ~562):
function mapMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({
      Ok: chain(toAction(action), toAction(Result.ok)),
      Err: Result.err,
    }),
    Option: branch({
      Some: chain(toAction(action), toAction(Option.some)),
      None: Option.none,
    }),
  })));
}

// After:
function mapMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({
      Ok: chain(toAction(action), toAction(Result.ok)),
      Err: Result.err,
    }),
    Option: branch({
      Some: chain(toAction(action), toAction(Option.some)),
      None: Option.none,
    }),
    Iterator: branch({
      Iterator: chain(
        toAction(forEach(action)),
        toAction(tag("Iterator", "Iterator")),
      ),
    }),
  })));
}
```

Same pattern for `andThenMethod`:

```ts
// Before (andThenMethod, line ~589):
function andThenMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({ Ok: action, Err: Result.err }),
    Option: branch({ Some: action, None: Option.none }),
  })));
}

// After:
function andThenMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({ Ok: action, Err: Result.err }),
    Option: branch({ Some: action, None: Option.none }),
    Iterator: branch({
      Iterator: chain(
        toAction(forEach(chain(toAction(action), toAction(getField("value"))))),
        chain(toAction(flatten()), toAction(tag("Iterator", "Iterator"))),
      ),
    }),
  })));
}
```

**Note:** The `branch({ Iterator: ... })` unwraps `value` (the inner array) via auto-unwrap, so the branch handler receives `T[]` directly. Then `forEach(chain(action, getField("value")))` maps each element through the action (which returns `Iterator<U>`), extracts its inner array, and `flatten()` concatenates the results.

For `filterMethod` — this is currently Option-only. Add Iterator case:

```ts
// Before (filterMethod, line ~666):
function filterMethod(this: TypedAction, predicate: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Some: predicate,
    None: Option.none,
  })));
}

// After — dispatch via matchPrefix to handle both Option and Iterator:
function filterMethod(this: TypedAction, predicate: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Option: branch({
      Some: predicate,
      None: Option.none,
    }),
    Iterator: branch({
      Iterator: chain(
        toAction(forEach(all(identity(), predicate))),
        chain(
          toAction(collectWhere()),
          toAction(tag("Iterator", "Iterator")),
        ),
      ),
    }),
  })));
}
```

**Complication:** The existing `filterMethod` used direct `branch` (no `matchPrefix`) because it was Option-only. Adding Iterator requires wrapping in `matchPrefix`. This changes the AST shape for existing Option.filter calls — they'll now go through `extractPrefix` first. Functionally equivalent but different AST. Existing Option.filter tests should still pass.

Add `collectMethod` Iterator case:

```ts
// Before (collectMethod, line ~685):
function collectMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(Option.collect()));
}

// After — dispatch via matchPrefix:
function collectMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Option: Option.collect(),
    Iterator: branch({
      Iterator: identity(),
    }),
  })));
}
```

**Note:** Iterator's `.collect()` in the branch receives the unwrapped `value` (the inner `T[]`) via branch auto-unwrap, then passes it through with `identity()`.

##### 4.3: Import `collectWhere` in `ast.ts`

**File:** `libs/barnum/src/ast.ts` (line ~16, in the builtins import)

```ts
// Before:
import {
  constant,
  drop,
  extractPrefix,
  flatten as flattenBuiltin,
  getField,
  getIndex,
  identity,
  merge,
  panic,
  pick,
  splitFirst,
  splitLast,
  tag,
  wrapInField,
} from "./builtins/index.js";

// After — add collectWhere:
import {
  collectWhere,
  constant,
  drop,
  extractPrefix,
  flatten as flattenBuiltin,
  getField,
  getIndex,
  identity,
  merge,
  panic,
  pick,
  splitFirst,
  splitLast,
  tag,
  wrapInField,
} from "./builtins/index.js";
```

Also import `flatten` and `forEach` for use in method implementations (forEach is already used in `forEachMethod`; `flatten` is imported as `flattenBuiltin`):

The `flatten` import is already `flattenBuiltin`. Use that name in the method implementations, or add a second reference. The `all` import is needed too:

```ts
// Add at top of ast.ts:
import { all as allStandalone } from "./all.js";
```

Then use `allStandalone` in `filterMethod`.

---

#### Task 5: Tests

**File:** `libs/barnum/tests/iterator.test.ts` (new file)

Following the pattern in `option.test.ts`:
- Type tests (compile-time): verify input/output types of each combinator
- AST structure tests: verify the produced AST shape
- Execution tests (via `runPipeline`): end-to-end behavior

Tests to write:

**Type tests:**
- `Iter.wrap()` — input `T[]`, output `Iterator<T>`
- `Iter.collect()` — input `Iterator<T>`, output `T[]`
- `Iter.map(f)` — input `Iterator<T>`, output `Iterator<U>`
- `Iter.andThen(f)` — input `Iterator<T>`, output `Iterator<U>`
- `Iter.filter(pred)` — input `Iterator<T>`, output `Iterator<T>`
- `Iter.fromOption()` — input `Option<T>`, output `Iterator<T>`
- `Iter.fromResult()` — input `Result<T,E>`, output `Iterator<T>`
- Postfix `.iterate()` on Option — input `Option<T>`, output `Iterator<T>`
- Postfix `.iterate()` on Result — input `Result<T,E>`, output `Iterator<T>`
- Postfix `.map(f)` on Iterator output
- Postfix `.andThen(f)` on Iterator output
- Postfix `.filter(pred)` on Iterator output
- Postfix `.collect()` on Iterator output

**Execution tests:**
- `Iter.wrap()` wraps array: `[1,2,3]` → `{ kind: "Iterator.Iterator", value: [1,2,3] }`
- `Iter.collect()` unwraps: `{ kind: "Iterator.Iterator", value: [1,2,3] }` → `[1,2,3]`
- Round-trip: `pipe(constant([1,2,3]), Iter.wrap(), Iter.collect())` → `[1,2,3]`
- `Iter.map(f)` transforms each element
- `Iter.andThen(f)` flat-maps (f returns Iterator)
- `Iter.filter(pred)` keeps true elements, discards false
- `Iter.filter(pred)` with all-false → empty iterator
- `Iter.filter(pred)` with all-true → same elements
- `.iterate()` on Some → Iterator with one element
- `.iterate()` on None → empty Iterator
- `.iterate()` on Ok → Iterator with one element
- `.iterate()` on Err → empty Iterator
- Full chain: `option.iterate().map(f).collect()`
- Full chain: `result.iterate().filter(pred).collect()`

---

### Phase 2: Migration

#### Task 6: Remove `.map()`, `.andThen()`, `.forEach()` from shared dispatch

**Goal:** These postfix methods no longer dispatch across Option/Result. `.map()` and `.andThen()` only handle Iterator (Option/Result users call `.iterate()` first). `.forEach()` is removed entirely.

##### 6.1: Simplify `mapMethod`

**File:** `libs/barnum/src/ast.ts`

```ts
// Before (after Task 4):
function mapMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: ...,
    Option: ...,
    Iterator: branch({
      Iterator: chain(toAction(forEach(action)), toAction(tag("Iterator", "Iterator"))),
    }),
  })));
}

// After — Iterator only, no matchPrefix needed:
function mapMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(Iter.map(action)));
}
```

##### 6.2: Simplify `andThenMethod`

```ts
// After — Iterator only:
function andThenMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(Iter.andThen(action)));
}
```

##### 6.3: Remove `forEachMethod` and its registration

**File:** `libs/barnum/src/ast.ts`

Delete `forEachMethod` function (line ~509). Remove `forEach` from `Object.defineProperties` (line ~714). Remove the `.forEach()` type signature from the `TypedAction` type (lines ~187-190).

##### 6.4: Remove Option/Result overloads from `.map()` and `.andThen()` type signatures

**File:** `libs/barnum/src/ast.ts`

Remove the Option and Result overloads from `.map()` and `.andThen()` in the `TypedAction` type. Keep only the Iterator overloads.

##### 6.5: Simplify `filterMethod` and `collectMethod`

Remove the `matchPrefix` dispatch — both become Iterator-only:

```ts
function filterMethod(this: TypedAction, predicate: Action): TypedAction {
  return chain(toAction(this), toAction(Iter.filter(predicate)));
}

function collectMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(Iter.collect()));
}
```

Remove the Option `.filter()` overload from `TypedAction` type. The Option `.collect()` type overload is also removed (see Task 7).

##### 6.6: Update TypedAction type signatures

Remove from `TypedAction` type:
- `.forEach()` entirely
- `.map()` Option and Result overloads (keep Iterator overload)
- `.andThen()` Option and Result overloads (keep Iterator overload)
- `.filter()` Option overload (keep Iterator overload)
- `.collect()` Option overload (keep Iterator overload)

---

#### Task 7: Remove `Option.collect()` and `CollectSome` builtin

**Goal:** `Option.collect()` is replaced by `Iterator.filter()`. Remove the namespace method, builtin, and all references.

##### 7.1: Remove from `Option` namespace

**File:** `libs/barnum/src/option.ts` — delete `Option.collect` method (lines ~99-104)

##### 7.2: Remove `CollectSome` from TypeScript `BuiltinKind`

**File:** `libs/barnum/src/ast.ts` — remove `| { kind: "CollectSome" }` from `BuiltinKind` type

##### 7.3: Remove `CollectSome` from Rust `BuiltinKind`

**File:** `crates/barnum_ast/src/lib.rs` — remove `CollectSome` variant
**File:** `crates/barnum_builtins/src/lib.rs` — remove `CollectSome` match arm and tests

##### 7.4: Remove `collectMethod` Option path

Already handled in Task 6.5.

##### 7.5: Update tests

**File:** `libs/barnum/tests/option.test.ts` — remove `Option.collect` type tests, AST tests, and execution tests (lines ~111-116, ~211-216, ~357-377)

---

#### Task 8: Migrate demos

Each demo change below replaces `forEach`/`Option.collect` patterns with Iterator equivalents. The `forEach` standalone combinator still exists (used internally by `Iter.map`) but the postfix `.forEach()` is gone.

##### 8.1: `identify-and-address-refactors/run.ts`

**File:** `demos/identify-and-address-refactors/run.ts`

```ts
// Before (lines 48-68):
runPipeline(
  pipe(
    constant({ folder: srcDir }),
    listTargetFiles,
    forEach(analyze).flatten(),
    forEach(assessWorthiness).then(Option.collect()),
    forEach(
      withResource({
        create: createBranchWorktree,
        action: implementAndReview,
        dispose: deleteWorktree,
      }),
    ),
  ),
);

// After:
runPipeline(
  constant({ folder: srcDir })
    .then(listTargetFiles)
    .then(Iter.wrap())
    .andThen(analyze)
    .filter(assessWorthiness)
    .map(withResource({
      create: createBranchWorktree,
      action: implementAndReview,
      dispose: deleteWorktree,
    }))
    .collect(),
);
```

Update imports: remove `pipe`, `forEach`, `Option`. Add `Iter`.

**Complication:** `analyze` currently returns `Refactor[]` (array). For `.andThen()`, it must return `Iterator<Refactor>`. Either: (a) modify `analyze` handler to wrap its output, or (b) compose: `.andThen(chain(analyze, Iter.wrap()))`. Option (b) avoids changing the handler.

**Complication:** `assessWorthiness` currently returns `Option<T>` (for the `forEach + Option.collect` pattern). For `.filter()`, it must return `boolean`. This requires modifying the handler, or composing: `.filter(chain(assessWorthiness, Option.isSome()))`. Option (b) composes without handler changes.

##### 8.2: `convert-folder-to-ts/run.ts`

**File:** `demos/convert-folder-to-ts/run.ts`

```ts
// Before (line 26):
listFiles.forEach(migrate({ to: "Typescript" })).drop(),

// After:
listFiles.then(Iter.wrap()).map(migrate({ to: "Typescript" })).drop(),
```

##### 8.3: `simple-workflow/run.ts`

**File:** `demos/simple-workflow/run.ts`

```ts
// Before (lines 17-27):
runPipeline(
  listFiles.forEach(
    pipe(
      implementRefactor,
      typeCheckFiles,
      fixTypeErrors,
      commitChanges,
      createPullRequest,
    ),
  ),
);

// After:
runPipeline(
  listFiles.then(Iter.wrap()).map(
    implementRefactor
      .then(typeCheckFiles)
      .then(fixTypeErrors)
      .then(commitChanges)
      .then(createPullRequest),
  ).collect(),
);
```

##### 8.4: `babysit-prs/run.ts`

**File:** `demos/babysit-prs/run.ts`

```ts
// Before (lines 42-64):
runPipeline(
  loop<void, number[]>((recur, done) =>
    pipe(
      forEach(
        bindInput<number>((prNumber) =>
          prNumber.then(checkPR).branch({
            ChecksFailed: fixIssues.drop().then(prNumber)
              .then(tag<"Option", OptionDef<number>, "Some">("Some", "Option")),
            ChecksPassed: landPR.drop()
              .then(tag<"Option", OptionDef<number>, "None">("None", "Option")),
            Landed: drop
              .then(tag<"Option", OptionDef<number>, "None">("None", "Option")),
          }),
        ),
      ),
      Option.collect<number>(),
      classifyRemaining.branch({ ... }),
    ),
  ),
  [101, 102, 103],
);

// After:
runPipeline(
  loop<void, number[]>((recur, done) =>
    Iter.wrap<number>()
      .filter(
        bindInput<number>((prNumber) =>
          prNumber.then(checkPR).branch({
            ChecksFailed: fixIssues.drop().then(constant(true)),
            ChecksPassed: landPR.drop().then(constant(false)),
            Landed: drop.then(constant(false)),
          }),
        ),
      )
      .collect()
      .then(classifyRemaining.branch({
        HasPRs: bindInput<number[], never>((prs) =>
          sleep(10_000).then(prs).then(recur),
        ),
        AllDone: done,
      })),
  ),
  [101, 102, 103],
);
```

**Complication:** The input to the loop body is `number[]`. We need to enter Iterator first. `Iter.wrap()` is a standalone action (`TypedAction<T[], Iterator<T>>`), so `.filter()` chains from it. In the loop, the input is `number[]` which recur provides. So `Iter.wrap()` is the entry point.

**Complication:** The branch handlers change from returning `Option<number>` to returning `boolean`. Side effects (`fixIssues`, `landPR`) still run — the bool just determines whether to keep the element. This changes handler signatures but the side effects are preserved.

##### 8.5: `*/handlers/type-check-fix.ts` (both demos)

**File:** `demos/identify-and-address-refactors/handlers/type-check-fix.ts` (line 148)
**File:** `demos/convert-folder-to-ts/handlers/type-check-fix.ts` (line 148)

```ts
// Before:
HasErrors: forEach(fix).drop().then(recur),

// After:
HasErrors: Iter.wrap<TypeError>().then(Iter.map(fix)).drop().then(recur),
```

The `HasErrors` branch handler receives `TypeError[]` (auto-unwrapped). `Iter.wrap()` wraps it as `Iterator<TypeError>`, then `.map(fix)` runs fix on each, `.drop()` discards the result, and `.then(recur)` loops. `Iter.fromOption()` / `.iterate()` are wrong here — the input is a bare array, not Option/Result.

---

#### Task 9: Update existing tests

##### 9.1: Remove shared dispatch tests

**File:** `libs/barnum/tests/option.test.ts`

Remove or update tests that use `.map()` and `.andThen()` as postfix on Option output — these will no longer work after Phase 2. Specifically:
- "postfix .map on Option output dispatches correctly" (line ~438) — remove
- Any other postfix tests that rely on shared dispatch

##### 9.2: Update `forEach` tests

**File:** `libs/barnum/tests/forEach.test.ts`

Remove tests of the postfix `.forEach()` method. Keep tests of the `forEach` standalone combinator (it's still used internally).

##### 9.3: Update `branch.test.ts`

**File:** `libs/barnum/tests/branch.test.ts`

Tests that use `forEach(fix)` in branch cases should use `Iter.wrap<TypeError>().then(Iter.map(fix))` instead.

##### 9.4: Update `loop.test.ts`

**File:** `libs/barnum/tests/loop.test.ts`

Tests that use `forEach(fix).drop()` in loop bodies should use `Iter.wrap<TypeError>().then(Iter.map(fix)).drop()` instead.

---

### Phase 3: Iterator expansion (future — not part of this implementation)

Methods to add when needed. All compose from existing builtins + Phase 1 Iterator infrastructure unless noted.

| Method | Needs builtin? | Implementation |
|--------|---------------|----------------|
| `.first()` | No | `getField("value")` → `splitFirst()` → `Option.map(getIndex(0).unwrap())` |
| `.last()` | No | `getField("value")` → `splitLast()` → `Option.map(getIndex(1).unwrap())` |
| `.find(pred)` | No | `Iter.filter(pred)` → `Iter.first()` |
| `.splitFirst()` | No | `getField("value")` → `splitFirst()` (independent of `.first()`) |
| `.splitLast()` | No | `getField("value")` → `splitLast()` (independent of `.last()`) |
| `.collectResult()` | **Yes** | New `CollectResult` builtin: fold array, short-circuit on Err |
| `.collectOption()` | **Yes** | New `CollectOption` builtin: fold array, short-circuit on None |
| `.count()` | **Yes** | New `ArrayLength` builtin: `getField("value")` → length |
| `.nth(n)` | No | `getField("value")` → `getIndex(n)` (already returns `Option<T>`) |
| `.any(pred)` | No | `Iter.find(pred)` → `Option.isSome()` |
| `.all(pred)` | Needs design | Name collision with `all()` combinator |
| `.take(n)` | **Yes** | New `Take` builtin |
| `.skip(n)` | **Yes** | New `Skip` builtin |
| `.reverse()` | **Yes** | New `Reverse` builtin |
| `.join(sep)` | **Yes** | New `Join` builtin |
| `.chain(other)` | No | Unwrap both → concat (flatten) → rewrap |
| `.zip(other)` | **Yes** | New `Zip` builtin |
