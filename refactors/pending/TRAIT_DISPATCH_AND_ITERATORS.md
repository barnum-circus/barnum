# Trait Dispatch and Iterators

## Context

Union postfix dispatch (`__union`) gives us a runtime trait system on TypedAction AST nodes. Constructors attach a dispatch table, combinators propagate it, and postfix methods dispatch through it. Currently Option and Result have dispatch tables.

This doc extends the concept to **Iterator** — a type that wraps a sequence and enables uniform iteration methods (`map`, `filter`, `find`, `collect`, etc.) across Option, Result, and arrays.

---

## Current state: `__union` as a trait table

`__union` is a `UnionMethods` object on TypedAction. It maps method names to implementations:

```ts
// optionMethods: UnionMethods
{
  map: (action) => Option.map(action),
  andThen: (action) => Option.andThen(action),
  isSome: () => Option.isSome(),
  // ...
}
```

Postfix methods dispatch through it:

```ts
function mapMethod(this: TypedAction, action: Action): TypedAction {
  const methods = this.__union;
  if (!methods?.map) throw new Error("...");
  return chain(this, methods.map(action)) as TypedAction;
}
```

This is a trait system. `UnionMethods` is the trait. `optionMethods` and `resultMethods` are impl blocks.

---

## Design question: are shared method names a "trait"?

Currently `map`, `andThen`, `flatten`, `unwrapOr` share a single `UnionMethods` dispatch table. This implies they're methods on a shared trait (like Haskell's `Functor`/`Monad`). But Rust doesn't have these traits — `Option::map`, `Result::map`, and `Iterator::map` are just methods that happen to share a name. There's no `Mappable` trait.

However, some methods really are trait methods in Rust:
- `flatten` is a method on the `Iterator` trait (provided method). `Option::flatten` and `Result::flatten` are inherent methods that happen to share the name.
- In barnum, `flatten` dispatches through `__union` for Option/Result and falls back to the array builtin. This makes it behave like a trait method in our system.

Three framings:

**Framing A — Single dispatch table (current):** `UnionMethods` is one big interface with all possible methods. Each type fills in its subset. Simple, works today. Downside: the interface grows unboundedly as we add types (Iterator adds `find`, `count`, `collect`; future types add more).

**Framing B — Per-type dispatch tables:** Each type has its own interface (`OptionMethods`, `ResultMethods`, `IteratorMethods`). `__union` is typed as their union. Method names can overlap without being part of a shared trait — the dispatch table is type-specific.

**Framing C — Named traits with explicit impl:** Define actual trait interfaces (`Mappable`, `Flattenable`, `IntoIterator`) and types register which traits they implement. Most principled, but heavy machinery for what amounts to "these three types all have a `.map()` method."

Current approach (A) works and is pragmatic. The shared `UnionMethods` interface is effectively a vtable — it's a bag of optional method slots. This is fine as long as the number of types stays small (Option, Result, Iterator). If we ever add many more types, we'd want to refactor toward B or C.

**Update:** `__union` is now `{ name: string, methods: UnionMethods } | null` — always present, with a name identifying the type for error messages. This is a step toward Framing B but stays within A's single-table approach.

**Recommendation:** Keep A for now. If Iterator makes the interface unwieldy, refactor to B (per-type dispatch tables, `__union: OptionMethods | ResultMethods | IteratorMethods`). Don't build trait infrastructure (C) until we need it.

---

## Iterator<T> — a wrapper type with its own dispatch

### What is Iterator<T>?

`Iterator<T>` is a wrapper type — like Option and Result — with its own dispatch table (`iteratorMethods`). `.intoIter()` converts Option/Result/Array into an Iterator, which enables a different set of methods.

**Why `.map()` means different things for different types:**

| Type | `.map(f)` | Output |
|------|-----------|--------|
| `Option<T>` | Apply f to Some value | `Option<U>` |
| `Result<T, E>` | Apply f to Ok value | `Result<U, E>` |
| `Iterator<T>` | Apply f to each element | `Iterator<U>` |

Dispatch keeps them straight. `.intoIter()` switches from Option/Result dispatch to Iterator dispatch.

### Runtime representation — tagged wrapper

`Iterator<T>` is a tagged union wrapper, consistent with Option and Result:

```ts
type IteratorDef<T> = { Iterator: T[] };
type Iterator<T> = TaggedUnion<IteratorDef<T>>;
// Runtime: { kind: "Iterator", value: [1, 2, 3] }
```

