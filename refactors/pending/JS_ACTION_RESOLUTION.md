# JS Action Resolution

**Depends on:** ACTION_PARAMS_NESTING (landed)
**Sub-refactors:** JS_ACTION_HANDLERS.md, EXECUTOR_CLI_FLAG.md

## Motivation

`dispatch_task` (`runner/mod.rs:696-730`) matches on `ActionKind` to construct either a `PoolAction` or `ShellAction`. Adding a new action kind requires a Rust enum variant, a match arm, and a new `Action` impl. Both Pool and Command ultimately spawn a subprocess and pipe stdin/stdout. The per-kind Rust dispatch is unnecessary coupling.

This refactor makes JS the execution layer. Rust dispatches every task by spawning a JS executor subprocess, passing the action config and task value via stdin. The JS executor looks up a handler by action kind and runs it. Rust keeps the state machine, timeouts, retries, and concurrency control. JS handles action-specific execution.

## Execution Model

Three process boundaries exist in the new world. Each one is a subprocess spawn.

### Boundary 1: JS spawns Rust (once per workflow)

**When:** The user calls `BarnumConfig.run()` from their Node process, or runs `barnum run` from the CLI.

**What happens:** Both paths go through `cli.cjs` — the single entry point that spawns the Rust binary. `cli.cjs` detects the JS runtime, resolves the executor command, and injects a hidden `--executor` flag when spawning the Rust binary. Users never see or interact with this flag.

```
CLI path:                              JS API path:
  barnum run --config ...                BarnumConfig.run(opts)
  │                                      │
  └─ cli.cjs (npm bin entry)             └─ spawns cli.cjs with args
       │                                      │
       ├─ detect runtime (Node/Bun/Deno)      ├─ (same)
       ├─ resolve executor command:            ├─ (same)
       │    Node: node <require.resolve("tsx/cli")> <executor.ts>
       │    Bun:  <process.execPath> <executor.ts>
       │    Deno: <process.execPath> run <executor.ts>
       │
       └─ spawn(rustBinary, [...userArgs, "--executor", executorCommand])
            │
            └─ [barnum Rust process — runs for entire workflow]
```

`BarnumConfig.run()` calls `cli.cjs` rather than the Rust binary directly. This keeps executor resolution logic in a single place. tsx is a declared dependency, so `require.resolve("tsx/cli")` works in all install modes (npm, pnpm, yarn PnP, dlx).

The `--executor` flag is internal — hidden from `--help`, not documented. If the Rust binary is invoked directly without `--executor`, it errors. The only supported entry points are `cli.cjs` (CLI) and `BarnumConfig.run()` (JS API), both of which always provide it.

### Boundary 2: Rust spawns JS executor (once per task)

**When:** `Engine::dispatch_task` runs for each task in the workflow.

**What happens:** Rust constructs a `ShellAction` whose script is the executor command, writes an enriched envelope to stdin, and reads follow-up tasks from stdout. The worker thread manages the subprocess lifetime and timeout.

Here is the exact code path from dispatch to result:

```
Engine main thread (receiving on rx channel)
  │
  ├─ dispatch_task(task_id, task)
  │    │
  │    ├─ look up step from step_map
  │    ├─ compute timeout from step.options.timeout
  │    ├─ construct ShellAction with executor_script and envelope context
  │    │
  │    └─ spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout)
  │         │
  │         └─ thread::spawn ─────────────────────────────────────────────────┐
  │                                                                          │
  │    ┌─────────────────────── worker thread ───────────────────────────┐    │
  │    │                                                                │    │
  │    │  run_action(action, &value.0, timeout)                         │    │
  │    │    │                                                           │    │
  │    │    ├─ deadline = Instant::now() + timeout                      │    │
  │    │    ├─ handle = action.start(value)                             │    │
  │    │    │    │                                                      │    │
  │    │    │    ├─ serialize envelope JSON (action + task + step + config)   │
  │    │    │    ├─ Command::new("sh").arg("-c").arg(executor_script)   │    │
  │    │    │    │    stdin: enriched envelope                          │    │
  │    │    │    │    stdout: piped                                     │    │
  │    │    │    │    stderr: piped                                     │    │
  │    │    │    ├─ write envelope to stdin, close pipe                 │    │
  │    │    │    ├─ spawn reader thread (reads stdout+stderr to completion)   │
  │    │    │    └─ return ActionHandle { rx, ProcessGuard { child } }  │    │
  │    │    │                                                           │    │
  │    │    └─ handle.rx.recv_timeout(remaining)                        │    │
  │    │         │                                                      │    │
  │    │         ├─ Ok(stdout) → ActionResult { output: Ok(stdout) }    │    │
  │    │         ├─ Timeout → ProcessGuard drops → child.kill()         │    │
  │    │         └─ Disconnected → "action panicked"                    │    │
  │    │                                                                │    │
  │    │  tx.send(WorkerResult { task_id, task, result })               │    │
  │    │                                                                │    │
  │    └────────────────────────────────────────────────────────────────┘    │
  │                                                                          │
  └─ rx.recv() → process_worker_result → apply entries → flush_dispatches
```

