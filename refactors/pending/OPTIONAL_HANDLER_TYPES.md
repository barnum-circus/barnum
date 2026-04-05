# Optional Handler Types

**Blocks:** `HANDLER_SCHEMAS_IN_AST.md`

## TL;DR

Add `outputValidator`. Make `stepConfigValidator` optional. Enable typed input without a validator via explicit type parameters. Preserve `Handler<never, ...>` for source handlers.

---

## Problems with current state

1. **No `outputValidator`.** Can't validate handler output.
2. **`stepConfigValidator` is required** on `createHandlerWithConfig`.
3. **No way to type input without a validator.** Forced to choose between providing a Zod schema or getting `never`.

---

## Design

### Why source handlers must stay `Handler<never, ...>`

`WorkflowAction` requires `__in?: void`. Since `never extends void` but `unknown` does NOT extend `void`, source handlers MUST produce `Handler<never, ...>` to remain usable as workflow entry points via `config()` and `workflowBuilder().workflow()`.

### `HandlerDefinition`

```ts
// Before
export interface HandlerDefinition<TValue = unknown, TOutput = unknown, TStepConfig = unknown> {
  inputValidator?: z.ZodType<TValue>;
  stepConfigValidator?: z.ZodType<TStepConfig>;
  handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
}

// After
export interface HandlerDefinition<TValue = unknown, TOutput = unknown, TStepConfig = unknown> {
  inputValidator?: z.ZodType<TValue>;
  outputValidator?: z.ZodType<TOutput>;
  stepConfigValidator?: z.ZodType<TStepConfig>;
  handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
}
```

### `createHandler` overloads (4 overloads)

Each validator is either required or absent. TS excess property checking on object literals discriminates: providing a property not in the overload → excess → skip; missing a required property → skip.

Every overload has `<TValue, TOutput>`. When `inputValidator` is absent, `TValue` defaults to `never` (source handler). Explicit type params override the default: `createHandler<string, number>({...})` sets `TValue = string`.

```ts
// 1. inputValidator + outputValidator
export function createHandler<TValue, TOutput>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    outputValidator: z.ZodType<TOutput>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;

// 2. inputValidator only
export function createHandler<TValue, TOutput>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;

// 3. outputValidator only
export function createHandler<TValue = never, TOutput = unknown>(
  definition: {
    outputValidator: z.ZodType<TOutput>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;

// 4. no validators
export function createHandler<TValue = never, TOutput = unknown>(
  definition: {
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;
```

### `createHandlerWithConfig` overloads (8 overloads)

Same principle. 3 validators → 2³ = 8 combinations. Every overload has `<TValue, TOutput, TStepConfig>`. When `inputValidator` is absent, `TValue` defaults to `never`. When `stepConfigValidator` is absent, `TStepConfig` defaults to `unknown`. Explicit type params override defaults.

```ts
// --- inputValidator present (4 overloads) ---

// 1. input + output + stepConfig
export function createHandlerWithConfig<TValue, TOutput, TStepConfig>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    outputValidator: z.ZodType<TOutput>;
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// 2. input + output
export function createHandlerWithConfig<TValue, TOutput, TStepConfig = unknown>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    outputValidator: z.ZodType<TOutput>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// 3. input + stepConfig
export function createHandlerWithConfig<TValue, TOutput, TStepConfig>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// 4. input only
export function createHandlerWithConfig<TValue, TOutput, TStepConfig = unknown>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// --- inputValidator absent (4 overloads) ---

// 5. output + stepConfig
export function createHandlerWithConfig<TValue = never, TOutput = unknown, TStepConfig = unknown>(
  definition: {
    outputValidator: z.ZodType<TOutput>;
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// 6. output only
export function createHandlerWithConfig<TValue = never, TOutput = unknown, TStepConfig = unknown>(
  definition: {
    outputValidator: z.ZodType<TOutput>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// 7. stepConfig only
export function createHandlerWithConfig<TValue = never, TOutput = unknown, TStepConfig = unknown>(
  definition: {
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// 8. no validators
export function createHandlerWithConfig<TValue = never, TOutput = unknown, TStepConfig = unknown>(
  definition: {
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;
```

### `UntypedHandlerDefinition`

```ts
// Before
interface UntypedHandlerDefinition {
  inputValidator?: z.ZodType;
  stepConfigValidator?: z.ZodType;
  handle: (...args: any[]) => Promise<unknown>;
}

// After
interface UntypedHandlerDefinition {
  inputValidator?: z.ZodType;
  outputValidator?: z.ZodType;
  stepConfigValidator?: z.ZodType;
  handle: (...args: any[]) => Promise<unknown>;
}
```

