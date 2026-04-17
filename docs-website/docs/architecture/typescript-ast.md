# TypeScript AST

Barnum's TypeScript library is a DSL that produces a serializable AST. `listFiles.then(forEach(refactor))` builds a JSON tree describing the workflow structure. The Rust runtime receives this tree and executes it.

The DSL maintains three properties:

1. **Type safety** — `pipe(a, b)` only compiles if `a`'s output matches `b`'s input
2. **Clean serialization** — `JSON.stringify()` produces a minimal tree with no type metadata
3. **Composability** — every combinator returns the same `TypedAction` type, so they nest freely

## The nine action types

The AST is a closed algebra of nine node types:

```ts
type Action =
  | InvokeAction        // Leaf: call a handler
  | ChainAction         // Sequential: run first, pipe output to rest
  | AllAction           // Fan-out: same input to all children, collect as tuple
  | ForEachAction       // Map: apply action to each array element
  | BranchAction        // Dispatch: route on { kind, value } discriminant
  | ResumeHandleAction  // Resume-style effect handler
  | ResumePerformAction // Raise a resume-style effect
  | RestartHandleAction // Restart-style effect handler
  | RestartPerformAction; // Raise a restart-style effect
```

`Invoke` is the only leaf — it calls either a TypeScript handler (subprocess) or a builtin (inline data transform like `Identity`, `Drop`, `Tag`, `Merge`, `GetField`). Every other node is structural: it composes children into larger workflows.

## Phantom types

Enforcing `In → Out` type matching across pipeline steps without polluting the serialized JSON requires **phantom types** — type-level fields that exist for the TypeScript compiler but are never set at runtime.

```ts
type TypedAction<In, Out> = Action & {
  __phantom_in?: (input: In) => void;       // contravariant
  __phantom_out?: () => Out;                 // covariant
  __phantom_out_check?: (output: Out) => void; // contravariant
  __in?: In;                                 // covariant
};
```

These four fields enforce **invariance** on both `In` and `Out`:

- **Input invariance**: `__phantom_in` (contravariant) + `__in` (covariant) together mean `In` must match exactly. A handler expecting `{ name: string }` won't accept `{ name: string; age: number }` or `string`.
- **Output invariance**: `__phantom_out` (covariant) + `__phantom_out_check` (contravariant) together mean `Out` must match exactly.