This means:
- `.intoIter()` wraps the array: `[1, 2, 3]` → `{ kind: "Iterator", value: [1, 2, 3] }`
- Iterator methods operate on `.value` (the inner array), then re-wrap
- `.collect()` unwraps: `{ kind: "Iterator", value: [1, 2, 3] }` → `[1, 2, 3]`
- `__union: iteratorMethods` is attached to the TypedAction for dispatch

Why tagged wrapper over phantom brand:
- Consistent with every other barnum type (Option, Result, all TaggedUnion)
- `.branch()` works on it (you can pattern-match on `{ kind: "Iterator" }`)
- The Rust engine can recognize and optimize it
- Handlers that receive an Iterator see a proper `{ kind, value }` object, not a bare array that happens to be branded

The wrap/unwrap overhead is real but small — it's a Rust builtin (WrapInField/GetField), not a subprocess call.

### IntoIterator — conversion to Iterator

| Self type | `.intoIter()` | Runtime behavior |
|-----------|---------------|------------------|
| `Option<T>` | `Option<T> → Iterator<T>` | Branch: Some → `[value]`, None → `[]`, then wrap |
| `Result<T, E>` | `Result<T, E> → Iterator<T>` | Branch: Ok → `[value]`, Err → `[]`, then wrap |
| `T[]` | `T[] → Iterator<T>` | Wrap in `{ kind: "Iterator", value: array }` |

`intoIter` is a dispatched method on Option and Result. For arrays, it could be a standalone function or a postfix on any `T[]` output (no dispatch needed — just attach `iteratorMethods`).

### Implementation

```ts
// wrapInArray: T → T[]
// all(identity()) produces [T] from T — no new builtin needed
const wrapInArray = all(identity());

// wrapAsIterator: T[] → Iterator<T>
// tag("Iterator") wraps as { kind: "Iterator", value: T[] }
const wrapAsIterator = tag("Iterator");

// Option.intoIter: Option<T> → Iterator<T>
const optionIntoIter = withUnion(
  chain(
    branch({ Some: wrapInArray, None: constant([]) }),
    wrapAsIterator,
  ),
  iteratorMethods,
);

// Result.intoIter: Result<T, E> → Iterator<T>
const resultIntoIter = withUnion(
  chain(
    branch({ Ok: wrapInArray, Err: constant([]) }),
    wrapAsIterator,
  ),
  iteratorMethods,
);

// T[].intoIter: T[] → Iterator<T>
const arrayIntoIter = withUnion(wrapAsIterator, iteratorMethods);
```

---

## Iterator methods

Once you have `Iterator<T>`, these methods are available via `iteratorMethods` dispatch:

### Core (compose from existing AST nodes)

All iterator methods unwrap `{ kind: "Iterator", value: T[] }` → operate on `T[]` → re-wrap. The pattern is: `chain(getField("value"), <array operation>, tag("Iterator"))`.

These align with Rust's `Iterator` trait provided methods:

| Method | Rust equivalent | Signature | Implementation | Notes |
|--------|----------------|-----------|----------------|-------|
| `.map(f)` | `Iterator::map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → rewrap | Per-element transform |
| `.filter(pred)` | `Iterator::filter` | `Iterator<T> → Iterator<T>` | Unwrap → `forEach(pred)` → collectSome → rewrap | pred: `T → Option<T>` (see open questions) |
| `.find(pred)` | `Iterator::find` | `Iterator<T> → Option<T>` | Unwrap → `forEach(pred)` → collectSome → first | Exits Iterator, enters Option. Not short-circuiting (see open questions) |
| `.andThen(f)` | `Iterator::flat_map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → flatten → rewrap | Rust calls this `flat_map`; we use `andThen` for consistency with Option/Result |
| `.flatten()` | `Iterator::flatten` | `Iterator<Iterator<T>> → Iterator<T>` | Unwrap outer → forEach(unwrap inner) → flatten → rewrap | Trait method in Rust, dispatched in barnum |
| `.collect()` | `Iterator::collect` | `Iterator<T> → T[]` | Unwrap (getField("value")) | Exit Iterator. Rust's collect is generic over destination; ours always returns `T[]` |
| `.first()` | `Iterator::next` | `Iterator<T> → Option<T>` | Unwrap → splitFirst → map getIndex(0) | Exit Iterator, enter Option. Rust equivalent is `next()` |
| `.last()` | `Iterator::last` | `Iterator<T> → Option<T>` | Unwrap → splitLast → map getIndex(1) | Exit Iterator, enter Option. Consumes iterator in Rust |
| `.count()` | `Iterator::count` | `Iterator<T> → number` | Unwrap → Arr.length | Needs builtin. Consumes iterator in Rust |
| `.any(pred)` | `Iterator::any` | `Iterator<T> → boolean` | `find(pred).isSome()` | Not short-circuiting (see open questions) |
| `.all(pred)` | `Iterator::all` | `Iterator<T> → boolean` | Needs design | Name collision with `all()` combinator. Not short-circuiting |

