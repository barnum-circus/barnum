# Option.or

**Status:** Design. Not approved. No implementation started.

## Goal

Add `Option.or`, the None-fallback dual of `Result.or`. Some keeps the first Option. None evaluates `fallback` and returns that Option.

Specified in `refactors/past/OPTION_TYPES.md` section 5. Never implemented.

## Motivation

Two `Option<T>` values, keep the first Some:

```ts
localFailures.or(ciFailures)
```

Today that is a nested `branch`:

```ts
localFailures.branch({
  None: ciFailures.branch({
    None: Result.ok<null, string>(),
    Some: grok,
  }),
  Some: grok,
})
```

## Current state

`Option.or` is not on the namespace. `libs/barnum/src/option.ts` has `map`, `andThen`, `unwrap`, `unwrapOr`, `filter`, `collect`, `isSome`, `isNone`, `transpose`.

`Result.or` exists (`libs/barnum/src/result.ts:64-71`):

```ts
or<TValue, TError, TErrorOut>(
  fallback: Pipeable<TError, ResultT<TValue, TErrorOut>>,
): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
  return branch({
    Ok: Result.ok<TValue, TErrorOut>(),
    Err: fallback,
  }) as TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>>;
}
```

Postfix `.or` is Result-only.

Type (`libs/barnum/src/ast.ts:368-372`):

```ts
or<TIn, TValue, TError, TErrorOut>(
  this: TypedAction<TIn, Result<TValue, TError>>,
  fallback: Pipeable<TError, Result<TValue, TErrorOut>>,
): TypedAction<TIn, Result<TValue, TErrorOut>>;
```

Runtime (`libs/barnum/src/ast.ts:835-844`):

```ts
function orMethod(this: TypedAction, fallback: Action): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: branch({
      Ok: Result.ok(),
      Err: fallback,
    }),
  });
}
```

`andThen` and `unwrapOr` already dispatch Option and Result through `branchFamily`. `.or` does not.

`refactors/pending/API_SURFACE_AUDIT.md` lists `Result.or` as exists. It does not list `Option.or`.

## Design

Same shape as `OPTION_TYPES.md` section 5, same dispatch as `.andThen` / `.unwrapOr`.

`or` and `or_else` collapse to one method. The fallback is an action, already lazy.

Same inner type on both sides. Rust `Option::or` is `Option<T> → Option<T>`. `Result.or` can change the Err type because Err is the fallback side. None has no payload to retarget.

`Option.and` is also specified in `OPTION_TYPES.md` and also missing. This doc does not add it.

### `Option.or`

```ts
Option.or<TValue>(
  fallback: Pipeable<void, Option<TValue>>,
): TypedAction<Option<TValue>, Option<TValue>>
```

Desugars to:

```ts
branch({
  Some: Option.some<TValue>(),
  None: fallback,
})
```

Some receives `TValue` and rewraps. None receives `void` and runs `fallback`.

`Pipeable<void, Option<TValue>>` matches `Option.unwrapOr`. `constant(...)` is `TypedAction<any, T>`. A `VarRef` is `TypedAction<any, TValue>` (`libs/barnum/src/bind.ts:28`). Both assign. This is the call site:

```ts
issue.getField("localFailures").or(issue.getField("ciFailures"))
```

`issue` is a `VarRef`. `issue.getField("ciFailures")` is `ResumePerform` then `GetField`. It does not read the None payload.

### Postfix `.or`

Add an Option overload above the existing Result overload. Option-first matches `.andThen` and `.unwrapOr`.

```ts
or<TIn, TValue>(
  this: TypedAction<TIn, Option<TValue>>,
  fallback: Pipeable<void, Option<TValue>>,
): TypedAction<TIn, Option<TValue>>;
or<TIn, TValue, TError, TErrorOut>(
  this: TypedAction<TIn, Result<TValue, TError>>,
  fallback: Pipeable<TError, Result<TValue, TErrorOut>>,
): TypedAction<TIn, Result<TValue, TErrorOut>>;
```

`orMethod` becomes `branchFamily`, same as `andThenMethod` (`libs/barnum/src/ast.ts:788-797`):

```ts
function orMethod(this: TypedAction, fallback: Action): TypedAction {
  return typedAction({
    kind: "Chain",
    first: this,
    rest: branchFamily({
      Result: branch({
        Ok: Result.ok(),
        Err: fallback,
      }),
      Option: branch({
        Some: Option.some(),
        None: fallback,
      }),
    }),
  });
}
```

The `Option` namespace function stays a plain `branch`. Only the postfix goes through `orMethod`.

`withRetry` unrolls `action.or(action)` (`libs/barnum/src/retry.ts:18-20`). After this change those `.or` calls still see a Result. `extractPrefix` on `Result.Ok` / `Result.Err` yields `Result`, then the Result arm runs. No change to `retry.ts`.

## Files

### `libs/barnum/src/option.ts`

