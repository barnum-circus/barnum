# TypeScript Handler Invocation

The final step to a working proof of concept: actually executing TypeScript handlers and feeding results back to the engine.

**Depends on:** COMPLETION.md (advance/complete cycle), ENGINE.md (design)

**Scope:** A minimal runtime that takes dispatches from the engine, executes TypeScript handlers, and delivers results back via `on_task_completed`. No persistence, no restart, no scheduling — just the synchronous dispatch/execute/complete loop.

## Current state

The engine produces `Dispatch { handler_id, value }`. The caller resolves `handler_id` to a `HandlerKind::TypeScript { module, func, .. }` via `engine.handler(id)`. But nothing actually runs the TypeScript.

On the TypeScript side, `createHandler()` in `libs/barnum/src/handler.ts` captures:
- `__filePath`: absolute path to the handler module (captured via V8 stack trace)
- `__exportName`: the exported function name
- `__definition.handle`: the actual `async (context) => result` function

The Rust side sees `module` (file path) and `func` (export name) in the serialized config. It needs to load that module, call that export, and get JSON back.

## Architecture: single long-lived Node.js process

Spawn one Node.js subprocess at startup. Communicate via stdin/stdout using NDJSON (one JSON object per line).

Why a single process instead of one per dispatch:
- Module loading is expensive. A persistent process caches `require()`/`import()` results.
- Parallel dispatches share the same process — just send multiple requests, collect responses keyed by task ID.
- No subprocess spawn overhead per handler call.

### Protocol

**Rust → Node.js (stdin):** dispatch request

```json
{"taskId": 0, "module": "/app/handlers/setup.ts", "func": "setup", "value": {"project": "my-app"}}
```

**Node.js → Rust (stdout):** task result

```json
{"taskId": 0, "status": "success", "value": {"initialized": true}}
```

or

```json
{"taskId": 0, "status": "failure", "error": "TypeError: Cannot read properties of undefined"}
```

### Node.js worker script

A small harness (`libs/barnum/src/worker.ts` or `worker.cjs`) that:

1. Reads NDJSON lines from stdin.
2. For each line: dynamically imports the module, looks up the export, calls it with `{ value }`.
3. Writes the result as a JSON line to stdout.
4. Handles errors (import failure, export not found, handler throws) as `failure` results.

```typescript
import { createInterface } from "readline";

const rl = createInterface({ input: process.stdin });

for await (const line of rl) {
    const { taskId, module, func, value } = JSON.parse(line);
    try {
        const mod = await import(module);
        const handler = mod[func];
        if (!handler?.__definition?.handle) {
            throw new Error(`${module}:${func} is not a barnum handler`);
        }
        const result = await handler.__definition.handle({ value, stepConfig: {} });
        console.log(JSON.stringify({ taskId, status: "success", value: result }));
    } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        console.log(JSON.stringify({ taskId, status: "failure", error }));
    }
}
```

### Rust side: subprocess management

In `barnum_cli` (or a new `barnum_runtime` crate if needed):

```rust
struct TypeScriptRuntime {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}
```

- Spawn `node worker.cjs` (or `tsx worker.ts`) at startup.
- `dispatch(task_id, module, func, value)` → write JSON line to stdin.
- `recv() -> (TaskId, Result<Value, String>)` → read JSON line from stdout, parse.

### The main loop

```
let flat_config = flatten(config)?;
let mut engine = Engine::new(flat_config);
engine.start(input)?;

loop {
    let dispatches = engine.take_pending_dispatches();
    if dispatches.is_empty() {
        // Workflow complete or stuck — shouldn't happen if engine is correct.
        break;
    }
    for dispatch in &dispatches {
        let handler = engine.handler(dispatch.handler_id);
        let HandlerKind::TypeScript(ts) = handler;
        runtime.dispatch(dispatch.task_id, &ts.module, &ts.func, &dispatch.value);
    }
    // Wait for results. For POC, just read one at a time.
    for _ in 0..dispatches.len() {
        let (task_id, result) = runtime.recv();
        if let Some(terminal) = engine.on_task_completed(task_id, result) {
            // Workflow done.
            return terminal;
        }
    }
}
```

This is synchronous and blocking. It dispatches all pending tasks, waits for all of them to complete, then repeats. Good enough for a POC. A real scheduler would use async I/O and not block on all dispatches completing before processing results.

**Important subtlety:** `on_task_completed` can produce new dispatches (Chain trampoline, Loop re-enter). The inner loop should drain dispatches after each `on_task_completed` call, not assume the count matches the original batch. Revised:

```
loop {
    let dispatches = engine.take_pending_dispatches();
    if dispatches.is_empty() {
        break;
    }
    for dispatch in &dispatches {
        let handler = engine.handler(dispatch.handler_id);
        let HandlerKind::TypeScript(ts) = handler;
        runtime.dispatch(dispatch.task_id, &ts.module, &ts.func, &dispatch.value);
    }
    // Read ONE result at a time and check for new dispatches.
    let (task_id, result) = runtime.recv();
    if let Some(terminal) = engine.on_task_completed(task_id, result) {
        return terminal;
    }
    // Loop back — take_pending_dispatches will pick up any newly produced dispatches.
}
```

## What this does NOT cover

- **Async/concurrent dispatch execution:** The POC processes one result at a time. A real runtime would use tokio + async subprocess I/O.
- **Step config:** Handlers can receive `stepConfig` in addition to `value`. The POC passes `stepConfig: {}`.
- **Value/config validation:** Zod validators exist on handlers but aren't invoked. The POC trusts the types.
- **Bash handlers:** Only TypeScript handlers. Bash handlers (DEFERRED_FEATURES.md) would need a different execution path.
- **Timeouts:** No handler timeout. A hung handler blocks forever.
- **Persistence/restart:** No state persistence. If the process dies, the workflow is lost.
- **Module resolution:** The POC assumes `module` paths are absolute and directly importable by Node.js. TypeScript files need `tsx` or compilation.

## Implementation order

1. Write the Node.js worker script (`libs/barnum/src/worker.ts`).
2. Add `TaskId` and `task_to_parent` to the engine (from COMPLETION.md).
3. Implement `complete` and `error` (from COMPLETION.md).
4. Write the Rust subprocess wrapper (`TypeScriptRuntime`).
5. Wire the main loop in `barnum_cli`.
6. Test end-to-end with a simple Chain(A, B) workflow.

## Test plan

Manual end-to-end: a TypeScript config that chains two handlers. First handler returns a transformed value, second handler receives it. Assert the final workflow result matches expectations.

```typescript
// test-workflow.ts
import { config, pipe, createHandler } from "barnum";

const double = createHandler({
    stepValueValidator: z.number(),
    handle: async ({ value }) => value * 2,
}, "double");

const addTen = createHandler({
    stepValueValidator: z.number(),
    handle: async ({ value }) => value + 10,
}, "addTen");

export default config(pipe(double(), addTen()));
// Input: 5 → double → 10 → addTen → 20
```

```
$ barnum run --config "$(tsx -e 'import c from "./test-workflow"; console.log(JSON.stringify(c))')" --input 5
20
```
