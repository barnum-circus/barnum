# Handler Union Dispatch

How should handlers declare that they produce Result, Option, or other union types?

## Problem

When a handler returns `Result<string, string>`, the user expects to call `.unwrapOr()`, `.mapErr()`, etc. on it. These postfix methods dispatch through `__union`, which is attached to TypedAction pipeline nodes at construction time. But `createHandler` produces an Invoke node with `__union: null` — it doesn't know the handler's output is a Result.

Two layers, two lifetimes:
- **Pipeline construction** (import time): TypedAction nodes are created, `__union` is set on nodes that know their output type (e.g., `Result.ok()`, `Option.map()`, `withTimeout`)
- **Handler execution** (runtime): Handler bodies return plain JSON (`{ kind: "Ok", value: "validated" }`), which the Rust engine serializes and passes downstream

The pipeline is static. The values are dynamic. `__union` is a property of the static graph. The handler body has no way to communicate back to the pipeline node.

## Current implementation (baseline)

```ts
// handler.ts — createHandler accepts optional `returns`
export const stepA = createHandler({
  returns: Result,              // ← sets __union on the Invoke node
  outputValidator: StepResultValidator,
  handle: async (): Promise<StepResult> => {
    return { kind: "Ok", value: "validated" };
  },
}, "stepA");

stepA.unwrapOr(done)  // works — __union is set
```

`Result.dispatch` and `Option.dispatch` are `UnionDispatch` objects (`{ name, methods }`) on the namespace objects. `createHandler` passes these to `withUnion` when `returns` is present.

This works, but it's a manual annotation that's easy to forget. The failure mode — a runtime error when calling a postfix method — is poor. The rest of this doc explores whether we can do better.

---

## Approach 1: `ok(value)` embeds Result identity in the value

The user's ideal. In Rust, `Ok("validated")` creates a `Result<String, _>` — the type is carried by the value. Can we do the same?

### Why it doesn't work (directly)

The handler body returns a JavaScript value. That value is serialized to JSON and sent to the Rust engine via stdout. The engine knows nothing about `__union`. The JSON is:

```json
{ "kind": "Ok", "value": "validated" }
```

Even if we provided `Result.ok("validated")` as a runtime function (not a pipeline node), it would still produce the same JSON. There's no place in the wire format to attach dispatch info, and the Rust engine wouldn't know what to do with it.

The deeper issue: `__union` is a property of TypedAction *pipeline nodes*, not of the *values* flowing through the pipeline. `Result.ok()` already exists as a pipeline node constructor — but in the handler body, we're producing runtime values, not pipeline nodes.

### Variant: marker in the return value

We could add a marker to handler return values:

```ts
// Handler body returns a marked value
handle: async () => Result.create.ok("validated")
// Produces: { kind: "Ok", value: "validated", __resultMarker: true }
```

The worker would detect the marker and tell the Invoke node to set `__union`. But:
- Pollutes the wire format (Rust engine sees unknown fields)
- Requires worker-level plumbing to propagate a marker back to pipeline node metadata
- `__union` is set at pipeline construction time, but this marker appears at execution time — these are different phases
- Doesn't help with TypeScript type inference (the `handle` return type doesn't affect the TypedAction's phantom types)

### Variant: output validator inference

If the handler uses `Result.schema(...)` as its output validator, we could detect that and auto-set `__union`:

```ts
export const stepA = createHandler({
  outputValidator: Result.schema(z.string(), z.string()),  // ← detected as Result
  handle: async () => ({ kind: "Ok", value: "validated" }),
}, "stepA");
```

`Result.schema()` could mark its return value (e.g., a symbol property). `createHandler` checks for the mark and sets `__union` automatically.

Pros:
- Zero additional API when validators are present
- The validator already *is* the declaration of the output type

Cons:
- Not all handlers have output validators — handlers without validators still need another mechanism
- Creates coupling between validation and dispatch (one concept serving two purposes)
- The marker on the schema object is invisible magic

**Verdict**: interesting as an enhancement, not sufficient as the primary mechanism.

---

## Approach 2: `createResultHandler` / `createOptionHandler`

Separate factory functions for each union family.

```ts
export const stepA = createResultHandler({
  outputValidator: StepResultValidator,
  handle: async (): Promise<StepResult> => {
    return { kind: "Ok", value: "validated" };
  },
}, "stepA");
```

Internally, `createResultHandler` calls `createHandler` + `withUnion(action, "Result", resultMethods)`.

Pros:
- Crystal clear at the call site
- No new concepts — just a function name
- Type signature can enforce `TOutput extends Result<any, any>`