Insert after `andThen`, mirroring `Result.or` after `Result.andThen`.

```ts
/**
 * Fallback if None. If Some, keep it. If None, evaluate fallback.
 * `Option<TValue> → Option<TValue>`
 */
or<TValue>(
  fallback: Pipeable<void, OptionT<TValue>>,
): TypedAction<OptionT<TValue>, OptionT<TValue>> {
  return branch({
    Some: Option.some<TValue>(),
    None: fallback,
  }) as TypedAction<OptionT<TValue>, OptionT<TValue>>;
},
```

The `as` cast matches every other `Option.*` combinator in this file.

### `libs/barnum/src/ast.ts`

1. Add the Option `.or` overload at line 368, keep the Result overload.
2. Replace `orMethod` with the `branchFamily` version above.

`typedAction` already attaches `or: { value: orMethod }` (line 1132). No change there.

### `libs/barnum/tests/option.test.ts`

Follow the existing three-layer pattern (types, AST, execution).

Types, next to `Option.andThen`:

```ts
it("Option.or(fallback): Option<T> -> Option<T>", () => {
  const action = O.or<string>(
    pipe(drop, tag<"Option", OptionDef<string>, "None">("None", "Option")),
  );
  assertIO<typeof action, Option<string>, Option<string>>();
});
```

AST, next to `Option.andThen()`:

```ts
it("Option.or(fallback) desugars correctly", () => {
  const fallback = tag<"Option", OptionDef<string>, "Some">("Some", "Option");
  const action = O.or(fallback);
  const branchNode = action as { kind: "Branch"; cases: any };
  expect(branchNode.kind).toBe("Branch");
  expect(branchNode.cases.Some.rest).toEqual(expectedTagAst("Option.Some"));
  expect(branchNode.cases.None.rest).toBe(fallback);
});
```

Execution, next to the `andThen` block:

```ts
it("Option.or on Some keeps the first value", async () => {
  const result = await pipe(
    constant("a").some(),
    O.or(constant("b").some()),
  ).run();
  expect(result).toEqual({ kind: "Option.Some", value: "a" });
});

it("Option.or on None runs fallback", async () => {
  const result = await pipe(
    pipe(constant(null), O.none<string>()),
    O.or(constant("b").some()),
  ).run();
  expect(result).toEqual({ kind: "Option.Some", value: "b" });
});

it("postfix .or on None runs fallback", async () => {
  const result = await constant(null)
    .then(O.none<string>())
    .or(constant("b").some())
    .run();
  expect(result).toEqual({ kind: "Option.Some", value: "b" });
});
```

`constant("b").some()` is `TypedAction<any, Option<string>>`. It assigns to `Pipeable<void, Option<string>>`.

### `libs/barnum/tests/result.test.ts`

`Result.or(...)` namespace tests stay. They do not go through `orMethod`.

Add one postfix execution test so the `branchFamily` change is covered:

```ts
it("postfix .or on Err applies fallback", async () => {
  const result = await constant("fail")
    .err()
    .or(pipe(constant(99), R.ok<number, string>()))
    .run();
  expect(result).toEqual({ kind: "Result.Ok", value: 99 });
});
```

Existing `Result.or on Err applies fallback` stays as the namespace test.

### `docs-website/docs/reference/builtins.md`

In the `Option<T>` table, after `Option.andThen`:

```
| `Option.or(fallback)` | `Option<T> → Option<T>` | If Some, keep it. If None, evaluate fallback (`void → Option<T>`) |
```

Update the postfix sentence under that table. Current text only mentions `.mapOption`. After this change `.or` dispatches Option and Result, like `.andThen` and `.unwrapOr`. Write:

```
**Postfix:** `.or(fallback)` on an `Option<T>` output is `Option.or`. `.or(fallback)` on a `Result<T, E>` output is `Result.or`. Prefer postfix.
```

Do not edit `versioned_docs/`.

### `refactors/pending/API_SURFACE_AUDIT.md`

In Self: `Option<T>` exists table, after `Option.andThen`:

```
| `Option.or(fallback)` | `Option<T> → Option<T>` | exists, postfix | `fallback: void → Option<T>`. `.or()` dispatches across Option/Result. |
```

## Order

1. Failing tests in `option.test.ts` and the Result postfix test in `result.test.ts`.
2. `Option.or` in `option.ts`.
3. Overload and `orMethod` in `ast.ts`.
4. Drop failure markers.
5. Docs and `API_SURFACE_AUDIT.md`.

1 without 2–3 does not typecheck. 2 without 3 ships the namespace only; postfix `.or` on Option is still a type error. Land 2 and 3 in the same implementation commit.

## End-user experience

```ts
localFailures.or(ciFailures)
```

Some on the left: that Some, `ciFailures` does not run. None on the left: the `ciFailures` Option.

```ts
pipe(maybe, Option.or(constant("fallback").some()))
```

is the same pipeline.
