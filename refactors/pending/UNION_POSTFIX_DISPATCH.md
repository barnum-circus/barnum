# Union Postfix Dispatch

## Problem

Postfix methods like `.map()`, `.andThen()`, `.unwrapOr()` exist on both Option and Result. Currently we disambiguate with suffixes (`.mapOption()`, `.mapResult()`). This is ugly. Ideally `.map()` just works regardless of whether the output is an Option or a Result.

## Idea: runtime union identity tag

Every TypedAction already carries phantom types (`__phantom_out`, etc.) for compile-time checking. We could add a **runtime** (non-phantom) tag that identifies which union family the output belongs to. Postfix methods check this tag and dispatch to the correct implementation.

### How constructors attach the tag

Every union constructor (`Option.some()`, `Option.none()`, `Result.ok()`, `Result.err()`) would attach a union identity tag to the TypedAction it produces:

```ts
Option.some<T>()
// Returns a TypedAction with:
//   - Phantom: TypedAction<T, Option<T>>
//   - Runtime tag: { __union: optionMethods }

Result.ok<TValue, TError>()
// Returns a TypedAction with:
//   - Phantom: TypedAction<TValue, Result<TValue, TError>>
//   - Runtime tag: { __union: resultMethods }
```

The tag is a reference to a **methods object** — a lookup table of implementations for that union family.

### The methods objects

```ts
const optionMethods = {
  map: (action) => /* Option.map desugaring */,
  andThen: (action) => /* Option.andThen desugaring */,
  unwrapOr: (action) => /* Option.unwrapOr desugaring */,
  filter: (predicate) => /* Option.filter desugaring */,
  flatten: () => /* Option.flatten desugaring */,
  isSome: () => /* ... */,
  isNone: () => /* ... */,
};

const resultMethods = {
  map: (action) => /* Result.map desugaring */,
  mapErr: (action) => /* Result.mapErr desugaring */,
  andThen: (action) => /* Result.andThen desugaring */,
  or: (fallback) => /* Result.or desugaring */,
  unwrapOr: (action) => /* Result.unwrapOr desugaring */,
  flatten: () => /* Result.flatten desugaring */,
  isOk: () => /* ... */,
  isErr: () => /* ... */,
  ok: () => /* Result → Option<TValue> */,
  err: () => /* Result → Option<TError> */,
  transpose: () => /* ... */,
};
```

### How postfix dispatch works

The generic `.map()` method on TypedAction:

```ts
function mapMethod(this: TypedAction, action: Action): TypedAction {
  const methods = this.__union;
  if (!methods?.map) {
    throw new Error("No .map() available — output is not a tagged union with map support");
  }
  return this.then(methods.map(action));
}
```

Or more concretely — the postfix method constructs a new TypedAction by chaining `this` with the desugared combinator from the methods table.

### Tag propagation

When combinators produce a union output, the tag must propagate. For example:

```ts
pipe(someAction, Result.map(transform))
// Result.map returns Result<U, E> — the output is still a Result.
// The tag must carry through.
```

Each combinator in the methods table knows its output family. `Result.map` produces a Result, so the returned TypedAction carries `__union: resultMethods`. `Result.toOption` produces an Option, so it carries `__union: optionMethods`.

Combinators that DON'T produce a union (like `Result.unwrapOr`, which extracts the raw value) would NOT attach `__union`. So calling `.map()` after `.unwrapOr()` would fail — correct behavior.

### What about `.then()` and other generic combinators?

`pipe(Result.ok(), someHandler)` — the output of `someHandler` isn't necessarily a union. The `__union` tag from `Result.ok()` shouldn't propagate through arbitrary chains.

The tag should only be set by:
1. Union constructors (`Option.some`, `Result.ok`, etc.)
2. Union combinators that preserve the family (`Result.map`, `Option.andThen`, etc.)

Generic combinators like `pipe`, `.then()`, `.branch()` do NOT propagate the tag. The tag lives on the **output** of the most recent union-aware operation.

Implementation: `typedAction()` does NOT copy `__union` from inputs. Only union-aware functions set it explicitly.

### Type-level: how does TypeScript know `.map()` is available?

Option 1: `.map()` is always available on TypedAction, but the `this` constraint restricts it:

```ts
map<TIn, TOut>(
  this: TypedAction<TIn, Option<any> | Result<any, any>>,
  action: Pipeable<???, ???>,
): TypedAction<TIn, ???>;
```

Problem: the return type depends on whether it's Option or Result. TypeScript can't branch on this in a single overload.

Option 2: Overloads.

```ts
// Option overload
map<TIn, T, U>(
  this: TypedAction<TIn, Option<T>>,
  action: Pipeable<T, U>,
): TypedAction<TIn, Option<U>>;

// Result overload
map<TIn, TValue, TOut, TError>(
  this: TypedAction<TIn, Result<TValue, TError>>,
  action: Pipeable<TValue, TOut>,
): TypedAction<TIn, Result<TOut, TError>>;
```

TypeScript picks the right overload based on the `this` type. This works.

### What needs to be a "real thing"

For this to work, each union family needs:

1. **A methods object** — maps method names to implementations. This is the runtime dispatch table.
2. **Constructors that attach the tag** — every `Option.some()`, `Result.ok()`, etc. sets `__union` on the TypedAction.
3. **The `__union` property** — non-enumerable (invisible to JSON/toEqual), like the existing postfix methods.

The methods object is also the identity — `optionMethods === optionMethods` works for identity checks if needed.

### Open questions

**Can user-defined unions get postfix methods?** If a user defines `type StatusDef = { Pending: void; Active: Data; Closed: string }` and creates constructors via `tag()`, could they also provide a methods object? This would make the system extensible beyond Option and Result. Probably not needed now, but the architecture supports it.

**Performance of overloads.** Every shared postfix method (`.map`, `.andThen`, `.unwrapOr`, `.flatten`) needs N overloads on TypedAction (one per union family). With Option + Result that's 2 overloads per method. If we add more union families, this grows. Probably fine for a small number of known union types.

**Flatten collision.** `.flatten()` already exists unconditionally for arrays. Can we add `this`-constrained overloads that fire when the output is `Option<Option<T>>` or `Result<Result<T, E>, E>`? The array overload checks `Out extends (infer TElement)[][] ? ...`. If we add Option/Result overloads, TypeScript would need to pick between them. The array overload uses a conditional type on `Out`, so it might not interfere — but this needs testing. If it doesn't work, flatten stays namespace-only.

**Collect collision.** `.collect()` doesn't exist on TypedAction today. If we add it as a postfix method for `Option<T>[]` and `Result<T, E>[]` outputs, there's no collision. The `this` constraint gates it to array-of-union outputs.
