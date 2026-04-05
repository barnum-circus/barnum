# Optional Handler Types

**Blocks:** `HANDLER_SCHEMAS_IN_AST.md`

## TL;DR

Add `outputValidator`. Make `stepConfigValidator` optional. Enable typed input without a validator via explicit type parameters. Preserve `Handler<never, ...>` for source handlers.

---

## Current state

`createHandler` has two overloads discriminated by the **presence of `inputValidator`**:

```ts
// With inputValidator → typed input
export function createHandler<TValue, TOutput>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;

// Without inputValidator → source handler, input is never
export function createHandler<TOutput>(
  definition: {
    handle: () => Promise<TOutput>;
  },
  exportName?: string,
): Handler<never, HandlerOutput<TOutput>>;
```

`createHandlerWithConfig` has two overloads, both **requiring `stepConfigValidator`**:

```ts
// With inputValidator
export function createHandlerWithConfig<TValue, TOutput, TStepConfig>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// Without inputValidator
export function createHandlerWithConfig<TOutput, TStepConfig>(
  definition: {
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { stepConfig: TStepConfig }) => Promise<TOutput>;
  },
): (config: TStepConfig) => TypedAction<never, HandlerOutput<TOutput>>;
```

### Problems

1. **No `outputValidator`.** Can't validate handler output at all.
2. **`stepConfigValidator` is required.** Can't have config-based handlers without writing a Zod schema.
3. **No way to type input without a validator.** Want `Handler<MyType, ...>` but don't want to write `z.object(...)` — forced to choose between a validator or `never`.

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

### `createHandler` overloads (3 overloads)

```ts
// Overload 1: with inputValidator — TValue inferred from validator
export function createHandler<TValue, TOutput>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    outputValidator?: z.ZodType<TOutput>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;

// Overload 2: source handler — no inputValidator, handle takes no args
export function createHandler<TOutput = unknown>(
  definition: {
    outputValidator?: z.ZodType<TOutput>;
    handle: () => Promise<TOutput>;
  },
  exportName?: string,
): Handler<never, HandlerOutput<TOutput>>;

// Overload 3: explicit type params — no inputValidator, handle takes value
// Only reachable via explicit type params (overload 2 catches implicit cases)
export function createHandler<TValue, TOutput>(
  definition: {
    outputValidator?: z.ZodType<TOutput>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;
```

#### Overload resolution

Overload 2 has 1 type parameter (`TOutput`). When the caller provides 2 explicit type params (e.g. `createHandler<string, number>(...)`), TypeScript skips overload 2 entirely (wrong arity). It tries overload 1 (needs `inputValidator`, absent → fail), then overload 3 (matches).

Without explicit type params, overload 2 catches all no-`inputValidator` calls because `(ctx: { value: T }) => void` is assignable to `() => void` in TypeScript (fewer params accepted). This is correct — without explicit types or a validator, there's nothing to infer input from.

#### Usage

```ts
// With validator — type inferred
createHandler({
  inputValidator: z.string(),
  handle: async ({ value }) => value.toUpperCase(),
}, "myHandler");
// → Handler<string, string>

// With explicit type params — no validator needed
createHandler<string, number>({
  handle: async ({ value }) => value.length,
}, "myHandler");
// → Handler<string, number>

// Source handler — no input
createHandler({
  handle: async () => ["auth.ts", "routes.ts"],
}, "listFiles");
// → Handler<never, string[]>

// Output validator — TOutput from validator
createHandler({
  inputValidator: z.string(),
  outputValidator: z.number(),
  handle: async ({ value }) => value.length,
}, "myHandler");
// → Handler<string, number>
```

### `createHandlerWithConfig` overloads (3 overloads)

For `createHandlerWithConfig`, the handle context object type serves as a natural discriminant: `{ stepConfig }` vs `{ value, stepConfig }` are NOT interchangeable under `strictFunctionTypes` because a function expecting `{ value, stepConfig }` requires `value` in its parameter, and `{ stepConfig }` doesn't satisfy `{ value, stepConfig }` contravariantly.

```ts
// Overload 1: with inputValidator — TValue from validator
export function createHandlerWithConfig<TValue, TOutput, TStepConfig = unknown>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    outputValidator?: z.ZodType<TOutput>;
    stepConfigValidator?: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// Overload 2: config-only — no value in handle context
export function createHandlerWithConfig<TOutput = unknown, TStepConfig = unknown>(
  definition: {
    outputValidator?: z.ZodType<TOutput>;
    stepConfigValidator?: z.ZodType<TStepConfig>;
    handle: (context: { stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<never, HandlerOutput<TOutput>>;

// Overload 3: explicit type params with value, no inputValidator
export function createHandlerWithConfig<TValue, TOutput, TStepConfig = unknown>(
  definition: {
    outputValidator?: z.ZodType<TOutput>;
    stepConfigValidator?: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;
```

#### Overload resolution for `createHandlerWithConfig`

Unlike `createHandler`, the handle context provides a structural discriminant under `strictFunctionTypes`:

- `({ stepConfig }) => ...` — overload 2 matches (value not destructured)
- `({ value, stepConfig }) => ...` — overload 2 FAILS because `{ stepConfig: T }` does not satisfy `{ value: U; stepConfig: T }` contravariantly. Falls to overloads 1 or 3.

This means overload 3 is reachable both via explicit type params AND by destructuring `value` without providing `inputValidator`.

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

  it("without inputValidator, destructuring value: input is unknown (overload 3)", () => {
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
