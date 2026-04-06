---
image: /img/og/repertoire-linear-pipeline.png
---

# Linear Pipeline

Process data through a sequence of steps, where each step's output becomes the next step's input.

## Pattern

```ts
pipe(analyze, review, implement)
```

## Example

Analyze a file for refactoring opportunities, review the suggestions, then implement the chosen refactor:

```ts
import { createHandler } from "@barnum/barnum";
import { z } from "zod";

const RefactorSuggestion = z.object({
  description: z.string(),
  file: z.string(),
});

export const analyze = createHandler({
  inputValidator: z.string(),
  outputValidator: RefactorSuggestion,
  handle: async ({ value: file }) => {
    const response = await callClaude({
      prompt: `Analyze ${file} for one refactoring opportunity. Return JSON: { "description": "...", "file": "..." }`,
      allowedTools: ["Read"],
    });
    return JSON.parse(response);
  },
}, "analyze");

export const review = createHandler({
  inputValidator: RefactorSuggestion,
  outputValidator: RefactorSuggestion,
  handle: async ({ value }) => {
    const response = await callClaude({
      prompt: `Review this refactor suggestion and refine it:\n${JSON.stringify(value)}`,
    });
    return JSON.parse(response);
  },
}, "review");

export const implement = createHandler({
  inputValidator: RefactorSuggestion,
  handle: async ({ value }) => {
    await callClaude({
      prompt: `Implement this refactor: ${value.description}\nFile: ${value.file}`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "implement");
```

```ts
// run.ts
import { runPipeline, pipe } from "@barnum/barnum";
import { analyze, review, implement } from "./handlers/steps.js";

runPipeline(
  pipe(analyze, review, implement),
);
```

## Key points

- Each handler only sees its immediate input — not the full pipeline.
- Zod validators enforce the contract between steps. If `analyze` returns something that doesn't match `RefactorSuggestion`, the workflow fails fast.
- `pipe` is type-safe: TypeScript verifies that each step's output matches the next step's input.