`spawn_worker` and `run_action` are defined in `runner/action.rs`. They do not change in this refactor. `spawn_worker` spawns a thread, calls `run_action`, sends the result. `run_action` computes a deadline, calls `action.start()`, and blocks on `recv`/`recv_timeout`. On timeout, the `ActionHandle` drops, which drops the `ProcessGuard`, which kills the child process. The behavior is identical to today's `ShellAction` dispatch — the only difference is what script the ShellAction runs and what stdin it receives.

### Boundary 3: JS handler spawns subprocess (once per task, inside executor)

**When:** The executor's handler calls out to an external process.

**What happens:** The executor reads the envelope from stdin, looks up the handler by `action.kind`, validates params with the handler's Zod schema, and calls `handle()`. The handler spawns its own subprocess depending on the action kind.

```
node tsx executor.ts                    (spawned by Rust at Boundary 2)
  │
  ├─ read stdin → parse envelope
  ├─ getAction(envelope.action.kind)    → handler definition
  ├─ handler.validate.parse(params)     → runtime param validation
  │
  ├─ handler.handle({ params, task, step, config })
  │    │
  │    ├─ [Pool handler]
  │    │    └─ execFileSync("troupe", ["submit_task", "--pool", pool, "--data", payload, ...])
  │    │         └─ troupe manages agent lifecycle, returns response
  │    │
  │    └─ [Command handler]
  │         └─ execSync(params.script, { shell: "/bin/sh", input: JSON.stringify(task) })
  │              └─ user's shell script receives { kind, value } on stdin
  │
  └─ process.stdout.write(JSON.stringify(followUpTasks))
```

The handler controls what its subprocess receives. The Command handler strips the envelope down to `{ kind, value }` for backward compatibility with existing user scripts. The Pool handler constructs the troupe payload (task, instructions/docs, optional timeout_seconds).

### Timeout semantics

Two distinct timeouts exist, operating at different boundaries.

**Barnum worker timeout** (`step.options.timeout`, defaulting to `config.options.timeout`):
- Applied at Boundary 2 by `run_action` via `recv_timeout`
- When it fires, `ProcessGuard::drop` sends SIGKILL to the executor process
- This is barnum's kill deadline for the entire task execution
- Resolved during `ConfigFile::resolve()`: `step.timeout.or(global.timeout)` (`config.rs:255`)

**Pool agent timeout** (`action.params.timeout` in Pool actions):
- Applied at Boundary 3 by the Pool handler, passed to troupe as `timeout_seconds`
- Controls how long the troupe agent gets to work
- Opaque to Rust — a field in the action params JSON, forwarded by the handler

These timeouts are independent. The barnum worker timeout is the outer envelope. If it fires, the executor process dies regardless of the agent timeout.

## Envelope Format

Rust pipes a JSON envelope to the executor's stdin for each task. The envelope contains everything the handler needs:

