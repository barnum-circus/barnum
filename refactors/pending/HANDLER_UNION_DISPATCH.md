# Union Dispatch via `enumKind`

Replace `__union` dispatch tables on pipeline nodes with self-describing values that carry their enum identity.

## Problem

Postfix methods (`.unwrapOr()`, `.map()`, `.mapErr()`) need to know the union family (Result vs Option) to dispatch. Currently this is tracked via `__union` on TypedAction pipeline nodes, but most ways of producing a union-typed output don't set it: `createHandler`, `getField`, `branch`, `identity`. There's no general fix — `__union` is a property of the static pipeline graph, and TypeScript compile-time type knowledge doesn't exist at runtime.

## Approach: `enumKind` in the wire format

Values carry their enum identity:

```
{ kind: "Ok", enumKind: "Result", value: "nice" }
{ kind: "Some", enumKind: "Option", value: 42 }
```

The engine reads `enumKind` at runtime to dispatch union operations. `__union` is eliminated entirely.

### Alternative: namespaced kind strings

One field instead of two: `{ kind: "Result.Ok", value: "nice" }`.

The engine extracts the enum name by splitting on `.`. Branch strips the prefix when matching: value `kind: "Result.Ok"` matches case key `"Ok"`.

**TS type divergence problem:** `TaggedUnion` currently produces `kind: "Ok"`. Changing to `kind: "Result.Ok"` means either (a) TS types diverge from wire format, or (b) branch cases must use full names: `branch({ "Result.Ok": ... })`. Neither is great. With `enumKind` as a separate field, `kind` stays `"Ok"` everywhere — no divergence.

---

## Changes

### 1. Wire format

Tagged union values gain an `enumKind` field:

```
// Before
{ kind: "Ok", value: "nice" }

// After
{ kind: "Ok", enumKind: "Result", value: "nice" }
```

Branch matching is unchanged — still matches on `kind`. `enumKind` is read only by the new union-aware AST nodes.

### 2. Runtime value constructors for handler bodies

Handlers currently return bare objects. Add constructors that inject `enumKind`:

```ts
Result.create.ok("nice")   // → { kind: "Ok", enumKind: "Result", value: "nice" }
Result.create.err("bad")   // → { kind: "Err", enumKind: "Result", value: "bad" }
Option.create.some(42)     // → { kind: "Some", enumKind: "Option", value: 42 }
Option.create.none()       // → { kind: "None", enumKind: "Option", value: null }
```

Separate from the pipeline combinators (`Result.ok()` etc.), which remain as `TypedAction` constructors.

### 3. Tag builtin gets `enum_kind`

The Rust Tag builtin must inject `enumKind` when producing tagged values in the pipeline:

```rust
// Current
Builtin::Tag { kind: String }
// → { "kind": "Ok", "value": ... }

// New
Builtin::Tag { kind: String, enum_kind: Option<String> }
// → { "kind": "Ok", "enumKind": "Result", "value": ... }
```

SDK-side:
- `Result.ok()` → emits `Tag { kind: "Ok", enum_kind: Some("Result") }`
- `Option.some()` → emits `Tag { kind: "Some", enum_kind: Some("Option") }`
- `tag("Foo")` → emits `Tag { kind: "Foo", enum_kind: None }` (user-defined unions, no dispatch)

### 4. New AST nodes replace Branch-based dispatch

Postfix methods stop using `__union` dispatch tables. Instead they emit new AST node types that the engine interprets at runtime using `enumKind`:

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

The engine reads `enumKind` from the runtime value, looks up the enum family's variant mapping (`Result → { success: "Ok", failure: "Err" }`, `Option → { success: "Some", failure: "None" }`), and dispatches.

The new nodes also handle re-tagging: `MapInner` on a Result runs the action on the Ok value and re-wraps as `{ kind: "Ok", enumKind: "Result", value: result }`. The engine preserves `enumKind` through the re-tag.

### 5. Remove `__union` from the TypeScript SDK

Delete:
- `UnionMethods`, `UnionDispatch`, `withUnion` from ast.ts
- `__union` property from `TypedAction` type
- `requireDispatch` helper
- `resultMethods` dispatch table from result.ts
- `optionMethods` dispatch table from option.ts
- `dispatch` property from Result and Option namespaces
- `returns` field from `createHandler` / `createHandlerWithConfig`
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

### 6. Result/Option namespace simplification

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

The dispatch tables (`resultMethods`, `optionMethods`) go away. The Result and Option namespaces shrink to constructors + thin AST wrappers.

### 7. Rust engine: enum family registry

The engine needs a mapping from `enumKind` string to variant names:

```rust
struct EnumFamily {
    success_variant: &'static str,  // "Ok" for Result, "Some" for Option
    failure_variant: &'static str,  // "Err" for Result, "None" for Option
}

// Built-in families
"Result" → EnumFamily { success_variant: "Ok", failure_variant: "Err" }
"Option" → EnumFamily { success_variant: "Some", failure_variant: "None" }
```

When the engine encounters `UnwrapOr { default_action }`:
1. Read `enumKind` from the input value → `"Result"`
2. Look up family → `{ success: "Ok", failure: "Err" }`
3. Read `kind` from the input value → `"Ok"`
4. `kind` matches success → pass through `value`
5. If `kind` matched failure → run `default_action`

For user-defined enums, the config could carry additional family registrations.

### 8. Migration

Existing handlers return bare `{ kind: "Ok", value: ... }` without `enumKind`. These must change to use runtime constructors:

```ts
// Before
handle: async () => ({ kind: "Ok", value: "validated" })

// After
handle: async () => Result.create.ok("validated")
```

The output validator (`Result.schema(...)`) could be enhanced to inject `enumKind` during validation — values without `enumKind` get it added. This would ease migration: handlers returning bare objects would still work if they have validators. But this is optional sugar, not required.

---

## Open questions

1. **User-defined enums.** How does a user register a custom enum family? Something like `defineEnum("TaskStatus", { success: "Done", failure: "Failed" })`? Or is this only for Result/Option?
2. **`enumKind` required or optional?** If optional, values without it can't use postfix methods (runtime error, same as today). If required, all tagged union values need it. Recommend: optional, with good error messages.
3. **Naming.** `enumKind` vs `enum` vs `unionKind` vs `family`. `enumKind` parallels `kind` and is explicit.
4. **Flatten ambiguity.** `.flatten()` currently dispatches for arrays, Option, and Result. With `enumKind`, the engine can distinguish Option/Result flatten from array flatten by checking whether the input has `enumKind`. But this means the engine inspects values to decide behavior — is that acceptable?
5. **Does the engine need to preserve `enumKind` through all transformations?** E.g., after `MapInner`, the output must still have `enumKind`. The Tag builtin handles this for re-tagging. But does `GetField`, `Identity`, etc. need to do anything? No — they pass values through unchanged, and `enumKind` is just a field on the JSON object.
