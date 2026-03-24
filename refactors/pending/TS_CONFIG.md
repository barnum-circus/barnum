# TypeScript Config Architecture

**Supersedes:** JS_ACTION_RESOLUTION.md, JS_ACTION_HANDLERS.md (those docs describe an intermediate design; this is the target)

## Overview

The config is a TypeScript file that exports a config object. The CLI takes a path to this file, evaluates it, and passes the serialized config to Rust. Rust handles scheduling, concurrency, retries, and timeouts. Handlers live in JavaScript.

There are two action kinds: Bash and TypeScript. A Bash action is a shell script string. A TypeScript action is a path to a `.ts` handler file. Both are serialized functions — strings that Rust can execute as subprocesses. The TypeScript handler file path becomes a bash command (`tsx ./handler.ts`) at dispatch time.

## Config file format

The config file is a TypeScript script that creates a config and calls `.run()`:

```typescript
// barnum.config.ts
import { BarnumConfig } from "@barnum/barnum";

const config = BarnumConfig.fromConfig({
  entrypoint: "Analyze",
  steps: [
    {
      name: "Analyze",
      action: {
        kind: "TypeScript",
        params: {
          path: "./handlers/analyze.ts",
          instructions: "Analyze the code.",
          pool: "demo",
        },
      },
      next: ["FanOut"],
      value_schema: { type: "object", properties: { file: { type: "string" } }, required: ["file"] },
    },
    {
      name: "FanOut",
      action: {
        kind: "Bash",
        params: { script: "jq -r '.value.files[]' | xargs -I{} echo '{\"kind\": \"Analyze\", \"value\": {\"file\": \"'{}'\"}}'  | jq -s ." },
      },
      next: ["Analyze"],
    },
  ],
});

config.run({ entrypointValue: '{"file": "src/main.rs"}' });
```

The user runs this file directly: `tsx barnum.config.ts`. The file IS the entry point — it creates the config, calls `.run()`, and `.run()` spawns Rust.

`BarnumConfig.fromConfig()` Zod-parses and returns a `BarnumConfig` instance. Step constructors (like `createTroupeStep` and `createBashStep`) are convenience helpers defined elsewhere — they produce `StepFile` objects with the action pre-filled. This doc describes the underlying config shape they produce, not the constructors themselves.

### How the config reaches Rust

`BarnumConfig.run()` (defined in `libs/barnum/run.ts`) serializes the config to JSON and passes it to the Rust binary via `--config`. This is the existing behavior — `run()` already calls `JSON.stringify(this.config)` and spawns the binary with `["run", "--config", <json>]`.

The change: `.run()` also injects the `--executor` flag (runtime-aware executor command, resolved from cli.cjs). This is the EXECUTOR_CLI_FLAG work from JS_ACTION_RESOLUTION.md.

JSON configs (`.json`/`.jsonc`) still work via the CLI: `barnum run --config config.json`. Rust parses those directly.

## Action kinds

### Bash

Runs a shell script. Rust handles this directly — `sh -c <script>`, piping `{ kind, value }` to stdin. The handler is the script string itself.

Config shape:
```json
{ "kind": "Bash", "params": { "script": "jq -r '.value.files[]' | xargs -I{} echo '{\"kind\": \"Analyze\", \"value\": {\"file\": \"'{}'\"}}'  | jq -s ." } }
```

Stdin (what the script receives):
```json
{ "kind": "Analyze", "value": { "file": "src/main.rs" } }
```

Stdout (what the script must produce):
```json
[{ "kind": "Implement", "value": { "plan": "..." } }]
```

This is today's `Command` action renamed. The stdin/stdout contract is unchanged.

### TypeScript

Runs a TypeScript handler file as a subprocess. Rust constructs the command `<executor> <handler_path>` and pipes the full envelope to stdin. The executor command comes from the `--executor` flag (injected by cli.cjs, hidden from users).

Config shape:
```json
{
  "kind": "TypeScript",
  "params": {
    "path": "./handlers/analyze.ts",
    "export": "default",
    "instructions": "Analyze the code.",
    "pool": "demo",
    "timeout": 300
  }
}
```

`path` and `export` are dispatch params — Rust uses them to construct the subprocess command. Everything else is handler config — Rust stores it as opaque JSON and passes it through in the envelope. The handler reads what it needs from `action.params`.

`export` defaults to `"default"`. Named exports are supported for modules that export multiple handlers.

### Dispatch

From Rust's perspective, both action kinds produce a subprocess command:

- Bash: `sh -c <script>`, stdin = `{ kind, value }`
- TypeScript: `<executor> libs/barnum/actions/run-handler.ts <path> [export]`, stdin = `{ action, task, step, config }`

