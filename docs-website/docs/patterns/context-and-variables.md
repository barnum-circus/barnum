# Context and Variables

Pipeline data flows forward — each step receives the previous step's output. `bind` and `bindInput` make earlier values available to later steps without threading them through every intermediate handler.

## The problem

Say you need both a user and a config in step 3, but step 2 only needs the user:

```ts
// Doesn't work: config is lost after getUser
pipe(
  getConfig,    // output: Config
  getUser,      // output: User (Config is gone)
  processUser,  // needs both User and Config — but Config was lost
)
```

## bindInput — named parameter access

`bindInput` captures the pipeline input and makes it available as a named parameter throughout the body.

From [`demos/identify-and-address-refactors/handlers/refactor.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/identify-and-address-refactors/handlers/refactor.ts):

```ts
export const implementAndReview = bindInput<ImplementAndReviewParams>((params) => pipe(
  params.pick("worktreePath", "description").then(implement).drop(),
  params.pick("worktreePath").then(typeCheckFix).drop(),

  loop((recur) =>
    pipe(judgeRefactor, classifyJudgment).branch({
      NeedsWork: pipe(
        applyFeedback.drop(),
        params.pick("worktreePath").then(typeCheckFix),
      ).drop().then(recur),
      Approved: drop,
    }),
  ).drop(),

  params.pick("worktreePath").then(commit).drop(),
  pipe(params.pick("branch", "description"), preparePRInput, createPR),
));
```

`params` is a handle to the original input. `params.pick("worktreePath", "description")` extracts those fields at any point in the pipeline — even deep inside a loop. Without `bindInput`, you'd have to thread `worktreePath` through every intermediate step.

## bind — concurrent variable capture

`bind` runs multiple actions concurrently and makes their results available as variables:

```ts
bind(
  [fetchUser, fetchPermissions, fetchConfig],
  ([user, permissions, config]) =>
    pipe(
      user.then(processUser),
      permissions.then(checkAccess),
      config.then(applySettings),
    ),
)
```

All three fetches run in parallel. Once complete, `user`, `permissions`, and `config` are variable references — calling them at any point in the body retrieves the captured value.

## How it works

`bind` compiles to `All` (for concurrent execution) followed by nested `ResumeHandle` layers (one per variable). Each variable reference is a `ResumePerform` that reads from the handler's state. See [algebraic effect handlers](../architecture/algebraic-effect-handlers.md) for the compilation details.
