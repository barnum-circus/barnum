# Tagged Union Constructors: Values → Functions

## Motivation

`Option.some`, `Option.none`, `Result.ok`, and `Result.err` are currently **values** — the result of calling `tag(kind, enumName)` at module load time. Because `tag`'s generic parameter `TDef` is unconstrained at the call site, TypeScript infers `Record<string, unknown>`, erasing the element type:

```ts
// option.ts — current
export const Option = {
  some: tag("Some", "Option"),
  // Inferred: TypedAction<unknown, TaggedUnion<"Option", Record<string, unknown>>>
  // Lost: the T in Option<T>
};
```

This forces verbose explicit type parameters at every call site:

```ts
// demos/babysit-prs/run.ts — current
.then(tag<"Option", OptionDef<number>, "Some">("Some", "Option"))
```

And test helpers must also use the verbose form:

```ts
// libs/barnum/tests/option.test.ts — current
function optionSome<T>(value: T): TypedAction<any, Option<T>> {
  return pipe(constant(value), tag<"Option", OptionDef<T>, "Some">("Some", "Option"));
}
```

**Fix:** Make all four constructors generic functions. The generic parameter captures the element type, and TypeScript infers it from chain context:

```ts
// After:
.then(Option.some())          // T inferred from chain as number
Option.some<number>()         // explicit when needed — still much less verbose
```

---

## Current state

### `option.ts` (lines 33-36)

```ts
export const Option = {
  some: tag("Some", "Option"),
  none: tag("None", "Option"),
  // ...
} as const;
```

### `result.ts` (lines 19-21)

```ts
export const Result = {
  ok: tag("Ok", "Result"),
  err: tag("Err", "Result"),
  // ...
} as const;
```

### Call sites in library code (`ast.ts`)

All postfix method implementations reference these as values in branch cases:

```ts
// mapMethod (line ~565)
Some: chain(toAction(action), toAction(Option.some)),
None: Option.none,

// unwrapOrMethod (line ~584)
Option: branch({ Some: identity(), None: defaultAction }),

// andThenMethod (line ~591)
Option: branch({ Some: action, None: Option.none }),

// transposeMethod (lines ~598-612)
Some: chain(toAction(Option.some), toAction(Result.ok)),
None: chain(toAction(chain(toAction(drop), toAction(Option.none))), toAction(Result.ok)),
```

### Call sites in namespace code (`option.ts`, `result.ts`, `race.ts`)

**`option.ts`** — Option methods that reference their own constructors and Result constructors:

```ts
// Option.map: Option.some
// Option.andThen: Option.none
// Option.filter: Option.none
// Option.transpose: Option.some, Option.none, Result.ok, Result.err
```

**`result.ts`** — Result methods that reference their own constructors and Option constructors:

```ts
// Result.map: Result.ok, Result.err
// Result.mapErr: Result.ok, Result.err
// Result.andThen: Result.err
// Result.or: Result.ok
// Result.and: Result.err
// Result.toOption: Option.some, Option.none
// Result.toOptionErr: Option.some, Option.none
// Result.transpose: Result.ok, Result.err, Option.some, Option.none
```

**`race.ts`** — `withTimeout` uses `Result.ok` (line 147) and `Result.err` (line 153).

### Call sites in tests

```ts
// option.test.ts (lines 51-56)
function optionSome<T>(value: T): TypedAction<any, Option<T>> {
  return pipe(constant(value), tag<"Option", OptionDef<T>, "Some">("Some", "Option"));
}
function optionNone<T>(): TypedAction<any, Option<T>> {
  return pipe(constant(null), tag<"Option", OptionDef<T>, "None">("None", "Option"));
}

// result.test.ts (lines 54-58)
function resultOk<TValue, TError = unknown>(value: TValue): TypedAction<any, Result<TValue, TError>> {
  return pipe(constant(value), tag<"Result", ResultDef<TValue, TError>, "Ok">("Ok", "Result"));
}
function resultErr<TValue, TError>(error: TError): TypedAction<any, Result<TValue, TError>> {
  return pipe(constant(error), tag<"Result", ResultDef<TValue, TError>, "Err">("Err", "Result"));
}
```

### Call sites in demos

```ts
// demos/babysit-prs/run.ts (lines 50-52)
.then(tag<"Option", OptionDef<number>, "Some">("Some", "Option")),
.then(tag<"Option", OptionDef<number>, "None">("None", "Option")),
.then(tag<"Option", OptionDef<number>, "None">("None", "Option")),
```

---

## Proposed changes

### Constructor signatures

`tag` already accepts type parameters `<TEnumName, TDef, TKind>`, so passing explicit type params produces the correct return type — no cast needed.

```ts
// option.ts — after
export const Option = {
  some<T>(): TypedAction<T, OptionT<T>> {
    return tag<"Option", OptionDef<T>, "Some">("Some", "Option");
  },
  none<T>(): TypedAction<void, OptionT<T>> {
    return tag<"Option", OptionDef<T>, "None">("None", "Option");
  },
  // ... rest unchanged
} as const;
```