---

## Type tests

All tests use the existing `assertExact<IsExact<...>>()` pattern from `types.test.ts`. Tests marked `@ts-expect-error` must fail to compile — if they don't, the test itself fails.

### createHandler: input/output extraction

```ts
describe("optional handler types", () => {
  // --- createHandler with inputValidator (existing behavior, preserved) ---

  it("inputValidator infers TValue", () => {
    const h = createHandler({
      inputValidator: z.object({ name: z.string() }),
      handle: async ({ value }) => value.name.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, { name: string }>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });

  it("inputValidator + outputValidator infers both", () => {
    const h = createHandler({
      inputValidator: z.string(),
      outputValidator: z.number(),
      handle: async ({ value }) => value.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, string>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });

  // --- createHandler source handler (no inputValidator, no args) ---

  it("source handler: input is never", () => {
    const h = createHandler({
      handle: async () => "hello",
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, never>>();
    assertExact<IsExact<ExtractOutput<typeof h>, string>>();
  });

  it("source handler with outputValidator", () => {
    const h = createHandler({
      outputValidator: z.array(z.string()),
      handle: async () => ["a", "b"],
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, never>>();
    assertExact<IsExact<ExtractOutput<typeof h>, string[]>>();
  });

  // --- createHandler with explicit type params (overload 3) ---

  it("explicit type params: typed input without validator", () => {
    const h = createHandler<{ id: number }, string>({
      handle: async ({ value }) => String(value.id),
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, { id: number }>>();
    assertExact<IsExact<ExtractOutput<typeof h>, string>>();
  });

  it("explicit type params with outputValidator", () => {
    const h = createHandler<string, number>({
      outputValidator: z.number(),
      handle: async ({ value }) => value.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, string>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });
```

### createHandler: type errors when lying

```ts
  // --- handle must match declared types ---

  it("rejects handle that returns wrong type for explicit TOutput", () => {
    // @ts-expect-error — handle returns string, TOutput is number
    createHandler<string, number>({
      handle: async ({ value }) => value.toUpperCase(),
    }, "h");
  });

  it("rejects handle that uses wrong type for explicit TValue", () => {
    createHandler<string, number>({
      handle: async ({ value }) => {
        // @ts-expect-error — value is string, not number; .toFixed doesn't exist
        return value.toFixed(2);
      },
    }, "h");
  });

  // --- validators must match declared types ---

  it("rejects inputValidator that contradicts explicit TValue", () => {
    // @ts-expect-error — TValue is string but validator is z.number()
    createHandler<string, number>({
      inputValidator: z.number(),
      handle: async ({ value }) => value.length,
    }, "h");
  });

  it("rejects outputValidator that contradicts explicit TOutput", () => {
    // @ts-expect-error — TOutput is number but validator is z.string()
    createHandler<string, number>({
      outputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "h");
  });

  it("rejects outputValidator that contradicts inferred TOutput", () => {
    // @ts-expect-error — handle returns number, outputValidator is z.string()
    createHandler({
      inputValidator: z.string(),
      outputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "h");
  });

  it("rejects inputValidator that contradicts handle parameter", () => {
    // @ts-expect-error — validator says number, handle destructures string methods
    createHandler({
      inputValidator: z.number(),
      handle: async ({ value }) => value.toUpperCase(),
    }, "h");
  });

  // --- validators must match explicit types invariantly ---
  // When you provide both explicit type params AND a validator, the validator's
  // type must exactly match the explicit type. Wider or narrower should reject.

  it("rejects inputValidator wider than explicit TValue", () => {
    // @ts-expect-error — TValue is "hello" but validator accepts any string
    createHandler<"hello", string>({
      inputValidator: z.string(),
      handle: async ({ value }) => value,
    }, "h");
  });

  it("rejects outputValidator wider than explicit TOutput", () => {
    // @ts-expect-error — TOutput is "ok" but validator accepts any string
    createHandler<string, "ok">({
      inputValidator: z.string(),
      outputValidator: z.string(),
      handle: async ({ value }) => "ok" as const,
    }, "h");
  });

  it("rejects inputValidator narrower than explicit TValue", () => {
    // @ts-expect-error — TValue is string but validator only accepts "hello"
    createHandler<string, string>({
      inputValidator: z.literal("hello"),
      handle: async ({ value }) => value,
    }, "h");
  });

  it("accepts inputValidator that exactly matches explicit TValue", () => {
    const h = createHandler<string, number>({
      inputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, string>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });

  it("accepts outputValidator that exactly matches explicit TOutput", () => {
    const h = createHandler<string, number>({
      inputValidator: z.string(),
      outputValidator: z.number(),
      handle: async ({ value }) => value.length,
    }, "h");
    assertExact<IsExact<ExtractInput<typeof h>, string>>();
    assertExact<IsExact<ExtractOutput<typeof h>, number>>();
  });
```

