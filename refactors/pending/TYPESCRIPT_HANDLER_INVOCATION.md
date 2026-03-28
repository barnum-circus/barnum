# TypeScript Handler Invocation

Executing TypeScript handlers from the Rust runtime and feeding results back to WorkflowState.

**Depends on:** RUNTIME.md (Scheduler + run_workflow loop)

**Scope:** Subprocess management, the worker script, the stdin/stdout protocol. Errors panic the workflow for now — no structured error handling.

## Architecture: one subprocess per dispatch

Match master's approach: spawn one short-lived Node.js process per handler invocation. Each process imports the handler module, calls it, writes the result to stdout, and exits.

Why one-per-dispatch instead of a long-lived daemon:
- Simpler — no multiplexing, no request/response correlation by task ID
- Clean isolation — no state leaks between handler calls
- Matches master's proven approach

The tradeoff is startup overhead per invocation (Node.js boot + module import). For a POC this is fine. A long-lived daemon can replace this later as an optimization without changing the Scheduler interface.

## Protocol

**Rust → Node.js (stdin):** handler input as JSON

```json
{"value": null}
```

The input is whatever value WorkflowState passed in the Dispatch. For the first handler in a workflow, this is `null` (workflows have no input).

**Node.js → Rust (stdout):** handler result as JSON

```json
{"initialized": true}
```

Just the return value, not wrapped in an envelope. If the handler throws or the process exits non-zero, the workflow panics. No structured error handling for now.

## Worker script

`libs/barnum/src/worker.ts` — invoked as `tsx worker.ts <module> <export>`:

```typescript
const [modulePath, exportName = "default"] = process.argv.slice(2);

// Read entire stdin
const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const input = JSON.parse(Buffer.concat(chunks).toString());

// Import handler, call it
const mod = await import(modulePath);
const handler = mod[exportName];
if (!handler?.__definition?.handle) {
    throw new Error(`${modulePath}:${exportName} is not a barnum handler`);
}
const result = await handler.__definition.handle({ value: input.value });

// Write result to stdout
process.stdout.write(JSON.stringify(result));
```

## Executor resolution

Use `tsx` to run TypeScript handlers. Resolve it from the project's `node_modules`:

```typescript
import { createRequire } from "module";

function resolveExecutor(): string {
    const require = createRequire(process.argv[1] || import.meta.url);
    const tsxPath = require.resolve("tsx/cli");
    return `node ${tsxPath}`;
}
```

Master does this in `libs/barnum/run.ts`. The resolved executor string is passed from the JS config layer to Rust.

## Rust side: Scheduler integration

The Scheduler's `dispatch()` currently spawns a tokio task with a no-op handler. To execute real TypeScript handlers, replace the no-op with subprocess spawning:

```rust
pub fn dispatch(&self, dispatch: &Dispatch, handler: &HandlerKind) {
    let result_tx = self.result_tx.clone();
    let task_id = dispatch.task_id;
    let HandlerKind::TypeScript(ts) = handler;
    let module = ts.module.lookup().to_owned();
    let func = ts.func.lookup().to_owned();
    let value = dispatch.value.clone();

    tokio::spawn(async move {
        let result = execute_typescript(&module, &func, &value).await;
        let _ = result_tx.send((task_id, result));
    });
}

async fn execute_typescript(module: &str, func: &str, value: &Value) -> Value {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("{executor} {worker_path} {module} {func}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn handler process");

    // Write input to stdin
    let stdin = child.stdin.take().expect("no stdin");
    let input = serde_json::to_vec(&json!({ "value": value })).expect("serialize failed");
    // ... write input, close stdin

    // Read stdout
    let output = child.wait_with_output().await.expect("wait failed");
    assert!(output.status.success(), "handler failed: {}", String::from_utf8_lossy(&output.stderr));

    serde_json::from_slice(&output.stdout).expect("invalid handler output")
}
```

This is the tokio async version of master's `ShellAction::start`. Each dispatch spawns a child process, writes the input, reads the output, parses JSON.

## What this does NOT cover

- **Structured error handling:** Handler failures panic. No Result wrapping, no retry. See DEFERRED_FEATURES.md.
- **Step config:** Handlers receive `{ value }` only. Step config is deferred.
- **Validation:** Zod validators exist on handlers but aren't invoked.
- **Timeouts:** A hung handler blocks its tokio task forever.
- **Module resolution:** Assumes `module` paths are absolute. The TypeScript config layer resolves paths before serializing.

## Implementation order

1. Write `libs/barnum/src/worker.ts`.
2. Add `execute_typescript` to the Scheduler (replace the no-op).
3. Wire the executor path through config or CLI args.
4. Test end-to-end with a Chain(A, B) workflow.
