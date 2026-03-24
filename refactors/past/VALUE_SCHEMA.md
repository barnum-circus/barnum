# Value Schema Validation

**Depends on:** ADD_TYPESCRIPT_DISPATCH

## Motivation

Steps should be able to declare the shape of their task values so invalid payloads are caught before dispatch. This validation applies to both Bash and TypeScript actions — Bash actions have no handler-side validation, so config-level schema validation is their only defense. TypeScript actions have `getStepValueValidator` in the handler, but config-level validation catches bad values earlier (before subprocess spawn).

## Design direction

The value schema is defined in JS (Zod or JSON Schema literal) on the step config. The JS layer converts it to JSON Schema and passes it to Rust in the serialized config. Rust validates task values against the schema before dispatching.

```typescript
{
  name: "Analyze",
  action: { kind: "Bash", script: "..." },
  valueSchema: z.object({ file: z.string() }),
  next: [],
}
```

The JS layer converts `valueSchema` from Zod to JSON Schema before serializing to Rust. Rust stores the compiled JSON Schema per-step and validates each task's value before dispatch.

## Open questions

- Should TypeScript actions use the same mechanism, or is handler-side Zod validation sufficient?
- Zod-to-JSON-Schema conversion: use `zod-to-json-schema` or a custom converter?
- Error reporting: how should validation failures surface (stderr, state log, both)?
