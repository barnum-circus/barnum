# Getting the Value Out of a Workflow

How does the caller retrieve the final value when a workflow completes?

## Current state

`runPipeline(pipeline)` calls the Rust CLI via `spawn` with `stdio: ["inherit", "inherit", "pipe"]`. The Rust event loop prints the final value to stdout as JSON. The TypeScript side doesn't capture or return it — `runPipeline()` returns `Promise<void>`.

```ts
// run.ts
export function run(config: Config): void {
  execFileSync(binary, [
    "run", "--config", configJson, "--executor", executor, "--worker", worker,
  ], { stdio: "inherit" });
}
```

The final workflow value is printed to stdout but not programmatically accessible to the caller.

## Problem

There's no way to:
1. Capture the workflow result as a typed value in TypeScript
2. Assert on the result in tests
3. Use the result in a larger program that orchestrates multiple workflows

## Options

### Option A: Capture stdout

`run()` changes `stdio` from `"inherit"` to `"pipe"`, captures stdout, and parses the JSON result.

```ts
export function run<Out>(config: Config<Out>): Out {
  const result = execFileSync(binary, [...args], { encoding: "utf-8" });
  return JSON.parse(result);
}
```

Pros: Simple, no Rust changes needed.
Cons: Handler `console.log` output would pollute the result (handlers should use `stderr` for logging). The Rust side must guarantee the final stdout write is valid JSON.

### Option B: Result file

The Rust CLI writes the result to a temp file, and `run()` reads it.

```ts
export function run<Out>(config: Config<Out>): Out {
  const resultFile = path.join(os.tmpdir(), `barnum-result-${Date.now()}.json`);
  execFileSync(binary, [...args, "--result-file", resultFile], { stdio: "inherit" });
  const result = readFileSync(resultFile, "utf-8");
  unlinkSync(resultFile);
  return JSON.parse(result);
}
```

Pros: stdout stays clean for handler logging. Clear separation of result vs log output.
Cons: Filesystem overhead, temp file cleanup.

### Option C: Structured protocol on stdout

The Rust CLI wraps all output in a protocol: handler log lines get `LOG:` prefix, the final result gets `RESULT:` prefix. The TypeScript side parses the protocol.

Pros: Single channel, no temp files.
Cons: Complex, fragile, handlers need to cooperate with the protocol.

### Option D: stderr for logs, stdout for result only

Enforce that handlers use `stderr` for logging (they already do via `console.error`). The Rust CLI only writes the final result to stdout. `run()` captures stdout.

This is essentially Option A with a convention that handlers never write to stdout (which the worker protocol already enforces — handler stdout is the return value).

Wait — the worker already uses stdout for the handler return value. The Rust event loop captures that per-handler. The Rust CLI's own stdout is separate. So the Rust CLI can print the final result to its stdout without conflicting with handler output.

The only issue: `stdio: "inherit"` means the Rust process's stdout/stderr go to the terminal. Change to capture stdout, pipe stderr.

## Recommendation

**Option A / D hybrid**: `run()` captures the Rust CLI's stdout (the final result) and pipes stderr through (handler logs visible in terminal).

```ts
export function run<Out>(config: Config<Out>): Out {
  const stdout = execFileSync(binary, [...args], {
    encoding: "utf-8",
    stdio: ["inherit", "pipe", "inherit"],  // stdin: inherit, stdout: capture, stderr: inherit
  });
  return JSON.parse(stdout);
}
```

The Rust CLI already prints the final result to stdout. We just need to capture it instead of letting it flow to the terminal.

`run()` becomes synchronous and returns the typed result. The `Config<Out>` type parameter carries through.

## API change

```ts
// Before
runPipeline(pipeline);  // Promise<void>

// After
const result = await runPipeline(pipeline);  // typed Out
```

`runPipeline()` changes from `Promise<void>` to `Promise<Out>`.

## Testing implications

With a return value, workflows become testable:

```ts
const result = await runPipeline(
  pipe(constant({ x: 1 }), transform),
);
expect(result).toEqual({ x: 2 });
```

## Open question: async vs sync

Currently `spawnBarnum()` uses `spawn` (async). `runPipeline()` awaits the child process completion.

For long-running workflows, a truly async `run()` (using `execFile` instead of `execFileSync`) would be better. But that's a separate concern from capturing the result.
