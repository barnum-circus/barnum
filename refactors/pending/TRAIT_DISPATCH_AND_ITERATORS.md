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
- `.filter(pred)` — `Option<T> → Option<T>` (inherent to Option)
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

```ts
const wrapInArray = all(identity());
// Defined once in the Iter namespace: Iter.wrap = tag("Iterator", "Iterator")
const wrapAsIterator = Iter.wrap;

// Option.iterate: Option<T> → Iterator<T>
const optionIntoIter = branch({
  Some: chain(toAction(wrapInArray), toAction(wrapAsIterator)),
  None: chain(toAction(constant([])), toAction(wrapAsIterator)),
});

// Result.iterate: Result<T, E> → Iterator<T>
const resultIntoIter = branch({
  Ok: chain(toAction(wrapInArray), toAction(wrapAsIterator)),
  Err: chain(toAction(constant([])), toAction(wrapAsIterator)),
});

// T[].iterate: T[] → Iterator<T>
const arrayIntoIter = wrapAsIterator;
```

---

## Iterator methods

All Iterator methods unwrap `{ kind: "Iterator.Iterator", value: T[] }` → operate on `T[]` → re-wrap (for methods that stay in Iterator) or exit (for methods that produce Option, Result, or plain values). The pattern is: `getField("value")` → array operation → `Iter.wrap`.

### Phase 1 — implement now (used in demos)

| Method | Rust equivalent | Signature | Implementation | Notes |
|--------|----------------|-----------|----------------|-------|
| `.map(f)` | `Iterator::map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → rewrap | Per-element transform |
| `.andThen(f)` | `Iterator::flat_map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → flatten → rewrap | Rust calls this `flat_map`; we use `andThen` for consistency |
| `.collect()` | `Iterator::collect` | `Iterator<T> → T[]` | Unwrap (getField("value")) | Exit Iterator |
| `.first()` | `Iterator::next` | `Iterator<T> → Option<T>` | Unwrap → splitFirst → Option wrap | Exit Iterator, enter Option |

### Future — add when needed

| Method | Rust equivalent | Signature | Implementation | Notes |
|--------|----------------|-----------|----------------|-------|
| `.filter(pred)` | `Iterator::filter` | `Iterator<T> → Iterator<T>` | Unwrap → `forEach(pred)` → collectSome → rewrap | pred: `T → Option<T>` (see open questions) |
| `.find(pred)` | `Iterator::find` | `Iterator<T> → Option<T>` | Unwrap → `forEach(pred)` → collectSome → first | Exits Iterator, enters Option. Not short-circuiting |
| `.collectResult()` | `collect::<Result<Vec,E>>` | `Iterator<Result<T,E>> → Result<T[],E>` | Unwrap → fold with short-circuit on Err | Exit Iterator, enter Result |
| `.collectOption()` | `collect::<Option<Vec>>` | `Iterator<Option<T>> → Option<T[]>` | Unwrap → fold with short-circuit on None | Exit Iterator, enter Option |
| `.last()` | `Iterator::last` | `Iterator<T> → Option<T>` | Unwrap → splitLast → Option wrap | Exit Iterator, enter Option |
| `.count()` | `Iterator::count` | `Iterator<T> → number` | Unwrap → Arr.length | Needs builtin |
| `.any(pred)` | `Iterator::any` | `Iterator<T> → boolean` | `find(pred).isSome()` | Not short-circuiting |
| `.all(pred)` | `Iterator::all` | `Iterator<T> → boolean` | Needs design | Name collision with `all()` combinator |

### Needs new builtins

| Method | Rust equivalent | Signature | Notes |
|--------|----------------|-----------|-------|
| `.enumerate()` | `Iterator::enumerate` | `Iterator<T> → Iterator<{index: number, value: T}>` | New Rust builtin |
| `.take(n)` | `Iterator::take` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.skip(n)` | `Iterator::skip` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.reverse()` | `Iterator::rev` | `Iterator<T> → Iterator<T>` | Always available on our eager arrays |
| `.join(sep)` | `slice::join` | `Iterator<string> → string` | Not on Iterator trait in Rust. Include for ergonomics |
| `.zip(other)` | `Iterator::zip` | Needs design | |
| `.concat(other)` | `Iterator::chain` | `Iterator<T> → Iterator<T>` | Renamed to `.concat()` to avoid collision with barnum's `chain()` |
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

// andThen (flat_map): Iterator<T[]> → Iterator<T>
nestedArrays                                 // T[][]
  .iterate()                                 // Iterator<T[]>
  .andThen(iterateArray)                     // Iterator<T>
  .collect()                                 // T[]
