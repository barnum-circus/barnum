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

## Constraints

- Names should read naturally in imperative-style code with `const` + `.call()`
- Names should communicate intent without requiring knowledge of FP jargon
- Renaming is cheap (no users, backward compat doesn't matter)

## Open questions

- Does `sequential` pull its weight over `pipe`? `pipe` is short and universally understood, even if the metaphor is slightly off.
- Should `parallel` / `parallelObject` share a prefix, or is the relationship obvious enough?
- `.bind()` collides with `Function.prototype.bind` in mental model (even if not technically). `.let()` is Kotlin-inspired and clear. `.capture()` is explicit but verbose.
- Are there other combinators whose names feel wrong in the imperative style? (`earlyReturn`, `tryCatch`, `loop` all seem fine.)
