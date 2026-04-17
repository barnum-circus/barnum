---
image: /img/og/repertoire-adversarial-review.png
---

# Adversarial Review

Implement a change, have a separate agent judge it, and loop until the judge approves. The implementing agent never sees the judge's full criteria, and the judge never sees the implementation instructions — each has focused context.

## Pattern

```ts
loop((recur, done) =>
  implement.then(judge).branch({
    Approved: done,
    NeedsWork: revise.then(recur),
  })
)
```

## Example

Refactor a file, then loop until a reviewer approves:

```ts
const JudgeResult = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("Approved"), value: z.void() }),
  z.object({ kind: z.literal("NeedsWork"), value: z.object({ feedback: z.string() }) }),
]);

export const implement = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    await callClaude({
      prompt: `Refactor ${file} to improve readability and reduce complexity.`,
      allowedTools: ["Read", "Edit"],
    });
    return file;
  },
}, "implement");

export const judge = createHandler({
  inputValidator: z.string(),
  outputValidator: JudgeResult,
  handle: async ({ value: file }) => {
    const response = await callClaude({
      prompt: `Review the recent changes to ${file} (git diff HEAD~1). Return JSON: { "kind": "Approved", "value": null } or { "kind": "NeedsWork", "value": { "feedback": "..." } }`,
      allowedTools: ["Bash"],
    });
    return JSON.parse(response);
  },
}, "judge");

export const revise = createHandler({
  inputValidator: z.object({ feedback: z.string() }),
  outputValidator: z.string(),
  handle: async ({ value }) => {
    await callClaude({
      prompt: `Address this feedback: ${value.feedback}`,
      allowedTools: ["Read", "Edit"],
    });
    return value.feedback; // pass through for next iteration
  },
}, "revise");
```

```ts
runPipeline(
  implement.then(
    loop((recur, done) =>
      judge.branch({
        Approved: done,
        NeedsWork: revise.then(implement).then(recur),
      })
    ),
  ),
);
```

## Key points

- The implementing agent doesn't know a judge exists. It just receives instructions.
- The judge doesn't know the implementation instructions. It just reviews the diff.
- `loop` + `branch` creates a type-safe retry loop with explicit termination conditions.
- Be careful with unbounded loops — consider adding `withTimeout` or a maximum iteration count.
