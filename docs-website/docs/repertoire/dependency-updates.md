# Dependency Updates

Bump dependencies across a project (or multiple projects), fix breaking API changes, verify tests pass, and open PRs — all concurrently.

## Workflow

```ts
runPipeline(
  pipe(
    listOutdatedDeps,
    forEach(
      withResource({
        create: createUpdateBranch,
        action: pipe(
          bumpDependency,
          tryCatch(
            (throwError) => pipe(
              runTests.unwrapOr(throwError).drop(),
              typeCheck.unwrapOr(throwError).drop(),
            ),
            pipe(diagnoseBreakage, applyFix),
          ),
          createPR,
        ),
        dispose: cleanupBranch,
      }),
    ),
  ).drop(),
);
```

## Stages

1. **List outdated deps** — query the package manager for available updates.
2. **For each dependency** — process concurrently in isolated branches:
   - **Bump** — update the dependency version.
   - **Test and type-check** — run the test suite and type checker. If either fails, diagnose and fix.
   - **Open PR** — create a pull request with the update.
3. **Cleanup** — `withResource` ensures the branch is cleaned up even if the update fails.

## Key points

- Each dependency update runs in its own branch via `withResource`, so updates don't interfere.
- `tryCatch` catches test/type-check failures and routes to a diagnosis step rather than aborting.
- The diagnosis agent sees only the failure output — it doesn't know about other dependency updates.
- Combine with the [babysitting PRs](./babysitting-prs.md) workflow to monitor the resulting PRs through CI and review.
