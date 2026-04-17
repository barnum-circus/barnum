---
image: /img/og/repertoire-legal-review.png
---

# Legal Review

Fan out a contract review into parallel specialist analyses, then synthesize a final recommendation.

## Pattern

```ts
all(courtCaseAnalysis, financialAnalysis, liabilityAnalysis)
  .then(synthesize)
```

## Example

```ts
export const courtCaseAnalysis = createHandler({
  inputValidator: z.string(),
  outputValidator: z.object({ category: z.literal("courtCases"), findings: z.array(z.string()) }),
  handle: async ({ value: contract }) => {
    const response = await callClaude({
      prompt: `Analyze this contract for relevant court case precedents. Return JSON: { "category": "courtCases", "findings": ["..."] }\n\n${contract}`,
    });
    return JSON.parse(response);
  },
}, "courtCaseAnalysis");

export const financialAnalysis = createHandler({
  inputValidator: z.string(),
  outputValidator: z.object({ category: z.literal("financial"), findings: z.array(z.string()) }),
  handle: async ({ value: contract }) => {
    const response = await callClaude({
      prompt: `Analyze this contract for financial risks and claims. Return JSON: { "category": "financial", "findings": ["..."] }\n\n${contract}`,
    });
    return JSON.parse(response);
  },
}, "financialAnalysis");

export const liabilityAnalysis = createHandler({
  inputValidator: z.string(),
  outputValidator: z.object({ category: z.literal("liability"), findings: z.array(z.string()) }),
  handle: async ({ value: contract }) => {
    const response = await callClaude({
      prompt: `Analyze this contract for liability exposure. Return JSON: { "category": "liability", "findings": ["..."] }\n\n${contract}`,
    });
    return JSON.parse(response);
  },
}, "liabilityAnalysis");

export const synthesize = createHandler({
  inputValidator: z.tuple([
    z.object({ category: z.literal("courtCases"), findings: z.array(z.string()) }),
    z.object({ category: z.literal("financial"), findings: z.array(z.string()) }),
    z.object({ category: z.literal("liability"), findings: z.array(z.string()) }),
  ]),
  handle: async ({ value: analyses }) => {
    await callClaude({
      prompt: `Synthesize these parallel analyses into a final recommendation:\n${JSON.stringify(analyses, null, 2)}`,
    });
  },
}, "synthesize");
```

```ts
runPipeline(
  all(courtCaseAnalysis, financialAnalysis, liabilityAnalysis)
    .then(synthesize),
);
```

## Key points

- `all` runs the three specialist analyses concurrently on the same input.
- Each specialist has narrow expertise and focused instructions.
- The synthesizer receives all three analyses as a typed tuple and combines them.
- The tuple type is checked at compile time — TypeScript verifies the synthesizer expects exactly the outputs the specialists produce.
