# JS Action Resolution

## Motivation

ACTION_REGISTRY moves the kind-specific match out of Engine into a Rust-side factory registry. But the end goal is for action kinds to be defined and resolved in JavaScript, with Rust acting as a dumb executor. This doc explores how JS-driven action resolution would work and whether the Rust registry is a useful intermediate step or a detour.

## Architecture

The JS layer (`BarnumConfig`) already validates config, constructs CLI args, and spawns the Rust binary. The proposal: extend this layer to also resolve each step's action kind into a concrete execution command. Rust receives a config where every step has a command string instead of a typed `ActionKind`. Rust spawns the command, pipes the task value on stdin, reads the result from stdout.

```
User config (kind: "Pool", kind: "Command", kind: "Claude")
  → JS resolution (each kind resolved to a command)
    → Resolved config (every step has a command string)
      → Rust (spawns commands, manages state tree, handles timeouts)
```

## The JS module interface

Each action kind is a JS module with a default export. The function receives the action's parameters and runtime context, and returns an execution spec.

```typescript
// Type definitions
interface ActionContext {
  /** Troupe root directory */
  root: string;
  /** Pool name (if applicable) */
  pool: string;
  /** Working directory for the barnum config file */
  workingDir: string;
  /** The full resolved config */
  config: ResolvedConfig;
  /** The step this action belongs to */
  step: ResolvedStep;
}

interface ExecutionSpec {
  /** Command to spawn (receives task value as JSON on stdin, writes result JSON to stdout) */
  command: string;
  /** Arguments to the command */
  args: string[];
}

// Module interface
type ActionModule = (params: unknown, context: ActionContext) => ExecutionSpec;
```

The function runs once per step at config time (inside `BarnumConfig.run()`). It returns the command that Rust will spawn at dispatch time. The command receives `{"kind": "<StepName>", "value": <payload>}` on stdin and writes a JSON array of follow-up tasks to stdout.

## Built-in action modules

### Pool

The pool module returns a command that invokes a bundled executor script. The executor handles troupe submission, including step docs generation and payload formatting.

```typescript
// @barnum/barnum/actions/pool.ts
import { resolve } from "node:path";

const executorPath = resolve(import.meta.dirname, "pool-executor.js");

export default function pool(
  params: { instructions: string },
  context: ActionContext
): ExecutionSpec {
  const docs = generateStepDocs(context.step, context.config);
  const docsBase64 = Buffer.from(docs).toString("base64");

  return {
    command: "node",
    args: [
      executorPath,
      "--root", context.root,
      "--pool", context.pool,
      "--docs", docsBase64,
      ...(context.step.options.timeout
        ? ["--timeout", String(context.step.options.timeout)]
        : []),
    ],
  };
}
```

The executor script (`pool-executor.js`) reads stdin, builds the troupe payload, invokes `troupe submit_task`, and writes the result to stdout. This is a self-contained Node.js script bundled with the package.

```typescript
// @barnum/barnum/actions/pool-executor.ts (compiled to pool-executor.js)
import { parseArgs } from "node:util";
import { execFileSync } from "node:child_process";

const { values } = parseArgs({
  options: {
    root: { type: "string" },
    pool: { type: "string" },
    docs: { type: "string" },
    timeout: { type: "string" },
  },
});

// Read task value from stdin
const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const input = JSON.parse(Buffer.concat(chunks).toString());

// Build troupe payload
const docs = Buffer.from(values.docs!, "base64").toString();
const payload = JSON.stringify({
  kind: "Task",
  task: { instructions: docs, data: input },
  ...(values.timeout && { timeout_seconds: parseInt(values.timeout) }),
});

// Submit to troupe
const result = execFileSync("troupe", [
  "submit_task",
  "--pool", values.pool!,
  "--root", values.root!,
  "--notify", "file",
  "--timeout-secs", "86400",
  "--data", payload,
], { encoding: "utf-8" });

const response = JSON.parse(result);
if (response.kind === "Processed") {
  process.stdout.write(response.stdout);
} else {
  process.stderr.write("not processed by pool");
  process.exit(1);
}
```

### Command

The command module returns the user's script directly, wrapped in `sh -c`.

```typescript
// @barnum/barnum/actions/command.ts
export default function command(
  params: { script: string },
  _context: ActionContext
): ExecutionSpec {
  return {
    command: "sh",
    args: ["-c", params.script],
  };
}
```

## What changes in Rust

Rust's resolved config replaces `ActionKind` with a flat execution descriptor:

```rust
// resolved.rs — replaces ActionKind enum
#[derive(Debug, Serialize, Deserialize)]
pub struct ActionExecution {
    pub command: String,
    pub args: Vec<String>,
}
```

The `Step` struct changes:

```rust
pub struct Step {
    pub name: StepName,
    pub value_schema: Option<serde_json::Value>,
    pub action: ActionExecution,  // was: ActionKind
    pub next: Vec<StepName>,
    pub finally_hook: Option<HookScript>,
    pub options: Options,
}
```

`dispatch_task` becomes trivial:

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    info!(step = %task.step, command = %step.action.command, "dispatching task");
    let action = Box::new(ShellAction {
        command: step.action.command.clone(),
        args: step.action.args.clone(),
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
    });
    spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
}
```

`ShellAction` changes to spawn the command directly instead of wrapping in `sh -c`:

```rust
// Before: Command::new("sh").arg("-c").arg(&self.script)
// After:
Command::new(&self.command)
    .args(&self.args)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .current_dir(&self.working_dir)
    .spawn()
