---
image: /img/og/repertoire-error-recovery.png
---

# Error Recovery

Catch failures and route to a recovery handler instead of crashing the workflow.

## Pattern

```ts
tryCatch(riskyStep, {
  catch: recoveryStep,
})
```

## Example

Attempt a refactor, and if it fails, restore from backup and log the error:

```ts
export const attemptRefactor = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    await callClaude({
      prompt: `Refactor ${file} aggressively to reduce line count by 50%.`,
      allowedTools: ["Read", "Edit"],
    });
    return file;
  },
}, "attemptRefactor");

export const verifyBuild = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    const { execSync } = await import("child_process");
    try {
      execSync("pnpm exec tsc --noEmit", { stdio: "pipe" });
    } catch {
      throw new Error(`Build failed after refactoring ${file}`);
    }
    return file;
  },
}, "verifyBuild");

export const rollback = createHandler({
  handle: async () => {
    const { execSync } = await import("child_process");
    execSync("git checkout .", { stdio: "pipe" });
    console.error("Rolled back failed refactor");
  },
}, "rollback");
```

```ts
await workflowBuilder()
  .workflow(() =>
    tryCatch(
      pipe(attemptRefactor, verifyBuild),
      { catch: rollback },
    )
  )
  .run();
```

## Key points

- `tryCatch` catches errors from the body and routes to the recovery handler.
- The recovery handler receives error information and can clean up, retry, or escalate.
- Combine with `loop` for retry patterns: `loop((recur, done) => tryCatch(pipe(work, done), { catch: pipe(fix, recur) }))`.
