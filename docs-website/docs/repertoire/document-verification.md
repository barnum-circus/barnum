---
image: /img/og/repertoire-document-verification.png
---

# Document Verification

Extract verifiable claims from a document, then fact-check each one independently in parallel.

## Pattern

```ts
pipe(extractClaims, forEach(verifyClaim))
```

## Example

```ts
const Claim = z.object({
  claim: z.string(),
  source: z.string(),
});

const Verdict = z.object({
  claim: z.string(),
  verified: z.boolean(),
  evidence: z.string(),
});

export const extractClaims = createHandler({
  inputValidator: z.string(),
  outputValidator: z.array(Claim),
  handle: async ({ value: document }) => {
    const response = await callClaude({
      prompt: `Extract all verifiable factual claims from this document. Return JSON array: [{ "claim": "...", "source": "..." }]\n\n${document}`,
    });
    return JSON.parse(response);
  },
}, "extractClaims");

export const verifyClaim = createHandler({
  inputValidator: Claim,
  outputValidator: Verdict,
  handle: async ({ value }) => {
    const response = await callClaude({
      prompt: `Verify this claim: "${value.claim}" (source: ${value.source}). Research whether it's true. Return JSON: { "claim": "...", "verified": true/false, "evidence": "..." }`,
    });
    return JSON.parse(response);
  },
}, "verifyClaim");
```

```ts
await workflowBuilder()
  .workflow(() =>
    pipe(extractClaims, forEach(verifyClaim))
  )
  .run();
```

## Key points

- The extraction agent focuses only on finding claims — it doesn't evaluate truth.
- Each verification agent evaluates a single claim with focused context.
- `forEach` processes all claims in parallel, so verification scales with the number of claims.
- Add an adversarial variant by replacing `verifyClaim` with a `loop` + `branch` pattern where two agents debate and a judge decides.
