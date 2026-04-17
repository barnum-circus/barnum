# Racing

`race` runs multiple actions concurrently and returns the result of the first one to finish. All other in-flight actions are cancelled.

## Basic race

```ts
race(
  queryPrimaryDB,
  queryReplicaDB,
  queryCache,
)
```

All three actions receive the same input and start concurrently. The first to complete wins — its result becomes the output. The other two are torn down (their subprocesses are orphaned and their completions are silently dropped).

## Race for redundant work

When multiple agents can produce the same result, race them:

```ts
race(
  constant("gpt-4").then(callModel),
  constant("claude").then(callModel),
)
```

## How it works

`race` compiles to `All` inside a `RestartHandle`. Each branch chains the action with a `RestartPerform`. The first to complete fires the effect, tearing down the `All` and all its siblings. See [algebraic effect handlers](../architecture/algebraic-effect-handlers.md) for the compilation details.
