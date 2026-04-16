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

### `@barnum/barnum/pipeline` — pipeline construction

Everything for building the workflow DAG:

```ts
import { pipe, loop, tryCatch, withTimeout, constant, Result, runPipeline } from "@barnum/barnum/pipeline";
import type { TypedAction } from "@barnum/barnum/pipeline";
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
- Namespaces: `Result` (pipeline combinators: `.map`, `.andThen`, `.mapErr`, `.unwrap`, `.unwrapOr`, etc.), `Option` (`.map`, `.andThen`, `.unwrap`, `.unwrapOr`, `.filter`, `.collect`, etc.)
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
    "./pipeline": {
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

### 5. Clean up `src/index.ts` — this becomes the pipeline export

The top-level export becomes purely pipeline construction. No handler-authoring code leaks through. Changes:

- **Remove `export * from "./handler.js"`** — `createHandler` moves to `/runtime` only. Pipeline files import handler *instances* (the exported `TypedAction` values), not `createHandler` itself.
- **Remove `Result.ok`, `Result.err`** from the `Result` namespace — dead code, never called anywhere. All pipeline re-tagging uses bare `tag("Ok")` / `tag("Err")`.
- **Remove `Option.some`, `Option.none`** from the `Option` namespace — same, dead code. Pipeline uses `tag("Some")` / `tag("None")`.
- **Remove `Result.schema`, `Option.schema`** from the namespaces — these are handler-authoring concerns, now `resultSchema` / `optionSchema` in `/runtime`.
- **Do NOT re-export from `./runtime.ts`** — the two entry points are independent.

### 6. Update docs and README

#### Add "Imports" section to docs

Add a section (to `docs-website/docs/quickstart.md` or as a standalone `docs-website/docs/imports.md` page) explaining the two import paths:

> **`@barnum/barnum/runtime`** — for handler files. Import `createHandler`, `createHandlerWithConfig`, value constructors (`ok`, `err`, `some`, `none`), schema builders (`resultSchema`, `optionSchema`, `taggedUnionSchema`), and types (`Result`, `Option`, `Handler`).
>
> **`@barnum/barnum/pipeline`** — for pipeline files. Import combinators (`pipe`, `loop`, `tryCatch`, `forEach`, etc.), builtins (`constant`, `identity`, `drop`, etc.), namespaces (`Result`, `Option`), `runPipeline`, and pipeline types (`TypedAction`, `Pipeable`, etc.).
>
> Handler files define what runs — they produce runtime values. Pipeline files define the workflow DAG — they compose `TypedAction` nodes. The two never need each other's exports.

The quickstart should show both imports side-by-side early, so the split is clear from the start. The handlers reference page should import from `/runtime`.

#### Update existing import lines

All docs that import `createHandler` from `@barnum/barnum` need to switch to `@barnum/barnum/runtime`. All pipeline imports switch to `@barnum/barnum/pipeline`.

- `README.md` — handler example imports `createHandler` from `@barnum/barnum`
- `docs-website/docs/index.md` — same
- `docs-website/docs/quickstart.md` — same
- `docs-website/docs/reference/handlers.md` — imports `createHandler` and `createHandlerWithConfig`
- `docs-website/versioned_docs/version-0.3/index.md` — same
- `docs-website/versioned_docs/version-0.3/quickstart.md` — same

Pipeline imports (`runPipeline`, `pipe`, etc.) switch to `@barnum/barnum/pipeline`.

### 7. Update demo handler files

All demo handler files should use `@barnum/barnum/runtime` — demos are best-practice examples. Handler files to migrate:

- `demos/workflow-output/handlers/steps.ts` — `createHandler`
- `demos/analyze-file/handlers/analyze.ts` — `createHandler`
- `demos/simple-workflow/handlers/steps.ts` — `createHandler`
- `demos/identify-and-address-refactors/handlers/git.ts` — `createHandler`
- `demos/convert-folder-to-ts/handlers/convert.ts` — `createHandler`, `createHandlerWithConfig`
- `demos/babysit-prs/handlers/steps.ts` — `createHandler`, `taggedUnionSchema`
- `demos/peano-arithmetic/handlers/steps.ts` — `createHandler`, `taggedUnionSchema`
- `demos/retry-on-error/handlers/steps.ts` — `createHandler`, `Result` (→ `resultSchema`), type `Result`
- `demos/identify-and-address-refactors/handlers/refactor.ts` — `createHandler`, `createHandlerWithConfig`, `taggedUnionSchema`, type `TaggedUnion`
- `demos/identify-and-address-refactors/handlers/type-check-fix.ts` — same
- `demos/convert-folder-to-ts/handlers/type-check-fix.ts` — same

Pipeline files (`run.ts`) switch to `@barnum/barnum/pipeline`.

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

Pipeline files: switch `@barnum/barnum` → `@barnum/barnum/pipeline`.

## Open questions

~~1. Should there be a bare `@barnum/barnum` top-level export?~~ No. Two subpath exports only: `/runtime` and `/pipeline`.
~~2. Should `/runtime` export the `z` from zod for convenience, or let handler authors import zod directly?~~ No. Handler authors import zod directly.
~~3. Should `Result.schema()` / `Option.schema()` be removed from the pipeline namespaces, or kept as aliases?~~ Removed.
