# Babysitting PRs

Monitor open pull requests, respond to reviewer comments, fix CI failures, and keep PRs moving toward merge — without human intervention.

## Workflow

```ts
runPipeline(
  listOpenPRs.forEach(
    loop((recur, done) =>
      pipe(checkPRStatus, classifyStatus).branch({
        CIFailing: pipe(diagnoseCIFailure, applyFix, pushAndWait, recur),
        ReviewComments: pipe(addressComments, pushAndWait, recur),
        Approved: done,
        Stale: pipe(rebase, pushAndWait, recur),
      })
    )
  ),
);
```

## Stages

1. **List open PRs** — query GitHub for PRs opened by the bot or assigned to it.
2. **Check status** — for each PR, check CI status, review comments, and staleness.
3. **Classify** — route to the appropriate handler:
   - **CI failing**: diagnose the failure from logs, apply a fix, push, wait for CI to re-run.
   - **Review comments**: address each comment, push the changes.
   - **Approved**: exit the loop — the PR is ready.
   - **Stale**: rebase onto the latest base branch and re-check.
4. **Loop** — after each action, re-check the PR status and loop until approved.

## Key points

- Each PR is babysit independently and concurrently via `forEach`.
- The CI diagnosis agent only sees the failure logs — it doesn't know about review comments or the PR's history.
- `loop` + `branch` creates a state machine that handles the full lifecycle of a PR.
- Consider adding `withTimeout` around the outer loop to abandon PRs that can't be fixed within a time budget.
