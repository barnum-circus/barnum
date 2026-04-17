# API Contract Verification

Compare an API specification (OpenAPI, GraphQL schema, protobuf) against the actual implementation, flag drift, and optionally generate fixes.

## Workflow

```ts
runPipeline(
  all(parseAPISpec, analyzeImplementation)
    .then(compareContracts)
    .then(forEach(classifyDrift))
    .then(branch({
      Breaking: forEach(generateFix),
      NonBreaking: drop,
    })),
);
```

## Stages

1. **Parallel extraction** — parse the API spec and analyze the implementation concurrently.
2. **Compare** — diff the declared contract against the actual behavior. Output: list of discrepancies.
3. **Classify** — for each discrepancy, determine if it's a breaking change or non-breaking drift.
4. **Fix breaking changes** — generate patches for breaking discrepancies. Non-breaking drift is logged but not fixed.

## Key points

- The spec parser is deterministic (no LLM) — it reads the OpenAPI/protobuf file and outputs structured data.
- The implementation analyzer uses an LLM to read route handlers and extract the actual request/response shapes.
- `forEach` processes all discrepancies concurrently.
- The fix generator sees one discrepancy at a time with focused context.
