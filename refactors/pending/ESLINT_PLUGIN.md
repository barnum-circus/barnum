# ESLint Plugin

## Motivation

Two common mistakes cause silent, hard-to-diagnose failures:

1. **Fork bomb.** Defining a handler in the same file that calls `runPipeline` causes exponential subprocess spawning when the framework imports the handler module.
2. **Silent `any`.** Calling `loop`, `earlyReturn`, or `defineRecursiveFunctions` without explicit type parameters produces `any` output, silently disabling type checking for the rest of the pipeline.

Both are documented in best practices, but documentation doesn't prevent mistakes. A lint rule catches them at edit time.

## Rules

### `barnum/no-handler-with-run-pipeline`

**Error** if a file contains both a `createHandler` (or `createHandlerWithConfig`) call and a `runPipeline` call.

```ts
// ERROR: handler and runPipeline in same file
import { createHandler } from "@barnum/barnum/runtime";
import { runPipeline } from "@barnum/barnum/pipeline";

export const analyze = createHandler({ ... }, "analyze");
runPipeline(pipe(analyze, report)); // ← lint error here
```

The constraint is specifically about top-level `runPipeline` calls — a `runPipeline` that executes on module load. An exported function that *contains* `runPipeline` is fine (it doesn't execute on import). The rule should only flag `runPipeline` calls that are at module scope (top-level statements, not inside a function/arrow/method body).

Detection: check if a file contains both a `createHandler`/`createHandlerWithConfig` call (anywhere) and a `runPipeline` call at module scope (not nested inside a function expression, arrow function, or method). Report on the `runPipeline` call.

Error message: `"Top-level runPipeline in the same file as a handler definition causes a fork bomb — importing the handler re-triggers the pipeline. Move the handler to a separate file, or wrap runPipeline in a function that isn't called on import."`

### `barnum/require-type-params`

**Error** if `loop`, `earlyReturn`, or `defineRecursiveFunctions` is called without explicit type arguments.

```ts
// ERROR: missing type parameter
loop((recur, done) => ...);
earlyReturn((ret) => ...);

// OK: type parameter provided
loop<Result>((recur, done) => ...);
earlyReturn<ErrorReport>((ret) => ...);
```

Detection: for each `CallExpression` where the callee is `loop`, `earlyReturn`, or `defineRecursiveFunctions`, check that `node.typeArguments` (or `node.typeParameters` depending on parser) is present and non-empty. Report on the call expression.

Error message: `"'loop' requires explicit type parameters. Without them, the output type is 'any'. Add type arguments: loop<TBreak>(...)"`

## Package structure

```
libs/eslint-plugin-barnum/
├── package.json
├── tsconfig.json
├── src/
│   ├── index.ts              # plugin entry, exports rules + recommended config
│   ├── rules/
│   │   ├── no-handler-with-run-pipeline.ts
│   │   └── require-type-params.ts
│   └── utils.ts              # shared AST helpers if needed
└── tests/
    ├── no-handler-with-run-pipeline.test.ts
    └── require-type-params.test.ts
```

Published as `@barnum/eslint-plugin`. The recommended config enables both rules as errors.

## Open questions

1. **Should `require-type-params` also cover `defineRecursiveFunctions`?** The type annotation there is more complex (function signatures in a tuple). Might be hard to lint for "sufficient" type information vs just "has type args." Could start with `loop` and `earlyReturn` only.

2. **Import aliasing.** If someone writes `import { loop as myLoop } from "..."`, should the rule track the alias? ESLint's scope analysis handles this, but it adds complexity. Pragmatic answer: don't bother — our codebase doesn't alias these.

3. **`@typescript-eslint/parser` dependency.** `require-type-params` needs the TypeScript AST (to check `typeArguments`). This means the plugin depends on `@typescript-eslint/parser`. That's fine — we already use it.