Data crosses a serialization boundary to handlers that may run in Rust, Python, or any future language. Structural subtyping (TypeScript's default) would let extra fields through — fields the receiving handler doesn't know about. Invariance catches this at compile time.

### Why phantom fields need to be optional

Phantom fields use `?:` (optional) because they're never assigned. At runtime, a `TypedAction` is just a plain `Action` object — the phantom fields are `undefined`. The `?:` makes TypeScript treat them as present-but-optional rather than erroring on their absence.

### Non-enumerable methods

`TypedAction` also has methods (`.then()`, `.forEach()`, `.branch()`, `.drop()`, etc.) attached via `Object.defineProperties` as **non-enumerable**:

```ts
function typedAction(action: Action): TypedAction {
  Object.defineProperties(action, {
    then: { value: thenMethod, configurable: true },
    forEach: { value: forEachMethod, configurable: true },
    branch: { value: branchMethod, configurable: true },
    // ...
  });
  return action;
}
```

Non-enumerable means invisible to `JSON.stringify()`. The serialized AST contains only the structural `Action` fields — no methods, no phantom types, no handler implementations.

## How combinators build trees

Every combinator returns a `TypedAction`. They compose by nesting:

### pipe

```ts
pipe(a, b, c)
// Produces: Chain(a, Chain(b, c))
// Type: TypedAction<InOfA, OutOfC>
```

`pipe` right-folds its arguments into nested `Chain` nodes. The TypeScript overloads (up to 12 arguments) enforce that each step's output matches the next step's input.

### forEach

```ts
forEach(action)
// Produces: { kind: "ForEach", action }
// Type: TypedAction<In[], Out[]>
```

When chained after an action that outputs `string[]`, `forEach` applies the inner action to each element and collects results back into an array.

### all

```ts
all(a, b, c)
// Produces: { kind: "All", actions: [a, b, c] }
// Type: TypedAction<In, [OutA, OutB, OutC]>
```

All children receive the same input and run concurrently. The output is a tuple of their results.

### branch

```ts
branch({ Ok: handleOk, Err: handleErr })
// Produces: { kind: "Branch", cases: { Ok: Chain(GetField("value"), handleOk), ... } }
// Type: TypedAction<TaggedUnion<{ Ok: TOk; Err: TErr }>, OutOk | OutErr>
```

Branch dispatches on the `kind` field of a tagged union. Each case handler receives the unwrapped `value` — the `GetField("value")` is inserted automatically. This auto-unwrapping means case handlers work with payloads directly, not the full `{ kind, value }` wrapper.

### loop, tryCatch, earlyReturn

These desugar into `RestartHandle` + `RestartPerform` + `Branch`. See [algebraic effect handlers](./algebraic-effect-handlers.md) for the compilation.

## Tagged unions

Barnum uses a `{ kind, value }` convention for discriminated unions:

```ts
type TaggedUnion<TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: {
    kind: K;
    value: TDef[K];
    __def?: TDef;  // phantom: carries the full variant map
  };
}[keyof TDef & string];
```

`__def` carries the full variant map (`{ Ok: string; Err: number }`) as a phantom field, so `.branch()` can decompose the union via `keyof ExtractDef<Out>` instead of conditional types. Never set at runtime.

Standard library types build on this:

```ts
type Option<T> = TaggedUnion<{ Some: T; None: void }>;
type Result<TValue, TError> = TaggedUnion<{ Ok: TValue; Err: TError }>;
```

## The PipeIn escape hatch

Handlers that ignore their input (like `constant(42)`) have input type `never`. But `never` is the bottom type — nothing is assignable to it, so `pipe(something, constant(42))` would fail.

The fix:

```ts
type PipeIn<T> = [T] extends [never] ? any : T;
```

When `In` is `never`, `PipeIn` widens it to `any`, letting the action sit anywhere in a pipeline. The `[T] extends [never]` syntax (tuple form) prevents TypeScript from distributing over union members.

## CaseHandler: relaxed variance for branch cases

Branch case handlers use a separate type with **contravariant-only input** and **covariant-only output**:

```ts
type CaseHandler<TIn, TOut> = Action & {
  __phantom_in?: (input: TIn) => void;  // contravariant only
  __phantom_out?: () => TOut;            // covariant only
};
```

This is intentionally less strict than `TypedAction`'s invariance:

- **Contravariant input**: A handler accepting `unknown` (like `drop`) can handle any variant payload. `(input: unknown) => void` is assignable to `(input: SpecificType) => void`.
- **Covariant output**: Branch case outputs are inferred from the actual handlers, not constrained. This lets `TypedAction<TError, never>` (a throw token) be assignable to `CaseHandler<TError, TValue>`.

## createHandler: from Zod to AST

`createHandler` bridges the runtime world (Zod validators, async functions) and the AST world (serializable JSON):

```ts
const handler = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.string(),
  handle: async ({ value }) => { /* ... */ },
}, "myHandler");
```

Internally:

1. **Detect caller file** via V8's `Error.prepareStackTrace` — the handler's module path is captured from the call stack, not passed explicitly.
2. **Compile Zod → JSON Schema** via `zodToCheckedJsonSchema()` (see [validation](./validation.md)).
3. **Build AST node**: `{ kind: "Invoke", handler: { kind: "TypeScript", module, func, input_schema, output_schema } }`.
4. **Attach non-enumerable metadata**: `__definition` (the Zod validators and handle function) and `HANDLER_BRAND` are set as non-enumerable properties. The worker subprocess reads `__definition` to find and execute the handler. `JSON.stringify` never sees them.

## Serialization example

```ts
const workflow = listFiles
  .then(forEach(refactor.then(typeCheck).then(fix)))
  .drop();
```

Serializes to:

```json
{
  "kind": "Chain",
  "first": {
    "kind": "Chain",
    "first": { "kind": "Invoke", "handler": { "kind": "TypeScript", "module": "./steps.ts", "func": "listFiles" } },
    "rest": {
      "kind": "ForEach",
      "action": {
        "kind": "Chain",
        "first": { "kind": "Invoke", "handler": { "kind": "TypeScript", "module": "./steps.ts", "func": "refactor" } },
        "rest": {
          "kind": "Chain",
          "first": { "kind": "Invoke", "handler": { "kind": "TypeScript", "module": "./steps.ts", "func": "typeCheck" } },
          "rest": { "kind": "Invoke", "handler": { "kind": "TypeScript", "module": "./steps.ts", "func": "fix" } }
        }
      }
    }
  },
  "rest": { "kind": "Invoke", "handler": { "kind": "Builtin", "builtin": { "kind": "Drop" } } }
}
```

No types, no methods, no phantom fields. Just structure. The Rust runtime deserializes this into `Action` variants via serde and executes it.
