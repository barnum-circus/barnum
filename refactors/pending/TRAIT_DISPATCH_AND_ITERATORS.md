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

### Runtime representation

Handlers only accept JSON. There's no lazy iterator in JSON-land. At runtime, `Iterator<T>` **must** be `T[]` — the only sequence type in JSON.

The wrapping is purely at the type level + AST dispatch level:
- `__union` lives on the TypedAction (AST node), not on the runtime value
- `.intoIter()` produces a TypedAction with `__union: iteratorMethods`
- TypeScript `this` constraints gate which methods are available
- `.collect()` is identity at runtime — it just changes the type back to `T[]` and drops the iterator dispatch

Two options for the type-level representation:

**Option A — Branded phantom type:**
```ts
type Iterator<T> = T[] & { __iter?: T };
```
Runtime value is `T[]`. Phantom `__iter` brand distinguishes it from plain arrays for TypeScript overload resolution. No runtime overhead.

**Option B — Tagged wrapper:**
```ts
type Iterator<T> = TaggedUnion<{ Iterator: T[] }>;
// Runtime: { kind: "Iterator", value: [1, 2, 3] }
```
Consistent with barnum's tagged union convention. Adds wrap/unwrap overhead at every step.

**Recommendation: Option A (phantom brand).** The dispatch is on the AST node (`__union`), not the runtime value. Wrapping/unwrapping at every `.map()` step is pure overhead. The brand is enough for TypeScript to distinguish Iterator from plain array.

### IntoIterator — conversion to Iterator

| Self type | `.intoIter()` | Runtime behavior |
|-----------|---------------|------------------|
| `Option<T>` | `Option<T> → Iterator<T>` | Branch: Some → `[value]`, None → `[]` |
| `Result<T, E>` | `Result<T, E> → Iterator<T>` | Branch: Ok → `[value]`, Err → `[]` |
| `T[]` | `T[] → Iterator<T>` | Identity (already an array, just changes dispatch) |

`intoIter` is a dispatched method on Option and Result. For arrays, it could be a standalone function or a postfix on any `T[]` output (no dispatch needed — just attach `iteratorMethods`).

### Implementation

```ts
// wrapInArray: T → T[]
// all(identity()) produces [T] from T — no new builtin needed
const wrapInArray = all(identity());

// Option.intoIter: Option<T> → Iterator<T>
const optionIntoIter = withUnion(
  branch({ Some: wrapInArray, None: constant([]) }),
  iteratorMethods,
);

// Result.intoIter: Result<T, E> → Iterator<T>
const resultIntoIter = withUnion(
  branch({ Ok: wrapInArray, Err: constant([]) }),
  iteratorMethods,
);
```

---

## Iterator methods

Once you have `Iterator<T>`, these methods are available via `iteratorMethods` dispatch:

### Core (compose from existing AST nodes)

| Method | Signature | Implementation | Notes |
|--------|-----------|----------------|-------|
| `.map(f)` | `Iterator<T> → Iterator<U>` | `forEach(f)` | Per-element transform. Returns Iterator, not plain array. |
| `.filter(pred)` | `Iterator<T> → Iterator<T>` | `forEach(pred).then(collectSome())` | pred: `T → Option<T>`. Keeps Somes, drops Nones. |
| `.find(pred)` | `Iterator<T> → Option<T>` | `filter(pred).collect().first()` | First match. Exits Iterator, enters Option. |
| `.flatMap(f)` | `Iterator<T> → Iterator<U>` | `forEach(f).then(flatten())` | Map + flatten one level |
| `.flatten()` | `Iterator<Iterator<T>> → Iterator<T>` | Flatten builtin | Flatten one level of nesting |
| `.collect()` | `Iterator<T> → T[]` | Identity at runtime | Exit Iterator, get plain array. Drops iteratorMethods dispatch. |
| `.first()` | `Iterator<T> → Option<T>` | splitFirst + map getIndex(0) | Exit Iterator, enter Option. |
| `.last()` | `Iterator<T> → Option<T>` | splitLast + map getIndex(1) | Exit Iterator, enter Option. |
| `.count()` | `Iterator<T> → number` | Needs `Arr.length` builtin | |
| `.any(pred)` | `Iterator<T> → boolean` | `find(pred).isSome()` | Not short-circuiting (evaluates all elements) |
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
| `.collect()` | `T[]` | None (plain array, use `.forEach()` etc. directly) |
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
  .intoIter()              // Iterator<string>
  .map(validate)           // Iterator<ValidResult>
  .collect()               // ValidResult[]

