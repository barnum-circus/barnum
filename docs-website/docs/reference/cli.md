# CLI Reference

Barnum workflows are TypeScript programs. There is no separate CLI to learn — you write a `.ts` file, import combinators, and call `runPipeline()`.

## Running a workflow

```ts
// run.ts
import { runPipeline } from "barnum";
import { listFiles, processFile, commit } from "./handlers.js";

await runPipeline(
  listFiles.iterate().map(processFile).collect().drop().then(commit),
);
```

```bash
tsx run.ts
```

Or via a package.json script:

```json
{
  "scripts": {
    "demo": "tsx run.ts"
  }
}
```

```bash
pnpm demo
```

## What `runPipeline()` does

1. Serializes the pipeline AST to JSON.
2. Resolves the Rust `barnum` binary (see [Binary resolution](#binary-resolution)).
3. Spawns `barnum run --config <json> --executor <tsx-path> --worker <worker-path>`.
4. The Rust scheduler orchestrates execution, spawning TypeScript worker subprocesses for each handler invocation.
5. Exits when the pipeline completes.

```ts
async function runPipeline(
  pipeline: Action,
  input?: unknown,
): Promise<void>
```

If `input` is provided, it is prepended as a `constant` node at the start of the pipeline.

## Binary resolution

`runPipeline()` resolves the `barnum` Rust binary in this order:

1. **`BARNUM` environment variable** — explicit path to the binary.
2. **Local dev repo** — `target/debug/barnum` relative to the repo root.
3. **node_modules artifact** — platform-specific binary from `@barnum/barnum/artifacts/<platform>/<arch>/barnum`.

Supported platforms: `macos-arm64`, `macos-x64`, `linux-arm64`, `linux-x64`, `win-x64`.

## TypeScript executor resolution

The Rust binary spawns TypeScript workers. The executor is resolved as:

1. If running under Bun (`process.versions.bun` is set), uses `bun` directly.
2. Otherwise, resolves `tsx/cli` from node_modules and runs as `node <tsx-path>`.

## `callClaude()`

A utility for invoking an LLM from within handlers. Spawns a Claude CLI subprocess.

```ts
async function callClaude(args: {
  prompt: string;
  allowedTools?: string[];
  cwd?: string;
}): Promise<string>
```

- `prompt` — the instruction to send.
- `allowedTools` — restrict which tools the agent can use (e.g., `["Bash", "Read"]`).
- `cwd` — working directory for the subprocess.

Returns the agent's text output as a string.

```ts
const result = await callClaude({
  prompt: `Review ${filePath} for security issues`,
  allowedTools: ["Read"],
});
```

## Environment variables

| Variable | Description |
|---|---|
| `BARNUM` | Override the path to the Rust `barnum` binary |

## Internal: `barnum run`

The Rust binary's `run` command is not user-facing — `runPipeline()` calls it internally. Documented here for debugging.

```
barnum run --config <JSON> --executor <PATH> --worker <PATH>
```

- `--config` — the serialized pipeline AST as JSON.
- `--executor` — path to the TypeScript executor (tsx or bun).
- `--worker` — path to `worker.ts`, which handles subprocess communication.

## Internal: `barnum config`

Diagnostic subcommands for inspecting pipeline configs.

```bash
# Validate a config
barnum config validate --config config.json

# Generate markdown documentation
barnum config docs --config config.json

# Generate DOT graph (for GraphViz)
barnum config graph --config config.json | dot -Tpng -o workflow.png

# Print the config schema
barnum config schema
barnum config schema --type json
```
