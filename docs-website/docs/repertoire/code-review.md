---
image: /img/og/repertoire-code-review.png
---

# Code Review

Review changed files in parallel with multiple checks per file: coding standards, security, performance. Each check runs independently with focused instructions.

## Pattern

```ts
listChangedFiles
  .then(iterate().map(
    all(checkStandards, checkSecurity, checkPerformance)
  ).collect())
```

## Example

```ts
export const listChangedFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    const { execSync } = await import("child_process");
    const output = execSync("git diff --name-only HEAD~1", { encoding: "utf-8" });
    return output.trim().split("\n").filter(Boolean);
  },
}, "listChangedFiles");

export const checkStandards = createHandler({
  inputValidator: z.string(),
  outputValidator: z.object({ file: z.string(), issues: z.array(z.string()) }),
  handle: async ({ value: file }) => {
    const response = await callClaude({
      prompt: `Review ${file} for coding standards violations: naming conventions, documentation, error handling. Return JSON: { "file": "...", "issues": ["..."] }`,
      allowedTools: ["Read"],
    });
    return JSON.parse(response);
  },
}, "checkStandards");

export const checkSecurity = createHandler({
  inputValidator: z.string(),
  outputValidator: z.object({ file: z.string(), issues: z.array(z.string()) }),
  handle: async ({ value: file }) => {
    const response = await callClaude({
      prompt: `Review ${file} for security issues: injection, XSS, secrets, unsafe patterns. Return JSON: { "file": "...", "issues": ["..."] }`,
      allowedTools: ["Read"],
    });
    return JSON.parse(response);
  },
}, "checkSecurity");
```

```ts
runPipeline(
  listChangedFiles
    .then(iterate().map(
      all(checkStandards, checkSecurity),
    ).collect()),
);
```

## Key points

- `all` runs multiple checks on the same input concurrently. Each check is independent.
- `.iterate().map()` + `all` gives you parallelism at two levels: across files and across check types.
- Each reviewer has narrow, focused instructions. The security reviewer doesn't see the standards criteria and vice versa.
- Add more checks by adding more arguments to `all` — no structural changes needed.