### createHandler: source handlers in workflows

```ts
  it("source handler is accepted as workflow entry point", () => {
    const h = createHandler({
      handle: async () => "result",
    }, "h");
    // Handler<never, string> — __in is never, which extends void
    workflowBuilder().workflow(() => h);
  });

  it("typed handler is rejected as workflow entry point", () => {
    const h = createHandler({
      inputValidator: z.string(),
      handle: async ({ value }) => value,
    }, "h");
    // @ts-expect-error — Handler<string, string> can't be a workflow entry point
    workflowBuilder().workflow(() => h);
  });

  it("explicit-typed handler is rejected as workflow entry point", () => {
    const h = createHandler<string, string>({
      handle: async ({ value }) => value,
    }, "h");
    // @ts-expect-error — Handler<string, string> can't be a workflow entry point
    workflowBuilder().workflow(() => h);
  });
```

### createHandler: pipeline composition

```ts
  it("validator-typed handlers compose in pipe", () => {
    const toLength = createHandler({
      inputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "toLength");
    const double = createHandler({
      inputValidator: z.number(),
      handle: async ({ value }) => value * 2,
    }, "double");
    const p = pipe(toLength, double);
    assertExact<IsExact<ExtractInput<typeof p>, string>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });

  it("explicit-typed handlers compose in pipe", () => {
    const toLength = createHandler<string, number>({
      handle: async ({ value }) => value.length,
    }, "toLength");
    const double = createHandler<number, number>({
      handle: async ({ value }) => value * 2,
    }, "double");
    const p = pipe(toLength, double);
    assertExact<IsExact<ExtractInput<typeof p>, string>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });

  it("mixed validator + explicit compose in pipe", () => {
    const toLength = createHandler({
      inputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "toLength");
    const double = createHandler<number, number>({
      handle: async ({ value }) => value * 2,
    }, "double");
    const p = pipe(toLength, double);
    assertExact<IsExact<ExtractInput<typeof p>, string>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });

  it("pipe rejects mismatched adjacent types", () => {
    const toString = createHandler({
      inputValidator: z.string(),
      handle: async ({ value }) => value.toUpperCase(),
    }, "toString");
    const fromNumber = createHandler({
      inputValidator: z.number(),
      handle: async ({ value }) => value * 2,
    }, "fromNumber");
    // @ts-expect-error — toString outputs string, fromNumber expects number
    pipe(toString, fromNumber);
  });

  it("source handler composes at pipe start", () => {
    const source = createHandler({
      handle: async () => 42,
    }, "source");
    const double = createHandler({
      inputValidator: z.number(),
      handle: async ({ value }) => value * 2,
    }, "double");
    const p = pipe(source, double);
    assertExact<IsExact<ExtractInput<typeof p>, any>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });

  it("postfix .then() works with explicit-typed handler", () => {
    const toLength = createHandler({
      inputValidator: z.string(),
      handle: async ({ value }) => value.length,
    }, "toLength");
    const double = createHandler<number, number>({
      handle: async ({ value }) => value * 2,
    }, "double");
    const chained = toLength.then(double);
    assertExact<IsExact<ExtractInput<typeof chained>, string>>();
    assertExact<IsExact<ExtractOutput<typeof chained>, number>>();
  });
```

### createHandlerWithConfig: all validators optional

```ts
  // --- stepConfigValidator optional ---

  it("omitting stepConfigValidator: stepConfig is unknown", () => {
    const factory = createHandlerWithConfig({
      handle: async ({ stepConfig }) => String(stepConfig),
    }, "h");
    const action = factory("anything");
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("stepConfigValidator provided: stepConfig is typed", () => {
    const factory = createHandlerWithConfig({
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ stepConfig }) => stepConfig.retries,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  it("explicit TStepConfig without validator", () => {
    const factory = createHandlerWithConfig<never, string, { retries: number }>({
      handle: async ({ stepConfig }) => String(stepConfig.retries),
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  // --- inputValidator optional on createHandlerWithConfig ---

  it("with inputValidator: input is typed", () => {
    const factory = createHandlerWithConfig({
      inputValidator: z.string(),
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ value, stepConfig }) => `${value}:${stepConfig.retries}`,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("without inputValidator, destructuring value: input is unknown (overload 12)", () => {
    const factory = createHandlerWithConfig({
      handle: async ({ value, stepConfig }) => String(value),
    }, "h");
    const action = factory("anything");
    assertExact<IsExact<ExtractInput<typeof action>, unknown>>();
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });

  it("explicit type params without inputValidator", () => {
    const factory = createHandlerWithConfig<string, number, { retries: number }>({
      handle: async ({ value, stepConfig }) => value.length + stepConfig.retries,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  // --- all validators present ---

  it("all three validators", () => {
    const factory = createHandlerWithConfig({
      inputValidator: z.string(),
      outputValidator: z.number(),
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ value, stepConfig }) => value.length + stepConfig.retries,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });
```

