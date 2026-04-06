# Patterns

Patterns are the technical building blocks of Barnum workflows. Each pattern isolates a single combinator concept with code pulled directly from the [demos](https://github.com/barnum-circus/barnum/tree/master/demos).

| Pattern | Combinator | What it does |
|---|---|---|
| [Serial execution](./serial-execution.md) | `pipe`, `.then()` | Chain steps sequentially |
| [Parallel execution](./parallel-execution.md) | `all`, `forEach` | Run work concurrently, collect results |
| [Branching](./branching.md) | `branch` | Route on tagged unions |
| [Looping](./looping.md) | `loop` | Retry until a condition is met |
| [Error handling](./error-handling.md) | `tryCatch` | Catch failures, route to recovery |
| [Timeout](./timeout.md) | `withTimeout` | Race a handler against a timer |
| [Racing](./racing.md) | `race` | First to finish wins |
| [Context and variables](./context-and-variables.md) | `bind`, `bindInput` | Share data across pipeline steps |
| [Early return](./early-return.md) | `earlyReturn` | Exit a scope before it finishes |

For real-world workflows that combine these patterns, see the [Repertoire](../repertoire/index.md).
