# Add Run Handler

**Parent:** TS_CONFIG.md
**Depends on:** ADD_TYPESCRIPT_DISPATCH

## Motivation

ADD_TYPESCRIPT_DISPATCH makes Rust dispatch TypeScript actions as `{executor} {run_handler_path} {handler_path} {export_name}`, piping the envelope to stdin. This refactor implements `run-handler.ts` — the actual bridge that imports a user's TypeScript handler and calls it.

No validation occurs in run-handler.ts. Validation is a Rust concern.

## run-handler.ts

**File:** `libs/barnum/actions/run-handler.ts` (replace placeholder)

Rust invokes this as: `node /path/to/tsx/cli.mjs /path/to/run-handler.ts /abs/path/to/handler.ts default`

The script reads the envelope from stdin, imports the handler module, calls the exported function, and writes the result to stdout.

```typescript
const [handlerPath, exportName = "default"] = process.argv.slice(2);

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope = JSON.parse(Buffer.concat(chunks).toString());

const mod = await import(handlerPath);
const handler = mod[exportName];

const results = await handler.handle({
  value: envelope.value,
  config: envelope.config,
  stepName: envelope.stepName,
  stepConfig: envelope.stepConfig ?? {},
});

process.stdout.write(JSON.stringify(results));
```

If the handler throws, the process exits non-zero with the error on stderr. Rust treats this as a failed action.

## Demo

Add a TypeScript handler demo alongside the existing Bash demos:

**File:** `crates/barnum_cli/demos/typescript-handler/handler.ts`

```typescript
export default {
  async handle({ value }) {
    return [{ kind: "Done", value: { greeting: `Hello, ${value.name}!` } }];
  },
};
```

**File:** `crates/barnum_cli/demos/typescript-handler/barnum.config.ts`

A config with a TypeScript action step that uses the handler above, verifying the full path works end-to-end: `run.ts` → Rust → executor → `run-handler.ts` → `handler.ts`.

## What this does NOT do

- Does not define TypeScript types for the handler interface (ADD_HANDLER_VALIDATION)
- Does not validate handler inputs/outputs — that's a Rust concern
- Does not implement executor discovery (tsx vs bun vs node) — already handled by `run.ts` and `cli.cjs`
