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
2. Be a structural combinator that takes multiple arguments — `.branch({ ... })` is confusing because the receiver is the input, not a branch

## Good candidates

### `.tap(action)` — run side effect, preserve value

```ts
implement.tap(commit).tap(typeCheck).then(createPR)
// equivalent to: pipe(tap(implement), tap(commit), tap(typeCheck), createPR)
```

Wait — this reads wrong. `implement.tap(commit)` means "run implement, then tap commit." But the receiver is `implement`, not the tap target. Let's think again.

Actually: `action.tap(sideEffect)` means "run action, then run sideEffect on the result for side effects, then return the result of action." This is:

```ts
pipe(action, tap(sideEffect))
```

That reads fine: "do action, and tap sideEffect along the way."

### `.augment(action)` — enrich with extra fields

```ts
handler.augment(enricher)
// equivalent to: pipe(handler, augment(enricher))
```

"Do handler, then augment with enricher's output."

### `.mapOption(action)` — map over Some, pass through None

```ts
extractField("name").mapOption(normalize)
// equivalent to: pipe(extractField("name"), mapOption(normalize))
```

### `.unwrapOr(default)` — unwrap Option with default

```ts
extractField("name").unwrapOr("anonymous")
```

### `.branch(cases)` — dispatch on tagged union output

```ts
classifyErrors.branch({
  HasErrors: pipe(extractField("errors"), fix),
  Clean: drop(),
})
// equivalent to: pipe(classifyErrors, branch({ HasErrors: ..., Clean: ... }))
```

This is actually a strong candidate. The receiver produces the tagged union, and `.branch()` dispatches on it. Reads naturally: "classify errors, then branch."

### `.loop()` — wrap in loop

```ts
body.loop()
// equivalent to: loop(body)
```

This one is iffy. "body.loop()" reads as "run body in a loop," which is correct, but it obscures that `body` must produce Continue/Break tags.

### `.flatten()` — flatten nested array

```ts
forEach(analyze).flatten()
// equivalent to: pipe(forEach(analyze), flatten())
```

Reads well: "for each, analyze, then flatten."

### `.merge()` — merge array of objects

```ts
parallel(a, b, c).merge()
// equivalent to: pipe(parallel(a, b, c), merge())
```

Reads well: "run in parallel, then merge."

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

### `.extractField(field)` — extract field from output

```ts
handler.extractField("name")
// equivalent to: pipe(handler, extractField("name"))
```

Reads fine but is verbose. Maybe `.field("name")` or `.get("name")`?

### `.tryAction()` — wrap in error handler

```ts
riskyHandler.try()
// equivalent to: tryAction(riskyHandler)
```

Hmm, `.try()` is a reserved word in JS. `.attempt()`?

## Candidates to avoid

### `.parallel(...)` — NO

```ts
action.parallel(other)  // Confusing — does this run action and other in parallel?
```

`parallel` is a fan-out from a single input. The receiver-as-input pattern is confusing.

### `.pipe(...)` — NO

```ts
action.pipe(next)  // Redundant with .then()
```

`.then()` already does this.

### `.loop()` — MAYBE

Could go either way. It's not terrible but loop semantics (must produce Continue/Break) aren't obvious from the method call.

## Implementation

Each method is added in `typedAction()`:

```ts
function typedAction<In, Out, Refs>(action: Action): TypedAction<In, Out, Refs> {
  Object.defineProperties(action, {
    then: { value: thenMethod, configurable: true },
    forEach: { value: forEachMethod, configurable: true },
    tap: { value: tapMethod, configurable: true },
    augment: { value: augmentMethod, configurable: true },
    branch: { value: branchMethod, configurable: true },
    flatten: { value: flattenMethod, configurable: true },
    merge: { value: mergeMethod, configurable: true },
    drop: { value: dropMethod, configurable: true },
    // ...
  });
  return action as TypedAction<In, Out, Refs>;
}
```

Each method creates `pipe(this, combinator(...args))` and returns a new `TypedAction`.

## Type signatures

The tricky part: each method needs correct generic types on the TypedAction interface.

```ts
interface TypedAction<In, Out, Refs> extends Action {
  then<T, R extends string>(next: TypedAction<Out, T, R>): TypedAction<In, T, Refs | R>;
  forEach(): TypedAction<In extends (infer E)[] ? In : never, Out[], Refs>;
  tap(action: TypedAction<any, any>): TypedAction<In, Out, Refs>;
  augment<T extends Record<string, unknown>>(action: TypedAction<any, T>): TypedAction<In, Out & T, Refs>;
  branch<Cases extends Record<string, TypedAction<any, any>>>(cases: Cases): TypedAction<In, ...>;
  flatten(): TypedAction<In, Out extends (infer E)[][] ? E[] : never, Refs>;
  merge(): TypedAction<In, ..., Refs>;
  drop(): TypedAction<In, never, Refs>;
  tag<K extends string>(kind: K): TypedAction<In, { kind: K; value: Out }, Refs>;
  extractField<F extends keyof Out & string>(field: F): TypedAction<In, Out[F], Refs>;
}
```

`branch` typing is hard — it needs to infer the union of all case outputs. The prefix function already handles this with overloads. The postfix method would need the same treatment.

## Recommendation

**Phase 1**: Add `.tap()`, `.augment()`, `.branch()`, `.flatten()`, `.merge()`, `.drop()`, `.tag()`, `.extractField()`.

These are all "do this, then apply transformation to the output" — the natural postfix pattern.

**Phase 2**: Add Option/Result methods (`.mapOption()`, `.unwrapOr()`, `.mapOk()`, etc.) once those combinators exist.

**Skip**: `.parallel()`, `.pipe()`, `.loop()` — these don't read naturally as postfix operations.
