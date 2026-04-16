# Handler Union Dispatch

How should pipeline nodes declare that they produce Result, Option, or other union types?

## Problem

Postfix methods like `.unwrapOr()`, `.mapErr()`, `.map()` dispatch through `__union`, which is attached to TypedAction pipeline nodes at construction time. But most ways of producing a Result/Option-typed output don't set `__union`:

- `createHandler` where the handler body returns `{ kind: "Ok", value: ... }`
- `handler.getField("foo")` where `foo` is a Result
- `branch({ A: ..., B: ... })` where a case returns a Result
- `identity()` when the input happens to be a Result

The root cause: `__union` is a property of the static pipeline graph, not of the runtime values flowing through it. TypeScript knows at compile time that `getField("foo")` returns `Result<string, string>`, but that type information doesn't exist at runtime.

Two layers, two lifetimes:
- **Pipeline construction** (import time): TypedAction nodes are created, `__union` is set on nodes that know their output type (e.g., `Result.ok()`, `Option.map()`, `withTimeout`)
- **Handler execution** (runtime): Handler bodies return plain JSON (`{ kind: "Ok", value: "validated" }`), which the Rust engine serializes and passes downstream

Chain propagation (copying `__union` from the `rest` arg of `chain`) handles cases where the final step in a chain is a union-aware combinator like `Result.map()`. But it can't help when a non-union-aware combinator (`getField`, `identity`, `branch`) produces a union-typed output.

### Why `ok(value)` can't embed Result identity

In Rust, `Ok("validated")` creates a `Result<String, _>` — the type is carried by the value. In barnum, the handler body returns a JavaScript value serialized to JSON:

```json
{ "kind": "Ok", "value": "validated" }
```

Even if we provided `Result.ok("validated")` as a runtime constructor, it would produce the same JSON. There's no place in the wire format to attach dispatch info. The Rust engine doesn't know about `__union`. And `__union` lives on pipeline nodes (static graph), not on values (runtime data) — these are different phases.

### The `{ foo: Result }` problem

The problem is not specific to `createHandler`. Consider a handler that returns a struct containing a Result:

```ts
const myHandler = createHandler({
  handle: async (): Promise<{ foo: Result<string, string>; bar: number }> => {
    return { foo: { kind: "Ok", value: "hello" }, bar: 42 };
  },
});

myHandler.getField("foo")  // TypedAction<..., Result<string, string>> — __union is null
  .unwrapOr(fallback)       // RUNTIME ERROR
```

Any approach that annotates `createHandler` (like `returns: Result` or `createResultHandler`) fails here — the handler's top-level output isn't a Result. The Result appears only after `.getField("foo")`. This rules out handler-specific solutions as the general answer.

---

## Approach A: `Result.from(action)` — explicit wrapper at the consumption site

```ts
// Handler returns a bare Result
const stepA = Result.from(createHandler({
  handle: async (): Promise<StepResult> => ok("validated"),
}));
stepA.unwrapOr(done)

// Handler returns a struct containing a Result
const myHandler = createHandler({
  handle: async () => ({ foo: ok("hello"), bar: 42 }),
});
Result.from(myHandler.getField("foo")).unwrapOr(fallback)
```

Implementation: `Result.from()` takes a `Pipeable<TIn, Result<TValue, TError>>` and returns a `TypedAction` with `__union` set. Identity at the AST level — no new nodes, just attaches dispatch.

```ts
// In result.ts
from<TIn, TValue, TError>(
  action: Pipeable<TIn, Result<TValue, TError>>,
): TypedAction<TIn, Result<TValue, TError>> {
  return withUnion(typedAction(toAction(action)), "Result", resultMethods);
}
```

Pros:
- Works everywhere: handlers, getField, branch, identity, any source
- Compositional — not tied to createHandler
- Type-safe — constrains the input to actually be a `Result<T, E>`
- One mechanism, not two
- No magic, no inference, no markers

Cons:
- Every Result/Option-producing action needs explicit wrapping
- Verbose: `Result.from(handler.getField("foo"))` vs just `handler.getField("foo")`
- Easy to forget (runtime error)

## Approach B: `Result.from()` for the general case, plus `returns: Result` on createHandler for the common case

Keep `returns: Result` on createHandler as sugar for the common case (handler's top-level output is a Result). Add `Result.from()` for everything else.

```ts
// Common case: handler returns Result directly
const stepA = createHandler({
  returns: Result,
  handle: async (): Promise<StepResult> => ok("validated"),
});
stepA.unwrapOr(done)

// General case: Result extracted from a struct
const myHandler = createHandler({
  handle: async () => ({ foo: ok("hello"), bar: 42 }),
});
Result.from(myHandler.getField("foo")).unwrapOr(fallback)
```

Pros:
- Ergonomic for the common case (handler returns bare Result)
- General mechanism available for the struct case

Cons:
- Two mechanisms for the same concept
- `returns: Result` only covers the top-level-output case, creating a false sense of completeness

---

## Recommendation

**Approach B: `returns: Result` on createHandler for the common case, `Result.from()` / `Option.from()` as the general escape hatch.**

The `{ foo: Result }` case is real but uncommon. It's solvable at the consumption site — it's a tax, not a blocker:

```ts
// Extract the field and annotate
handler.getField("foo").asResult().unwrapOr(fallback)

// Or destructure with bindInput
handler.bindInput(input =>
  mapObject({ foo: Result.from(input.getField("foo")), bar: input.getField("bar") })
)
```

Most handlers return bare Result or Option as their top-level output. `returns: Result` handles that 80% case with zero ceremony. Making 100% of cases equally verbose (`Result.from(createHandler({...}))`) to maintain "one mechanism" purity isn't worth the ergonomic cost.

### What to implement

1. Keep `returns: Result` on `createHandler` / `createHandlerWithConfig` (already done)
2. Add `Result.from()` / `Option.from()` as general wrappers for non-handler sources
3. Optionally add `.asResult()` / `.asOption()` as postfix sugar for `Result.from()` — reads better in chains
