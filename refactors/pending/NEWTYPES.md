# Newtypes as Single-Variant Enums

## Context

Barnum's type system is built on `TaggedUnion<TEnumName, TDef>` — a discriminated union where each variant has a namespaced kind string (`"Option.Some"`, `"Result.Err"`). Multi-variant enums (Option, Result) are the primary use case.

But we also have **newtypes** — types that wrap an inner value to give it distinct identity, with exactly one variant. Two exist or are proposed:

- **Iterator<T>**: wraps `T[]` as `{ kind: "Iterator.Iterator", value: T[] }` (see TRAIT_DISPATCH_AND_ITERATORS.md)
- **HashMap<T>**: wraps `Record<string, T>` to distinguish dynamic key-value maps from structs (see API_SURFACE_AUDIT.md)

Both follow the same pattern: a single-variant `TaggedUnion` where the variant name matches the enum name. This doc explores whether that's the right representation, and what newtypes mean for the system.

---

## What is a newtype?

In Rust, a newtype is `struct Foo(Bar)` — a wrapper that gives `Bar` a distinct type identity without changing its runtime representation. Newtypes enable:

1. **Type safety**: `UserId(u64)` vs `PostId(u64)` — same inner type, different identities, can't be mixed up.
2. **Method namespacing**: impl blocks on the newtype provide methods specific to the wrapped type. `Iterator<T>` wraps `Vec<T>` but has different methods.
3. **Trait impl separation**: you can impl traits on your newtype that you can't impl on the inner type (orphan rules).

In barnum, newtypes serve purposes 1 and 2. The inner value needs to be wrapped so that:
- The type system (TypeScript) can distinguish `Iterator<T>` from `T[]`
- Postfix methods (via `matchPrefix`) can dispatch to the right implementation
- The Rust engine can treat it as a tagged value uniformly

---

## Current representation: single-variant TaggedUnion

Iterator uses `TaggedUnion<"Iterator", { Iterator: T[] }>`, producing:

```ts
// TypeScript type
type Iterator<T> = { kind: "Iterator.Iterator"; value: T[]; __def?: { Iterator: T[] } }

// Runtime value
{ kind: "Iterator.Iterator", value: [1, 2, 3] }
```

This works but has quirks:

1. **Redundant kind string.** `"Iterator.Iterator"` — the prefix and suffix are identical. The `.Iterator` suffix carries no information because there's only one variant.

2. **`branch()` on a single variant is ceremony.** `branch({ Iterator: handler })` always matches. It's just an unwrap with extra steps.

3. **`matchPrefix` does two levels of dispatch for one.** `ExtractPrefix` splits `"Iterator.Iterator"` into `{ kind: "Iterator", value: original }`, then the outer branch matches `"Iterator"` and unwraps. The inner value is `{ kind: "Iterator.Iterator", value: T[] }` — still wrapped. For multi-variant enums, a second `branch` dispatches on the variant. For newtypes, you skip that and `getField("value")` directly.

4. **Consistency is the upside.** The Rust engine, `branch()`, `matchPrefix`, `tag()`, and all the tagged union machinery work on newtypes without special cases. A newtype is just an enum that happens to have one variant.

---

## Alternative: dedicated newtype representation

Instead of fitting newtypes into `TaggedUnion`, we could have a simpler wrapper:

```ts
// Dedicated newtype representation
{ kind: "Iterator", value: [1, 2, 3] }
```

No dot in the kind string. `ExtractPrefix` wouldn't split anything (no `.` to split on — `ExtractPrefix` returns the full kind as the prefix when there's no dot). `branch({ Iterator: handler })` matches directly.

**Pros:**
- Simpler runtime values
- No redundant `"Foo.Foo"` pattern
- `branch()` works directly without the two-level dispatch

**Cons:**
- New concept in the type system — newtypes are no longer "just enums"
- `matchPrefix` wouldn't match them the same way as multi-variant enums. A `matchPrefix({ Iterator: ..., Option: ... })` call would need to handle both dotted kinds (`"Option.Some"`) and un-dotted kinds (`"Iterator"`) in the same dispatch.
- `tag()` constructor would need a different form
- The Rust engine would need to know about the distinction

---

## Recommendation: keep single-variant enums

The current `TaggedUnion` representation is the right call. The redundancy is cosmetic, and the benefits of uniformity are real:

1. **One representation for all tagged types.** The engine, `branch()`, `matchPrefix`, `tag()`, serialization — all of it works on `{ kind: string, value: T }` with dot-separated namespacing. No special cases.

