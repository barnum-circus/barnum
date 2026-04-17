# Trait Dispatch and Iterators

## Context

Dynamic dispatch in barnum uses **prefix-based dispatch** via the `ExtractPrefix` builtin and `matchPrefix` combinator. Tagged union values carry namespaced kind strings (`"Option.Some"`, `"Result.Ok"`). `ExtractPrefix` splits on `'.'` to restructure the value so `branch()` can dispatch on the family first, then the variant. No runtime dispatch tables — the AST encodes the dispatch.

This doc extends the concept to **Iterator** — a type that wraps a sequence and enables uniform iteration methods (`map`, `filter`, `find`, `collect`, etc.) across Option, Result, and arrays.

---

## Current state: prefix-based dispatch

Tagged values self-describe their family via namespaced kind strings. Postfix methods compose `extractPrefix()` with `branch()` to dispatch across families:

```ts
// matchPrefix = chain(extractPrefix(), branch(cases))
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

Execution trace for `{ kind: "Result.Ok", value: 42 }` through `.map(f)`:

1. `ExtractPrefix` → `{ kind: "Result", value: { kind: "Result.Ok", value: 42 } }`
2. Outer `branch` matches `"Result"`, auto-unwraps → `{ kind: "Result.Ok", value: 42 }`
3. Inner `branch` matches `"Ok"`, auto-unwraps → `42`
4. `f` runs on `42`, then re-wraps via `Result.ok`

Family-specific methods (e.g., `mapErr` on Result, `filter` on Option) skip prefix dispatch and branch directly on variant names.

---

## Design question: how does Iterator fit into prefix dispatch?

Adding Iterator means postfix methods that are shared across all three families (map, andThen, flatten) grow a third case in the `matchPrefix` call:

```ts
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
    Iterator: /* iterator map implementation */,
  })));
}
```

This is the natural extension of the prefix dispatch system — no new mechanism needed. The AST grows by one branch per shared method per new family. With 3 built-in families, shared methods get 3 inner branches. Only one executes per invocation.

Iterator-only methods (find, first, last, collect, count) don't need `matchPrefix` at all — they branch directly on `"Iterator"` variant, just like Result-only methods branch on Ok/Err.

---

## Iterator<T> — a wrapper type with its own prefix

### What is Iterator<T>?

`Iterator<T>` is a tagged union wrapper — like Option and Result — with namespaced kind: `"Iterator.Iterator"`. `.intoIter()` converts Option/Result/Array into an Iterator, which enables a different set of methods.

**Why `.map()` means different things for different types:**

| Type | `.map(f)` | Output |
|------|-----------|--------|
| `Option<T>` | Apply f to Some value | `Option<U>` |
| `Result<T, E>` | Apply f to Ok value | `Result<U, E>` |
| `Iterator<T>` | Apply f to each element | `Iterator<U>` |

Prefix dispatch keeps them straight. `.intoIter()` switches from Option/Result to Iterator dispatch.

### Runtime representation — tagged wrapper

`Iterator<T>` is a tagged union wrapper, consistent with Option and Result:

```ts
type IteratorDef<T> = { Iterator: T[] };
type Iterator<T> = TaggedUnion<"Iterator", IteratorDef<T>>;
// Runtime: { kind: "Iterator.Iterator", value: [1, 2, 3] }
```

This means:
- `.intoIter()` wraps the array: `[1, 2, 3]` → `{ kind: "Iterator.Iterator", value: [1, 2, 3] }`
- Iterator methods operate on `.value` (the inner array), then re-wrap
- `.collect()` unwraps: `{ kind: "Iterator.Iterator", value: [1, 2, 3] }` → `[1, 2, 3]`
- `matchPrefix` dispatches on the `"Iterator"` prefix

Why tagged wrapper over phantom brand:
- Consistent with every other barnum type (Option, Result, all TaggedUnion)
- `.branch()` works on it (you can pattern-match on `{ kind: "Iterator.Iterator" }`)
- The Rust engine can recognize and optimize it
- Handlers that receive an Iterator see a proper `{ kind, value }` object, not a bare array that happens to be branded

The wrap/unwrap overhead is real but small — it's a Rust builtin (WrapInField/GetField), not a subprocess call.

### IntoIterator — conversion to Iterator

| Self type | `.intoIter()` | Runtime behavior |
|-----------|---------------|------------------|
| `Option<T>` | `Option<T> → Iterator<T>` | Branch: Some → `[value]`, None → `[]`, then wrap |
| `Result<T, E>` | `Result<T, E> → Iterator<T>` | Branch: Ok → `[value]`, Err → `[]`, then wrap |
| `T[]` | `T[] → Iterator<T>` | Wrap in `{ kind: "Iterator.Iterator", value: array }` |

`intoIter` is a postfix method that uses `matchPrefix` for Option/Result. For arrays, it could be a standalone function or a postfix on any `T[]` output (no prefix dispatch needed — just wraps and tags).

### Implementation

```ts
// wrapInArray: T → T[]
// all(identity()) produces [T] from T — no new builtin needed
const wrapInArray = all(identity());

