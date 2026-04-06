---
image: /img/og/repertoire-fan-out-finally.png
---

# Fan-Out with Aggregation

Run work in parallel, then follow up with a single step after everything completes. In Barnum, this is just a `pipe` — the step after `forEach` waits for all parallel work to finish.

## Pattern

```ts
pipe(
  listFiles.forEach(convertFile).drop(),
  fixTypeErrors,
)
```

## Example

Convert JavaScript files to TypeScript in parallel, then fix any type errors across the whole project:

```ts
export const listJsFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    return readdirSync("src", { recursive: true })
      .filter((f): f is string => typeof f === "string" && f.endsWith(".js"))
      .map((f) => `src/${f}`);
  },
}, "listJsFiles");

export const convertFile = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    await callClaude({
      prompt: `Convert ${file} from JavaScript to TypeScript. Add type annotations. Rename to .ts.`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "convertFile");

export const fixTypeErrors = createHandler({
  handle: async () => {
    await callClaude({
      prompt: "Run tsc --noEmit and fix all type errors across the project.",
      allowedTools: ["Read", "Edit", "Bash"],
    });
  },
}, "fixTypeErrors");
```

```ts
await workflowBuilder()
  .workflow(() =>
    pipe(
      listJsFiles.forEach(convertFile).drop(),
      fixTypeErrors,
    )
  )
  .run();
```

## Key points

- `forEach(convertFile)` processes all files in parallel and returns an array of results.
- `.drop()` discards the array (since `convertFile` returns void).
- `fixTypeErrors` runs only after all parallel conversions complete.
- This pattern replaces the old `finally` hook — in Barnum, sequencing after parallel work is just `pipe`.
