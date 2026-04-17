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
array.iterate().map(process).collect()
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

`Iterator<T>` is a tagged union wrapper — like Option and Result — with namespaced kind: `"Iterator.Iterator"`. `.iterate()` converts Option/Result/Array into an Iterator, which is the single place where transformation methods live.

### Runtime representation — tagged wrapper

```ts
type IteratorDef<T> = { Iterator: T[] };
type Iterator<T> = TaggedUnion<"Iterator", IteratorDef<T>>;
// Runtime: { kind: "Iterator.Iterator", value: [1, 2, 3] }
```

This means:
- `.iterate()` wraps the array: `[1, 2, 3]` → `{ kind: "Iterator.Iterator", value: [1, 2, 3] }`
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

| Self type | `.iterate()` | Runtime behavior |
|-----------|---------------|------------------|
| `Option<T>` | `Option<T> → Iterator<T>` | Branch: Some → `[value]`, None → `[]`, then wrap |
| `Result<T, E>` | `Result<T, E> → Iterator<T>` | Branch: Ok → `[value]`, Err → `[]`, then wrap |
| `T[]` | `T[] → Iterator<T>` | Wrap in `{ kind: "Iterator.Iterator", value: array }` |

`.iterate()` is a postfix method that uses `matchPrefix` for Option/Result. For arrays, no prefix dispatch needed — just wrap and tag.

### Implementation

`.iterate()` uses `matchPrefix` for Option/Result (need to know the variant), direct wrap for arrays:

```ts
// Option.iterate / Result.iterate — dispatches via matchPrefix
// Some(value) / Ok(value) → Iterator([value])
// None / Err(_)           → Iterator([])

// T[].iterate — no dispatch needed, just wrap
// [1, 2, 3] → { kind: "Iterator.Iterator", value: [1, 2, 3] }
```

---

## Iterator methods

All Iterator methods unwrap `{ kind: "Iterator.Iterator", value: T[] }` → operate on `T[]` → re-wrap (for methods that stay in Iterator) or exit (for methods that produce Option, Result, or plain values). The pattern is: `getField("value")` → array operation → `Iter.wrap`.

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
  .iterate()                                 // Iterator<User>
  .map(getName)                              // Iterator<string>
  .collect()                                 // string[]

// Array → Iterator → map → collect
users                                        // User[]
  .iterate()                                 // Iterator<User>
  .map(getName)                              // Iterator<string>
  .collect()                                 // string[]

// andThen (monadic bind): Iterator<File> → Iterator<Refactor>
files                                        // File[]
  .iterate()                                 // Iterator<File>
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

2. ~~**Array → Iterator**~~ **Decided:** Postfix `.iterate()` on any TypedAction with `T[]` output. Hardcoded, not dispatched — just wraps and tags via `Iter.wrap`. No `matchPrefix` needed for arrays since they have no prefix.

3. ~~**`filter` predicate type**~~ **Decided:** `T → bool`. New `Filter` Rust builtin. Consistent with Rust's `Iterator::filter`.

4. ~~**Short-circuit semantics**~~ **Not an issue now:** All Phase 1 methods (map, andThen, filter, collect) are inherently non-short-circuiting. Short-circuit matters for find/any/all — those are future phases.

5. **`collect` destination types**: Rust's `Iterator::collect` is generic over the destination type via `FromIterator`. Barnum uses separate named methods:
   - `.collect()` → `T[]` (default, like `Vec`)
   - `.collectResult()`: `Iterator<Result<T, E>> → Result<T[], E>` — if all Ok, returns `Result.Ok(values[])`; on first Err, returns that `Result.Err`. This works because TypedAction carries the type info: the TypeScript types constrain `.collectResult()` to only be callable when the inner type is `Result<T, E>`. At runtime, it unwraps the Iterator, iterates the array branching each element on Ok/Err, and short-circuits on first Err. This is the Rust `Iterator<Result<T,E>>::collect::<Result<Vec<T>,E>>()` equivalent.
   - `.collectOption()`: `Iterator<Option<T>> → Option<T[]>` — if all Some, returns `Option.Some(values[])`; on first None, returns `Option.None`. Same pattern.

6. **`.forEach()` removal**: `.forEach()` is removed as a postfix method on arrays. Arrays use `.iterate().map(f).collect()` for element-wise transforms. The `ForEach` AST node remains — it's the internal mechanism that Iterator's `.map()` compiles to. See "ForEach AST node" section below.