// wrapAsIterator: T[] → Iterator<T>
// tag("Iterator", "Iterator") wraps as { kind: "Iterator.Iterator", value: T[] }
const wrapAsIterator = tag("Iterator", "Iterator");

// Option.intoIter: Option<T> → Iterator<T>
// Uses branch directly — this is a standalone namespace method
const optionIntoIter = branch({
  Some: chain(toAction(wrapInArray), toAction(wrapAsIterator)),
  None: chain(toAction(constant([])), toAction(wrapAsIterator)),
});

// Result.intoIter: Result<T, E> → Iterator<T>
const resultIntoIter = branch({
  Ok: chain(toAction(wrapInArray), toAction(wrapAsIterator)),
  Err: chain(toAction(constant([])), toAction(wrapAsIterator)),
});

// T[].intoIter: T[] → Iterator<T>
const arrayIntoIter = wrapAsIterator;
```

---

## Iterator methods

Once you have `Iterator<T>`, these methods are available as postfix methods via prefix dispatch or as standalone `Iter.*` namespace methods.

### Core (compose from existing AST nodes)

All iterator methods unwrap `{ kind: "Iterator.Iterator", value: T[] }` → operate on `T[]` → re-wrap. The pattern is: `chain(getField("value"), <array operation>, tag("Iterator", "Iterator"))`.

These align with Rust's `Iterator` trait provided methods:

| Method | Rust equivalent | Signature | Implementation | Notes |
|--------|----------------|-----------|----------------|-------|
| `.map(f)` | `Iterator::map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → rewrap | Per-element transform |
| `.filter(pred)` | `Iterator::filter` | `Iterator<T> → Iterator<T>` | Unwrap → `forEach(pred)` → collectSome → rewrap | pred: `T → Option<T>` (see open questions) |
| `.find(pred)` | `Iterator::find` | `Iterator<T> → Option<T>` | Unwrap → `forEach(pred)` → collectSome → first | Exits Iterator, enters Option. Not short-circuiting (see open questions) |
| `.andThen(f)` | `Iterator::flat_map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → flatten → rewrap | Rust calls this `flat_map`; we use `andThen` for consistency with Option/Result |
| `.flatten()` | `Iterator::flatten` | `Iterator<Iterator<T>> → Iterator<T>` | Unwrap outer → forEach(unwrap inner) → flatten → rewrap | |
| `.collect()` | `Iterator::collect` | `Iterator<T> → T[]` | Unwrap (getField("value")) | Exit Iterator. Rust's collect is generic over destination; ours always returns `T[]` |
| `.first()` | `Iterator::next` | `Iterator<T> → Option<T>` | Unwrap → splitFirst → map getIndex(0) | Exit Iterator, enter Option |
| `.last()` | `Iterator::last` | `Iterator<T> → Option<T>` | Unwrap → splitLast → map getIndex(1) | Exit Iterator, enter Option |
| `.count()` | `Iterator::count` | `Iterator<T> → number` | Unwrap → Arr.length | Needs builtin |
| `.any(pred)` | `Iterator::any` | `Iterator<T> → boolean` | `find(pred).isSome()` | Not short-circuiting (see open questions) |
| `.all(pred)` | `Iterator::all` | `Iterator<T> → boolean` | Needs design | Name collision with `all()` combinator. Not short-circuiting |

### Needs new builtins

| Method | Rust equivalent | Signature | Notes |
|--------|----------------|-----------|-------|
| `.enumerate()` | `Iterator::enumerate` | `Iterator<T> → Iterator<{index: number, value: T}>` | New Rust builtin |
| `.take(n)` | `Iterator::take` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.skip(n)` | `Iterator::skip` | `Iterator<T> → Iterator<T>` | New Rust builtin |
| `.reverse()` | `Iterator::rev` | `Iterator<T> → Iterator<T>` | Rust: `rev()`. Always available on our eager arrays |
| `.join(sep)` | `slice::join` | `Iterator<string> → string` | Not on Iterator trait in Rust — it's on slices. Include for ergonomics |
| `.zip(other)` | `Iterator::zip` | Needs design | |
| `.concat(other)` | `Iterator::chain` | `Iterator<T> → Iterator<T>` | Renamed to `.concat()` to avoid collision with barnum's `chain()` |
| `.nth(n)` | `Iterator::nth` | `Iterator<T> → Option<T>` | Indexed access. Trivial: unwrap → getIndex → Option wrap |