2. **`matchPrefix` just works.** `matchPrefix({ Iterator: ..., Option: ..., Result: ... })` dispatches uniformly. The Iterator case handler receives the tagged value after outer unwrap and does `getField("value")` to extract the inner `T[]`. The fact that there's no inner `branch` for Iterator is fine — it's just a simpler case handler, not a different mechanism.

3. **Future-proofing.** A newtype today could gain variants tomorrow. `Iterator` might grow a `Lazy` variant. HashMap might distinguish `Empty` from `NonEmpty`. Keeping them as enums means this is a non-breaking extension.

4. **`branch()` on single-variant is a valid pattern.** It's explicit, self-documenting, and compiles to the same code as `getField("value")`. If you see `branch({ Iterator: handler })`, you know what's happening. No magic.

### Convention for newtypes

When the variant name matches the enum name, it's a newtype. This is a naming convention, not a type-level distinction:

```ts
// Newtype: variant name = enum name
type Iterator<T> = TaggedUnion<"Iterator", { Iterator: T[] }>;
// → { kind: "Iterator.Iterator", value: T[] }

type HashMap<T> = TaggedUnion<"HashMap", { HashMap: Record<string, T> }>;
// → { kind: "HashMap.HashMap", value: Record<string, T> }

// Multi-variant: variant names differ from enum name
type Option<T> = TaggedUnion<"Option", { Some: T; None: void }>;
// → { kind: "Option.Some", value: T } | { kind: "Option.None", value: null }
```

No new types, no new builtins, no new engine support. Just a pattern.

---

## Newtypes in the API

### Iterator<T> (see TRAIT_DISPATCH_AND_ITERATORS.md)

Wraps `T[]`. Entry via `.iterate()`. Exit via `.collect()`, `.first()`, `.last()`, etc. Transformation methods (map, filter, find) operate on the sequence. Participates in `matchPrefix` dispatch alongside Option and Result.

### HashMap<T> (see API_SURFACE_AUDIT.md)

Wraps `Record<string, T>`. Distinguishes dynamic key-value maps from structs (which have fields known at compile time). Methods like `.get(key)` → `Option<T>`, `.insert(key, value)`, `.keys()`, `.values()`, `.entries()`.

HashMap needs the newtype wrapper because:
- `getField(key)` on a struct is a compile-time operation — the key is a literal, the field is known to exist. It returns `T` directly.
- `HashMap.get(key)` is a runtime operation — the key is a string value, the entry might not exist. It returns `Option<T>`.
- Without distinct types, the engine can't tell whether `getField("name")` means "access the struct field `name`" or "look up the key `"name"` in a hashmap."

### Potential future newtypes

| Type | Wraps | Why newtype |
|------|-------|-------------|
| `Set<T>` | `T[]` (deduplicated) | Distinct from array — different methods (contains, union, intersection) |
| `NonEmpty<T>` | `T[]` (guaranteed non-empty) | `.first()` returns `T` not `Option<T>` |
| `Sorted<T>` | `T[]` (sorted) | Enables binary search, merge operations |

These are speculative. The point is that the single-variant enum pattern scales to any wrapper type without new machinery.

---

## Implementation pattern

Every newtype follows the same template:

```ts
// 1. Type definition
type FooDef<T> = { Foo: T };
type Foo<T> = TaggedUnion<"Foo", FooDef<T>>;

// 2. Constructor (wrap)
const Foo = {
  new: tag("Foo", "Foo"),  // T → Foo<T>
};

// 3. Unwrap (for internal use in methods)
const unwrapFoo = getField("value");  // Foo<T> → T

// 4. Methods operate as: unwrap → transform → rewrap
const fooMap = (action) => chain(
  toAction(unwrapFoo),
  toAction(action),
  toAction(Foo.new),
);
```

For postfix dispatch via `matchPrefix`, the newtype's case handler is always: `getField("value")` → operation → `tag(Name, Name)`. No inner branch.

---

## Open questions

1. **Should newtypes have a type-level marker?** E.g., a `Newtype<TName, TInner>` alias that's sugar for `TaggedUnion<TName, { [TName]: TInner }>`. This would make the single-variant pattern explicit in the type system without changing runtime behavior.

2. **`branch()` vs `getField("value")` for unwrapping:** When a postfix method dispatches via `matchPrefix` and the newtype case needs to unwrap, is `getField("value")` or `branch({ Foo: handler })` preferred? Both work. `getField("value")` is more direct. `branch({ Foo: handler })` is more self-documenting and auto-unwraps. Either way, it's the case handler inside `matchPrefix`, not a top-level concern.