7. ~~**`chain` naming collision**~~ **Not an issue:** barnum's `chain()` is an internal combinator; users see `.then()`. Iterator can use `.chain()` for concatenation without ambiguity.

---

## ForEach AST node

`ForEach` is a fundamental AST node — `{ kind: "ForEach", action: Action }` applies an action to every element of an array. It's how the Rust engine does element-wise operations. Currently it's exposed as:

1. **Standalone combinator:** `forEach(action)` — `TypedAction<T[], U[]>`. Used internally by Iterator's `.map()`.
2. **Postfix method:** `array.forEach(f)` — sugar for `chain(array, forEach(f))`.

With Iterator as the sole transformation interface, the **postfix method is removed**. Users write `array.iterate().map(f).collect()` instead of `array.forEach(f)`.

The **standalone combinator stays** — it's the implementation mechanism for Iterator's `.map()`:

```ts
// Iterator.map internally:
chain(getField("value"), forEach(action), Iter.wrap)
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
  .iterate()
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
  .iterate()                                    // T[] → Iterator<T>
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
listFiles.iterate().map(migrate({ to: "Typescript" })).drop(),
```

### `simple-workflow/run.ts`

```ts
// BEFORE (line 18):
listFiles.forEach(pipe(implementRefactor, typeCheckFiles, fixTypeErrors, commitChanges, createPullRequest)),

// AFTER:
listFiles.iterate().map(
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
HasErrors: Iter.iterate().map(fix).drop().then(recur),
```

---

## Testing

Per `refactors/PROCESS.md`, every implementation task follows test-first:

1. **Commit 1:** Add failing tests for Iterator type, `Iter` namespace methods, `.iterate()` postfix, and each Iterator postfix method (map, andThen, filter, collect, first). Tests assert correct behavior but fail because the implementation doesn't exist yet.
2. **Commit 2:** Implement the feature, making tests pass.
3. **Commit 3:** Remove failure markers.

Tests should cover:
- `Iter.wrap` produces `{ kind: "Iterator.Iterator", value: [...] }`
- `Iter.map(f)` transforms each element and re-wraps
- `Iter.andThen(f)` flat-maps and re-wraps
- `Iter.filter(pred)` keeps elements where `pred` returns true, re-wraps
- `Iter.collect()` unwraps to plain array
- `.iterate()` on Option (Some → `[value]`, None → `[]`)
- `.iterate()` on Result (Ok → `[value]`, Err → `[]`)
- `.iterate()` on arrays (direct wrap)
- Phase 2 migration: existing tests that use `.map()`, `.forEach()`, `.andThen()` on Option/Result are updated to use Iterator equivalents

---

## Priority

**Phase 0** (done): `matchPrefix` + `ExtractPrefix` prefix-based dispatch, `unwrapOr`, `unwrap`, `mapErr`, `transpose`, `Panic` builtin

**Phase 1** (Iterator foundation — implement now):
- `Iterator<T>` tagged wrapper type + `IteratorDef`
- `Iter` namespace with standalone methods (`Iter.wrap`, `Iter.map`, `Iter.andThen`, `Iter.filter`, `Iter.collect`, `Iter.iterate`)
- `.iterate()` postfix method (uses `matchPrefix` for Option/Result, direct wrap for arrays)
- Iterator postfix methods: `.map()`, `.andThen()`, `.filter()`, `.collect()`
- Tests for all of the above

**Phase 2** (migration — remove shared postfix methods):
- Remove `.map()`, `.andThen()`, `.forEach()` postfix methods from Option/Result/arrays
- Update all demos to use `.iterate()` → Iterator methods → exit pattern
- Remove multi-family `matchPrefix` dispatch from `mapMethod`, `andThenMethod`

**Phase 3** (Iterator expansion — builtins as needed):
- `.find(pred)`, `.last()`, `.first()`
- `.splitFirst()`, `.splitLast()` (independent of `first`/`last`)
- `.collectResult()`, `.collectOption()`
- `.any(pred)`, `.all(pred)`, `.count()`, `.nth(n)` (returns `Option<T>`)
- `.take(n)`, `.skip(n)`
- No `.enumerate()` — not planned