// Result → Iterator → find
result                     // Result<User[], Error>
  .intoIter()              // Iterator<User[]>
  .flatMap(identity())     // Iterator<User>  (flatten the inner array)
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
  flatMap: (action) => Iter.flatMap(action),
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

  // Iterator-only (iteratorMethods implements these)
  // map, filter, flatten are shared names — dispatch handles them
  // These are NEW:
  flatMap?: (action: Action) => Action;
  find?: (predicate: Action) => Action;
  count?: () => Action;
  // ... etc
}
```

Note: `map`, `filter`, `flatten` are already in `UnionMethods` (shared by Option/Result). Iterator just provides different implementations for them. No new interface fields needed for those — just different values in the dispatch table.

---

## Open questions

1. **Naming**: `.intoIter()` vs `.iter()` vs `.iterator()`?
   - `.iter()` is concise
   - `.intoIter()` is the Rust convention
   - `.iterator()` is what was originally suggested

2. **Array → Iterator**: How does `.intoIter()` work on `T[]`? Arrays don't have `__union`. Options:
   - Standalone `intoIter()` combinator that attaches `iteratorMethods` (identity at runtime)
   - Postfix `.intoIter()` on any TypedAction with `T[]` output (hardcoded, not dispatched)
   - Skip it — arrays already have `.forEach()`, `.first()`, `.last()` as postfix methods. Only Option/Result need `.intoIter()`.

3. **`filter` predicate type**: Rust's filter takes `T → bool`. Barnum has no boolean-to-conditional. Two options:
   - `T → Option<T>` (consistent with `Option.filter`, composable as `forEach(pred).collect()`)
   - `T → bool` (requires a new `FilterByBool` builtin in Rust)
   - Recommendation: `T → Option<T>`. Different from Rust but internally consistent.

4. **Short-circuit semantics**: `find`, `any`, `all` in Rust short-circuit. In barnum, `forEach` processes all elements. True short-circuit needs engine support (early exit from ForEach). For now, compose eagerly.

5. **`collect` destination**: In Rust, `.collect()` is generic over the destination type (`collect::<Vec<_>>()`, `collect::<HashMap<_, _>>()`). In barnum, `.collect()` always produces `T[]`. If we later add HashMap, we might want `collectInto<HashMap>()` or similar. Cross that bridge when we get there.

6. **`.forEach()` ambiguity**: Current `.forEach(f)` on arrays returns `U[]`. On `Iterator<T>`, `.map(f)` is the same operation but returns `Iterator<U>`. Should we:
   - Keep `.forEach()` on plain arrays, `.map()` on Iterator (different names for same operation)?
   - Rename array `.forEach()` to `.map()` via dispatch?
   - For now: `.forEach()` stays on arrays (no dispatch needed). `.map()` is Iterator-only via dispatch.

---

## Priority

**Phase 0** (independent, do now): `mapOption→map`, `unwrapOr` widening, `mapErr→dispatch`, `Option.transpose`

**Phase 1** (Iterator foundation):
- `Iterator<T>` type (branded phantom)
- `iteratorMethods` dispatch table
- `Option.intoIter()`, `Result.intoIter()`
- `.map()`, `.filter()`, `.collect()`, `.find()`, `.first()`, `.last()`

**Phase 2** (Iterator expansion):
- `.flatMap()`, `.flatten()`, `.enumerate()`, `.take()`, `.skip()`
- `.any()`, `.count()`
- Array `.intoIter()` (if needed)

**Phase 3** (builtins):
- `Arr.length`, `Arr.reverse`, `Arr.join`, etc.
