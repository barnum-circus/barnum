---
image: /img/og/repertoire-branching-refactor.png
---

# Branching Refactor

Route files to specialized refactoring agents based on what each file needs. An analyzer classifies the work, then `branch` dispatches to the right specialist.

## Pattern

```ts
pipe(
  analyze,
  branch({
    ExtractFunction: extractFunctionHandler,
    RenameVariables: renameHandler,
    SimplifyConditions: simplifyHandler,
  }),
)
```

## Example

Analyze each file, then route to a specialist:

```ts
const RefactorKind = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("ExtractFunction"), value: z.object({ file: z.string(), description: z.string() }) }),
  z.object({ kind: z.literal("RenameVariables"), value: z.object({ file: z.string(), description: z.string() }) }),
  z.object({ kind: z.literal("SimplifyConditions"), value: z.object({ file: z.string(), description: z.string() }) }),
]);

export const analyzeFile = createHandler({
  inputValidator: z.string(),
  outputValidator: RefactorKind,
  handle: async ({ value: file }) => {
    const response = await callClaude({
      prompt: `Analyze ${file} and classify the most impactful refactor as one of: ExtractFunction, RenameVariables, SimplifyConditions. Return JSON: { "kind": "...", "value": { "file": "...", "description": "..." } }`,
      allowedTools: ["Read"],
    });
    return JSON.parse(response);
  },
}, "analyzeFile");

export const extractFunction = createHandler({
  inputValidator: z.object({ file: z.string(), description: z.string() }),
  handle: async ({ value }) => {
    await callClaude({
      prompt: `Extract the function described: ${value.description}\nFile: ${value.file}`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "extractFunction");

// renameVariables and simplifyConditions follow the same pattern
```

```ts
await workflowBuilder()
  .workflow(() =>
    listFiles.forEach(
      pipe(
        analyzeFile,
        branch({
          ExtractFunction: extractFunction,
          RenameVariables: renameVariables,
          SimplifyConditions: simplifyConditions,
        }),
      )
    ).drop()
  )
  .run();
```

## Key points

- Each specialist handler has narrow, focused instructions — it only knows about its refactoring type.
- The analyzer never performs the refactor. Separation of analysis and implementation means each agent has minimal context.
- Combine with `forEach` to route files in parallel, each to its own specialist.
