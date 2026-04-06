---
image: /img/og/repertoire-validation.png
---

# Schema Validation

Validate handler inputs and outputs with Zod schemas. Barnum compiles schemas into JSON Schema validators at workflow init and enforces them at every handler boundary.

## Pattern

```ts
export const myHandler = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.object({ status: z.enum(["success", "failure"]) }),
  handle: async ({ value }) => {
    // ...
    return { status: "success" };
  },
}, "myHandler");
```

## Example

A handler that processes files and must return a structured result:

```ts
const FileInput = z.object({
  file: z.string(),
  language: z.enum(["typescript", "javascript", "python"]),
});

const ProcessResult = z.object({
  file: z.string(),
  linesChanged: z.number(),
  summary: z.string(),
});

export const processFile = createHandler({
  inputValidator: FileInput,
  outputValidator: ProcessResult,
  handle: async ({ value }) => {
    const response = await callClaude({
      prompt: `Refactor ${value.file} (${value.language}). Return JSON: { "file": "...", "linesChanged": N, "summary": "..." }`,
      allowedTools: ["Read", "Edit"],
    });
    return JSON.parse(response);
  },
}, "processFile");
```

## Key points

- Validators are optional — omit `inputValidator` for handlers that accept any input, omit `outputValidator` for handlers that return void.
- Validation happens at the boundary: before dispatch (input) and after completion (output).
- If validation fails, the workflow stops with a clear error showing what was expected vs. what was received.
- Zod schemas are compiled to JSON Schema at init, so validation is fast at runtime.
