---
image: /img/og/repertoire-branching.png
---

# Branching

Route to different actions based on a tagged union output. An analyzer handler classifies the input, then `branch` dispatches to the appropriate handler.

## Pattern

```ts
pipe(
  classify,
  branch({
    NeedsWork: fix,
    LooksGood: drop,
  }),
)
```

## Example

Review a PR and either approve it or request changes:

```ts
const ReviewDecision = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("Approve"), value: z.object({ summary: z.string() }) }),
  z.object({ kind: z.literal("RequestChanges"), value: z.object({ feedback: z.string() }) }),
]);

export const reviewPR = createHandler({
  inputValidator: z.string(),
  outputValidator: ReviewDecision,
  handle: async ({ value: prUrl }) => {
    const response = await callClaude({
      prompt: `Review the PR at ${prUrl}. Return JSON: either { "kind": "Approve", "value": { "summary": "..." } } or { "kind": "RequestChanges", "value": { "feedback": "..." } }`,
      allowedTools: ["Bash"],
    });
    return JSON.parse(response);
  },
}, "reviewPR");

export const applyFeedback = createHandler({
  inputValidator: z.object({ feedback: z.string() }),
  handle: async ({ value }) => {
    await callClaude({
      prompt: `Address this PR feedback: ${value.feedback}`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "applyFeedback");
```

```ts
runPipeline(
  pipe(
    reviewPR,
    branch({
      Approve: drop,
      RequestChanges: applyFeedback,
    }),
  ),
);
```

## Key points

- The classifier handler must return a tagged union: `{ kind: string, value: T }`.
- `branch` auto-unwraps — each branch handler receives the `value`, not the full tagged union.
- TypeScript enforces exhaustive matching: every variant must have a corresponding branch.
