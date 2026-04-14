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

| Method | Signature | Implementation | Notes |
|--------|-----------|----------------|-------|
| `.map(f)` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → rewrap | Per-element transform |
| `.filter(pred)` | `Iterator<T> → Iterator<T>` | Unwrap → `forEach(pred)` → collectSome → rewrap | pred: `T → Option<T>` |
| `.find(pred)` | `Iterator<T> → Option<T>` | Unwrap → `forEach(pred)` → collectSome → first | Exits Iterator, enters Option |
| `.andThen(f)` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → flatten → rewrap | Map + flatten (Rust: `flat_map`) |
| `.flatten()` | `Iterator<Iterator<T>> → Iterator<T>` | Unwrap outer → forEach(unwrap inner) → flatten → rewrap | Flatten one level |
| `.collect()` | `Iterator<T> → T[]` | Unwrap (getField("value")) | Exit Iterator, get plain array |
| `.first()` | `Iterator<T> → Option<T>` | Unwrap → splitFirst → map getIndex(0) | Exit Iterator, enter Option |
| `.last()` | `Iterator<T> → Option<T>` | Unwrap → splitLast → map getIndex(1) | Exit Iterator, enter Option |
| `.count()` | `Iterator<T> → number` | Unwrap → Arr.length | Needs builtin |
| `.any(pred)` | `Iterator<T> → boolean` | `find(pred).isSome()` | Not short-circuiting |
| `.all(pred)` | `Iterator<T> → boolean` | Needs design | Name collision with `all()` combinator |

### Needs new builtins

| Method | Signature | Notes |
|--------|-----------|-------|
| `.enumerate()` | `Iterator<T> → Iterator<{index: number, value: T}>` | New Rust builtin |
| `.take(n)` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.skip(n)` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.reverse()` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.join(sep)` | `Iterator<string> → string` | New Rust builtin |
| `.zip(other)` | Needs design | |

### Family transitions

Iterator methods that return a new collection type **change the dispatch table**:

| Method | Returns | Dispatch after |
|--------|---------|----------------|
| `.map(f)` | `Iterator<U>` | `iteratorMethods` (stay in Iterator) |
| `.filter(pred)` | `Iterator<T>` | `iteratorMethods` (stay) |
| `.collect()` | `T[]` | None (plain array) |
| `.first()` | `Option<T>` | `optionMethods` (enter Option) |
| `.last()` | `Option<T>` | `optionMethods` (enter Option) |
| `.find(pred)` | `Option<T>` | `optionMethods` (enter Option) |
| `.count()` | `number` | None (exit) |
| `.any(pred)` | `boolean` | None (exit) |

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
  map: (action) => Iter.map(action),
  filter: (predicate) => Iter.filter(predicate),
  andThen: (action) => Iter.andThen(action),  // Rust: flat_map
  flatten: () => Iter.flatten(),
  // find, first, last exit Iterator — return Option with optionMethods
  // collect exits Iterator — returns plain T[]
  // count, any, all exit Iterator — return primitive
};
```

---

## `UnionMethods` expansion

Current `UnionMethods` needs new fields for Iterator-specific methods:

```ts
export interface UnionMethods {
  // ... existing Option/Result methods ...

  // IntoIterator (Option + Result implement this)
  intoIter?: () => Action;

  // Iterator-only
  // map, filter, flatten, andThen are shared names — dispatch handles them
  // These are NEW:
  find?: (predicate: Action) => Action;
  count?: () => Action;
  collect?: () => Action;  // Iterator.collect exits to T[]
  // ... etc
}
```

`map`, `filter`, `flatten`, `andThen` are already in `UnionMethods` (shared by Option/Result). Iterator provides different implementations. `collect` already exists (Option.collect) — Iterator.collect is different (unwrap the tagged wrapper), but the same dispatch field works.

---

## Open questions

1. **Naming**: `.intoIter()` vs `.iter()` vs `.iterator()`?
   - `.iter()` is concise
   - `.intoIter()` is the Rust convention
   - `.iterator()` is what was originally suggested

2. **Array → Iterator**: How does `.intoIter()` work on `T[]`? Arrays don't have `__union`. Options:
   - Postfix `.intoIter()` on any TypedAction with `T[]` output (hardcoded, not dispatched — wraps in tag("Iterator") and attaches iteratorMethods)
   - Standalone `Iter.fromArray()` combinator
   - Both?

3. **`filter` predicate type**: Rust's filter takes `T → bool`. Barnum has no boolean-to-conditional. Two options:
   - `T → Option<T>` (consistent with `Option.filter`, composable as `forEach(pred).collectSome()`)
   - `T → bool` (requires a new `FilterByBool` builtin in Rust)
   - Recommendation: `T → Option<T>`. Different from Rust but internally consistent.

4. **Short-circuit semantics**: `find`, `any`, `all` in Rust short-circuit. In barnum, `forEach` processes all elements. True short-circuit needs engine support (early exit from ForEach). For now, compose eagerly.

5. **`collect` destination types**: In Rust, `.collect()` is generic over the destination type. Possible barnum equivalents:
   - `.collect()` → `T[]` (default, like `Vec`)
   - `.toResult()` where `self: Iterator<Result<T, E>>` → `Result<T[], E>` (stop on first Err)
   - `.toOption()` where `self: Iterator<Option<T>>` → `Option<T[]>` (stop on first None)
   - These are separate methods, not generic collect. Each has a `this` constraint.

6. **`.forEach()` vs `.map()` naming**: Current `.forEach(f)` on arrays returns `U[]`. On `Iterator<T>`, `.map(f)` is the same operation but returns `Iterator<U>`. Resolution:
   - `.forEach()` stays on plain arrays (no dispatch needed)
   - `.map()` on Iterator dispatches via `__union` — different from array's `.forEach()`
   - No ambiguity since arrays don't have `__union`

---

## Priority

**Phase 0** (done): `mapOption→map`, `unwrapOr` widening, `mapErr→dispatch`, `Option.transpose`, `flatten` dispatch

**Phase 1** (Iterator foundation):
- `Iterator<T>` tagged wrapper type + `IteratorDef`
- `iteratorMethods` dispatch table
- `Option.intoIter()`, `Result.intoIter()`, array `.intoIter()`
- `.map()`, `.filter()`, `.collect()`, `.find()`, `.first()`, `.last()`

**Phase 2** (Iterator expansion):
- `.andThen()` (flat_map), `.flatten()`, `.enumerate()`, `.take()`, `.skip()`
- `.any()`, `.count()`
- Typed collect destinations: `.toResult()`, `.toOption()`

**Phase 3** (builtins):
- `Arr.length`, `Arr.reverse`, `Arr.join`, etc.
