---
image: /img/og/repertoire-commands.png
---

# Deterministic Steps

Not every step needs an LLM. Use regular TypeScript handlers for deterministic operations: listing files, running builds, calling APIs, committing changes. Save the agent for the parts that require judgment.

## Pattern

```ts
export const listFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    return readdirSync("src", { recursive: true })
      .filter((f): f is string => typeof f === "string" && f.endsWith(".ts"))
      .map((f) => `src/${f}`);
  },
}, "listFiles");
```

## Examples

### Run a type-checker

```ts
export const typeCheck = createHandler({
  inputValidator: z.string(),
  outputValidator: z.object({ success: z.boolean(), errors: z.string() }),
  handle: async ({ value: file }) => {
    const { execSync } = await import("child_process");
    try {
      execSync(`pnpm exec tsc --noEmit ${file}`, { stdio: "pipe" });
      return { success: true, errors: "" };
    } catch (e: any) {
      return { success: false, errors: e.stdout?.toString() ?? "" };
    }
  },
}, "typeCheck");
```

### Commit changes

```ts
export const commitChanges = createHandler({
  inputValidator: z.object({ file: z.string(), message: z.string() }),
  handle: async ({ value }) => {
    const { execSync } = await import("child_process");
    execSync(`git add ${value.file} && git commit -m "${value.message}"`, { stdio: "pipe" });
  },
}, "commitChanges");
```

### Call an API

```ts
export const fetchPRComments = createHandler({
  inputValidator: z.number(),
  outputValidator: z.array(z.object({ body: z.string(), author: z.string() })),
  handle: async ({ value: prNumber }) => {
    const response = await fetch(
      `https://api.github.com/repos/owner/repo/pulls/${prNumber}/comments`,
    );
    return response.json();
  },
}, "fetchPRComments");
```

## Key points

- Deterministic handlers are fast, cheap, and reliable. No token costs, no LLM variability.
- Use agents for analysis, judgment, and creative work. Use handlers for everything else.
- Handlers run in isolated subprocesses, so they can safely use `execSync` without blocking other handlers.
