# Postfix Operators (Method Chaining on TypedAction)

## Current state

TypedAction has `.then()` and `.forEach()` as postfix operators:

```ts
handler.then(nextHandler)           // pipe(handler, nextHandler)
handler.forEach()                   // forEach(handler)
```

These work because `typedAction()` attaches methods to the action object via `Object.defineProperties`.

## Design criteria

A postfix operator should:
1. **Read naturally left-to-right** — the method describes what happens *next* to the output
2. **Not create ambiguity** — it should be obvious what the operator does
3. **Be high-frequency** — worth the API surface

A postfix operator should NOT:
1. Take the action as an *input* (that's a prefix combinator) — `.parallel()` and `.loop()` are wrong because they wrap the action, they don't chain after it
2. Be a structural combinator that takes multiple arguments — `.pipe()` is redundant with `.then()`

## Approved (Phase 1)

### `.branch(cases)` — dispatch on tagged union output

```ts
classifyErrors.branch({
  HasErrors: pipe(getField("errors"), fix),
  Clean: drop(),
})
// equivalent to: pipe(classifyErrors, branch({ HasErrors: ..., Clean: ... }))
```

The receiver produces the tagged union, and `.branch()` dispatches on it. Reads naturally: "classify errors, then branch."

### `.flatten()` — flatten nested array

```ts
forEach(analyze).flatten()
// equivalent to: pipe(forEach(analyze), flatten())
```

Reads well: "for each, analyze, then flatten."

### `.drop()` — discard output

```ts
sideEffect.drop()
// equivalent to: pipe(sideEffect, drop())
```

### `.tag(kind)` — wrap output as tagged union

```ts
value.tag("Ok")
// equivalent to: pipe(value, tag("Ok"))
```

### `.getField(field)` — extract field from output

```ts
handler.getField("name")
// equivalent to: pipe(handler, getField("name"))
```

Renamed from `.getField()` — shorter, reads well as postfix.

## Deferred

### `.tap(action)` — run side effect, preserve value

Want this. The postfix form solves a real ergonomic problem: the standalone `tap()` requires three type parameters (`TInput`, `TOutput`, `TRefs`) because it can't infer `TInput` from the pipeline context. The postfix form knows `Out` from `this`, so zero explicit type params are needed:

```ts
// Standalone: three type params, two of which are noise
tap<Ctx, any, "TypeCheck">(stepRef("TypeCheck"))

// Postfix: zero type params
someCtxAction.tap(stepRef("TypeCheck"))
```

Signature: `tap<TRefs extends string = never>(action: Pipeable<Out, any, TRefs>): TypedAction<In, Out, Refs | TRefs>`. `Out` is known, `TRefs` is inferred from the argument, `TOutput` is hardcoded to `any` (tap discards it).

### `.augment(action)` — enrich with extra fields

Confusing as a postfix. Defer.

### `.merge()` — merge array of objects

Need to rethink the parallel + merge pattern first. `parallel(a, b, c)` gives a tuple, and `.merge()` would flatten it into an object. But maybe `parallel` should produce an object directly (keyed parallel), making `.merge()` unnecessary. Defer until parallel semantics are settled.

### `.loop()` — wrap in loop

Rejected. Loop semantics (must produce Continue/Break) aren't obvious from the method call. Use the prefix `loop(body)` instead.

### `.try()` / `.attempt()` — wrap in error handler

Skip for now. Need to think about how error handling works first.

## Option/Result postfix operators (Phase 2)

The Rust-inspired Option combinators (`mapOption`, `flatMapOption`, `unwrapOr`, etc.) could work as postfix operators **if** they are only available when `Out` matches the Option shape.

TypeScript can enforce this via `this` parameter constraints or conditional return types:

```ts
// Only callable when Out is Option<T>
mapOption<TNext>(
  this: TypedAction<In, { kind: "Some"; value: unknown } | { kind: "None" }, Refs>,
  action: TypedAction</* inferred from Some's value */, TNext>,
): TypedAction<In, { kind: "Some"; value: TNext } | { kind: "None" }, Refs>;

unwrapOr(
  this: TypedAction<In, { kind: "Some"; value: unknown } | { kind: "None" }, Refs>,
  defaultValue: TypedAction<never, /* inferred from Some's value */>,
): TypedAction<In, /* Some's value type */, Refs>;
```

Note: `unwrapOr` takes an **action** (AST), not a raw value. Use `unwrapOr(constant("anonymous"))`, not `unwrapOr("anonymous")`.

The naming convention: include "option" in the name (`mapOption`, `flatMapOption`, `unwrapOr`, `optionOr`) to distinguish from potential Result variants. But if the `this` constraint is tight enough, maybe just `.map()`, `.flatMap()`, `.or()` work.

Open question: can TypeScript reliably infer `T` from a `this` constraint of `TypedAction<In, { kind: "Some"; value: T } | { kind: "None" }, Refs>`? Need to prototype.

## Candidates to avoid

### `.parallel(...)` — NO

`parallel` is a fan-out from a single input. The receiver-as-input pattern is confusing.

### `.pipe(...)` — NO

Redundant with `.then()`.

### `.loop()` — NO

Loop semantics aren't obvious from the method call.

## Implementation

Each method is a shared closure, added in `typedAction()` via `Object.defineProperties`:

```ts
function branchMethod(this: TypedAction, cases: Record<string, Action>): TypedAction {
  return typedAction({ kind: "Chain", first: this, rest: { kind: "Branch", cases } });
}

function flattenMethod(this: TypedAction): TypedAction {
  return typedAction({
    kind: "Chain", first: this,
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Flatten" } } },
  });
}

// dropMethod, tagMethod, getMethod follow the same pattern
```

Future consideration: prototype chain instead of `Object.defineProperties`. Would avoid per-instance property attachment. Not blocking — the current approach works and the performance difference is negligible for an AST builder.

## Type signatures

```ts
export type TypedAction<In, Out, Refs extends string = never> = Action & {
  // ... existing phantoms and methods ...

  branch<TCases extends Record<string, ChainableAction<any, any, any>>>(
    cases: TCases,
  ): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs | ExtractRefs<TCases[keyof TCases & string]>>;

  flatten(): TypedAction<In, Out extends (infer TElement)[][] ? TElement[] : never, Refs>;

  drop(): TypedAction<In, never, Refs>;

  tag<TKind extends string>(kind: TKind): TypedAction<In, { kind: TKind; value: Out }, Refs>;

  get<TField extends keyof Out & string>(field: TField): TypedAction<In, Out[TField], Refs>;
};
```