### createHandlerWithConfig: type errors when lying

```ts
  it("rejects wrong stepConfigValidator", () => {
    // @ts-expect-error — explicit TStepConfig is { retries: number }, validator is z.string()
    createHandlerWithConfig<never, string, { retries: number }>({
      stepConfigValidator: z.string(),
      handle: async ({ stepConfig }) => String(stepConfig.retries),
    }, "h");
  });

  it("rejects handle that lies about stepConfig shape", () => {
    createHandlerWithConfig({
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ stepConfig }) => {
        // @ts-expect-error — stepConfig.retries is number, not string method
        return stepConfig.retries.toUpperCase();
      },
    }, "h");
  });

  it("rejects outputValidator contradicting handle return", () => {
    // @ts-expect-error — handle returns number, outputValidator is z.string()
    createHandlerWithConfig({
      outputValidator: z.string(),
      handle: async ({ stepConfig }) => 42,
    }, "h");
  });

  // --- validators must match explicit types invariantly ---

  it("rejects stepConfigValidator wider than explicit TStepConfig", () => {
    // @ts-expect-error — TStepConfig is { retries: 3 } but validator accepts any { retries: number }
    createHandlerWithConfig<never, string, { retries: 3 }>({
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ stepConfig }) => String(stepConfig.retries),
    }, "h");
  });

  it("rejects stepConfigValidator narrower than explicit TStepConfig", () => {
    // @ts-expect-error — TStepConfig is { retries: number } but validator only accepts { retries: 3 }
    createHandlerWithConfig<never, string, { retries: number }>({
      stepConfigValidator: z.object({ retries: z.literal(3) }),
      handle: async ({ stepConfig }) => String(stepConfig.retries),
    }, "h");
  });

  it("accepts all validators exactly matching explicit types", () => {
    const factory = createHandlerWithConfig<string, number, { retries: number }>({
      inputValidator: z.string(),
      outputValidator: z.number(),
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ value, stepConfig }) => value.length + stepConfig.retries,
    }, "h");
    const action = factory({ retries: 3 });
    assertExact<IsExact<ExtractInput<typeof action>, string>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });
```

### createHandlerWithConfig: pipeline composition

```ts
  it("withConfig handler composes in pipe", () => {
    const source = createHandler({
      handle: async () => "hello",
    }, "source");
    const withRetries = createHandlerWithConfig({
      inputValidator: z.string(),
      stepConfigValidator: z.object({ retries: z.number() }),
      handle: async ({ value, stepConfig }) => `${value}:${stepConfig.retries}`,
    }, "withRetries");
    const p = pipe(source, withRetries({ retries: 3 }));
    assertExact<IsExact<ExtractInput<typeof p>, any>>();
    assertExact<IsExact<ExtractOutput<typeof p>, string>>();
  });

  it("explicit-typed withConfig handler composes in pipe", () => {
    const source = createHandler({
      handle: async () => "hello",
    }, "source");
    const transform = createHandlerWithConfig<string, number, { n: number }>({
      handle: async ({ value, stepConfig }) => value.length + stepConfig.n,
    }, "transform");
    const p = pipe(source, transform({ n: 10 }));
    assertExact<IsExact<ExtractInput<typeof p>, any>>();
    assertExact<IsExact<ExtractOutput<typeof p>, number>>();
  });
});
```

---

## What this does NOT include

- **No Zod-to-JSON-Schema conversion.** That's `HANDLER_SCHEMAS_IN_AST.md`.
- **No schema fields on the AST node.** That's `HANDLER_SCHEMAS_IN_AST.md`.
- **No runtime validation.** That's `HANDLER_VALIDATION.md`.
- **No demo output validator additions.** That's `HANDLER_SCHEMAS_IN_AST.md`.

This refactor is purely about the TypeScript type signatures and making validators optional with sensible defaults.
