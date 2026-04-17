# Context and Variables

Pipeline data flows forward — each step receives the previous step's output. `bind` and `bindInput` make earlier values available to later steps without threading them through every intermediate handler.

## The problem

Consider a workflow that implements a refactor, type-checks, reviews, and then commits. Several of these steps need the `worktreePath` from the original input, but intermediate steps like `implement` and `typeCheckFix` produce their own outputs.

Without `bindInput`, every handler has to accept and re-emit the fields that later steps need:

```ts
// Without bindInput: manual threading
const implementAndReview =
  // implement needs worktreePath + description, but must also pass worktreePath through
  augment(pick("worktreePath", "description").then(implement))
  // typeCheckFix needs worktreePath, must also pass it through
  .then(augment(pick("worktreePath").then(typeCheckFix)))
  // judge needs the full context, must pass worktreePath through
  .then(augment(judgeRefactor.then(classifyJudgment) /* ... handle NeedsWork/Approved ... */))
  // commit needs worktreePath
  .then(pick("worktreePath").then(commit).drop())
  // createPR needs branch + description — hope they survived all that augmenting
  .then(pick("branch", "description").then(preparePRInput).then(createPR));
```

Every step wraps in `augment` to merge its output back so downstream steps can access earlier fields. The pipeline becomes a mess of `augment` and `pick` calls just to thread data through. And if any step forgets to preserve a field, later steps break silently.

## bindInput — named parameter access

`bindInput` captures the pipeline input once and makes it available by name throughout the body:

From [`demos/identify-and-address-refactors/handlers/refactor.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/identify-and-address-refactors/handlers/refactor.ts):

```ts
export const implementAndReview = bindInput<ImplementAndReviewParams>((params) =>
  params.pick("worktreePath", "description").then(implement).drop()
    .then(params.pick("worktreePath").then(typeCheckFix).drop())

    .then(loop((recur) =>
      judgeRefactor.then(classifyJudgment).branch({
        NeedsWork: applyFeedback.drop()
          .then(params.pick("worktreePath").then(typeCheckFix))
          .drop().then(recur),
        Approved: drop,
      }),
    ).drop())

    .then(params.pick("worktreePath").then(commit).drop())
    .then(params.pick("branch", "description").then(preparePRInput).then(createPR)),
);
```

`params` is a reference to the original input. `params.pick("worktreePath")` retrieves those fields at any point — even deep inside the loop's `NeedsWork` branch. No threading, no augmenting. Each step `.drop()`s its own output because downstream steps pull what they need from `params` directly.

## bind — concurrent variable capture

`bind` runs multiple actions concurrently and makes their results available as variables:

```ts
bind(
  [fetchUser, fetchPermissions, fetchConfig],
  ([user, permissions, config]) =>
    user.then(processUser)
      .then(permissions.then(checkAccess))
      .then(config.then(applySettings)),
)
```

All three fetches run in parallel. Once complete, `user`, `permissions`, and `config` are variable references — dereferencing them at any point in the body retrieves the captured value.

## How it works

`bind` compiles to `All` (for concurrent execution) followed by nested `ResumeHandle` layers (one per variable). Each variable reference is a `ResumePerform` that reads from the handler's state. See [algebraic effect handlers](../architecture/algebraic-effect-handlers.md) for the compilation details.