### Needs new builtins

| Method | Rust equivalent | Signature | Notes |
|--------|----------------|-----------|-------|
| `.enumerate()` | `Iterator::enumerate` | `Iterator<T> → Iterator<{index: number, value: T}>` | New Rust builtin |
| `.take(n)` | `Iterator::take` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.skip(n)` | `Iterator::skip` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.reverse()` | `Iterator::rev` | `Iterator<T> → Iterator<T>` | Rust: `rev()`. Needs `DoubleEndedIterator` in Rust, but always available on our eager arrays |
| `.join(sep)` | `slice::join` | `Iterator<string> → string` | Not on Iterator trait in Rust — it's on slices. Include for ergonomics |
| `.zip(other)` | `Iterator::zip` | Needs design | |
| `.chain(other)` | `Iterator::chain` | `Iterator<T> → Iterator<T>` | **Name collision** with barnum's `chain()` (sequential composition). Needs rename or resolution |
| `.nth(n)` | `Iterator::nth` | `Iterator<T> → Option<T>` | Indexed access. Trivial: unwrap → getIndex → Option wrap |

### Family transitions

Iterator methods that return a new collection type **change the dispatch table**:

| Method | Returns | Dispatch after | Notes |
|--------|---------|----------------|-------|
| `.map(f)` | `Iterator<U>` | `iteratorMethods` (stay) | |
| `.filter(pred)` | `Iterator<T>` | `iteratorMethods` (stay) | |
| `.flatten()` | `Iterator<T>` | `iteratorMethods` (stay) | Dispatched (trait method), not hardcoded |
| `.andThen(f)` | `Iterator<U>` | `iteratorMethods` (stay) | |
| `.collect()` | `T[]` | null (exit) | |
| `.first()` | `Option<T>` | `optionMethods` (enter Option) | |
| `.last()` | `Option<T>` | `optionMethods` (enter Option) | |
| `.find(pred)` | `Option<T>` | `optionMethods` (enter Option) | |
| `.nth(n)` | `Option<T>` | `optionMethods` (enter Option) | |
| `.count()` | `number` | null (exit) | |
| `.any(pred)` | `boolean` | null (exit) | |
| `.all(pred)` | `boolean` | null (exit) | |

---

## Example chains

```ts
// Option → Iterator → collect
foo.getField("name")      // Option<string>
  .intoIter()              // Iterator<string> = { kind: "Iterator", value: string[] }
  .map(validate)           // Iterator<ValidResult>
  .collect()               // ValidResult[]

// Result → Iterator → find
result                     // Result<User[], Error>
  .intoIter()              // Iterator<User[]>
  .andThen(identity())     // Iterator<User>  (flatMap: map + flatten)
  .find(isAdmin)           // Option<User>
  .unwrapOr(defaultAdmin)  // User

// Array → Iterator → filter
users                      // User[]
  .intoIter()              // Iterator<User>
  .filter(isActive)        // Iterator<User>
  .map(getName)            // Iterator<string>
  .collect()               // string[]
```

---

## Dispatch table

```ts
const iteratorMethods: UnionMethods = {
  // Stay in Iterator family (dispatched):
  map: (action) => Iter.map(action),
  filter: (predicate) => Iter.filter(predicate),
  andThen: (action) => Iter.andThen(action),  // Rust: flat_map
  flatten: () => Iter.flatten(),               // Trait method — dispatched, not hardcoded

  // Exit to Option (dispatched):
  find: (predicate) => Iter.find(predicate),
  first: () => Iter.first(),
  last: () => Iter.last(),

  // Exit to plain value (dispatched):
  collect: () => Iter.collect(),
  count: () => Iter.count(),
  any: (predicate) => Iter.any(predicate),
};
```

Note: `flatten` is notable because it's one of the methods that is dispatched for **all three** families:
- `Option<Option<T>>.flatten()` → dispatched via `optionMethods.flatten`
- `Result<Result<T,E>,E>.flatten()` → dispatched via `resultMethods.flatten`
- `Iterator<Iterator<T>>.flatten()` → dispatched via `iteratorMethods.flatten`
- `T[][].flatten()` → fallback (array builtin, no dispatch)

In Rust, `flatten` on Iterator is a provided trait method. In barnum, it's a dispatched method on all union types, with a fallback for plain arrays.

---

## `UnionMethods` expansion

`__union` is now `{ name: string, methods: UnionMethods } | null`. The `name` field identifies the type for error messages (e.g. "Option", "Result", "Iterator"). `methods` is the dispatch table.

