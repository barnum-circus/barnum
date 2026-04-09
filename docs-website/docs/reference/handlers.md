# Handlers

Handlers are the leaf nodes of a Barnum workflow — the places where actual work happens. Everything else (pipes, loops, branches) is plumbing. Handlers are where you read files, call LLMs, run shell commands, or do any other side-effectful work.

A handler is an async function wrapped in `createHandler`. The wrapper does three things:

1. **Registers the handler** with the Barnum runtime by recording its file path and export name.
2. **Attaches Zod validators** (optional) so input and output are validated at runtime via JSON Schema.
3. **Returns a `TypedAction`** that can be composed with combinators like `pipe`, `forEach`, `branch`, etc.

Handlers run in isolated subprocesses. Each invocation gets its own process — handlers never share memory with each other or with the orchestrator.

## `createHandler`

```ts
import { createHandler } from "@barnum/barnum";

function createHandler<TValue = never, TOutput = unknown>(
  definition: {
    inputValidator?: z.ZodType<TValue>;
    outputValidator?: z.ZodType<NoInfer<TOutput>>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;
```

### Parameters

| Parameter | Required | Description |
|---|---|---|
| `definition.handle` | Yes | The async function that does the work. Receives `{ value }` as its argument. |
| `definition.inputValidator` | No | A Zod schema for the input. Converted to JSON Schema and validated by the Rust runtime before the handler is called. |
| `definition.outputValidator` | No | A Zod schema for the output. Validated after the handler returns. |
| `exportName` | No | The name of the export that this handler is assigned to. Defaults to `"default"`. See [Export name](#export-name). |

### Return type

Returns a `Handler`, which is a `TypedAction` branded with runtime metadata. It can be passed directly to any combinator (`pipe`, `forEach`, `branch`, etc.) or used with postfix methods (`.then()`, `.forEach()`, etc.).

If `handle` returns `Promise<void>`, the output type is `never` — fire-and-forget handlers compose naturally without needing `.drop()`.

## Rules

### Handlers must be exported

The Rust runtime executes handlers by importing the module and accessing the named export. If the handler isn't exported, the runtime can't find it.

```ts
// ✅ Correct — exported
export const listFiles = createHandler({ ... }, "listFiles");

// ❌ Wrong — not exported, runtime can't find it
const listFiles = createHandler({ ... }, "listFiles");
```

### Handlers must be called `createHandler`

`createHandler` uses V8 stack trace introspection to determine the file path of the caller. This is how the runtime knows which module to import when executing the handler. The handler must be created at the top level of a module via `createHandler` — you can't defer creation or wrap it in another function.

### The export name must match

The second argument to `createHandler` tells the runtime which export to look up when it imports the module. It must match the actual export name exactly.

```ts
// ✅ Correct — export name matches the variable name
export const listFiles = createHandler({
  handle: async () => { /* ... */ },
}, "listFiles");

// ❌ Wrong — export name doesn't match
export const listFiles = createHandler({
  handle: async () => { /* ... */ },
}, "getFiles");
```

If you omit the second argument, it defaults to `"default"`, meaning the handler must be a default export:

```ts
// ✅ Correct — no export name, so it must be the default export
export default createHandler({
  handle: async () => { /* ... */ },
});
```

## Examples

### No input (entry point)

Handlers with no `inputValidator` accept `never` — they can only appear at the start of a pipeline or after a `.drop()`.

```ts
export const listFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    return readdirSync("src").filter((f) => f.endsWith(".ts"));
  },
}, "listFiles");
```

### Input and output

```ts
export const analyze = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.array(RefactorValidator),
  handle: async ({ value }) => {
    const response = await callClaude({
      prompt: `Analyze ${value.file} for refactoring opportunities.`,
      allowedTools: ["Read"],
    });
    return JSON.parse(response);
  },
}, "analyze");
```

### Fire-and-forget (void output)

When `handle` returns `Promise<void>`, the handler's output type is `never`. It composes in pipes without needing `.drop()`.

```ts
export const implement = createHandler({
  inputValidator: z.object({
    worktreePath: z.string(),
    description: z.string(),
  }),
  handle: async ({ value }) => {
    await callClaude({
      prompt: `Implement this refactor: ${value.description}`,
      allowedTools: ["Read", "Edit"],
      cwd: value.worktreePath,
    });
  },
}, "implement");
```

## `createHandlerWithConfig`

For handlers that need configuration at composition time (not at runtime), use `createHandlerWithConfig`. It returns a factory function that takes the config and produces a `TypedAction`.

```ts
import { createHandlerWithConfig } from "@barnum/barnum";

function createHandlerWithConfig<
  TValue = never,
  TOutput = unknown,
  TStepConfig = unknown,
>(
  definition: {
    inputValidator?: z.ZodType<TValue>;
    outputValidator?: z.ZodType<NoInfer<TOutput>>;
    stepConfigValidator?: z.ZodType<TStepConfig>;
    handle: (context: {
      value: TValue;
      stepConfig: TStepConfig;
    }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;
```

### Example

```ts
export const migrate = createHandlerWithConfig({
  inputValidator: z.object({ file: z.string() }),
  stepConfigValidator: z.object({ to: z.string() }),
  handle: async ({ value, stepConfig }) => {
    await callClaude({
      prompt: `Convert ${value.file} to ${stepConfig.to}.`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "migrate");

// In the workflow — config is baked in at composition time
listFiles.forEach(migrate({ to: "TypeScript" }));
```

The same [rules](#rules) apply: the handler must be exported, and the export name must match.
