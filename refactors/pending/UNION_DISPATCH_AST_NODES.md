# Union Dispatch AST Nodes

Replace `__union` dispatch tables and Branch-based postfix methods with dedicated AST nodes. The engine dispatches by reading the namespaced prefix from `kind`.

**Depends on:** NAMESPACED_KIND_PREFIXES.md

## Problem

Postfix methods (`.unwrapOr()`, `.map()`, `.mapErr()`) need to know the union family (Result vs Option) to dispatch. Currently this is tracked via `__union` on TypedAction pipeline nodes, but most ways of producing a union-typed output don't set it: `createHandler`, `getField`, `branch`, `identity`. There's no general fix — `__union` is a property of the static pipeline graph, and TypeScript compile-time type knowledge doesn't exist at runtime.

With namespaced kind prefixes (previous refactor), values self-describe their enum family via `kind: "Result.Ok"`. The engine can read the prefix at runtime, eliminating `__union` entirely.

## Changes

### 1. New AST nodes replace Branch-based dispatch

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

### 2. Remove `__union` from the TypeScript SDK

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

### 3. Result/Option namespace simplification

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

### 4. User-defined enum dispatch

The engine doesn't need a pre-registered family for user-defined enums unless they want to use postfix dispatch (`.unwrapOr()`, `.map()`, etc.). For simple `branch`, the prefix is stripped automatically.

For postfix dispatch on user-defined enums, the engine would need the family registered. This could be part of the pipeline config or derived from the `taggedUnionSchema` call. **Deferred — not needed for initial implementation.**

---

## Open questions

1. **Flatten ambiguity.** `.flatten()` is overloaded across arrays, Option, and Result. Without `__union`, the postfix method can't disambiguate at build time. For now, Option/Result flatten use the existing Branch mechanism (prefix stripping makes this work), array flatten stays as the `Flatten` builtin. Revisit naming (`flattenToArray`, etc.) separately.
