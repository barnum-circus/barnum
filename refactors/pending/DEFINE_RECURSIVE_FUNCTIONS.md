# Define Recursive Functions

## Motivation

Steps were removed (ELIMINATE_STEP), but the one capability they provided beyond raw values — recursion (self-reference and mutual reference) — was never replaced. `loop` handles tail recursion with O(1) frames via RestartHandle. General recursion (non-tail calls, mutual recursion) has no combinator.

## API

One type parameter — an array of `[In, Out]` tuples, one per function. Returns a curried combinator:

```ts
defineRecursiveFunctions<[
  [number, boolean],  // isEven: number → boolean
  [number, boolean],  // isOdd: number → boolean
]>(
  (isEven, isOdd) => [
    // isEven body: can call isOdd
    classifyZero.branch({
      Zero: constant(true),
      NonZero: pipe(subtractOne, isOdd),
    }),
    // isOdd body: can call isEven
    classifyZero.branch({
      Zero: constant(false),
      NonZero: pipe(subtractOne, isEven),
    }),
  ]
)((isEven, _isOdd) => isEven)
// → TypedAction<number, boolean>
```

TypeScript can't infer the `[In, Out]` types from the circular definition (body A references B, body B references A), so they're explicit in the type parameter.

The call tokens (`isEven`, `isOdd`) are the same values in both callbacks. The first callback uses them for recursion inside function bodies. The second uses them for initial calls in the workflow body. Both execute inside the ResumeHandle's scope.

### Single-function convenience

```ts
defineRecursiveFunction<number, number>(
  (factorial) =>
    classifyZero.branch({
      Zero: constant(1),
      NonZero: pipe(subtractOne, factorial, multiply),
    }),
)((factorial) => factorial)
```

Sugar for `defineRecursiveFunctions` with a single `[In, Out]` tuple.

## Core mechanism

`bind` uses a ResumeHandle where the handler returns a captured value (state lookup). `defineRecursiveFunctions` uses a ResumeHandle where the handler **is** the function — the function body is embedded directly in the handler DAG. The function doesn't run until called (Perform fires), and the caller's pipeline is preserved across the call (resume semantics). Recursive calls fire Perform recursively within handler execution, forming a call stack of ResumePerformFrames.

## Desugaring

```ts
defineRecursiveFunctions<[...]>(
  (fnA, fnB) => [bodyA, bodyB]
)((fnA, fnB) => workflow)
```

produces:

```
Chain(
  All(Identity, Constant(null)),            // [value, null] — state is unused
  ResumeHandle(resumeHandlerId,
    body: Chain(GetIndex(0), workflow),  // extract value, run workflow body
    handler: All(                           // return [result, null]
      Chain(
        GetIndex(0),                    // payload from [payload, state]
        Branch({
          Call0: Chain(GetField("value"), bodyA),
          Call1: Chain(GetField("value"), bodyB),
        })
      ),
      Constant(null)                        // state passthrough (unused)
    )
  )
)
```

`All(Identity, Constant(null))` creates the `[value, null]` tuple that ResumeHandle expects — same pattern as `bind`'s `All(...bindings, Identity)`. State is null (unused). Body extracts the original value with `GetIndex(0)`. Handler dispatches to function bodies by tag, returns `[result, null]` — engine delivers `result` to the perform site and writes `null` to state.

### Call tokens

Each call token is `Chain(Tag("CallN"), ResumePerform(resumeHandlerId))` — a tagged ResumePerform. When `bodyA` calls `fnB` mid-pipeline: ResumePerform fires, engine walks ancestors and finds the enclosing ResumeHandle, creates a ResumePerformFrame, handler runs (dispatches to bodyB via Branch on `Call1`). `bodyA`'s pipeline is preserved — it resumes when bodyB completes.

### Frame accumulation

Tail recursion accumulates O(n) ResumePerformFrames. `loop` (RestartHandle) is O(1) for tail recursion because it tears down and restarts. `defineRecursiveFunctions` preserves the caller's pipeline across the call, so frames accumulate.

Rule of thumb: use `loop` for iteration, `defineRecursiveFunctions` for general recursion.

## Type signatures