Cons:
- Combinatorial explosion: `createResultHandler`, `createOptionHandler`, `createResultHandlerWithConfig`, `createOptionHandlerWithConfig` = 4 new functions (potentially more for custom unions)
- Naming is verbose
- Not extensible to user-defined unions without writing more factory functions

---

## Approach 3: `createHandler({ returns: Result })` (current)

A `returns` field on the definition object.

```ts
export const stepA = createHandler({
  returns: Result,
  outputValidator: StepResultValidator,
  handle: async (): Promise<StepResult> => { ... },
}, "stepA");
```

Pros:
- One function, keeps the existing API
- Extensible to any union (including future user-defined ones)
- Reads naturally: "this handler returns a Result"

Cons:
- `returns: Result` looks like a type annotation but is a runtime value
- Forgetting it produces a runtime error, not a compile error
- `Result` (the value) vs `Result` (the type) can be confusing — they're the same name but different things

### Variation: `kind: Result` or `outputFamily: Result`

Same mechanism, different field name. `kind` is overloaded (already means the tagged union discriminant). `outputFamily` or `outputType` is more precise but verbose.

---

## Approach 4: Wrapper combinator — `Result.from(handler)`

A post-hoc wrapper that attaches `__union` to any TypedAction.

```ts
const stepA = Result.from(createHandler({
  outputValidator: StepResultValidator,
  handle: async (): Promise<StepResult> => { ... },
}, "stepA"));
```

Or as a method: `createHandler({...}).asResult()`.

Pros:
- Compositional — works on any TypedAction, not just handlers
- Clean separation: createHandler creates the node, Result.from decorates it
- Could type-narrow: `Result.from<TValue, TError>(action: TypedAction<TIn, Result<TValue, TError>>)`

Cons:
- Extra wrapping step that's easy to forget (same failure mode as approach 3)
- `Result.from(createHandler({...}))` reads inside-out — the return type wraps the factory
- Doesn't compose well with `createHandlerWithConfig` (the factory returns a function, so where does the wrapper go?)

---

## Approach 5: Type-level auto-detection via overloads

Make `createHandler` automatically set `__union` when `TOutput` is `Result<any, any>` or `Option<any>`:

```ts
// Overload 1: output is Result → auto-sets __union
function createHandler<TValue, TValue2, TError>(
  def: { handle: (...) => Promise<Result<TValue2, TError>>; ... }
): Handler<TValue, Result<TValue2, TError>>;

// Overload 2: output is Option → auto-sets __union
function createHandler<TValue, TInner>(
  def: { handle: (...) => Promise<Option<TInner>>; ... }
): Handler<TValue, Option<TInner>>;

// Overload 3: anything else → no __union
function createHandler<TValue, TOutput>(
  def: { handle: (...) => Promise<TOutput>; ... }
): Handler<TValue, TOutput>;
```

At runtime, the implementation can't know which overload was selected. TypeScript overloads are compile-time only. So we'd still need a runtime mechanism — either inference from the output validator, or a marker. This approach only solves the type-level problem, not the runtime dispatch problem.

**Verdict**: doesn't solve the actual problem (setting `__union` at runtime) without combining with another approach.

---

## Approach 6: Combine validator inference with explicit fallback

Use validator inference as the primary mechanism, with `returns` as the explicit fallback:

```ts
// Auto-detected from Result.schema()
export const stepA = createHandler({
  outputValidator: Result.schema(z.string(), z.string()),
  handle: async (): Promise<StepResult> => { ... },
}, "stepA");

// Explicit fallback when no validator
export const stepB = createHandler({
  returns: Result,
  handle: async (): Promise<StepResult> => { ... },
}, "stepB");
```

`Result.schema()` and `Option.schema()` would attach a hidden `[UNION_DISPATCH]` symbol property to the Zod schema. `createHandler` checks the output validator for this symbol first, then falls back to `returns`.

Pros:
- Handlers with validators get dispatch for free
- Explicit fallback for handlers without validators
- Gradual — existing handlers with validators start working without changes

Cons:
- Two mechanisms doing the same thing
- If both `outputValidator` and `returns` are present, which wins? (Easy: they should agree; warn if they conflict)
- Symbol on Zod schema is invisible magic

---

## The real problem: `{ foo: Result, bar: string }`

All of the above approaches assume the handler's *top-level* output is a Result or Option. But what if the handler returns a struct that *contains* a Result?

```ts
const myHandler = createHandler({
  handle: async (): Promise<{ foo: Result<string, string>; bar: number }> => {
    return { foo: { kind: "Ok", value: "hello" }, bar: 42 };
  },
});

// User extracts the Result field
myHandler.getField("foo")  // TypedAction<..., Result<string, string>> — but __union is null
  .unwrapOr(fallback)       // RUNTIME ERROR
```