```

---

## What Iterator adds

1. **Single transformation interface.** `.map()`, `.andThen()`, `.filter()`, `.find()` — all transformation methods live on Iterator. Option/Result have `.iterate()` to enter, and `.first()`, `.collect()`, etc. to exit.

2. **Typed collect.** `.collectResult()` and `.collectOption()` provide type-directed collection — the Rust `collect::<Result<Vec<T>,E>>()` pattern.

3. **Simpler dispatch.** `.iterate()` dispatches via `matchPrefix` to convert Option/Result into Iterator. But transformation methods (map, filter, etc.) only operate on Iterator — no multi-family dispatch needed.

4. **Iterator-only methods.** find, first, last, count, enumerate, take, skip, etc. — these only make sense on sequences and don't exist on Option/Result.

---

## Open questions

1. ~~**Naming**~~ **Decided:** `.iterate()`.

2. **Array → Iterator**: How does `.iterate()` work on `T[]`? Arrays have no prefix to dispatch on. Options:
   - Postfix `.iterate()` on any TypedAction with `T[]` output (hardcoded, not dispatched — wraps and tags)
   - Standalone `Iter.fromArray()` combinator
   - Both?

3. **`filter` predicate type**: Rust's `Iterator::filter` takes `&T → bool`. Barnum has no boolean-to-conditional. Two options:
   - `T → Option<T>` (consistent with `Option.filter`, composable as `forEach(pred).collectSome()`)
   - `T → bool` (requires a new `FilterByBool` builtin in Rust)
   - Recommendation: `T → Option<T>`. Different from Rust but internally consistent.

4. **Short-circuit semantics**: Rust's `Iterator::find`, `Iterator::any`, `Iterator::all` all short-circuit. In barnum, `forEach` processes all elements eagerly. True short-circuit needs engine support (early exit from ForEach). For now, compose eagerly. Deliberate semantic difference from Rust.

5. **`collect` destination types**: Rust's `Iterator::collect` is generic over the destination type via `FromIterator`. Barnum uses separate named methods:
   - `.collect()` → `T[]` (default, like `Vec`)
   - `.collectResult()`: `Iterator<Result<T, E>> → Result<T[], E>` — if all Ok, returns `Result.Ok(values[])`; on first Err, returns that `Result.Err`. This works because TypedAction carries the type info: the TypeScript types constrain `.collectResult()` to only be callable when the inner type is `Result<T, E>`. At runtime, it unwraps the Iterator, iterates the array branching each element on Ok/Err, and short-circuits on first Err. This is the Rust `Iterator<Result<T,E>>::collect::<Result<Vec<T>,E>>()` equivalent.
   - `.collectOption()`: `Iterator<Option<T>> → Option<T[]>` — if all Some, returns `Option.Some(values[])`; on first None, returns `Option.None`. Same pattern.

6. **`.forEach()` removal**: `.forEach()` is removed as a postfix method on arrays. Arrays use `.iterate().map(f).collect()` for element-wise transforms. The `ForEach` AST node remains — it's the internal mechanism that Iterator's `.map()` compiles to. See "ForEach AST node" section below.

7. **`chain` naming collision**: Use `.concat()` for iterator concatenation to avoid collision with barnum's `chain()`.

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

## Priority

**Phase 0** (done): `matchPrefix` + `ExtractPrefix` prefix-based dispatch, `unwrapOr`, `unwrap`, `mapErr`, `transpose`, `Panic` builtin

**Phase 1** (Iterator foundation — implement now):
- `Iterator<T>` tagged wrapper type + `IteratorDef`
- `Iter` namespace with standalone methods
- `.iterate()` postfix method (uses `matchPrefix` for Option/Result, direct wrap for arrays)
- Iterator postfix methods: `.map()`, `.andThen()`, `.collect()`, `.first()`

**Phase 2** (migration — remove shared postfix methods):
- Remove `.map()`, `.andThen()`, `.forEach()` postfix methods from Option/Result/arrays
- Update all demos to use `.iterate()` → Iterator methods → exit pattern
- Remove multi-family `matchPrefix` dispatch from `mapMethod`, `andThenMethod`

**Phase 3** (Iterator expansion — add when needed):
- `.filter()`, `.find()`, `.last()`
- `.collectResult()`, `.collectOption()`
- `.any()`, `.all()`, `.count()`, `.nth()`
- `.enumerate()`, `.take()`, `.skip()`

**Phase 4** (builtins):
- `Arr.length`, `Arr.reverse`, `Arr.join`, etc.
