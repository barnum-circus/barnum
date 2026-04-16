# Namespaced Kind Prefixes

Replace bare `kind` strings with namespaced prefixes so tagged union values self-describe their enum family.

**Depends on:** subpath exports (done)
**Blocks:** UNION_DISPATCH_AST_NODES.md

## Problem

Tagged union values currently use bare kind strings: `{ kind: "Ok", value: "nice" }`. Nothing on the value tells you which enum family it belongs to. This matters for the union dispatch refactor (next doc), but it's also a standalone improvement — self-describing values are better wire format.

## Changes

### 1. Wire format

Tagged union values use namespaced kind strings:

```
// Before
{ kind: "Ok", value: "nice" }

// After
{ kind: "Result.Ok", value: "nice" }
```

Branch matching strips the prefix — still matches on bare variant names.

### 2. Runtime constructors produce namespaced kinds

The `ok`/`err`/`some`/`none` constructors from `@barnum/barnum/runtime` produce values with prefixed kinds:

```ts
ok("nice")    // → { kind: "Result.Ok", value: "nice" }
err("bad")    // → { kind: "Result.Err", value: "bad" }
some(42)      // → { kind: "Option.Some", value: 42 }
none()        // → { kind: "Option.None", value: null }
```

### 3. TS types reflect the wire format

`TaggedUnion` gains an enum name parameter and produces namespaced `kind` literals:

```ts
type TaggedUnion<TEnumName extends string, TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: {
    kind: `${TEnumName}.${K}`;
    value: VoidToNull<TDef[K]>;
  };
}[keyof TDef & string];

type Result<TValue, TError> = TaggedUnion<"Result", { Ok: TValue; Err: TError }>;
// = { kind: "Result.Ok"; value: TValue } | { kind: "Result.Err"; value: TError }

type Option<T> = TaggedUnion<"Option", { Some: T; None: void }>;
// = { kind: "Option.Some"; value: T } | { kind: "Option.None"; value: null }
```

User-defined enums work the same way:
```ts
type ClassifyResultDef = { HasErrors: TypeError[]; Clean: void };
type ClassifyResult = TaggedUnion<"ClassifyResult", ClassifyResultDef>;
// = { kind: "ClassifyResult.HasErrors"; value: TypeError[] } | { kind: "ClassifyResult.Clean"; value: null }
```

This means the existing `taggedUnionSchema` needs to accept the enum name too.

### 4. Branch uses short keys — type-level prefix stripping

Users write `branch({ Ok: ..., Err: ... })` with bare variant names. The branch type strips the prefix:

```ts
type StripPrefix<T extends string> = T extends `${string}.${infer Suffix}` ? Suffix : T;

function branch<TUnion extends { kind: string }>(
  cases: { [K in StripPrefix<TUnion["kind"]>]: Pipeable<...> }
): TypedAction<TUnion, ...>;
```

`branch({ Ok: identity(), Err: fallback })` works on `Result<T, E>` where `kind` is `"Result.Ok" | "Result.Err"` — `StripPrefix` maps those to `"Ok" | "Err"` for the case keys.

**Rust engine change:** one-line change in `advance.rs`:
```rust
// Current
.find(|(key, _)| key.lookup() == kind_str)

// New: strip prefix before matching
let match_str = kind_str.rsplit_once('.').map_or(kind_str, |(_, suffix)| suffix);
.find(|(key, _)| key.lookup() == match_str)
```

### 5. Tag builtin gets enum name

The Rust Tag builtin produces namespaced kinds:

```rust
// Current
Builtin::Tag { kind: String }
// → { "kind": "Ok", "value": ... }

// New
Builtin::Tag { kind: String, enum_name: String }
// → { "kind": "Result.Ok", "value": ... }
```

SDK-side `tag` takes a required enum name parameter:
- `tag("Ok", "Result")` → emits `Tag { kind: "Ok", enum_name: "Result" }` → Rust produces `{ kind: "Result.Ok", value: ... }`
- `tag("Some", "Option")` → same pattern
- `tag("HasErrors", "ClassifyResult")` → Rust produces `{ kind: "ClassifyResult.HasErrors", value: ... }`

### 6. Schema changes

- `resultSchema(okSchema, errSchema)` internally validates against `"Result.Ok"` / `"Result.Err"` instead of bare `"Ok"` / `"Err"`
- `optionSchema(valueSchema)` validates against `"Option.Some"` / `"Option.None"`
- `taggedUnionSchema` gains a required enum name parameter: `taggedUnionSchema("ClassifyResult", { HasErrors: ..., Clean: ... })`

### 7. Migration

Existing handlers return bare `{ kind: "Ok", value: ... }` without prefixes. These must change to use constructors:

```ts
// Before
handle: async () => ({ kind: "Ok", value: "validated" })

// After
handle: async () => ok("validated")
```

User-defined enum returns also change:

```ts
// Before
return { kind: "HasErrors", value: errors };

// After
return { kind: "ClassifyResult.HasErrors", value: errors };
```

### 8. User-defined enums

User-defined enums already exist (e.g., `ClassifyResult`, Peano `Nat`). They currently use `taggedUnionSchema` + `TaggedUnion` to define their types.

With namespaced prefixes, users provide an enum name when defining the union:

```ts
// In handler file
import { taggedUnionSchema } from "@barnum/barnum/runtime";
import type { TaggedUnion } from "@barnum/barnum/runtime";

type ClassifyResultDef = { HasErrors: TypeError[]; Clean: void };
type ClassifyResult = TaggedUnion<"ClassifyResult", ClassifyResultDef>;

const ClassifyResultValidator = taggedUnionSchema("ClassifyResult", {
  HasErrors: z.array(TypeErrorValidator),
  Clean: z.null(),
});
```

```ts
// In pipeline file
import { branch } from "@barnum/barnum/pipeline";

classifyErrors.branch({
  HasErrors: fixLoop,
  Clean: done,
})
```

Branch just works — prefix is stripped automatically.
