# Optional Handler Types

**Blocks:** `HANDLER_SCHEMAS_IN_AST.md`

## TL;DR

All validators (`inputValidator`, `outputValidator`, `stepConfigValidator`) become optional on every `createHandler` / `createHandlerWithConfig` variant. When omitted, types default to `unknown`. Explicit type parameters override without requiring a validator.

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
4. **Source handlers are `Handler<never, ...>`.** This conflates "doesn't receive input" with "input type is uninhabited." A handler that ignores its input is not the same as one that can't receive it.

---

## Design

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

### `createHandler` overloads

Collapse to a single overload. All validators optional. Types default to `unknown`.

```ts
// Before: two overloads discriminated by inputValidator presence
// After: single overload, everything optional
export function createHandler<TValue = unknown, TOutput = unknown>(
  definition: {
    inputValidator?: z.ZodType<TValue>;
    outputValidator?: z.ZodType<TOutput>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;
```

Usage:

```ts
// With validator — type inferred from validator
createHandler({
  inputValidator: z.string(),
  handle: async ({ value }) => value.toUpperCase(), // value: string
}, "myHandler");

// Explicit type params — no validator needed
createHandler<string, number>({
  handle: async ({ value }) => value.length, // value: string
}, "myHandler");

// No validator, no explicit type — defaults to unknown
createHandler({
  handle: async ({ value }) => String(value), // value: unknown
}, "myHandler");

// Source handler — ignores input (still works, just unknown not never)
createHandler({
  handle: async () => ["auth.ts", "routes.ts"],
}, "listFiles");
```

### `createHandlerWithConfig` overloads

Same principle. All validators optional.

```ts
// Before: two overloads, both requiring stepConfigValidator
// After: single overload, everything optional
export function createHandlerWithConfig<TValue = unknown, TOutput = unknown, TStepConfig = unknown>(
  definition: {
    inputValidator?: z.ZodType<TValue>;
    outputValidator?: z.ZodType<TOutput>;
    stepConfigValidator?: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;
```

Usage:

```ts
// All validators
createHandlerWithConfig({
  inputValidator: z.string(),
  outputValidator: z.string(),
  stepConfigValidator: z.object({ retries: z.number() }),
  handle: async ({ value, stepConfig }) => { ... },
}, "myHandler");

// Explicit types, no validators
createHandlerWithConfig<string, string, { retries: number }>({
  handle: async ({ value, stepConfig }) => { ... },
}, "myHandler");

// Just stepConfig validator, rest inferred
createHandlerWithConfig({
  stepConfigValidator: z.object({ retries: z.number() }),
  handle: async ({ stepConfig }) => { ... },
}, "myHandler");
```

### `UntypedHandlerDefinition`

Add `outputValidator` to match:

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

## Tradeoff: `Handler<never, ...>` → `Handler<unknown, ...>` for source handlers

Currently, source handlers (no `inputValidator`, `handle: () => ...`) produce `Handler<never, ...>`. This means the type system prevents placing them in the middle of a pipeline where they'd silently drop upstream output.

With a single overload, source handlers become `Handler<unknown, ...>` — they accept any input but ignore it. This is less precise.

We accept this tradeoff because:
1. The practical risk is low — source handlers that ignore input are a logic error caught in review, not a common mistake.
2. The benefit is large — one simple overload instead of a matrix of overloads that grows with each new validator.
3. If we later need to recover the `never` distinction, we can add `createSourceHandler` as a separate function.

---

## What this does NOT include

- **No Zod-to-JSON-Schema conversion.** That's `HANDLER_SCHEMAS_IN_AST.md`.
- **No schema fields on the AST node.** That's `HANDLER_SCHEMAS_IN_AST.md`.
- **No runtime validation.** That's `HANDLER_VALIDATION.md`.
- **No demo output validator additions.** That's `HANDLER_SCHEMAS_IN_AST.md`.

This refactor is purely about the TypeScript type signatures and making validators optional with sensible defaults.
