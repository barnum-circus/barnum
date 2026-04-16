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

## Approach A: `Result.from(action)` — explicit wrapper

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
- No magic, no inference, no markers

Cons:
- Every Result/Option-producing action needs explicit wrapping
- Verbose: `Result.from(handler.getField("foo"))` vs just `handler.getField("foo")`
- Easy to forget (runtime error)

---

## Approach B: Globally unique kind values — eliminate `__union` entirely

Instead of `{ kind: "Ok" }` and `{ kind: "Some" }`, use globally unique kind strings: `{ kind: "ResultOk" }`, `{ kind: "OptionSome" }`.

The kind string itself identifies which union family a value belongs to. No `__union` dispatch table needed — the postfix methods can determine the family from the kind prefix at runtime, or a global registry maps kind strings to dispatch tables.

```ts
// Handler body uses runtime constructors
handle: async (): Promise<StepResult> => Result.ok("validated")
// Produces: { kind: "ResultOk", value: "validated" }

// Branch cases use prefixed kind values
stepA.branch({ ResultOk: ..., ResultErr: ... })

// Postfix methods work without annotation — kind string is self-describing
handler.getField("foo").unwrapOr(fallback)  // inspects kind at pipeline construction? No...
```

Wait — this still doesn't work at pipeline construction time. `getField("foo")` creates a GetField node. At construction time, no values exist yet. The postfix method `.unwrapOr()` needs to know *now* (at construction time) which Branch to emit, but the value's kind string only exists at runtime.

### The real version: runtime dispatch in the Rust engine

The globally unique kind approach works if dispatch happens in the **engine** rather than in the **pipeline graph**. Instead of `.unwrapOr()` emitting a `Branch({ Ok: identity, Err: defaultAction })` at construction time, it would emit a generic `UnwrapOr(defaultAction)` node, and the Rust engine resolves the branch at runtime by inspecting the value's kind string.

This is a much deeper change — it moves union semantics from the TypeScript SDK into the Rust engine.

Pros:
- Eliminates `__union` entirely — no dispatch tables, no `withUnion`, no `returns`, no `Result.from()`
- The `{ foo: Result }` problem vanishes — the engine inspects the value, not the pipeline node
- Handlers just return values with the right kind strings; everything works automatically
- Kind strings are self-documenting in JSON logs/traces

Cons:
- **Breaks existing JSON ergonomics.** `{ kind: "Ok" }` → `{ kind: "ResultOk" }`. Uglier to write and read in handler bodies, though runtime constructors (`Result.ok("validated")`) mitigate this.
- **Namespace pollution.** Every union variant must be globally unique. User-defined unions can't reuse `Ok`/`Err`/`Some`/`None`. In practice, prefixing handles this.
- **Branch cases become verbose.** `branch({ Ok: ..., Err: ... })` → `branch({ ResultOk: ..., ResultErr: ... })`. Though sugar could restore the short form.
- **Rust engine changes.** The engine currently dispatches Branch by exact kind match. It would need to understand union families, either via a registry or kind-string convention (prefix parsing).
- **`branch()` becomes ambiguous.** A bare `branch({ A: ..., B: ... })` works for any tagged union. With globally unique kinds, how does the engine know whether `{ kind: "A" }` is part of this branch's union or some other union? Answer: it doesn't need to — branch still matches on exact kind. The global uniqueness only matters for *postfix methods* that need to know the family.

### Variant: global registry instead of prefixed kind strings

Keep `{ kind: "Ok" }`, but register kind-to-family mappings:

```ts
// At module init
registerUnionKind("Ok", Result);
registerUnionKind("Err", Result);
registerUnionKind("Some", Option);
registerUnionKind("None", Option);
```

Postfix methods look up the registry. No kind string changes.

Cons:
- `Ok` / `Err` are now globally reserved — user unions can't use them
- Registry is mutable global state
- Collisions are silent bugs

### Variant: runtime introspection by the postfix method

The postfix method could defer dispatch to runtime. Instead of `.unwrapOr()` emitting a concrete `Branch` at construction time, it emits a generic node:

```ts
function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  // Don't need __union — emit a node the engine resolves at runtime
  return chain(toAction(this), {
    kind: "UnwrapOr",
    default_action: toAction(defaultAction),
  });
}
```

The Rust engine sees `UnwrapOr`, inspects the runtime value's kind (`Ok` → pass through value, `Err` → run default; `Some` → pass through, `None` → run default), and dispatches.

This adds new AST node types to the Rust engine for each union operation (`Map`, `MapErr`, `UnwrapOr`, `AndThen`, etc.) but eliminates the entire `__union` mechanism from the TypeScript side.

Pros:
- No kind string changes — keeps `{ kind: "Ok" }`
- No `__union`, no `Result.from()`, no `returns`
- Works for the `{ foo: Result }` case automatically
- Postfix methods always work — no annotation needed

Cons:
- Significant Rust engine changes — new AST variants for every union operation
- The engine now knows about Result/Option semantics, breaking the current abstraction where the engine only knows about generic Branch
- Harder to extend with user-defined unions (engine needs to know each family's kind→arm mapping)

---

## Summary

| Approach | Solves `{ foo: Result }`? | Needs annotation? | Engine changes? |
|----------|--------------------------|-------------------|-----------------|
| A. `Result.from()` | Yes (explicit) | Yes, every time | None |
| B. Globally unique kinds | Yes (automatic) | No | Yes (significant) |
| B variant: runtime introspection | Yes (automatic) | No | Yes (new AST nodes) |

### Open questions

1. Is the `__union` mechanism worth keeping if the only general solution (approach A) requires explicit annotation at every consumption site? Or does that tax justify the engine investment of approach B?
2. If we go with runtime introspection (B variant), how do user-defined unions work? The engine would need a way to register custom kind→family mappings.
3. Could runtime introspection be limited to just `unwrapOr` and `map` (the most common operations), keeping `Branch` for everything else?