```ts
// result.ts — after
export const Result = {
  ok<TValue, TError = unknown>(): TypedAction<TValue, ResultT<TValue, TError>> {
    return tag<"Result", ResultDef<TValue, TError>, "Ok">("Ok", "Result");
  },
  err<TValue = unknown, TError = never>(): TypedAction<TError, ResultT<TValue, TError>> {
    return tag<"Result", ResultDef<TValue, TError>, "Err">("Err", "Result");
  },
  // ... rest unchanged
} as const;
```

**Note on defaults:** `Result.ok` defaults TError to `unknown` so `Result.ok<string>()` works without specifying the error type. `Result.err` defaults TValue to `unknown` for the same reason.

### Call site migration

Every `Option.some` → `Option.some()`, `Option.none` → `Option.none()`, `Result.ok` → `Result.ok()`, `Result.err` → `Result.err()`. Purely mechanical — add `()`.

Internal code (branch cases in `ast.ts`, `option.ts`, `result.ts`) doesn't benefit from the type info (everything is cast at the end), but the syntax change is required.

Demo code gets dramatically cleaner:

```ts
// Before:
.then(tag<"Option", OptionDef<number>, "Some">("Some", "Option"))

// After:
.then(Option.some())
```

Test helpers become trivial:

```ts
// Before:
function optionSome<T>(value: T): TypedAction<any, Option<T>> {
  return pipe(constant(value), tag<"Option", OptionDef<T>, "Some">("Some", "Option"));
}

// After:
function optionSome<T>(value: T): TypedAction<any, Option<T>> {
  return pipe(constant(value), Option.some<T>());
}
```

---

## Implementation tasks

### Task 1: Add type-loss assertion tests

**Goal:** Tests that prove the current value form loses type information. These tests pass now (documenting the problem). After the fix, we change them to assert type retention.

**File:** `libs/barnum/tests/option.test.ts`

Add to the "Type tests" describe block:

```ts
describe("constructor type info", () => {
  it("Option.some() retains element type", () => {
    const some = O.some<number>();
    assertExact<IsExact<ExtractInput<typeof some>, number>>();
    assertExact<IsExact<ExtractOutput<typeof some>, Option<number>>>();
  });

  it("Option.none() retains element type", () => {
    const none = O.none<string>();
    assertExact<IsExact<ExtractOutput<typeof none>, Option<string>>>();
  });

  it("Option.some() infers type from chain context", () => {
    const result = constant(42).then(O.some());
    assertExact<IsExact<ExtractOutput<typeof result>, Option<number>>>();
  });
});
```

**File:** `libs/barnum/tests/result.test.ts`

```ts
describe("constructor type info", () => {
  it("Result.ok() retains value type", () => {
    const ok = R.ok<string>();
    assertExact<IsExact<ExtractInput<typeof ok>, string>>();
    assertExact<IsExact<ExtractOutput<typeof ok>, Result<string, unknown>>>();
  });

  it("Result.err() retains error type", () => {
    const err = R.err<unknown, number>();
    assertExact<IsExact<ExtractInput<typeof err>, number>>();
  });

  it("Result.ok() infers type from chain context", () => {
    const result = constant("hello").then(R.ok());
    assertExact<IsExact<ExtractOutput<typeof result>, Result<string, unknown>>>();
  });
});
```

