# Imperative naming pass

## Motivation

With `.call()` as the primary composition primitive, pipelines now read imperatively — `const result = action.call(input)`. But many combinator names were chosen for the functional/dataflow mental model and feel off in the imperative style.

## Things to consider

| Current | Issue | Possible direction |
|---------|-------|--------------------|
| `pipe(a, b, c)` | "pipe" suggests data flowing through. In practice it's sequencing. | `sequential`? `seq`? `steps`? |
| `all(a, b, c)` | "all" is vague. It means concurrent execution. | `parallel`? `concurrent`? |
| `allObject({ k: v })` | Same problem + the "Object" suffix is a TS artifact. | `parallelObject`? `gather`? `join`? |
| postfix `.bindInput()` | "bindInput" made sense as a standalone — "bind the pipeline's input." As a postfix method on a value, it reads awkwardly: `action.bindInput((ref) => ...)`. You're not binding the "input" — you're binding the action's *output* as a ref. | `.bind()`? `.let()`? `.capture()`? |
| caching a value | Currently requires `bindInput` to avoid re-evaluation. In imperative style, you want `const cached = pipeline.cache()` — a VarRef that evaluates once regardless of how many times it's referenced. | `.cache()` method on TypedAction |

## Constraints

- Names should read naturally in imperative-style code with `const` + `.call()`
- Names should communicate intent without requiring knowledge of FP jargon
- Renaming is cheap (no users, backward compat doesn't matter)

## `.cache()` — imperative memoization

The re-evaluation problem today:

```ts
const result = expensiveComputation.call(input);
// BUG: expensiveComputation runs twice
return allObject({ summary: summarize.call(result), report: format.call(result) });
```

The fix today requires `bindInput`:

```ts
return expensiveComputation.call(input).bindInput<Out>((result) =>
  allObject({ summary: summarize.call(result), report: format.call(result) }),
);
```

With `.cache()`, it reads like normal code:

```ts
const result = expensiveComputation.call(input).cache();
// result is now a VarRef — evaluated once, referenced freely
return allObject({ summary: summarize.call(result), report: format.call(result) });
```

`.cache()` returns a `VarRef<Out>` (which is a `TypedAction<any, Out>`). Under the hood it's sugar for wrapping subsequent references in a `bindInput` scope, but the user doesn't need to think about that — they just know "this value won't re-run."

Open design question: scope. `bindInput` has an explicit scope (the callback body). `.cache()` would need to implicitly scope to the enclosing `bindInput`/`loop`/`earlyReturn` body. Is that always unambiguous?

## Open questions

- Does `sequential` pull its weight over `pipe`? `pipe` is short and universally understood, even if the metaphor is slightly off.
- Should `parallel` / `parallelObject` share a prefix, or is the relationship obvious enough?
- `.bind()` collides with `Function.prototype.bind` in mental model (even if not technically). `.let()` is Kotlin-inspired and clear. `.capture()` is explicit but verbose.
- Are there other combinators whose names feel wrong in the imperative style? (`earlyReturn`, `tryCatch`, `loop` all seem fine.)
