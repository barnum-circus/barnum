# Eliminate `as TypedAction` Casts

**Status: COMPLETE**

## Summary

Investigated and eliminated unnecessary `as TypedAction` casts where possible. The key finding: most `as TypedAction` casts in this codebase are **structurally necessary** due to the `BranchInput` type mismatch, but we can still improve type safety by adding explicit type parameters to leaf combinators and removing unnecessary `toAction` erasure.

## What changed

### option.ts — explicit type params, remove toAction wrappers

Added explicit type parameters to all leaf combinators (`Option.some<U>()`, `identity<T>()`, `constant<boolean>(...)`, etc.) and removed `toAction()` wrappers in `chain()` calls where phantom types should flow through. The `as TypedAction` casts remain.

### result.ts — same treatment

Same approach: explicit type params on all leaf combinators, removed `toAction()` wrappers in chain calls. Removed now-unused `toAction` import.

### ast.ts — removed redundant thenMethod cast

`chain(this, next)` already returns `TypedAction<TIn, TNext>` since `TypedAction` is assignable to `Pipeable`. The cast was redundant. Removed.

### iterator.ts — simplified fromOption/fromResult/filter

- `fromOption`/`fromResult`: removed unnecessary `chain(..., identity())` pattern and `toAction` wrappers. Now just `branch({Some: chain(wrapInArray<T>(), Iterator.fromArray<T>()), None: chain(constant<T[]>([]), Iterator.fromArray<T>())})`.
- `filter`: removed `toAction` on `drop` and `Option.none<T>()` (both already Pipeable).
- `map`/`flatMap`: kept `toAction` — `getField`/`forEach`/`flatten` return `unknown` output types, erasure is needed.

## Why the casts cannot be fully eliminated

### BranchInput / TaggedUnion kind namespace mismatch

`branch()` returns `TypedAction<BranchInput<TCases>, ...>` where `BranchInput` constructs types with short keys (e.g. `{kind: "Some", value: T}`). But our `TaggedUnion` type uses namespaced kinds (e.g. `{kind: "Option.Some", value: T}`). These are structurally incompatible — `"Option.Some"` is not assignable to `"Some"`. The `as TypedAction<OptionT<T>, ...>` cast bridges this gap.

This is a fundamental design decision: the Rust engine uses `rsplit_once('.')` to strip the namespace prefix at runtime, so branch keys use bare names. But the TypeScript types use the full namespaced kinds. These two representations are intentionally different (branch keys match the engine's dispatch, TaggedUnion types match the wire format).

### typedAction() factory

`typedAction<In, Out>(action: Action)` takes a plain `Action` (no phantom fields) and returns `TypedAction<In, Out>`. This cast is the core coercion point — it's where phantom types are asserted onto raw AST nodes. Cannot be eliminated.

### Unparameterized builtins (getField, forEach, flatten)

`getField("value")` returns `TypedAction<Record<string, unknown>, unknown>`. `forEach(action)` returns `TypedAction<unknown[], unknown[]>`. `flatten()` returns `TypedAction<unknown[][], unknown[]>`. These builtins don't carry the generic element types, so `toAction` erasure + final `as` cast is the correct pattern.

## Remaining casts

All remaining `as TypedAction` casts fall into one of three categories:

1. **BranchInput mismatch** (option.ts, result.ts, iterator.ts) — structurally necessary
2. **typedAction factory** (ast.ts) — inherently necessary
3. **Unparameterized builtin composition** (iterator.ts map/flatMap, builtins/struct.ts, builtins/with-resource.ts, builtins/tagged-union.ts) — intermediate types are too loose for chain inference
