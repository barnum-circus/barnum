# Subpath Exports: runtime vs pipeline

Split `@barnum/barnum` into two subpath exports to separate handler-authoring code from pipeline-construction code.

## Motivation

Handler files and pipeline files are different contexts. Handler files define what runs when the engine invokes a handler — they produce runtime values. Pipeline files define the static DAG of actions — they compose TypedAction nodes. Currently both import from `@barnum/barnum`, making it unclear what belongs where. Convenience constructors like `ok`, `err`, `some`, `none` are for handler bodies — they should not live alongside pipeline combinators. Separating now also gives us a clean place to put handler-authoring utilities as the API evolves.

## Subpath exports

### `@barnum/barnum/runtime` — handler authoring

Everything a handler file needs:

```ts
import { createHandler, ok, err } from "@barnum/barnum/runtime";
import type { Result } from "@barnum/barnum/runtime";
import { z } from "zod";

export const stepA = createHandler({
  outputValidator: resultSchema(z.string(), z.string()),
  handle: async (): Promise<Result<string, string>> => {
    if (Math.random() > 0.5) return ok("validated");
    return err("failed");
  },
}, "stepA");
```

Exports:
- `createHandler`, `createHandlerWithConfig`
- `ok`, `err` — runtime value constructors: `ok("foo")` → `{ kind: "Ok", value: "foo" }`
- `some`, `none` — runtime value constructors for Option: `some(42)` → `{ kind: "Some", value: 42 }`
- `resultSchema`, `optionSchema` — schema builders (currently `Result.schema()`, `Option.schema()`)
- `taggedUnionSchema` — for user-defined unions
- Types: `Result`, `Option`, `Handler`

### `@barnum/barnum` — pipeline construction

Everything for building the workflow DAG (current top-level export, minus handler-authoring stuff):

```ts
import { pipe, loop, tryCatch, withTimeout, constant, Result, runPipeline } from "@barnum/barnum";
import type { TypedAction } from "@barnum/barnum";
import { stepA, stepB } from "./handlers/steps";

runPipeline(
  loop<string, void>((recur, done) =>
    tryCatch(
      (throwError) => pipe(
        stepA.unwrapOr(done).drop(),
        stepB.unwrapOr(throwError).drop(),
      ),
      logError.then(recur),
    ),
  ),
);
```

Exports:
- Combinators: `pipe`, `chain`, `all`, `forEach`, `branch`, `loop`, `tryCatch`, `earlyReturn`, `recur`, `race`, `withTimeout`, `sleep`, `bind`, `bindInput`, `defineRecursiveFunctions`
- Builtins: `constant`, `identity`, `drop`, `tag`, `getField`, `getIndex`, `pick`, `merge`, `wrapInField`, `flatten`, `splitFirst`, `splitLast`, `range`, `withResource`, `first`, `last`, `panic`
- Namespaces: `Result` (pipeline combinators: `.ok`, `.err`, `.map`, `.andThen`, etc.), `Option` (`.some`, `.none`, `.map`, etc.)
- `runPipeline`
- Types: `TypedAction`, `Pipeable`, `Action`, `Config`, `ExtractInput`, `ExtractOutput`, `TaggedUnion`, etc.
- `resetEffectIdCounter` (test utility)

## Changes

### 1. New file: `src/runtime.ts`

Entry point for `@barnum/barnum/runtime`:

```ts
// Runtime value constructors
export { ok, err, some, none } from "./values.js";

// Handler creation
export { createHandler, createHandlerWithConfig, type Handler } from "./handler.js";

// Schema builders
export { resultSchema, optionSchema } from "./schemas.js";
export { taggedUnionSchema } from "./builtins.js";

// Types only
export type { Result, Option } from "./ast.js";
```

### 2. New file: `src/values.ts`

Convenience constructors so handler authors don't hand-write `{ kind, value }` objects:

```ts
import type { Result, Option } from "./ast.js";

export function ok<TValue, TError = unknown>(value: TValue): Result<TValue, TError> {
  return { kind: "Ok", value } as Result<TValue, TError>;
}

export function err<TValue = unknown, TError>(error: TError): Result<TValue, TError> {
  return { kind: "Err", value: error } as Result<TValue, TError>;
}

export function some<T>(value: T): Option<T> {
  return { kind: "Some", value } as Option<T>;
}

export function none<T = unknown>(): Option<T> {
  return { kind: "None", value: null } as Option<T>;
}
```

### 3. Extract schemas from Result/Option namespaces

Currently `Result.schema()` and `Option.schema()` live on the pipeline namespace objects. Extract them as standalone functions for the runtime export:

```ts
// src/schemas.ts
import { z } from "zod";
import type { Result, Option } from "./ast.js";

export function resultSchema<TValue, TError>(
  okSchema: z.ZodType<TValue>,
  errSchema: z.ZodType<TError>,
): z.ZodType<Result<TValue, TError>> {
  return z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("Ok"), value: okSchema }),
    z.object({ kind: z.literal("Err"), value: errSchema }),
  ]) as z.ZodType<Result<TValue, TError>>;
}

export function optionSchema<TValue>(
  valueSchema: z.ZodType<TValue>,
): z.ZodType<Option<TValue>> {
  return z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("Some"), value: valueSchema }),
    z.object({ kind: z.literal("None"), value: z.null() }),
  ]) as z.ZodType<Option<TValue>>;
}
```

`Result.schema()` and `Option.schema()` on the pipeline namespaces can delegate to these or be removed (they're only used in handler files, which will import from `/runtime`).

### 4. package.json: add subpath export

```json
{
  "exports": {
    ".": {
      "types": "./dist/index.d.ts",
      "default": "./dist/index.js"
    },
    "./runtime": {
      "types": "./dist/runtime.d.ts",
      "default": "./dist/runtime.js"
    },
    "./package.json": "./package.json"
  }
}
```

### 5. Existing `src/index.ts` stays as-is

The top-level export continues to export everything it does today. It does NOT re-export from `./runtime.ts` — the two entry points are independent. Pipeline files don't need `ok`/`err`/`some`/`none`.

### 6. Move `createHandler` out of top-level export

Currently `src/index.ts` has `export * from "./handler.js"`. Remove this — `createHandler` is only needed in handler files, which import from `/runtime`. Pipeline files import handler *instances* (the exported `TypedAction` values), not `createHandler` itself.

## Migration

Handler files:
```ts
// Before
import { createHandler, Result } from "@barnum/barnum";
import type { Result as ResultT } from "@barnum/barnum";

handle: async () => ({ kind: "Ok", value: "validated" })

// After
import { createHandler, ok, err, resultSchema } from "@barnum/barnum/runtime";
import type { Result } from "@barnum/barnum/runtime";

handle: async () => ok("validated")
```

Pipeline files: no change (they already import from `@barnum/barnum`).

## Open questions

1. Should `@barnum/barnum` re-export `createHandler` for backward compat, or is a clean break fine? (Clean break — no one is using this.)
2. Should `/runtime` export the `z` from zod for convenience, or let handler authors import zod directly?
3. Should `Result.schema()` / `Option.schema()` be removed from the pipeline namespaces, or kept as aliases? Removing is cleaner — they're handler-authoring concerns.
