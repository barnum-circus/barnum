# Remove __union Dispatch

Replace the `__union` dispatch table mechanism with a single new builtin (`ExtractPrefix`) composed with existing `Branch`. The AST encodes the dynamic dispatch — family info (Option vs Result) is only known at execution time from the kind prefix, so the dispatch must live in the AST.

**Depends on:** NAMESPACED_KIND_PREFIXES.md (completed)

## Problem

Postfix methods (`.unwrapOr()`, `.map()`, `.mapErr()`) need to know the union family (Result vs Option) to dispatch to the correct branch pattern. Currently tracked via `__union` on TypedAction — a runtime dispatch table that maps method names to implementations. But most paths that produce union-typed output don't set `__union`: `createHandler`, `getField`, `branch`, `identity`. There's no general fix because TypeScript type info (is this Result or Option?) is erased at runtime, and `__union` is a metadata property that must be manually propagated through every combinator.

This means postfix methods like `.unwrapOr()` throw at runtime when called on handlers returned from `createHandler`, fields extracted with `getField`, etc. — the most common usage patterns.

## Approach

With namespaced kind prefixes (completed), values self-describe their family at runtime: `{ kind: "Result.Ok", value: 42 }`. We add one new builtin — `ExtractPrefix` — that restructures a tagged value so its prefix becomes the dispatchable `kind`. Composed with existing `Branch`, this gives two-level dispatch: first on the family (prefix), then on the variant.

### ExtractPrefix builtin

Transforms a tagged value by extracting the prefix from `kind`:

```
Input:  { kind: "Result.Ok", value: 42 }
Output: { kind: "Result", value: { kind: "Result.Ok", value: 42 } }
```

The original value is preserved intact as the `value` field. The prefix becomes the new `kind`.

### matchPrefix combinator (TS SDK)

`matchPrefix` is a TS-level combinator, not an AST node. It composes `extractPrefix()` with `branch()`:

```ts
function matchPrefix<TCases extends Record<string, Action>>(
  cases: TCases,
): TypedAction<...> {
  return chain(extractPrefix(), branch(cases));
}
```

The outer `branch` dispatches on the prefix (`"Result"`, `"Option"`), auto-unwraps the value, and passes the original tagged value to the case handler. The case handler is typically an inner `branch` that dispatches on the variant:

```ts
// unwrapOr(f) — shared between Option and Result
matchPrefix({
  Result: branch({ Ok: identity(), Err: f }),
  Option: branch({ Some: identity(), None: f }),
})
```

Execution trace for `{ kind: "Result.Ok", value: 42 }` through `unwrapOr(f)`:

1. `ExtractPrefix` → `{ kind: "Result", value: { kind: "Result.Ok", value: 42 } }`
2. Outer `Branch` matches `"Result"`, auto-unwraps → `{ kind: "Result.Ok", value: 42 }`
3. Inner `Branch` strips prefix `"Result."` → matches `"Ok"`, auto-unwraps → `42`
4. `identity()` → `42`

### AST size

The AST encodes the dynamic dispatch: each postfix method emits branches for all applicable families. With 2 built-in families, shared methods get 2 inner branches. The user-provided action (`f`) appears once per family in the flat AST, but only one copy executes per invocation. The growth is bounded by the number of families and negligible for a workflow engine.

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

## Rust: new `ExtractPrefix` builtin

Goes through the existing Invoke → Builtin path, same as `GetField`, `Identity`, etc.

### `barnum_ast/src/lib.rs` — add variant to `BuiltinKind`

```rust
/// Extract the enum prefix from a tagged value's `kind` field.
///
/// Input: `{ kind: "Result.Ok", value: 42 }`
/// Output: `{ kind: "Result", value: { kind: "Result.Ok", value: 42 } }`
///
/// If `kind` contains no `'.'`, the entire kind string becomes the prefix.
ExtractPrefix,
```

