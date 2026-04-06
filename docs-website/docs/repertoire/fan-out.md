---
image: /img/og/repertoire-fan-out.png
---

# Fan-Out

Split one task into multiple parallel tasks. Each element of an array is processed independently and concurrently.

## Pattern

```ts
listFiles.forEach(processFile)
```

## Example

List all TypeScript files, then refactor each one in parallel:

```ts
export const listFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    return readdirSync("src", { recursive: true })
      .filter((f): f is string => typeof f === "string" && f.endsWith(".ts"))
      .map((f) => `src/${f}`);
  },
}, "listFiles");

export const refactorFile = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    await callClaude({
      prompt: `Refactor ${file} to use modern TypeScript patterns.`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "refactorFile");
```

```ts
await workflowBuilder()
  .workflow(() => listFiles.forEach(refactorFile).drop())
  .run();
```

## Key points

- `forEach` takes an action and applies it to each element of the input array.
- All elements are processed in parallel.
- `forEach` produces an array of results. Use `.drop()` if you don't need them.
- Each handler instance runs in its own subprocess with its own context.
