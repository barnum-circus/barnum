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

## Recommendation

None yet. The tradeoffs are:

| Approach | Forgettable? | Combinatorial? | Extensible? | Compile-time safe? |
|----------|-------------|----------------|-------------|-------------------|
| 1. ok() embeds | N/A — doesn't work for pipeline nodes | — | — | — |
| 2. createResultHandler | Same as forgetting to use the right function | Yes (4+ functions) | No | Yes (signature enforces) |
| 3. returns: Result | Yes (runtime error) | No | Yes | No |
| 4. Result.from(handler) | Yes (runtime error) | No | Yes | Partially (type narrows) |
| 5. Type-level overloads | Compile-time — but runtime is still broken | No | No (hard-coded) | Yes |
| 6. Validator + fallback | Less (auto when validator present) | No | Yes | No |

The fundamental tension: the "ideal" (type info embedded in the value) doesn't work because `__union` is a pipeline-node property and handler output is a runtime value. Every approach is some form of explicit annotation.

### Open questions

1. How common are handlers without output validators? If rare, approach 6 (validator inference) covers most cases automatically.
2. Should forgetting `returns` be a compile-time error? If so, we need overloads or a separate function (approaches 2 or 5).
3. Is `returns: Result` confusing because `Result` is both a type and a value? If so, a more explicit name like `Result.dispatch` might be better, though it's less readable.
4. Do we want `Result.from()` (approach 4) regardless, for non-handler cases where a generic TypedAction needs `__union` attached?
