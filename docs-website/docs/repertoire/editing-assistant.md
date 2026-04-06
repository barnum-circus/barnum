---
image: /img/og/repertoire-editing-assistant.png
---

# Editing Assistant

Run multiple independent writing analyses in parallel. Each check focuses on one dimension of quality.

## Pattern

```ts
all(checkThesis, checkLogic, checkStructure)
```

## Example

```ts
const WritingFeedback = z.object({
  category: z.string(),
  issues: z.array(z.string()),
  suggestions: z.array(z.string()),
});

export const checkThesis = createHandler({
  inputValidator: z.string(),
  outputValidator: WritingFeedback,
  handle: async ({ value: text }) => {
    const response = await callClaude({
      prompt: `Evaluate the thesis clarity of this text. Is the main argument clear and well-stated? Return JSON: { "category": "thesis", "issues": ["..."], "suggestions": ["..."] }\n\n${text}`,
    });
    return JSON.parse(response);
  },
}, "checkThesis");

export const checkLogic = createHandler({
  inputValidator: z.string(),
  outputValidator: WritingFeedback,
  handle: async ({ value: text }) => {
    const response = await callClaude({
      prompt: `Evaluate the logical rigor of this text. Are there unsupported claims, logical fallacies, or gaps in reasoning? Return JSON: { "category": "logic", "issues": ["..."], "suggestions": ["..."] }\n\n${text}`,
    });
    return JSON.parse(response);
  },
}, "checkLogic");

export const checkStructure = createHandler({
  inputValidator: z.string(),
  outputValidator: WritingFeedback,
  handle: async ({ value: text }) => {
    const response = await callClaude({
      prompt: `Evaluate the structural flow of this text. Do paragraphs connect logically? Is the organization effective? Return JSON: { "category": "structure", "issues": ["..."], "suggestions": ["..."] }\n\n${text}`,
    });
    return JSON.parse(response);
  },
}, "checkStructure");
```

```ts
await workflowBuilder()
  .workflow(() =>
    all(checkThesis, checkLogic, checkStructure)
  )
  .run();
```

## Key points

- `all` runs all three checks concurrently on the same text.
- Each reviewer has a single, focused dimension to evaluate.
- The output is a typed tuple of all three results.
- Add a synthesis step with `pipe(all(...), synthesize)` to combine feedback into a unified report.