This kills approaches 2, 3, 5, and 6. The handler's output isn't a Result — it has a Result *inside* it. `returns: Result` is wrong, `createResultHandler` is wrong, type-level overloads can't match, validator inference sees an object schema.

The problem isn't about `createHandler` at all. It's about **any TypedAction whose output happens to be a Result or Option, regardless of how it got there**. Sources of union-typed outputs that lack `__union`:

- `handler.getField("foo")` where `foo` is a Result
- `handler.getIndex(0)` where the element is an Option (getIndex already wraps in Option, but other indexing patterns might not)
- `branch({ A: ..., B: ... })` where a case returns a Result
- `identity()` when the input is a Result
- Any generic combinator that passes through or restructures data

Chain propagation (copying `__union` from the `rest` arg of `chain`) handles cases where the final step in a chain is a union-aware combinator like `Result.map()`. But it can't help when a non-union-aware combinator (`getField`, `identity`, `branch`) produces a union-typed output.

### The only general answer: explicit annotation at the consumption site

There is no way to automatically bridge TypeScript compile-time type knowledge into runtime `__union` dispatch. The compiler knows `getField("foo")` returns `Result<string, string>`, but that information doesn't exist at runtime.

This means the annotation must happen wherever a Result/Option-typed value *enters the pipeline from a non-union-aware source*. The question is what that annotation looks like.

---

## Revised approaches

Given the `{ foo: Result }` problem, only two patterns survive:

### A. `Result.from(action)` — explicit wrapper at the consumption site

```ts
// Handler returns a struct containing a Result
const myHandler = createHandler({ handle: async () => ({ foo: ok("hello"), bar: 42 }) });

// User wraps after extracting the Result field
Result.from(myHandler.getField("foo")).unwrapOr(fallback)
```

```ts
// Handler returns a bare Result — same pattern
const stepA = createHandler({ handle: async () => ok("validated") });
Result.from(stepA).unwrapOr(done)
```

Implementation: `Result.from()` takes a `Pipeable<TIn, Result<TValue, TError>>` and returns a `TypedAction` with `__union` set. It's identity at the AST level — no new nodes, just attaches dispatch.

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

### B. `createHandler({ returns: Result })` — for the common case, plus `Result.from()` for everything else

Keep `returns: Result` on createHandler for the common case (handler's top-level output is a Result). Add `Result.from()` for the general case (any TypedAction that produces a Result).

```ts
// Common case: handler returns Result directly
const stepA = createHandler({
  returns: Result,
  handle: async (): Promise<StepResult> => ok("validated"),
});
stepA.unwrapOr(done)  // works

// General case: Result extracted from a struct
const myHandler = createHandler({ handle: async () => ({ foo: ok("hello"), bar: 42 }) });
Result.from(myHandler.getField("foo")).unwrapOr(fallback)
```

Pros:
- Ergonomic for the common case (handler returns bare Result)
- General mechanism available for the struct case
- Doesn't break existing code

Cons:
- Two mechanisms for the same concept
- `returns: Result` only covers the top-level-output case, creating a false sense of completeness

---

## Recommendation

**Approach A: `Result.from()` / `Option.from()` as the sole mechanism.** Remove `returns` from createHandler.

Rationale:
- One mechanism, not two. It works for handlers returning bare Results, handlers returning structs containing Results, getField, branch, and any other source.
- `Result.from(stepA)` is barely more verbose than `returns: Result` and is much more honest — it tells you exactly where the annotation happens instead of hiding it inside createHandler.
- Having `returns` on createHandler creates a false expectation that handlers "just work" with postfix methods. They do for the top-level case, then break for the struct case. Better to have one consistent pattern.
- `Result.from()` / `Option.from()` are useful independently of handlers — they solve the general problem.

Usage:
```ts
// Handler returning bare Result
const stepA = Result.from(createHandler({
  handle: async (): Promise<StepResult> => ok("validated"),
}));
stepA.unwrapOr(done)

// Handler returning struct with Result field
const myHandler = createHandler({
  handle: async () => ({ foo: ok("hello"), bar: 42 }),
});
Result.from(myHandler.getField("foo")).unwrapOr(fallback)

// withTimeout already sets __union internally — no wrapping needed
withTimeout(constant(5000), body).unwrapOr(fallback)
```

### Open question

Is `Result.from(createHandler({...}))` too much ceremony for the common case? If most handlers return bare Result/Option, the wrapping is annoying boilerplate. If that's the case, keeping `returns` as sugar for the common case (approach B) is justified — just document clearly that it only covers the top-level output.