```json
{
  "action": { "kind": "Pool", "params": { "instructions": {...}, "pool": "demo", "timeout": 300 } },
  "task": { "kind": "Analyze", "value": { "file": "src/main.rs" } },
  "step": { "name": "Analyze", "action": {...}, "next": ["Implement", "Done"], "options": {...} },
  "config": { "steps": [...], "max_concurrency": 10 }
}
```

- **`action`**: The step's action config. With ACTION_PARAMS_NESTING landed, `serde_json::to_value(&step.action)` produces `{ "kind": "Pool", "params": { ... } }` — the exact shape the JS executor expects.
- **`task`**: The task being dispatched: `{ kind: step_name, value: payload }`.
- **`step`**: The full resolved step definition. Handlers that need context (e.g., Pool handler generating docs from `step.next` and value schemas) use this.
- **`config`**: The full resolved config. Handlers that need global context (e.g., Pool handler looking up next step schemas for docs) use this.

The `config` field is the same for every task in a workflow run. The Engine pre-serializes it once and reuses it for every dispatch.

## JS Handler Architecture

Detailed in **JS_ACTION_HANDLERS.md**. Summary:

Each handler file exports an action definition object with an optional `validate` Zod schema and a `handle` function. The Zod schema provides runtime validation and TypeScript type inference — `handle` receives typed `params` inferred from the schema.

```typescript
export default defineAction({
  validate: z.object({ script: z.string() }),
  handle: async ({ params, task }) => {
    // params is typed as { script: string }
    const stdout = execSync(params.script, { input: JSON.stringify(task), ... });
    return JSON.parse(stdout);
  },
});
```

File layout:

```
libs/barnum/actions/
├── executor.ts    # Reads stdin envelope, dispatches to handler, writes stdout
├── types.ts       # RawEnvelope, ActionContext, ActionDefinition, defineAction
├── pool.ts        # Pool handler: generates docs, submits to troupe
├── command.ts     # Command handler: spawns sh -c, pipes { kind, value }
├── docs.ts        # JS port of generate_step_docs (used by pool handler)
└── index.ts       # Handler registry: kind name → action definition
```

The registry is a hardcoded `Map<string, ActionDefinition>`. Adding a new kind means adding an import and a map entry. Future: `BarnumConfig.builder().action("Claude", claudeAction)` for user-registered kinds.

## Rust Changes

Two concerns, in separate sub-refactors:

**Executor plumbing** (EXECUTOR_CLI_FLAG.md): `cli.cjs` detects runtime, resolves executor command, injects hidden `--executor` flag. Rust threads it through `RunnerConfig` → `Engine` → `dispatch_task`.

**Enriched envelope** (this doc): ShellAction always pipes `{ action, task, step, config }` on stdin. Both `dispatch_task` and `dispatch_finally` construct ShellAction identically — the only difference is the script (executor command vs finally hook script). No optional fields, no branching, one code path.

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
    pub action_json: serde_json::Value,
    pub step_json: serde_json::Value,
    pub config_json: serde_json::Value,
}

impl Action for ShellAction {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
        let stdin_json = serde_json::to_string(&serde_json::json!({
            "action": self.action_json,
            "task": { "kind": &self.step_name, "value": &value },
            "step": self.step_json,
            "config": self.config_json,
        }))
        .unwrap_or_default();

