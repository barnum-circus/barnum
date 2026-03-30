# Thunk builtins: combinators accept `TypedAction | (() => TypedAction)`

## Problem

Every zero-arg builtin (`drop`, `identity`, `recur`, `done`, `merge`, `flatten`) is a generic function. TypeScript requires a function call to instantiate generics, so users must write `drop()`, `identity()`, `recur()`, etc. This is noisy — these read better as bare values like handlers do.

## Proposal

Combinators (`pipe`, `chain`, `all`, `forEach`, `loop`, `branch`) accept either a `TypedAction` or a `() => TypedAction`. If they receive a function, they call it at construction time to produce the action.

```ts
// Before
pipe(forEach(fix), drop(), recur())

// After
pipe(forEach(fix), drop, recur)
```

## Why this works for type inference

TypeScript contextually types function references against union positions. Given:

```ts
function pipe<A, B, C>(
  a: TypedAction<A, B> | (() => TypedAction<A, B>),
  b: TypedAction<B, C> | (() => TypedAction<B, C>),
): TypedAction<A, C>
```

When `b` receives `drop` (a `<T>() => TypedAction<T, never>`), TypeScript matches it against `() => TypedAction<B, C>`, infers `T = B`, and resolves `C = never`.

## Implementation

### Type alias

```ts
type ActionLike<In, Out, R extends string = never> =
  TypedAction<In, Out, R> | (() => TypedAction<In, Out, R>);
```

### Runtime resolution

```ts
function resolve<In, Out, R extends string>(
  x: ActionLike<In, Out, R>,
): TypedAction<In, Out, R> {
  return typeof x === "function" ? x() : x;
}
```

This is safe because `TypedAction` is a plain object (never callable). Handlers used to be callable but aren't since config desugaring.

### Combinator changes

Replace every `TypedAction` parameter with `ActionLike` in:
- `pipe` (all arity overloads)
- `chain`
- `all`
- `forEach`
- `loop`
- `branch` (the `Record<K, ...>` value type)

Each combinator calls `resolve()` on its arguments before constructing the AST node.

### Which builtins benefit

| Builtin        | Generic | Runtime arg | Benefits from thunk |
|---------------|---------|-------------|-------------------|
| `constant`    | `<T>`   | `value`     | No (has arg)      |
| `identity`    | `<T>`   | —           | Yes               |
| `drop`        | `<T>`   | —           | Yes               |
| `tag`         | `<T,K>` | `kind`      | No (has arg)      |
| `recur`       | `<T>`   | —           | Yes               |
| `done`        | `<T>`   | —           | Yes               |
| `merge`       | `<T>`   | —           | Yes               |
| `flatten`     | `<T>`   | —           | Yes               |
| `extractField`| `<O,F>` | `field`     | No (has arg)      |

## Risk: `branch` inference across mixed thunks

```ts
branch({
  HasErrors: pipe(extractField("errors"), forEach(fix), recur),
  Clean: done,
})
```

Both values in the record are thunks. TypeScript must unify `Out` across cases where some are `TypedAction` and others are `() => TypedAction`. This needs validation — if it doesn't work, `branch` could be excluded.

## Note: generic handlers need the step config trick

`createHandler` returns a concrete `Handler<In, Out>` — a fixed type determined by the validators. There's no way to make a handler generic (e.g., `T[] → T`), because the generic must be bound at handler creation time, before the pipeline's types flow through.

Workaround: use `createHandlerWithConfig`. The step config carries the type information that would otherwise be a generic parameter. For example, a "first element" handler that takes `T[]` and returns `T` could use a step config to carry the expected element schema, and the TypeScript type system sees the config-bearing wrapper as a function that returns a concrete `TypedAction<SpecificArray, SpecificElement>`.

This is a fundamental limitation of the handler-as-export-name architecture. A handler is a module + export name resolved at runtime. Generics would require the handler to be a TypeScript function called at config construction time, which is what `createHandlerWithConfig` provides via the curried `(config) => TypedAction` pattern.

## Alternative considered

Loop-with-closure (`loop(({ recur, done }) => body)`) solves recur/done specifically but doesn't help drop, identity, merge, flatten. The thunk approach is orthogonal and more general.
