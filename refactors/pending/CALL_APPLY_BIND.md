# Explore: `.call()` on TypedAction

## Motivation

Currently, supplying arguments to a pipeline step requires composing with `all`, `bindInput`, and `.split()`:

```ts
// Current: to pass (acc, element) to a body that needs both
Iterator.fold(
  constant(0),
  bindInput<[number, Item]>((state) => {
    const [acc, item] = state.split();
    return item.then(getScore).then(all(identity(), acc)).then(add);
  }),
)
```

This works but reads as infrastructure. The user's intent — "call `getScore` on `item`, then `add` with `acc`" — is buried under tuple unpacking ceremony. The question: can we give TypedAction a `.call()` method that lets users write something closer to:

```ts
// Hypothetical: what if actions had .call()?
getScore.call(item).then(add.call(pipeline, acc))
```

This doc explores what that API surface would look like, what it composes with, and where it breaks down.

---

## Design: `.call(...args)` — variadic invocation with VarRef arguments

**Signature:**
```ts
// Single argument: action expects TIn, we supply it as a VarRef
call<TIn>(this: TypedAction<TIn, TOut>, input: VarRef<TIn>): TypedAction<any, TOut>;

// Multiple arguments: action expects tuple [A, B, ...], we supply components
call<TArgs extends Array<unknown>>(
  this: TypedAction<TArgs, TOut>,
  ...args: { [K in keyof TArgs]: VarRef<TArgs[K]> }
): TypedAction<any, TOut>;
```

**Semantics:**
- `action.call(ref)` ≡ `ref.then(action)` — single VarRef provides the input
- `action.call(a, b)` ≡ `all(a, b).then(action)` — multiple VarRefs assembled into tuple

One method handles both cases. No need for separate `.apply()`.

**Usage:**
```ts
bindInput<[number, Item]>((state) => {
  const [acc, item] = state.split();
  return getScore.call(item).then(add.call(pipeline, acc));
});
```

---

## What `.call()` compiles to

Single arg: `action.call(ref)` → `ref.then(action)`. No new AST nodes. Purely syntactic sugar.

Multiple args: `action.call(a, b)` → `all(a, b).then(action)`. Also no new AST nodes.

```ts
// These pairs are identical:
getScore.call(item)          ≡  item.then(getScore)
processItem.call(acc, item)  ≡  all(acc, item).then(processItem)
```

The difference is reading direction. `item.then(getScore)` reads as "item flows into getScore." `getScore.call(item)` reads as "call getScore with item." The second is more natural when you're thinking imperatively about "I have references, I want to invoke functions on them."

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

With `.call()`:

```ts
bindInput<[Db, Config, Query]>((state) => {
  const [db, config, query] = state.split();
  return runQuery.call(db, query).then(formatResult.call(identity(), config));
});
```

Marginal improvement. The `all(...).then(action)` pattern is what the variadic form eliminates.

### It helps in `fold` bodies

```ts
// Current
Iterator.fold(
  constant(0),
  bindInput<[number, string]>((state) => {
    const [acc, item] = state.split();
    return item.then(getLength).then(add.call(acc, pipeline));
  }),
)
```

### It doesn't help for linear pipelines

For `a.then(b).then(c)` — no arguments to pass, data flows naturally. `.call()` is irrelevant here.

### It doesn't help when you only have one VarRef

If you split and only use one component, `item.then(action)` is already clean. `.call(item)` adds nothing over `.then()` when there's a single input.

---

## Problems

### 1. `.call()` is just `.then()` backwards (for single arg)

`getScore.call(item)` ≡ `item.then(getScore)`. The only difference is which side of the dot the action sits on. Is that worth a new method? It's arguably *less* clear than `.then()` because `.then()` is already established as "chain" and `.call()` borrows meaning from `Function.prototype.call` (which passes `this`, not input).

### 2. Variadic `.call()` is just `all(...).then()`

The only thing `action.call(a, b)` saves over `all(a, b).then(action)` is not needing to write `all(...)`. It saves one function call. Is that worth the API surface?

### 3. Naming collision with Function.prototype

`call` is a method on `Function.prototype`. While TypedAction isn't a function, the name overlap may confuse users who expect JavaScript-native semantics (passing `this`, argument lists, etc.). Alternative names:

- `.invoke(ref)` / `.invoke(a, b)`
- `.with(ref)` / `.with(a, b)`
- `.using(ref)` / `.using(a, b)`

### 4. It doesn't eliminate `bindInput` + `.split()`

You still need `bindInput` to capture the pipeline value as a VarRef. You still need `.split()` to destructure it. `.call()` just changes what you do *after* splitting. The ceremony of entering the VarRef context is unchanged.

### 5. `.bind()` (partial application) is not viable

Partial application (supply some tuple args upfront, get a narrower action back) requires TypeScript to split a tuple type: given `TypedAction<[A, B, C], Out>` and bound args `[A]`, infer the remaining `[B, C]`. TypeScript doesn't support variadic tuple subtraction. You'd need explicit type parameters:

```ts
queryDb.bind<[Config], [Query]>(config)
```

Which defeats the ergonomic goal. Skip `.bind()` entirely.

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

`.call(...refs)` as a single variadic method is the cleanest API surface. It collapses `.call()` and `.apply()` into one method. For single args, it's `ref.then(action)`. For multiple args, it's `all(...refs).then(action)`. No new AST nodes, purely sugar.

The question is whether it's enough of a readability win over the existing patterns (`ref.then(action)`, `all(a, b).then(action)`) to justify adding API surface.

**Recommendation:** Implement `.call(...refs)` (or `.invoke(...)`) as a single new variadic method. Skip `.bind()` / partial application entirely — TypeScript can't infer it. Keep the current `bindInput` + `.split()` pattern as the general-purpose approach for entering VarRef context.

---

## Open Questions

1. Is `action.call(ref)` actually more readable than `ref.then(action)` in practice? Write 5 real `fold` bodies both ways and compare.
2. Better name? `.invoke()`, `.with()`, `.using()`?
3. Does the named-argument alternative (objects instead of tuples in fold/withResource) address the readability concern better without new TypedAction surface?
4. How does `.call()` interact with the pipeline value? When you write `add.call(acc, ???)`, the second arg needs to be the "current pipeline value" — but that's not a VarRef. Do we need a way to reference it?
