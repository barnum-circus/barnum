# Explore: `.call()`, `.apply()`, `.bind()` on TypedAction

## Motivation

Currently, supplying arguments to a pipeline step requires composing with `all`, `bindInput`, and `.split()`:

```ts
// Current: to pass (acc, element) to a body that needs both
Iterator.fold(
  constant(0),
  bindInput<[number, Item]>((state) => {
    const [acc, item] = state.split();
    return item.then(getScore).then(add(acc));
  }),
)
```

This works but reads as infrastructure. The user's intent — "call `getScore` on `item`, then `add` with `acc`" — is buried under tuple unpacking ceremony. The question: can we give TypedAction `.call()` / `.apply()` / `.bind()` methods that let users write something closer to:

```ts
// Hypothetical: what if actions had .call()?
getScore.call(item).then(add.call(acc))
```

This doc explores what that API surface would look like, what it composes with, and where it breaks down.

---

## Design space

### `.call(...args)` — invoke an action with explicit VarRef arguments

**Signature:**
```ts
// Single argument: action expects TIn, we supply it as a VarRef
call<TIn>(this: TypedAction<TIn, TOut>, input: VarRef<TIn>): TypedAction<any, TOut>;

// The action is "called" with the VarRef's value instead of whatever
// is currently flowing through the pipeline.
```

**Semantics:** `action.call(ref)` ≡ `ref.then(action)`. The VarRef provides the input; the pipeline's current value is ignored.

**Usage:**
```ts
bindInput<[number, Item]>((state) => {
  const [acc, item] = state.split();
  return getScore.call(item).then(add.call(acc));
});
```

This reads more imperatively: "call getScore with item, then call add with acc."

### `.apply(arg1, arg2, ...)` — invoke with multiple arguments

**Signature:**
```ts
// Action expects a tuple input [A, B], we provide components separately
apply<TArgs extends Array<unknown>>(
  this: TypedAction<TArgs, TOut>,
  ...args: { [K in keyof TArgs]: VarRef<TArgs[K]> }
): TypedAction<any, TOut>;
```

**Semantics:** `action.apply(a, b)` ≡ `all(a, b).then(action)`. Assembles a tuple from multiple VarRefs and feeds it to the action.

**Usage:**
```ts
bindInput<[number, Item]>((state) => {
  const [acc, item] = state.split();
  return processItem.apply(acc, item);  // processItem: TypedAction<[number, Item], Result>
});
```

### `.bind(partialArgs)` — partial application returning a narrower action

**Signature:**
```ts
// Action expects [A, B, C], we provide A upfront → new action expects [B, C]
bind<TBound extends Array<unknown>, TRemaining extends Array<unknown>>(
  this: TypedAction<[...TBound, ...TRemaining], TOut>,
  ...args: { [K in keyof TBound]: VarRef<TBound[K]> }
): TypedAction<TRemaining extends [infer Single] ? Single : TRemaining, TOut>;
```

**Semantics:** Returns a new action with some arguments pre-filled. The remaining arguments come from the pipeline.

**Usage:**
```ts
bindInput<Config>((config) => {
  const narrowQuery = queryDb.bind(config);  // queryDb: [Config, Query] → Results
  return getQueries.iterate().map(narrowQuery).collect();
});
```

---

## What `.call()` compiles to

`.call(ref)` is just `ref.then(this)` — it chains the VarRef (which produces the value) into the action. No new AST nodes. This is purely syntactic sugar.

```ts
// These are identical:
getScore.call(item)
item.then(getScore)
```

The difference is reading direction. `item.then(getScore)` reads as "item flows into getScore." `getScore.call(item)` reads as "call getScore with item." The second is more natural when you're thinking imperatively about "I have references, I want to invoke functions on them."

## What `.apply()` compiles to

`.apply(a, b)` is just `all(a, b).then(this)`. Again, no new AST nodes.

```ts
// These are identical:
processItem.apply(acc, item)
all(acc, item).then(processItem)
```

## What `.bind()` compiles to

`.bind(config)` returns a new action: `bindInput<Query>((query) => all(config, query).then(this))`. This does use `bindInput` internally — it captures the remaining pipeline input and assembles the full tuple.

```ts
// queryDb.bind(config) compiles to:
bindInput<Query>((query) => all(config, query).then(queryDb))
```

---

## Analysis: when does this help?

### It helps in `bindInput` + `.split()` bodies

The main win is inside `bindInput` callbacks where you've split a tuple into named VarRefs and want to invoke actions on them. Currently:

```ts
bindInput<[Db, Config, Query]>((state) => {
  const [db, config, query] = state.split();
  return all(db, query).then(runQuery).then(all(identity(), config)).then(formatResult);
});
```

With `.call()` / `.apply()`:

```ts
bindInput<[Db, Config, Query]>((state) => {
  const [db, config, query] = state.split();
  return runQuery.apply(db, query).then(formatResult.apply(identity(), config));
});
```

Marginal improvement. The `all(...).then(action)` pattern is what `.apply()` eliminates.

### It helps in `fold` bodies

```ts
// Current
Iterator.fold(
  constant(0),
  bindInput<[number, string]>((state) => {
    const [acc, item] = state.split();
    return item.then(getLength).then(add.call(acc));
  }),
)
```

The `add.call(acc)` reads as "call add with acc (and the pipeline value)". But wait — `add` here would need to be `TypedAction<[number, number], number>`. So it's really `add.apply(acc, getLength.call(item))`. Which is... not simpler.

### It doesn't help for linear pipelines

For `a.then(b).then(c)` — no arguments to pass, data flows naturally. `.call()` is irrelevant here.

### It doesn't help when you only have one VarRef

If you split and only use one component, `item.then(action)` is already clean. `.call(item)` adds nothing over `.then()` when there's a single input.

---

## Problems

### 1. TypeScript can't infer tuple splitting for `.bind()`

`.bind()` requires TypeScript to split a tuple type: given `TypedAction<[A, B, C], Out>` and bound args `[A]`, infer the remaining `[B, C]`. TypeScript doesn't support variadic tuple subtraction in this way. You'd need explicit type parameters:

```ts
queryDb.bind<[Config], [Query]>(config)
```

Which defeats the ergonomic goal.

### 2. `.call()` is just `.then()` backwards

`getScore.call(item)` ≡ `item.then(getScore)`. The only difference is which side of the dot the action sits on. Is that worth a new method? It's arguably *less* clear than `.then()` because `.then()` is already established as "chain" and `.call()` borrows meaning from `Function.prototype.call` (which passes `this`, not input).

### 3. `.apply()` is just `all(...).then()`

The only thing `.apply(a, b)` saves over `all(a, b).then(action)` is not needing to write `all(...)`. It saves one function call. Is that worth the API surface and the name collision with `Function.prototype.apply`?

### 4. Naming collisions with Function.prototype

`call`, `apply`, `bind` are all methods on `Function.prototype`. While TypedAction isn't a function, the name overlap will confuse users who expect JavaScript-native semantics (passing `this`, argument lists, etc.). Alternative names:

- `.invoke(ref)` instead of `.call(ref)`
- `.invokeWith(a, b)` instead of `.apply(a, b)`
- `.partial(config)` instead of `.bind(config)`

### 5. It doesn't eliminate `bindInput` + `.split()`

You still need `bindInput` to capture the pipeline value as a VarRef. You still need `.split()` to destructure it. `.call()` just changes what you do *after* splitting. The ceremony of entering the VarRef context is unchanged.

---

## Alternative: what if fold/withResource accepted labeled arguments?

Instead of adding methods to TypedAction, what if the combinators that produce tuples could label their components?

```ts
// Hypothetical: fold passes named args instead of a tuple
Iterator.fold(
  constant(0),
  bindInput<{ acc: number; element: Item }>((state) => {
    const { acc, element } = state.split();
    return element.then(getScore).then(add.call(acc));
  }),
)
```

This doesn't require new TypedAction methods — it just changes `fold`'s body type from `Pipeable<[TAcc, TElement], TAcc>` to `Pipeable<{ acc: TAcc; element: TElement }, TAcc>`. More self-documenting, same destructuring pattern.

But this changes the fold contract and adds a named-tuple vs positional-tuple decision everywhere.

---

## Verdict

`.call(ref)` is the only one with a clean semantic: "invoke this action with that value." It's trivially implemented as `ref.then(this)`. The question is whether reversing the reading direction (`action.call(ref)` vs `ref.then(action)`) is enough of a win to justify the API surface.

`.apply()` and `.bind()` have TypeScript inference problems and don't eliminate enough ceremony to justify their complexity.

**Recommendation:** Explore `.call()` (or `.invoke()`) as a single new method. Skip `.apply()` and `.bind()` unless TypeScript's type system gets variadic tuple subtraction. Keep the current `bindInput` + `.split()` + `all().then()` pattern as the general-purpose approach.

---

## Open Questions

1. Is `action.call(ref)` actually more readable than `ref.then(action)` in practice? Write 5 real `fold` bodies both ways and compare.
2. Should `.call()` accept multiple args (becoming `.apply()`)? i.e., `action.call(a, b)` ≡ `all(a, b).then(action)`?
3. Better name? `.invoke()`, `.with()`, `.using()`?
4. Does the named-argument alternative (objects instead of tuples in fold/withResource) address the readability concern better without new TypedAction surface?