### Family transitions

Iterator methods that return a new collection type **change the prefix family**:

| Method | Returns | Dispatch after | Notes |
|--------|---------|----------------|-------|
| `.map(f)` | `Iterator<U>` | `Iterator` (stay) | |
| `.filter(pred)` | `Iterator<T>` | `Iterator` (stay) | |
| `.flatten()` | `Iterator<T>` | `Iterator` (stay) | |
| `.andThen(f)` | `Iterator<U>` | `Iterator` (stay) | |
| `.collect()` | `T[]` | none (exit) | |
| `.first()` | `Option<T>` | `Option` (enter Option) | |
| `.last()` | `Option<T>` | `Option` (enter Option) | |
| `.find(pred)` | `Option<T>` | `Option` (enter Option) | |
| `.nth(n)` | `Option<T>` | `Option` (enter Option) | |
| `.count()` | `number` | none (exit) | |
| `.any(pred)` | `boolean` | none (exit) | |
| `.all(pred)` | `boolean` | none (exit) | |

---

## Example chains

```ts
// Option → Iterator → collect
foo.getField("name")      // Option<string>
  .intoIter()              // Iterator<string> = { kind: "Iterator.Iterator", value: string[] }
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

## Shared postfix methods — Iterator additions

Shared postfix methods that already dispatch via `matchPrefix` gain an `Iterator` case:

```ts
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

function andThenMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({ Ok: action, Err: Result.err }),
    Option: branch({ Some: action, None: Option.none }),
    Iterator: branch({
      Iterator: chain(
        toAction(forEach(action)),
        toAction(flatten()),
        toAction(tag("Iterator", "Iterator")),
      ),
    }),
  })));
}
```

Iterator-only methods use direct `branch` on `"Iterator"` variant, no `matchPrefix` needed:

```ts
// Iter.collect: Iterator<T> → T[]
const collect = branch({ Iterator: identity() });

// Iter.first: Iterator<T> → Option<T>
const first = branch({ Iterator: /* splitFirst → Option wrap */ });
```

---

## Open questions

1. **Naming**: `.intoIter()` vs `.iter()` vs `.iterator()`?
   - `.iter()` is concise
   - `.intoIter()` is the Rust convention (consumes self)
   - `.iterator()` is what was originally suggested

2. **Array → Iterator**: How does `.intoIter()` work on `T[]`? Arrays have no prefix to dispatch on. Options:
   - Postfix `.intoIter()` on any TypedAction with `T[]` output (hardcoded, not dispatched — wraps and tags)
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
   - `.map()` on Iterator dispatches via prefix — different from array's `.forEach()`
   - No ambiguity since arrays have no Iterator prefix

7. **`chain` naming collision**: Rust's `Iterator::chain` concatenates two iterators. Barnum's `chain()` is sequential composition (the fundamental AST combinator). Resolution: use `.concat()` to avoid confusion.

8. **Single-variant tagged union**: `Iterator<T>` has only one variant (`Iterator`), so the inner branch in `matchPrefix` always has one case. This is a bit odd but consistent with the system — the `matchPrefix` outer branch selects the family, the inner branch selects the variant. A single-variant inner branch is just unwrapping.

---

## Priority

**Phase 0** (done): `mapOption→map`, `unwrapOr` widening, `mapErr→dispatch`, `Option.transpose`, `flatten` dispatch, `unwrap` (panicking), `Panic` builtin, `__union` → prefix-based dispatch via `matchPrefix` + `ExtractPrefix`

**Phase 1** (Iterator foundation):
- `Iterator<T>` tagged wrapper type + `IteratorDef`
- `Iter` namespace with standalone methods
- `Option.intoIter()`, `Result.intoIter()`, array `.intoIter()`
- `.map()`, `.filter()`, `.collect()`, `.find()`, `.first()`, `.last()`
- Add `Iterator` cases to shared postfix methods (map, andThen, flatten)

**Phase 2** (Iterator expansion):
- `.andThen()` (flat_map), `.flatten()`, `.enumerate()`, `.take()`, `.skip()`
- `.any()`, `.all()`, `.count()`, `.nth()`
- Typed collect destinations: `.toResult()`, `.toOption()`

**Phase 3** (builtins):
- `Arr.length`, `Arr.reverse`, `Arr.join`, etc.