        // ... spawn sh -c, pipe stdin, read stdout (unchanged)
    }
}
```

Engine pre-serializes `config_json` once at construction. `action_json` and `step_json` are serialized per-dispatch (different per step).

## What Doesn't Change

- `spawn_worker`, `run_action`, `ActionHandle`, `ProcessGuard` — generic action dispatch infrastructure
- `RunState` and all state machine logic (task tracking, child counting, finally detection)
- `CompiledSchemas` and value schema validation
- State log format (config entry stores the serialized `Config`)
- `barnum config validate`, `barnum config docs`, `barnum config graph` — operate on config types
- Finally hooks — direct `ShellAction` with hook script (not routed through executor), but same enriched envelope format

## Resume Behavior

On resume, Rust reads the state log which contains the serialized config. The `--executor` flag must be provided again (not stored in the log). Both entry points (`cli.cjs` and `BarnumConfig.run()`) always provide it.

## Implementation Order

### ✅ Step 1: EXECUTOR_CLI_FLAG (DONE)

Hidden `--executor` flag added to Rust CLI. cli.cjs detects runtime (Node/Bun), resolves executor command (tsx for Node, native for Bun), injects `--executor` when spawning Rust. Flag is accepted and ignored on the Rust side. See **EXECUTOR_CLI_FLAG.md**.

### Step 2: JS_ACTION_HANDLERS (next)

Create `libs/barnum/actions/` with all handler files (types.ts, command.ts, pool.ts, docs.ts, index.ts, executor.ts). Add `tsx` as a dependency. Purely additive JS code — no Rust changes. See **JS_ACTION_HANDLERS.md**.

### Step 3: Thread executor through RunnerConfig → Engine → dispatch_task

Wire the `--executor` value through Rust:

1. Add `executor_script: &'a str` to `RunnerConfig`
2. Store `executor_script: String` and pre-serialized `config_json: serde_json::Value` on `Engine`
3. Replace `dispatch_task`'s `ActionKind` match with a single `ShellAction` that runs `executor_script` and pipes the enriched envelope `{ action, task, step, config }` to stdin
4. `dispatch_finally` uses the same enriched envelope format (finally hook script instead of executor)

At this point the executor is required. Change the Rust flag from `Option<String>` to `String` and make it required. Tests that call the binary directly need `--executor` injected.

### Step 4: BarnumConfig.run() calls cli.cjs

**File:** `libs/barnum/run.ts`

`BarnumConfig.run()` stops calling the Rust binary directly and instead spawns `cli.cjs`, making cli.cjs the single source of truth for executor resolution.

```typescript
export class BarnumConfig {
  run(opts?: RunOptions): ChildProcess {
    const cliPath = resolve(import.meta.dirname, "cli.cjs");
    const args = opts?.resumeFrom
      ? ["run", "--resume-from", opts.resumeFrom]
      : ["run", "--config", JSON.stringify(this.config)];
    // ... rest of opts
    return spawn(process.execPath, [cliPath, ...args], { stdio: "inherit" });
  }
}
```

### Step 5: Make Step.action opaque + delete dead Rust code

**File:** `crates/barnum_config/src/resolved.rs`

Replace `ActionKind` with `serde_json::Value`:

```rust
pub struct Step {
    pub name: StepName,
    pub value_schema: Option<serde_json::Value>,
    pub action: serde_json::Value,  // was: ActionKind
    pub next: Vec<StepName>,
    pub finally_hook: Option<HookScript>,
    pub options: Options,
}
```

`ConfigFile::resolve()` converts `ActionFile` to `serde_json::Value` via `serde_json::to_value()`.

Delete dead code:
- `PoolAction` struct in `runner/action.rs` (the runtime Action impl)
- `submit.rs` (troupe submission: `build_agent_payload`, `submit_via_cli`)
- `ActionKind` enum and `PoolAction`/`CommandAction` structs in `resolved.rs`
- `Invoker<TroupeCli>` from `Engine` and `RunnerConfig`
- `cli_invoker` and `troupe_cli` dependencies from `barnum_config`
- `generate_step_docs` in `docs.rs` (moved to JS; `generate_full_docs` stays for `barnum config docs`)
- `Config::has_pool_actions()` in `resolved.rs`

Update `generate_full_docs` and `generate_graphviz` to extract fields from the opaque `serde_json::Value` action (read `action["kind"]` and `action["params"]["instructions"]` instead of matching on `ActionKind`).

## Relationship to Other Docs

- **ACTION_PARAMS_NESTING.md** — Prerequisite, landed. Makes `serde_json::to_value(action)` produce `{ "kind": ..., "params": { ... } }`.
- **PLUGGABLE_ACTION_KINDS.md** — End-state vision. User-defined kinds register via `BarnumConfig.builder().action(kind, handler)`. The hardcoded registry in `index.ts` becomes the default set of registrations.
- **CLAUDE_CLI_ACTION_KIND.md** — Claude becomes `actions/claude.ts` exporting a handler that spawns the Claude CLI. No Rust code needed.
