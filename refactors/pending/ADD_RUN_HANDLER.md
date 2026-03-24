# Add Run Handler

**Parent:** TS_CONFIG.md
**Depends on:** ADD_TYPESCRIPT_DISPATCH, ADD_HANDLER_VALIDATION

## Motivation

ADD_TYPESCRIPT_DISPATCH makes Rust dispatch TypeScript actions as `{executor} {run_handler_path} {handler_path} {export_name}`, piping the envelope to stdin. ADD_HANDLER_VALIDATION defines the handler interface and validation contract. This refactor implements the bridge: `run-handler.ts` (the subprocess entry point that imports and calls handlers) and the `run.ts` changes that inject `--executor` and `--run-handler-path` when spawning Rust.

## run-handler.ts

**File:** `libs/barnum/actions/run-handler.ts` (new file)

Rust invokes this as: `pnpm dlx tsx libs/barnum/actions/run-handler.ts /abs/path/to/handler.ts default`

The script reads the envelope from stdin, imports the handler module, runs validation, calls `handle`, and writes the result to stdout.

```typescript
const [handlerPath, exportName = "default"] = process.argv.slice(2);

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope = JSON.parse(Buffer.concat(chunks).toString());

const mod = await import(handlerPath);
const definition = mod[exportName];

// 1. Validate step config
const stepConfig = definition.stepConfigValidator.parse(envelope.stepConfig);

// 2. Validate value (schema can depend on step config)
const valueValidator = definition.getStepValueValidator(stepConfig);
const value = valueValidator.parse(envelope.value);

// 3. Call handler
const results = await definition.handle({
  stepConfig,
  value,
  config: envelope.config,
  stepName: envelope.stepName,
});

process.stdout.write(JSON.stringify(results));
```

The envelope fields match `HandlerContext` directly (after UNIFY_STDIN_ENVELOPE established the shape). The only work run-handler.ts does before calling `handle` is validation — parsing `stepConfig` and `value` through their Zod schemas. The `config` and `stepName` fields pass through unchanged.

If validation fails, Zod throws and the process exits non-zero with the error on stderr. Rust treats this as a failed action.

## run.ts changes

**File:** `libs/barnum/run.ts`

`.run()` injects `--executor` and `--run-handler-path` when spawning the Rust binary:

```typescript
run(opts?: RunOptions): ChildProcess {
  const args = opts?.resumeFrom
    ? ["run", "--resume-from", opts.resumeFrom]
    : ["run", "--config", JSON.stringify(this.config)];

  const runHandlerPath = new URL("./actions/run-handler.ts", import.meta.url).pathname;
  args.push("--executor", "pnpm dlx tsx");
  args.push("--run-handler-path", runHandlerPath);

  // ... rest of opts handling
  return spawnBarnum(args);
}
```

The executor command is `pnpm dlx tsx`. When invoked via `tsx barnum.config.ts`, the TS runtime is already available. The exact executor discovery logic (tsx vs bun vs node) can be refined later.

Both flags are required by the Rust CLI (ADD_TYPESCRIPT_DISPATCH). The JS layer always provides them.

## Tests

### Unit test for run-handler.ts

Create a test handler file and pipe a valid envelope through run-handler.ts:

```typescript
// test-handler.ts
import { z } from "zod";
export default {
  stepConfigValidator: z.object({ greeting: z.string() }),
  getStepValueValidator() { return z.object({ name: z.string() }); },
  async handle({ stepConfig, value }) {
    return [{ kind: "Next", value: { message: `${stepConfig.greeting}, ${value.name}` } }];
  },
};
```

```bash
echo '{"stepConfig":{"greeting":"Hello"},"value":{"name":"World"},"config":{},"stepName":"Test"}' | \
  pnpm dlx tsx libs/barnum/actions/run-handler.ts ./test-handler.ts default
# Expected: [{"kind":"Next","value":{"message":"Hello, World"}}]
```

### Validation failure test

Pipe an envelope with invalid stepConfig and verify non-zero exit:

```bash
echo '{"stepConfig":{},"value":{"name":"World"},"config":{},"stepName":"Test"}' | \
  pnpm dlx tsx libs/barnum/actions/run-handler.ts ./test-handler.ts default
# Expected: exit 1, Zod error on stderr (missing "greeting")
```

### Integration test

An end-to-end test with a TypeScript action in a Barnum config, verifying the full path: Rust dispatches the subprocess, run-handler.ts validates inputs, calls the handler, and Rust processes the follow-up tasks. This requires a TS runtime and belongs in the integration test suite or as a demo.

## What this does NOT do

- Does not change the Rust dispatch logic (ADD_TYPESCRIPT_DISPATCH)
- Does not define the handler types (ADD_HANDLER_VALIDATION)
- Does not implement executor discovery (tsx vs bun vs node) — hardcodes `pnpm dlx tsx`
- Does not implement `.validate()` on `BarnumConfig`
