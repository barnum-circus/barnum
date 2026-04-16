# Union Dispatch via Namespaced Kind Prefixes

Replace `__union` dispatch tables on pipeline nodes with self-describing values whose `kind` field carries the enum name.

## Problem

Postfix methods (`.unwrapOr()`, `.map()`, `.mapErr()`) need to know the union family (Result vs Option) to dispatch. Currently this is tracked via `__union` on TypedAction pipeline nodes, but most ways of producing a union-typed output don't set it: `createHandler`, `getField`, `branch`, `identity`. There's no general fix — `__union` is a property of the static pipeline graph, and TypeScript compile-time type knowledge doesn't exist at runtime.

## Approach: namespaced kind strings

Values carry their enum identity in the `kind` field itself:

```
{ kind: "Result.Ok", value: "nice" }
{ kind: "Option.Some", value: 42 }
```

The engine reads the prefix from `kind` at runtime to dispatch union operations. `__union` is eliminated entirely. One field, no extras.

**Advantages:**
- One field instead of two — simpler wire format, less noise in JSON
- Self-describing: the kind string alone tells you everything
- No phantom field polluting the type system
- Reads naturally: `"Result.Ok"` is immediately clear

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

### 6. New AST nodes replace Branch-based dispatch

Postfix methods stop using `__union` dispatch tables. Instead they emit new AST node types that the engine interprets at runtime by reading the prefix from `kind`:

| TypeScript postfix | Current (Branch-based) | New AST node |
|-------------------|------------------------|--------------|
| `.unwrapOr(f)` | `Branch({ Ok: identity, Err: f })` | `UnwrapOr { default_action }` |
| `.unwrap()` | `Branch({ Ok: identity, Err: Panic })` | `Unwrap {}` |
| `.map(f)` | `Branch({ Ok: Chain(f, Tag), Err: Tag })` | `MapInner { action }` |
| `.mapErr(f)` | `Branch({ Ok: Tag, Err: Chain(f, Tag) })` | `MapError { action }` |
| `.andThen(f)` | `Branch({ Ok: f, Err: Tag })` | `AndThen { action }` |
| `.or(f)` | `Branch({ Ok: Tag, Err: f })` | `OrElse { fallback }` |
| `.flatten()` | `Branch({ Some: identity, None: Tag })` | stays as Branch (or new `FlattenUnion`) |
| `.isOk()` / `.isSome()` | `Branch({ Ok: true, Err: false })` | `IsSuccessVariant {}` |
| `.isErr()` / `.isNone()` | `Branch({ Ok: false, Err: true })` | `IsFailureVariant {}` |
| `.toOption()` | `Branch({ Ok: Tag("Some"), Err: Drop+Tag("None") })` | `ToOption {}` |
| `.toOptionErr()` | `Branch({ Ok: Drop+Tag("None"), Err: Tag("Some") })` | `ToOptionErr {}` |
| `.transpose()` | nested Branch | `Transpose {}` |

The engine reads the prefix from `kind` (e.g., `"Result"` from `"Result.Ok"`), looks up the enum family's variant mapping, and dispatches. The family registry maps prefix to success/failure variants:

```rust
// Built-in families
"Result" → { success: "Ok", failure: "Err" }
"Option" → { success: "Some", failure: "None" }
```

When the engine encounters `UnwrapOr { default_action }`:
1. Read `kind` from the input value → `"Result.Ok"`
2. Extract prefix → `"Result"`, variant → `"Ok"`
3. Look up family → `{ success: "Ok", failure: "Err" }`
4. `"Ok"` matches success → pass through `value`
5. If it matched failure → run `default_action`

Re-tagging preserves the prefix: `MapInner` on a Result runs the action on the Ok value and re-wraps as `{ kind: "Result.Ok", value: result }`.

### 7. Remove `__union` from the TypeScript SDK

Delete:
- `UnionMethods`, `UnionDispatch`, `withUnion` from ast.ts
- `__union` property from `TypedAction` type
- `requireDispatch` helper
- `resultMethods` dispatch table from result.ts
- `optionMethods` dispatch table from option.ts
- Chain propagation of `__union` in chain.ts

Postfix method implementations simplify from dispatch-table lookup to direct AST node emission:

```ts
// Before
function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  const unwrapOr = requireDispatch(this.__union, "unwrapOr", (m) => m.unwrapOr);
  return chain(toAction(this), toAction(unwrapOr(defaultAction)));
}

// After
function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  return chain(toAction(this), typedAction({
    kind: "UnwrapOr",
    default_action: toAction(defaultAction),
  }));
}
```

### 8. Result/Option namespace simplification

Standalone combinators (`Result.map()`, `Result.unwrapOr()`, etc.) become thin wrappers that emit the new AST nodes:

```ts
// Before: builds Branch + withUnion
map<T, U>(action: Pipeable<T, U>): TypedAction<ResultT<T, E>, ResultT<U, E>> {
  return withUnion(branch({ Ok: chain(action, tag("Ok")), Err: tag("Err") }), ...);
}

// After: emits MapInner node
map<T, U>(action: Pipeable<T, U>): TypedAction<ResultT<T, E>, ResultT<U, E>> {
  return typedAction({ kind: "MapInner", action: toAction(action) });
}
```

The dispatch tables (`resultMethods`, `optionMethods`) go away. The Result and Option namespaces shrink to thin AST wrappers.

### 9. Migration

Existing handlers return bare `{ kind: "Ok", value: ... }` without prefixes. These must change to use constructors:

```ts
// Before
handle: async () => ({ kind: "Ok", value: "validated" })

// After
handle: async () => ok("validated")
```

The output validator (`resultSchema(...)`) could be enhanced to inject the prefix during validation — values with bare `"Ok"` get rewritten to `"Result.Ok"`. Optional migration sugar, not required.

### 10. User-defined enums

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

The engine doesn't need a pre-registered family for user-defined enums unless they want to use postfix dispatch (`.unwrapOr()`, `.map()`, etc.). For simple `branch`, the prefix is stripped automatically.

For postfix dispatch on user-defined enums, the engine would need the family registered. This could be part of the pipeline config or derived from the `taggedUnionSchema` call. **Deferred — not needed for initial implementation.**

---

## Open questions

1. **Flatten ambiguity.** `.flatten()` currently dispatches for arrays, Option, and Result. With prefixed kinds, the engine can distinguish Option/Result flatten from array flatten by checking whether `kind` has a prefix. But this means the engine inspects values to decide behavior — is that acceptable?
2. **Does `GetField`, `Identity`, etc. need to do anything special?** No — they pass values through unchanged, and `kind` is just a field on the JSON object. The prefix is preserved automatically.