```

`PoolAction` and the `PoolConnection` type are deleted from Rust entirely. The `submit.rs` module (troupe submission) is also deleted — that logic moves to the JS pool executor.

## What changes in JS

`BarnumConfig.run()` resolves actions before passing the config to Rust:

```typescript
run(opts?: RunOptions): ChildProcess {
  const resolvedConfig = this.resolveActions(opts);
  const args = ["run", "--config", JSON.stringify(resolvedConfig)];
  // ... rest of args
  return spawnBarnum(args);
}

private resolveActions(opts?: RunOptions): ResolvedConfig {
  return {
    ...this.config,
    steps: this.config.steps.map((step) => {
      const resolver = this.resolvers.get(step.action.kind);
      const context: ActionContext = {
        root: opts?.root ?? "",
        pool: opts?.pool ?? "",
        workingDir: process.cwd(),
        config: this.config,
        step,
      };
      const exec = resolver(step.action, context);
      return {
        ...step,
        action: { command: exec.command, args: exec.args },
      };
    }),
  };
}
```

The built-in resolvers are registered in the constructor:

```typescript
private resolvers = new Map<string, ActionModule>([
  ["Pool", poolAction],
  ["Command", commandAction],
]);
```

## Step docs generation

`generate_step_docs` currently lives in Rust (`crates/barnum_config/src/docs.rs`). It generates markdown instructions for pool agents based on the step config (valid next steps, schemas, etc.). With JS action resolution, this logic moves to JS:

- The pool action module calls `generateStepDocs(step, config)` at resolution time
- The result is base64-encoded and passed as an argument to the pool executor
- The executor decodes it and includes it in the troupe payload

The Rust `docs.rs` module continues to exist for `generate_full_docs` (used by the `docs` CLI subcommand) but `generate_step_docs` gets a JS equivalent. Alternatively, `generate_step_docs` could be removed from Rust and the `docs` subcommand could also be driven by JS.

## "Returns a command" vs "does the work itself"

The design above uses "returns a command" — the JS function runs at config time and returns an `ExecutionSpec`. Rust spawns the command at dispatch time.

The alternative ("does the work itself") means the JS function runs at dispatch time. For this to work, Rust would need to invoke JS per dispatch, which means either:

1. Spawning `node <module>` per dispatch (high overhead — Node startup per task)
2. A persistent Node process that Rust communicates with over a socket or pipe
3. Inverting the relationship so JS is the outer process managing dispatch

Option 1 is too slow. Option 2 adds substantial complexity. Option 3 is a major architectural change (Rust becomes a library, JS becomes the runtime).

"Returns a command" sidesteps all of this. If an action kind needs dynamic dispatch-time behavior, its command points to a script that has that logic. The script starts once per dispatch, reads stdin, does its work, writes stdout. This is exactly what the pool executor does. So "returns a command" and "does the work itself" converge: in both cases, a process runs at dispatch time. The question is just whether the process is a pre-known script path or an inline function.

For pragmatic purposes, "returns a command" is the right model. It keeps Rust simple, avoids Rust-JS communication overhead, and handles all current use cases.

## Relationship to ACTION_REGISTRY

ACTION_REGISTRY introduces a Rust-side factory registry. With JS action resolution, that registry is unnecessary — Rust receives a flat `ActionExecution` per step and has one execution path. There are two possible orderings:

**Path A: ACTION_REGISTRY first, then JS resolution**
1. ACTION_REGISTRY: Engine dispatch goes through factory registry (still typed ActionKind)
2. JS resolution: Replace ActionKind with ActionExecution, delete the registry

The registry serves as a mechanical stepping stone that decouples Engine from specific kinds. But it gets deleted in the next step, so the total work is higher.

**Path B: JS resolution directly**
1. Add `ActionExecution` to Rust resolved types
2. JS resolves actions in `BarnumConfig.run()`
3. Delete `ActionKind`, `PoolAction` (Rust), `PoolConnection`, `submit.rs`
4. `ShellAction` becomes the only executor, spawning `command + args`

Path B is less total work and reaches the end state faster. The risk is that it's a bigger single change — config types, JS layer, and Rust execution all change together.

## Open questions

1. **Troupe binary discovery.** The pool executor script needs to invoke `troupe`. Currently Rust uses `Invoker<TroupeCli>` which embeds the binary or finds it on PATH. In JS, we'd need the troupe binary path passed as context. The `@barnum/barnum` npm package could bundle or locate it the same way it bundles the barnum binary.

2. **Working directory for command actions.** Currently ShellAction uses `self.working_dir` (the config file's directory). With JS resolution, should the working directory be part of `ActionExecution` (set per-action by JS), or should Rust always use its own working directory? Per-action is more flexible (different kinds might want different working dirs) but adds a field.

3. **Resume behavior.** On resume, Rust reads the state log which contains the resolved config (including `ActionExecution` fields). The JS layer doesn't run again — Rust uses the serialized execution specs directly. This means the command paths must still be valid at resume time. If the pool executor script moved between runs, resume would break. This is probably fine in practice but worth noting.

4. **`Config::has_pool_actions()`.** This method in `resolved.rs` checks if any step uses Pool, so the runner knows whether it needs a troupe root. With flat `ActionExecution`, there's no "Pool" concept in Rust. The JS layer would need to signal this differently (e.g., a `requires_pool: bool` flag on the config), or Rust would just always require the root path.