Current `UnionMethods` needs new fields for Iterator-specific methods:

```ts
export interface UnionMethods {
  // ... existing Option/Result methods ...

  // IntoIterator (Option + Result implement this)
  intoIter?: () => Action;

  // Iterator-only (new):
  find?: (predicate: Action) => Action;
  first?: () => Action;
  last?: () => Action;
  count?: () => Action;
  any?: (predicate: Action) => Action;
  nth?: (n: Action) => Action;
  // ... etc
}
```

Shared method names that already exist in `UnionMethods`:
- `map`, `andThen`, `flatten` — already dispatched for Option/Result. Iterator provides different implementations via the same dispatch field.
- `collect` — already used by Option (collect Some values). Iterator.collect is different (unwrap the tagged wrapper), but the same dispatch field works since dispatch selects the right implementation based on which methods table is attached.
- `filter` — already used by Option. Iterator.filter has different semantics (filter a sequence vs. filter a single value).

---

## Open questions

1. **Naming**: `.intoIter()` vs `.iter()` vs `.iterator()`?
   - `.iter()` is concise
   - `.intoIter()` is the Rust convention (consumes self)
   - `.iterator()` is what was originally suggested

2. **Array → Iterator**: How does `.intoIter()` work on `T[]`? Arrays have `__union: null`. Options:
   - Postfix `.intoIter()` on any TypedAction with `T[]` output (hardcoded, not dispatched — wraps in tag("Iterator") and attaches iteratorMethods)
   - Standalone `Iter.fromArray()` combinator
   - Both?

3. **`filter` predicate type**: Rust's `Iterator::filter` takes `&T → bool`. Barnum has no boolean-to-conditional. Two options:
   - `T → Option<T>` (consistent with `Option.filter`, composable as `forEach(pred).collectSome()`)
   - `T → bool` (requires a new `FilterByBool` builtin in Rust)
   - Recommendation: `T → Option<T>`. Different from Rust but internally consistent.

4. **Short-circuit semantics**: Rust's `Iterator::find`, `Iterator::any`, `Iterator::all` all short-circuit. In barnum, `forEach` processes all elements eagerly. True short-circuit needs engine support (early exit from ForEach). For now, compose eagerly. This is a deliberate semantic difference from Rust.

5. **`collect` destination types**: Rust's `Iterator::collect` is generic over the destination type via `FromIterator`. Possible barnum equivalents:
   - `.collect()` → `T[]` (default, like `Vec`)
   - `.toResult()` where `self: Iterator<Result<T, E>>` → `Result<T[], E>` (stop on first Err)
   - `.toOption()` where `self: Iterator<Option<T>>` → `Option<T[]>` (stop on first None)
   - These are separate methods, not generic collect. Each has a `this` constraint.

6. **`.forEach()` vs `.map()` naming**: Current `.forEach(f)` on arrays returns `U[]`. Rust's `Iterator::map` is the equivalent. Resolution:
   - `.forEach()` stays on plain arrays (no dispatch needed)
   - `.map()` on Iterator dispatches via `__union` — different from array's `.forEach()`
   - No ambiguity since arrays have `__union: null`

7. **`chain` naming collision**: Rust's `Iterator::chain` concatenates two iterators. Barnum's `chain()` is sequential composition (the fundamental AST combinator). Options:
   - Rename barnum Iterator's chain to `.concat()` or `.append()`
   - Accept the collision since `chain()` standalone and `.chain()` postfix are different call sites
   - Recommendation: use `.concat()` to avoid confusion

---

## Priority

**Phase 0** (done): `mapOption→map`, `unwrapOr` widening, `mapErr→dispatch`, `Option.transpose`, `flatten` dispatch, `unwrap` (panicking), `Panic` builtin, `__union` → `{ name, methods }` shape

**Phase 1** (Iterator foundation):
- `Iterator<T>` tagged wrapper type + `IteratorDef`
- `iteratorMethods` dispatch table (with `name: "Iterator"`)
- `Option.intoIter()`, `Result.intoIter()`, array `.intoIter()`
- `.map()`, `.filter()`, `.collect()`, `.find()`, `.first()`, `.last()`

**Phase 2** (Iterator expansion):
- `.andThen()` (flat_map), `.flatten()`, `.enumerate()`, `.take()`, `.skip()`
- `.any()`, `.all()`, `.count()`, `.nth()`
- Typed collect destinations: `.toResult()`, `.toOption()`
- Resolve `chain` naming collision (probably `.concat()`)

**Phase 3** (builtins):
- `Arr.length`, `Arr.reverse`, `Arr.join`, etc.