### `barnum_builtins/src/lib.rs` — add match arm to `execute_builtin`

```rust
BuiltinKind::ExtractPrefix => {
    let Value::Object(obj) = input else {
        return Err(BuiltinError::TypeMismatch {
            builtin: "ExtractPrefix",
            expected: "object",
            actual: input.clone(),
        });
    };
    let kind_str = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| BuiltinError::TypeMismatch {
            builtin: "ExtractPrefix",
            expected: "object with string 'kind' field",
            actual: input.clone(),
        })?;
    let prefix = kind_str
        .split_once('.')
        .map_or(kind_str, |(prefix, _)| prefix);
    Ok(json!({ "kind": prefix, "value": input }))
}
```

### `libs/barnum/src/ast.ts` — add to `BuiltinKind` type

```ts
| { kind: "ExtractPrefix" }
```

### `libs/barnum/src/builtins.ts` — TS-side constructor

```ts
export function extractPrefix(): TypedAction {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "ExtractPrefix" } },
  });
}
```

### `libs/barnum/src/ast.ts` — `matchPrefix` combinator

Internal combinator used by postfix methods. Composes `extractPrefix()` with `branch()`:

```ts
export function matchPrefix(cases: Record<string, Action>): TypedAction {
  return chain(extractPrefix(), branch(cases));
}
```

No new structural AST nodes. No new engine dispatch logic.

### Future generalization

`ExtractPrefix` is a bespoke builtin for splitting on `'.'`. It could later be replaced by a more general primitive (e.g., regex-based string splitting). Tracked in the deferred backlog.

## Postfix method rewrites

### Shared methods (Option + Result)

These dispatch on the prefix first, then on the variant within each family.

```ts
function mapMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({
      Ok: chain(toAction(action), toAction(tag("Ok", "Result"))),
      Err: tag("Err", "Result"),
    }),
    Option: branch({
      Some: chain(toAction(action), toAction(tag("Some", "Option"))),
      None: tag("None", "Option"),
    }),
  })));
}

function unwrapMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({ Ok: identity(), Err: panic("called unwrap on Err") }),
    Option: branch({ Some: identity(), None: panic("called unwrap on None") }),
  })));
}

function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({ Ok: identity(), Err: defaultAction }),
    Option: branch({ Some: identity(), None: defaultAction }),
  })));
}

function andThenMethod(this: TypedAction, action: Action): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Result: branch({ Ok: action, Err: tag("Err", "Result") }),
    Option: branch({ Some: action, None: tag("None", "Option") }),
  })));
}

function transposeMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    // Option<Result<T,E>> → Result<Option<T>,E>
    Option: branch({
      Some: branch({
        Ok: chain(toAction(tag("Some", "Option")), toAction(tag("Ok", "Result"))),
        Err: tag("Err", "Result"),
      }),
      None: chain(toAction(drop.tag("None", "Option")), toAction(tag("Ok", "Result"))),
    }),
    // Result<Option<T>,E> → Option<Result<T,E>>
    Result: branch({
      Ok: branch({
        Some: chain(toAction(tag("Ok", "Result")), toAction(tag("Some", "Option"))),
        None: drop.tag("None", "Option"),
      }),
      Err: chain(toAction(tag("Err", "Result")), toAction(tag("Some", "Option"))),
    }),
  })));
}
```

### Result-only methods

No prefix dispatch needed — only Result cases. If called on Option at runtime, the inner branch fails with "no match" (TypeScript types prevent this at compile time).

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

Same pattern as Result-only: no prefix dispatch, just Option branch cases.

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

`Result.map()`, `Option.unwrapOr()`, etc. drop their `withUnion(...)` wrappers. The branch AST they emit is already correct — they just lose the dispatch table attachment:

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

The standalone methods remain useful for explicit `pipeline.then(Result.map(f))` patterns where the family is known at the call site. They produce smaller ASTs than the postfix equivalents (no prefix dispatch overhead).

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
