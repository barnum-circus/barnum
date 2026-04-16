# Remove __union Dispatch

Replace the `__union` dispatch table mechanism with direct polymorphic branches in postfix methods. Zero new Rust AST nodes. Everything desugars to existing primitives.

**Depends on:** NAMESPACED_KIND_PREFIXES.md (completed)

## Problem

Postfix methods (`.unwrapOr()`, `.map()`, `.mapErr()`) need to know the union family (Result vs Option) to dispatch to the correct branch pattern. Currently tracked via `__union` on TypedAction — a runtime dispatch table that maps method names to implementations. But most paths that produce union-typed output don't set `__union`: `createHandler`, `getField`, `branch`, `identity`. There's no general fix because TypeScript type info (is this Result or Option?) is erased at runtime, and `__union` is a metadata property that must be manually propagated through every combinator.

This means postfix methods like `.unwrapOr()` throw at runtime when called on handlers returned from `createHandler`, fields extracted with `getField`, etc. — the most common usage patterns.

## Approach

With namespaced kind prefixes (completed), values self-describe their family at runtime: `{ kind: "Result.Ok", value: 42 }`. The Rust engine already strips the prefix when matching branch cases (`"Result.Ok"` → `"Ok"`).

Postfix methods emit **polymorphic branches** — branches that include case keys for all applicable families. At runtime, the engine matches the one case that corresponds to the actual value's kind. Unmatched cases are dead branches, never entered.

```ts
// Before: requires __union, fails on createHandler output
function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  const unwrapOr = requireDispatch(this.__union, "unwrapOr", (m) => m.unwrapOr);
  return chain(toAction(this), toAction(unwrapOr(defaultAction)));
}

// After: works on any TypedAction regardless of provenance
function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: identity(),        // Result path
    Err: defaultAction,    // Result path
    Some: identity(),      // Option path
    None: defaultAction,   // Option path
  })));
}
```

Family-specific methods (`.mapErr()`, `.isSome()`, etc.) include only their family's case keys. If called on the wrong family at runtime, the branch fails with "no match" — correct behavior.

TypeScript overload signatures on `TypedAction` continue to gate method availability at compile time: `.mapErr()` requires `this: TypedAction<TIn, Result<...>>`, so calling it on an Option is a type error. The runtime-level "no match" is a safety net, not the primary enforcement.

## Deletions

From `ast.ts`:
- `UnionMethods` interface
- `UnionDispatch` interface
- `withUnion()` function
- `requireDispatch()` helper
- `__union` field from `TypedAction` type
- `__union` property definition from `typedAction()` function

From `result.ts`:
- `resultMethods` dispatch table
- All `withUnion(...)` calls — standalone methods become plain branches

From `option.ts`:
- `optionMethods` dispatch table
- All `withUnion(...)` calls — standalone methods become plain branches

From `chain.ts`:
- `__union` propagation (lines 18–22)

## Postfix method rewrites

### Shared methods (Option + Result) — polymorphic branch

These include case keys for both families. Exactly one pair matches at runtime.

```ts
function mapMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: chain(toAction(action), toAction(tag("Ok", "Result"))),
    Err: tag("Err", "Result"),
    Some: chain(toAction(action), toAction(tag("Some", "Option"))),
    None: tag("None", "Option"),
  })));
}

function unwrapMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: identity(),
    Err: panic("called unwrap on Err"),
    Some: identity(),
    None: panic("called unwrap on None"),
  })));
}

function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: identity(),
    Err: defaultAction,
    Some: identity(),
    None: defaultAction,
  })));
}

function andThenMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: action,
    Err: tag("Err", "Result"),
    Some: action,
    None: tag("None", "Option"),
  })));
}

function transposeMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    // Option<Result<T,E>> → Result<Option<T>,E>
    Some: branch({
      Ok: chain(toAction(tag("Some", "Option")), toAction(tag("Ok", "Result"))),
      Err: tag("Err", "Result"),
    }),
    None: chain(toAction(drop.tag("None", "Option")), toAction(tag("Ok", "Result"))),
    // Result<Option<T>,E> → Option<Result<T,E>>
    Ok: branch({
      Some: chain(toAction(tag("Ok", "Result")), toAction(tag("Some", "Option"))),
      None: drop.tag("None", "Option"),
    }),
    Err: chain(toAction(tag("Err", "Result")), toAction(tag("Some", "Option"))),
  })));
}
```

### Result-only methods

```ts
function mapErrMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: tag("Ok", "Result"),
    Err: chain(toAction(action), toAction(tag("Err", "Result"))),
  })));
}

function orMethod(this: TypedAction, fallback: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: tag("Ok", "Result"),
    Err: fallback,
  })));
}

function andPostfixMethod(this: TypedAction, other: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: chain(toAction(drop), toAction(other)),
    Err: tag("Err", "Result"),
  })));
}

function toOptionMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: tag("Some", "Option"),
    Err: drop.tag("None", "Option"),
  })));
}

function toOptionErrMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: drop.tag("None", "Option"),
    Err: tag("Some", "Option"),
  })));
}

function isOkMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: constant(true), Err: constant(false),
  })));
}

function isErrMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Ok: constant(false), Err: constant(true),
  })));
}
```

### Option-only methods

```ts
function filterMethod(this: TypedAction, predicate: Action): TypedAction {
  return chain(toAction(this), toAction(branch({
    Some: predicate,
    None: tag("None", "Option"),
  })));
}

function isSomeMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Some: constant(true), None: constant(false),
  })));
}

function isNoneMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(branch({
    Some: constant(false), None: constant(true),
  })));
}
```

### Unchanged

- `collectMethod` — already standalone, uses the `CollectSome` builtin directly

## Standalone namespace methods

`Result.map()`, `Option.unwrapOr()`, etc. drop their `withUnion(...)` wrappers. The branch AST they emit is already correct:

```ts
// Before
map(action) {
  return withUnion(
    branch({ Ok: chain(action, tag("Ok", "Result")), Err: tag("Err", "Result") }),
    "Result", resultMethods,
  );
}

// After
map(action) {
  return branch({
    Ok: chain(action, tag("Ok", "Result")),
    Err: tag("Err", "Result"),
  }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>;
}
```

The standalone methods remain useful for explicit `pipeline.then(Result.map(f))` patterns where the family is known.

## Flatten: split array vs union

`.flatten()` is genuinely ambiguous at runtime between array flatten (`T[][] → T[]`) and union flatten (`Option<Option<T>> → Option<T>`). Without `__union`, there's no runtime discriminant.

**Resolution:** `.flatten()` postfix does array flatten only. Union flatten uses the existing standalone methods:

```ts
// Array flatten — postfix
pipeline.flatten()                    // T[][] → T[]

// Union flatten — explicit namespace
pipeline.then(Option.flatten())       // Option<Option<T>> → Option<T>
pipeline.then(Result.flatten())       // Result<Result<T,E>,E> → Result<T,E>
```

Remove the Option/Result overloads from `flatten` on `TypedAction`. Keep only the array overload:

```ts
flatten<TIn, TElement>(
  this: TypedAction<TIn, TElement[][]>,
): TypedAction<TIn, TElement[]>;
```

## No Rust changes

Zero new Rust AST nodes. The engine already handles everything:
- `Branch` with prefix-stripping dispatch (from NAMESPACED_KIND_PREFIXES)
- `GetField("value")` auto-unwrap via `unwrapBranchCases`
- `Chain`, `Tag`, `Constant`, `Identity`, `Panic`, `Drop` composition
