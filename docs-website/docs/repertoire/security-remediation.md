# Security Remediation

Scan a codebase for security vulnerabilities, classify their severity, generate patches, and verify the fixes don't break anything.

## Workflow

```ts
runPipeline(
  runSecurityScan
    .then(forEach(classifySeverity))
    .then(forEach(
      generatePatch.then(
        tryCatch(
          (throwError) => applyPatch
            .then(runTests.unwrapOr(throwError).drop())
            .then(verifyNoRegression.unwrapOr(throwError).drop()),
          rollbackPatch,
        ),
      )
    )),
);
```

## Stages

1. **Scan** — run static analysis tools to find vulnerabilities. Output: array of findings.
2. **Classify** — for each finding, an LLM assesses severity and exploitability.
3. **For each vulnerability** (concurrently):
   - **Generate patch** — an agent writes a fix for the specific vulnerability.
   - **Apply and verify** — apply the patch, run tests, check for regressions. If anything fails, rollback.

## Key points

- The scanning step is deterministic (static analysis tools), not an LLM — use a plain TypeScript handler.
- The classification agent sees one vulnerability at a time, with focused context on that specific issue.
- `tryCatch` ensures patches that break tests are rolled back automatically.
- Combine with the [code review](./code-review.md) pattern to have separate agents review each patch for correctness and security.
