# Getting the Value Out of a Workflow

How does the caller retrieve the final value when a workflow completes?

## Current state

`runPipeline(pipeline)` spawns the Rust CLI via async `spawn` with `stdio: ["inherit", "inherit", "pipe"]` — stdout is inherited (flows to terminal), stderr is piped (captured + forwarded for error reporting). Returns `Promise<void>`.

```ts
// libs/barnum/src/run.ts (current)
export async function runPipeline(
  pipeline: Action,
  input?: unknown,
): Promise<void> {
  const workflow =
    input === undefined
      ? pipeline
      : (chain(constant(input) as Pipeable, pipeline as Pipeable) as Action);
  await spawnBarnum({ workflow });
}

function spawnBarnum(config: Config): Promise<void> {
  // ...
  return new Promise<void>((resolve, reject) => {
    const child = nodeSpawn(binaryResolution.path, [...args], {
      stdio: ["inherit", "inherit", "pipe"],
    });

    const stderrChunks: Buffer[] = [];
    child.stderr?.on("data", (chunk: Buffer) => {
      stderrChunks.push(chunk);
      process.stderr.write(chunk);
    });

    child.on("close", (code) => {
      if (code !== 0) {
        const stderr = Buffer.concat(stderrChunks).toString("utf8").trim();
        reject(new Error(...));
        return;
      }
      resolve();  // <-- void, no result captured
    });
  });
}
```

**Verified**: the Rust CLI does print the final workflow value to stdout as JSON. Running `runPipeline(constant(42))` prints `42` to the terminal. The value is there — we just don't capture it.

## Problem

`runPipeline()` returns `Promise<void>`. No way to:
1. Capture the workflow result as a typed value in TypeScript
2. Assert on the result in tests
3. Use the result in a larger program that orchestrates multiple workflows

## Approach

Flip stdio: capture stdout (the result), let stderr flow through (handler logs).

No Rust changes needed. The Rust CLI already writes the final value to stdout. The worker protocol already requires that handler stdout is exclusively the JSON return value — `console.log` inside a handler corrupts the worker protocol today, regardless of this change. All handler logging must use `console.error` (all demos already do). Each handler runs in a separate subprocess whose stdout the Rust engine captures; the Rust CLI's own stdout is a separate fd used only for the final result.

### Typing

`runPipeline` currently takes `Action` (untyped). To return a typed result, the naive approach `Pipeable<any, TOut>` fails because of phantom field variance — `TypedAction<never, ...>` (handlers with no input) has `__in?: (input: never) => void` which is not assignable to `__in?: (input: any) => void`.

The fix: constrain on `Action` and use `ExtractOutput` (already exists in `ast.ts`):

```ts
export async function runPipeline<TPipeline extends Action>(
  pipeline: TPipeline,
  input?: unknown,
): Promise<ExtractOutput<TPipeline>>
```

This infers the output type from the phantom `__out` field without constraining `__in`.

## Before / After

### `libs/barnum/src/run.ts`

```ts
// BEFORE
import type { Action, Config, Pipeable } from "./ast.js";

export async function runPipeline(
  pipeline: Action,
  input?: unknown,
): Promise<void> {
  const workflow = ...;
  await spawnBarnum({ workflow });
}

function spawnBarnum(config: Config): Promise<void> {
  // ...
  return new Promise<void>((resolve, reject) => {
    const child = nodeSpawn(binaryResolution.path, [...args], {
      stdio: ["inherit", "inherit", "pipe"],
    });

    const stderrChunks: Buffer[] = [];
    child.stderr?.on("data", (chunk: Buffer) => {
      stderrChunks.push(chunk);
      process.stderr.write(chunk);
    });

    child.on("close", (code) => {
      if (code !== 0) {
        const stderr = Buffer.concat(stderrChunks).toString("utf8").trim();
        const message = stderr
          ? `barnum exited with code ${code}:\n${stderr}`
          : `barnum exited with code ${code} (no stderr output)`;
        reject(new Error(message));
        return;
      }
      resolve();
    });
  });
}

// AFTER
import type { Action, Config, ExtractOutput, Pipeable } from "./ast.js";

export async function runPipeline<TPipeline extends Action>(
  pipeline: TPipeline,
  input?: unknown,
): Promise<ExtractOutput<TPipeline>> {
  const workflow = ...;
  return spawnBarnum({ workflow });
}

function spawnBarnum<TOut>(config: Config): Promise<TOut> {
  // ...
  return new Promise<TOut>((resolve, reject) => {
    const child = nodeSpawn(binaryResolution.path, [...args], {
      stdio: ["inherit", "pipe", "inherit"],  // flip: capture stdout, inherit stderr
    });

    const stdoutChunks: Buffer[] = [];
    child.stdout?.on("data", (chunk: Buffer) => {
      stdoutChunks.push(chunk);
    });

    child.on("close", (code) => {
      if (code !== 0) {
        reject(new Error(`barnum exited with code ${code}`));
        return;
      }
      const stdout = Buffer.concat(stdoutChunks).toString("utf8").trim();
      if (!stdout) {
        resolve(null as TOut);
        return;
      }
      try {
        resolve(JSON.parse(stdout) as TOut);
      } catch {
        reject(new Error(`barnum produced non-JSON output on stdout: ${stdout}`));
      }
    });
  });
}
```

### Key changes

1. **stdio flip**: `["inherit", "inherit", "pipe"]` → `["inherit", "pipe", "inherit"]`. Capture stdout (result), inherit stderr (handler logs go directly to terminal).
2. **Collect stdout**: replace `stderrChunks` with `stdoutChunks`, parse as JSON on close.
3. **Return value**: `resolve(JSON.parse(stdout))` instead of `resolve()`.
4. **Typed signature**: `<TPipeline extends Action>` + `ExtractOutput<TPipeline>` for the return type.
5. **Error reporting**: stderr is no longer captured — it flows directly to terminal via `"inherit"`. Error messages just report the exit code. (Handler logs are already on stderr, so they remain visible.)

### Demo callers — no changes required

Existing demos that ignore the return value continue to work:

```ts
// Still valid — just ignores the returned promise value
runPipeline(pipe(setup, listFiles.forEach(migrate).drop(), typeCheckFix));
```

Callers that want the result can now capture it:

```ts
const result = await runPipeline(pipe(constant({ x: 1 }), transform));
console.log(result);  // typed as ExtractOutput<typeof pipeline>
```