The stdin formats differ because Bash targets user-written shell scripts (simple contract) while TypeScript targets handler modules (rich context). `run-handler.ts` is a thin wrapper that imports the handler module, calls the exported function with the parsed envelope, and writes the result to stdout.

## Handler interface

A TypeScript handler is a module that exports an async function:

```typescript
// handlers/analyze.ts
import type { HandlerContext, FollowUpTask } from "@barnum/barnum";

export default async function handle(ctx: HandlerContext): Promise<FollowUpTask[]> {
  // ctx.params — action.params from the envelope (handler-specific config)
  // ctx.task — { kind: string, value: unknown }
  // ctx.step — { name, next, options, ... }
  // ctx.config — full resolved config
  return [{ kind: "Implement", value: { plan: "..." } }];
}
```

`HandlerContext` is:

```typescript
interface HandlerContext {
  params: Record<string, unknown>;
  task: Task;
  step: Step;
  config: Config;
}
```

`params` is `action.params` from the envelope with `path` and `export` stripped (the handler doesn't need dispatch metadata). The types `Task`, `Step`, `Config` come from the generated `barnum-resolved-schema.zod.ts`.

The handler is re-invoked as a fresh process for every task. No state persists between invocations.

### run-handler.ts

The thin wrapper that bridges the subprocess boundary:

```typescript
// libs/barnum/actions/run-handler.ts
import type { FollowUpTask } from "./types.js";

const [handlerPath, exportName = "default"] = process.argv.slice(2);

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope = JSON.parse(Buffer.concat(chunks).toString());

const mod = await import(handlerPath);
const handler = mod[exportName];

const { path: _, export: __, ...handlerParams } = envelope.action.params;
const results: FollowUpTask[] = await handler({
  params: handlerParams,
  task: envelope.task,
  step: envelope.step,
  config: envelope.config,
});

process.stdout.write(JSON.stringify(results));
```

This replaces the `executor.ts` from JS_ACTION_HANDLERS.md. The difference: instead of a hardcoded handler registry with a switch statement, it dynamically imports the handler module specified by the action params. Each handler file is self-contained.

## Invocation

The config file is a TypeScript script. The user runs it directly:

```bash
tsx barnum.config.ts
```

`.run()` accepts options for entrypoint value, resume, logging, etc. CLI argument parsing (if desired) is the user's responsibility — they control the script.

### Validation

`BarnumConfig` can also expose a `.validate()` method that checks the config without running the workflow:

```typescript
const errors = config.validate();
```

Structural validation (Zod parse) already happens in `fromConfig()`. `.validate()` can additionally:

1. Verify all handler paths resolve (TypeScript actions point to existing files)
2. Verify all handler modules export the expected function
3. Generate TypeScript files that type-check handler signatures against value schemas, run tsc on them
4. Verify step graph connectivity (all `next` references point to existing steps)

The TypeScript-level validation (point 3) is a future capability. The basic structural validations (points 1, 2, 4) are straightforward.

## Rust changes

### New action kinds

Replace `Pool` and `Command` with `Bash` and `TypeScript`:

```rust
pub enum ActionFile {
    Bash { script: String },
    TypeScript {
        path: String,
        #[serde(default)]
        export: Option<String>,
        #[serde(flatten)]
        handler_config: serde_json::Map<String, serde_json::Value>,
    },
}
```

In the resolved config, the same structure. `handler_config` is opaque — Rust stores it and passes it through in the envelope.

### Dispatch changes

`dispatch_task` no longer matches on action kind. For Bash, it constructs a `ShellAction` with `sh -c <script>`. For TypeScript, it constructs a `ShellAction` with `<executor> run-handler.ts <path> [export]`. Both produce `ShellAction` — the existing action dispatch infrastructure (`spawn_worker`, `run_action`, `ActionHandle`, `ProcessGuard`) is unchanged.

### Config loading

The Rust binary's `--config` flag accepts a JSON string (already the case). cli.cjs handles the .ts → JSON conversion before invoking Rust. No Rust changes needed for config loading.

### Dead code removal

Once TypeScript actions replace Pool:
- Delete `PoolAction` in `runner/action.rs`
- Delete `submit.rs` (troupe submission)
- Delete `Invoker<TroupeCli>` from `Engine` and `RunnerConfig`
- Delete `generate_step_docs` from `docs.rs` (moved to JS)
- Delete `Config::has_pool_actions()` from `resolved.rs`

## What stays the same

- `spawn_worker`, `run_action`, `ActionHandle`, `ProcessGuard` — generic action dispatch
- `RunState` and state machine logic (task tracking, child counting, finally hooks)
- `CompiledSchemas` and value schema validation (value_schema field on steps)
- State log format
- `barnum config docs`, `barnum config graph` — operate on config types
- Finally hooks — direct ShellAction with hook script, same envelope format
- Resume behavior — state log contains serialized config, `--executor` re-provided at resume