```ts
// libs/barnum/src/recursive.ts

type FunctionDef = [input: unknown, output: unknown];

type FunctionRefs<TDefs extends FunctionDef[]> = {
  [K in keyof TDefs]: TypedAction<TDefs[K][0], TDefs[K][1]>;
};

export function defineRecursiveFunctions<TDefs extends FunctionDef[]>(
  bodies: (...fns: FunctionRefs<TDefs>) => {
    [K in keyof TDefs]: Pipeable<TDefs[K][0], TDefs[K][1]>;
  },
): <TOut>(
  body: (...fns: FunctionRefs<TDefs>) => BodyResult<TOut>,
) => TypedAction<any, TOut>;

export function defineRecursiveFunction<TIn, TOut>(
  body: (self: TypedAction<TIn, TOut>) => Pipeable<TIn, TOut>,
): (
  body: (self: TypedAction<TIn, TOut>) => BodyResult<TOut>,
) => TypedAction<any, TOut>;
```

The explicit type parameter (`TDefs`) is required because TypeScript can't infer it from circular references.

## Implementation

### File: `libs/barnum/src/recursive.ts`

New file. Follows the same pattern as `bind.ts`:

1. Allocate one `ResumeHandlerId` (shared across all functions).
2. Create call tokens: `Chain(Tag("CallN"), ResumePerform(resumeHandlerId))` for each function.
3. Invoke the bodies callback with the call tokens to get function body ASTs.
4. Build the handler Branch: `Branch({ Call0: bodyA, Call1: bodyB, ... })` with `GetField("value")` auto-unwrap.
5. Return a function that takes the workflow body callback and produces the full AST.

```ts
export function defineRecursiveFunctions<TDefs extends FunctionDef[]>(
  bodiesFn: (...fns: FunctionRefs<TDefs>) => Action[],
): <TOut>(bodyFn: (...fns: FunctionRefs<TDefs>) => BodyResult<TOut>) => TypedAction<any, TOut> {
  const resumeHandlerId = allocateResumeHandlerId();

  // Create call tokens
  const fnCount = bodiesFn.length;
  const callTokens = Array.from({ length: fnCount }, (_, i) =>
    typedAction({
      kind: "Chain",
      first: {
        kind: "Invoke",
        handler: { kind: "Builtin", builtin: { kind: "Tag", value: `Call${i}` } },
      },
      rest: { kind: "ResumePerform", resume_handler_id: resumeHandlerId },
    }),
  );

  // Get function body ASTs
  const bodyActions = bodiesFn(...(callTokens as FunctionRefs<TDefs>));

  // Build Branch cases
  const cases: Record<string, Action> = {};
  for (let i = 0; i < bodyActions.length; i++) {
    cases[`Call${i}`] = {
      kind: "Chain",
      first: {
        kind: "Invoke",
        handler: { kind: "Builtin", builtin: { kind: "GetField", value: "value" } },
      },
      rest: bodyActions[i] as Action,
    };
  }

  // Return workflow body combinator
  return <TOut>(bodyFn: (...fns: FunctionRefs<TDefs>) => BodyResult<TOut>) => {
    const userBody = bodyFn(...(callTokens as FunctionRefs<TDefs>)) as Action;

    return typedAction<any, TOut>({
      kind: "Chain",
      first: {
        kind: "All",
        actions: [
          { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Identity" } } },
          { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: null } } },
        ],
      },
      rest: {
        kind: "ResumeHandle",
        resume_handler_id: resumeHandlerId,
        body: {
          kind: "Chain",
          first: {
            kind: "Invoke",
            handler: { kind: "Builtin", builtin: { kind: "GetIndex", value: 0 } },
          },
          rest: userBody,
        },
        handler: {
          kind: "All",
          actions: [
            {
              kind: "Chain",
              first: {
                kind: "Invoke",
                handler: { kind: "Builtin", builtin: { kind: "GetIndex", value: 0 } },
              },
              rest: { kind: "Branch", cases },
            },
            { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: null } } },
          ],
        },
      },
    });
  };
}
```

### File: `libs/barnum/src/index.ts`

Add export:

```ts
export { defineRecursiveFunctions, defineRecursiveFunction } from "./recursive.js";
```

### Tests

Add to `libs/barnum/tests/`:

1. **Self-recursion**: factorial via `defineRecursiveFunction`.
2. **Mutual recursion**: isEven/isOdd via `defineRecursiveFunctions`.
3. **Non-tail call**: function that does work after the recursive call returns.
4. **AST snapshot**: verify the desugared AST structure.

### Demo: `demos/peano-arithmetic/`

Mutual recursion with `defineRecursiveFunctions`. isEven calls isOdd, isOdd calls isEven, both subtract one each step. See the pattern doc for the full code.
