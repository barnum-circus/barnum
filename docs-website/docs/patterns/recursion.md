# Recursion

`defineRecursiveFunctions` defines mutually recursive functions that can call each other. Unlike `loop` (which is O(1) tail recursion via restart), recursive functions preserve the caller's pipeline across the call, forming a call stack of frames.

## Mutual recursion: Peano arithmetic

The classic is-even / is-odd mutual recursion. Each function checks if the input is zero (base case) or subtracts one and calls the other function (recursive case).

From [`demos/peano-arithmetic/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/peano-arithmetic/run.ts):

```ts
runPipeline(
  defineRecursiveFunctions<[
    [number, boolean], // isEven: number → boolean
    [number, boolean], // isOdd:  number → boolean
  ]>(
    (isEven, isOdd) => [
      // isEven: 0 → true, n → isOdd(n - 1)
      classifyZero.branch({
        Zero: constant(true),
        NonZero: pipe(subtractOne, isOdd),
      }),
      // isOdd: 0 → false, n → isEven(n - 1)
      classifyZero.branch({
        Zero: constant(false),
        NonZero: pipe(subtractOne, isEven),
      }),
    ],
  )((isEven, _isOdd) => isEven),
  7,
);
// isEven(7) → isOdd(6) → isEven(5) → isOdd(4) → isEven(3) → isOdd(2) → isEven(1) → isOdd(0) → false
```

The type parameter `<[[number, boolean], [number, boolean]]>` is explicit because TypeScript can't infer input/output types from circular definitions.

The first callback defines the function bodies — `isEven` and `isOdd` call each other via the call tokens. The second callback receives the same tokens and returns the workflow entry point.

## Self-recursion

`defineRecursiveFunction` (singular) is sugar for a single function:

```ts
defineRecursiveFunction<number, number>(
  (factorial) =>
    classifyZero.branch({
      Zero: constant(1),
      NonZero: pipe(subtractOne, factorial, multiply),
    }),
)((factorial) => factorial)
```

## When to use recursion vs. loop

| | `loop` | `defineRecursiveFunctions` |
|---|---|---|
| Frame cost | O(1) — tears down and restarts | O(n) — preserves caller across call |
| Mutual recursion | No — single body | Yes — multiple bodies call each other |
| Non-tail calls | No — body always loops or breaks | Yes — work after recursive call returns |
| Use case | Iteration, retry | Tree traversal, mutual recursion, general recursion |

Use `loop` for anything that's a while-loop. Use `defineRecursiveFunctions` when the recursion can't be expressed as tail recursion — mutual calls, post-recursion work, or tree-shaped call graphs.

## How it works

`defineRecursiveFunctions` desugars to a ResumeHandle with a Branch-based handler. Each function call is a tagged ResumePerform. The handler dispatches to the correct function body based on the tag (`Call0`, `Call1`, ...). The caller's pipeline is preserved as a ResumePerformFrame — when the callee completes, the caller resumes where it left off.

This is the same ResumeHandle/ResumePerform substrate that `bind` uses, but where `bind`'s handler looks up a value from state, the recursive handler **executes a function body**.

See [algebraic effect handlers](../architecture/algebraic-effect-handlers.md) for the full compilation model.