**Commit 1:** Add these tests with `@ts-expect-error` on the assertions that currently fail (the `some()` / `ok()` calls don't exist yet).
**Commit 2:** Implement the changes (Task 2). Tests compile and pass.
**Commit 3:** Remove any remaining failure markers.

---

### Task 2: Convert constructors to functions

##### 2.1: `option.ts` — change `some` and `none`

**File:** `libs/barnum/src/option.ts` (lines 33-36)

```ts
// Before:
some: tag("Some", "Option"),
none: tag("None", "Option"),

// After:
some<T>(): TypedAction<T, OptionT<T>> {
  return tag<"Option", OptionDef<T>, "Some">("Some", "Option");
},
none<T>(): TypedAction<void, OptionT<T>> {
  return tag<"Option", OptionDef<T>, "None">("None", "Option");
},
```

##### 2.2: Update call sites in `option.ts`

Every `Option.some` → `Option.some()`, `Option.none` → `Option.none()`:

```ts
// Option.map (line ~41):
Some: chain(toAction(action), toAction(Option.some())),
None: Option.none(),

// Option.andThen (line ~53):
None: Option.none(),

// Option.filter (line ~90):
None: Option.none(),

// Option.transpose (lines ~140-143):
Some: chain(toAction(Option.some()), toAction(Result.ok())),
None: chain(toAction(chain(toAction(drop), toAction(Option.none()))), toAction(Result.ok())),
```

##### 2.3: `result.ts` — change `ok` and `err`

**File:** `libs/barnum/src/result.ts`

```ts
// Before:
ok: tag("Ok", "Result"),
err: tag("Err", "Result"),

// After:
ok<TValue, TError = unknown>(): TypedAction<TValue, ResultT<TValue, TError>> {
  return tag<"Result", ResultDef<TValue, TError>, "Ok">("Ok", "Result");
},
err<TValue = unknown, TError = never>(): TypedAction<TError, ResultT<TValue, TError>> {
  return tag<"Result", ResultDef<TValue, TError>, "Err">("Err", "Result");
},
```

##### 2.4: Update call sites in `result.ts`

Every `Result.ok` → `Result.ok()`, `Result.err` → `Result.err()`.

##### 2.5: Update call sites in `ast.ts`

Every postfix method that references these constructors. Mechanical `→ ()` addition:

```ts
// mapMethod: Option.some → Option.some(), Result.ok → Result.ok(), Result.err → Result.err()
// andThenMethod: Option.none → Option.none(), Result.err → Result.err()
// transposeMethod: Option.some → Option.some(), Option.none → Option.none(),
//                  Result.ok → Result.ok(), Result.err → Result.err()
// mapErrMethod: Result.ok → Result.ok(), Result.err → Result.err()
// orMethod: Result.ok → Result.ok()
// andPostfixMethod: Result.err → Result.err()
// toOptionMethod: Option.some → Option.some(), Option.none → Option.none()
// toOptionErrMethod: Option.some → Option.some(), Option.none → Option.none()
```

##### 2.6: Update call sites in `race.ts`

**File:** `libs/barnum/src/race.ts` (lines 147, 153)

```ts
// Before:
toAction(Result.ok)
toAction(Result.err)

// After:
toAction(Result.ok())
toAction(Result.err())
```

---

### Task 3: Update tests

##### 3.1: Simplify test helpers

**File:** `libs/barnum/tests/option.test.ts` (lines 51-56)

```ts
// Before:
function optionSome<T>(value: T): TypedAction<any, Option<T>> {
  return pipe(constant(value), tag<"Option", OptionDef<T>, "Some">("Some", "Option"));
}
function optionNone<T>(): TypedAction<any, Option<T>> {
  return pipe(constant(null), tag<"Option", OptionDef<T>, "None">("None", "Option"));
}

// After:
function optionSome<T>(value: T): TypedAction<any, Option<T>> {
  return pipe(constant(value), O.some<T>());
}
function optionNone<T>(): TypedAction<any, Option<T>> {
  return pipe(constant(null), O.none<T>());
}
```

**File:** `libs/barnum/tests/result.test.ts` (lines 54-58)

```ts
// Before:
function resultOk<TValue, TError = unknown>(value: TValue): TypedAction<any, Result<TValue, TError>> {
  return pipe(constant(value), tag<"Result", ResultDef<TValue, TError>, "Ok">("Ok", "Result"));
}
function resultErr<TValue, TError>(error: TError): TypedAction<any, Result<TValue, TError>> {
  return pipe(constant(error), tag<"Result", ResultDef<TValue, TError>, "Err">("Err", "Result"));
}

// After:
function resultOk<TValue, TError = unknown>(value: TValue): TypedAction<any, Result<TValue, TError>> {
  return pipe(constant(value), R.ok<TValue, TError>());
}
function resultErr<TValue, TError>(error: TError): TypedAction<any, Result<TValue, TError>> {
  return pipe(constant(error), R.err<TValue, TError>());
}
```

##### 3.2: Remove `tag` import from tests that no longer need it

Both test files import `tag` for the helpers. After simplification, `tag` may only be needed for `expectedTagAst`. Check and remove if unused.

---

### Task 4: Update demos

##### 4.1: `babysit-prs/run.ts`

**File:** `demos/babysit-prs/run.ts` (lines 50-52)

```ts
// Before:
ChecksFailed: fixIssues
  .drop()
  .then(prNumber)
  .then(tag<"Option", OptionDef<number>, "Some">("Some", "Option")),
ChecksPassed: landPR.drop().then(tag<"Option", OptionDef<number>, "None">("None", "Option")),
Landed: drop.then(tag<"Option", OptionDef<number>, "None">("None", "Option")),

// After:
ChecksFailed: fixIssues
  .drop()
  .then(prNumber)
  .then(Option.some()),
ChecksPassed: landPR.drop().then(Option.none()),
Landed: drop.then(Option.none()),
```

Update imports: remove `tag` and `OptionDef` import, ensure `Option` is imported.

##### 4.2: Any other demos using bare `tag` for Option/Result construction

Search for `tag<"Option"` and `tag<"Result"` across all demos. Each instance should become the corresponding namespace constructor call.

---

## Relationship to TRAIT_DISPATCH_AND_ITERATORS

The Iterator refactor doc proposes `Iter.wrap` as:

```ts
wrap: tag("Iterator", "Iterator") as TypedAction<unknown[], IteratorT<unknown>>,
```

This has the same type-erasure problem. After this refactor lands, `Iter.wrap` should follow the same pattern:

```ts
wrap<TElement>(): TypedAction<TElement[], IteratorT<TElement>> {
  return tag<"Iterator", IteratorDef<TElement>, "Iterator">("Iterator", "Iterator");
},
```
