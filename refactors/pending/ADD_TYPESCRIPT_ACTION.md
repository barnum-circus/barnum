# Add TypeScript Action Kind

**Parent:** TS_CONFIG.md
**Depends on:** REMOVE_POOL_ACTION (Pool removed, only Command remains)

## Motivation

After REMOVE_POOL_ACTION lands, Rust has one action kind: `Command` (a shell script). TS_CONFIG describes two: `Bash` (renamed Command) and `TypeScript` (a handler file). This refactor renames Command to Bash and adds the TypeScript action kind.

TypeScript actions point to a handler file. Rust dispatches them as subprocesses — the same `ShellAction` infrastructure used by Bash, but with a richer stdin envelope and a different subprocess command. From Rust's perspective, a TypeScript action is just a Bash command that happens to invoke a TypeScript runtime.

## Current state

**Config types** (`crates/barnum_config/src/config.rs:169-176`):
```rust
#[serde(tag = "kind", content = "params")]
pub enum ActionFile {
    Command(CommandActionFile),
}
```

**Resolved types** (`crates/barnum_config/src/resolved.rs:89-96`):
```rust
#[serde(tag = "kind", content = "params")]
pub enum ActionKind {
    Command(CommandAction),
}
```

Both use adjacently tagged enums (`tag = "kind", content = "params"`), producing config like `{"kind": "Command", "params": {"script": "..."}}`. The `params` wrapper is unnecessary — fields should sit alongside `kind` at the top level.

**Dispatch** (`crates/barnum_config/src/runner/mod.rs:716-724`):
```rust
ActionKind::Command(CommandAction { script }) => {
    let action = Box::new(ShellAction {
        script: script.clone(),
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
    });
    spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
}
```

**ShellAction** (`crates/barnum_config/src/runner/action.rs`): Runs `sh -c <script>`, pipes `{"kind": step_name, "value": value}` to stdin, reads stdout as follow-up tasks JSON. This infrastructure is unchanged.

**RunnerConfig** (`crates/barnum_config/src/runner/mod.rs`): Currently holds `working_dir`, `wake_script`, `invoker`, `state_log_path`. The `invoker` field (for troupe CLI) is removed by REMOVE_POOL_ACTION.

**CLI** (`crates/barnum_cli/src/lib.rs:85-87`): Already has the `--executor` flag (hidden from help):
```rust
#[arg(long, hide = true)]
executor: Option<String>,
```

Currently unused — `main.rs:37` destructures it as `executor: _`.

**run.ts** (`libs/barnum/run.ts`): Serializes config to JSON and spawns the Rust binary. Does not currently pass `--executor`.

## Rename Command → Bash

Before adding TypeScript, rename the existing Command variant to Bash. This is a mechanical rename across all files.

### Config types

**File:** `crates/barnum_config/src/config.rs`

```rust
// Before
pub struct CommandActionFile { pub script: String }

#[serde(tag = "kind", content = "params")]
pub enum ActionFile {
    Command(CommandActionFile),
}
// produces: {"kind": "Command", "params": {"script": "..."}}

// After
pub struct BashActionFile { pub script: String }

#[serde(tag = "kind")]
pub enum ActionFile {
    Bash(BashActionFile),
}
// produces: {"kind": "Bash", "script": "..."}
```

### Resolved types

**File:** `crates/barnum_config/src/resolved.rs`

```rust
// Before
pub struct CommandAction { pub script: String }

#[serde(tag = "kind", content = "params")]
pub enum ActionKind {
    Command(CommandAction),
}

// After
pub struct BashAction { pub script: String }

#[serde(tag = "kind")]
pub enum ActionKind {
    Bash(BashAction),
}
```

### Config resolution

**File:** `crates/barnum_config/src/config.rs` (in `ActionFile::resolve`)

```rust
// Before
Self::Command(CommandActionFile { script }) =>
    Ok(ActionKind::Command(CommandAction { script }))

// After
Self::Bash(BashActionFile { script }) =>
    Ok(ActionKind::Bash(BashAction { script }))
```

### Dispatch

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
// Before
ActionKind::Command(CommandAction { script }) => { ... }

// After
ActionKind::Bash(BashAction { script }) => { ... }
```

### Tests

All test helpers that construct `ActionFile::Command(CommandActionFile { .. })` change to `ActionFile::Bash(BashActionFile { .. })`. Grep for `CommandActionFile`, `CommandAction`, `ActionFile::Command`, `ActionKind::Command` — update every occurrence.

### Demo configs

All demo configs already use `"kind": "Command"`. Change to `"kind": "Bash"`. This is a find-replace across `*.json` and `*.jsonc` in `crates/barnum_cli/demos/`.

### Schemas

Regenerate after the rename:
- `libs/barnum/barnum-config-schema.json`
- `libs/barnum/barnum-config-schema.zod.ts`
- `libs/barnum/barnum-resolved-schema.zod.ts`

## Add TypeScript action kind

### Config types

**File:** `crates/barnum_config/src/config.rs`

```rust
/// Run a TypeScript handler file as a subprocess.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TypeScriptActionFile {
    /// Path to the handler file (relative to config directory).
    pub path: String,

    /// Named export to use (default: "default").
    #[serde(default)]
    pub export: Option<String>,

    /// Step configuration passed through to the handler.
    /// Rust stores this as-is and includes it in the envelope.
    #[serde(default)]
    pub step_config: serde_json::Value,
}

#[serde(tag = "kind")]
pub enum ActionFile {
    Bash(BashActionFile),
    TypeScript(TypeScriptActionFile),
}
```

A config like:

```json
{
  "kind": "TypeScript",
  "path": "./handlers/analyze.ts",
  "stepConfig": {
    "instructions": "Analyze the code.",
    "pool": "demo"
  }
}
```

Deserializes with `path = "./handlers/analyze.ts"`, `export = None`, and `step_config = {"instructions": "Analyze the code.", "pool": "demo"}`. All fields sit at the same level as `kind` — no `params` wrapper.

### Resolved types

**File:** `crates/barnum_config/src/resolved.rs`

```rust
/// Resolved TypeScript action.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TypeScriptAction {
    /// Path to the handler file (resolved to absolute path).
    pub path: String,

    /// Named export (defaults to "default").
    pub export: String,

    /// Step configuration passed through to the handler. Opaque to Rust.
    pub step_config: serde_json::Value,
}

#[serde(tag = "kind")]
pub enum ActionKind {
    Bash(BashAction),
    TypeScript(TypeScriptAction),
}
```

In the resolved type, `export` is a non-optional `String` (defaulted to `"default"` during resolution). The `path` is resolved to an absolute path. `step_config` is passed through unchanged.

### Config resolution

**File:** `crates/barnum_config/src/config.rs` (in `ActionFile::resolve`)

```rust
Self::TypeScript(TypeScriptActionFile { path, export, step_config }) => {
    // Resolve path relative to config directory
    let resolved_path = base_path.join(&path);
    let canonical = resolved_path.canonicalize().map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("TypeScript handler not found: {}: {e}", resolved_path.display()),
        )
    })?;
    Ok(ActionKind::TypeScript(TypeScriptAction {
        path: canonical.to_string_lossy().into_owned(),
        export: export.unwrap_or_else(|| "default".to_string()),
        step_config,
    }))
}
```

Resolution validates the handler file exists at config load time — a typo in the path fails immediately, not at dispatch time.

### Dispatch

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
ActionKind::TypeScript(TypeScriptAction { path, export, ref step_config }) => {
    let executor = self.executor.as_deref().unwrap_or("npx tsx");
    let run_handler = self.run_handler_path.as_deref()
        .unwrap_or("node_modules/@barnum/barnum/actions/run-handler.ts");

    let script = format!(
        "{executor} {run_handler} {path} {export}",
    );

    info!(step = %task.step, handler = %path, "dispatching TypeScript handler");
    let action = Box::new(TypeScriptShellAction {
        script,
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
        step_config: step_config.clone(),
        step: step.clone(),
        config: self.config.clone(),
    });
    spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
}
```

TypeScript dispatch creates a `TypeScriptShellAction` (a new struct, not the existing `ShellAction`) because the stdin envelope is different.

### TypeScriptShellAction

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
pub struct TypeScriptShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
    pub step_config: serde_json::Value,
    pub step: Step,
    pub config: Config,
}

impl Action for TypeScriptShellAction {
    fn start(self: Box<Self>) -> ActionHandle {
        let child = std::process::Command::new("sh")
            .arg("-c")
            .arg(&self.script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(&self.working_dir)
            .spawn();

        // ... same spawn pattern as ShellAction, but with enriched envelope
    }
}
```

The key difference from `ShellAction` is the stdin envelope:

**Bash (ShellAction) stdin:**
```json
{"kind": "Analyze", "value": {"file": "src/main.rs"}}
```

**TypeScript (TypeScriptShellAction) stdin:**
```json
{
  "stepConfig": {"instructions": "Analyze the code.", "pool": "demo"},
  "task": {"kind": "Analyze", "value": {"file": "src/main.rs"}},
  "step": {"name": "Analyze", "next": ["Implement"], ...},
  "config": { ... }
}
```

The envelope gives the handler its step configuration, the task being processed, the step definition (including `next` steps and options), and the whole config. No stripping needed — `stepConfig` is already separate from dispatch params.

### RunnerConfig changes

**File:** `crates/barnum_config/src/runner/mod.rs`

After REMOVE_POOL_ACTION removes `invoker`, add:

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub state_log_path: &'a Path,
    /// Executor command for TypeScript handlers (e.g., "npx tsx").
    /// Injected by cli.cjs via --executor. None means TypeScript actions are unavailable.
    pub executor: Option<&'a str>,
    /// Path to run-handler.ts. Defaults to looking in node_modules.
    pub run_handler_path: Option<&'a str>,
}
```

The `executor` is the TypeScript runtime command (e.g., `npx tsx`, `bun`, `node --import tsx`). It's optional because JSON configs (without a JS entry point) may not have a TS runtime available. If a config uses TypeScript actions but no executor is provided, dispatch fails with a clear error.

### Engine changes

**File:** `crates/barnum_config/src/runner/mod.rs`

The `Engine` struct gains `executor`, `run_handler_path`, and `config` (an `Arc<Config>` for passing to TypeScript actions):

```rust
struct Engine {
    // ... existing fields ...
    executor: Option<String>,
    run_handler_path: Option<String>,
    config: Arc<Config>,
}
```

### CLI changes

**File:** `crates/barnum_cli/src/main.rs`

Wire the existing `--executor` flag through to `RunnerConfig`:

```rust
// Before (currently unused)
executor: _,

// After
let runner_config = RunnerConfig {
    working_dir: &config_dir,
    wake_script: wake.as_deref(),
    state_log_path: &state_log_path,
    executor: executor.as_deref(),
    run_handler_path: None,
};
```

## Handler interface

A TypeScript handler module exports a `HandlerDefinition` — an object with three concerns:

1. **Step configuration validator** (`stepConfigValidator`): Validates `stepConfig` from the envelope — the step-specific configuration from the config file. Rust passes it through as opaque JSON.

2. **Step value validator** (`getStepValueValidator`): A function that receives the validated step config and returns a Zod schema for the task value. This allows the value schema to depend on the step configuration (e.g., different fields based on config options). Called per-task.

3. **Handler function** (`handle`): Takes a single context object with four keys (`stepConfig`, `value`, `config`, `stepName`). Returns follow-up tasks.

```typescript
// handlers/analyze.ts
import { z } from "zod";
import type { HandlerDefinition } from "@barnum/barnum";

export default {
  stepConfigValidator: z.object({
    instructions: z.string(),
    pool: z.string(),
  }),

  getStepValueValidator(stepConfig) {
    // stepConfig is already validated by stepConfigValidator — fully typed
    return z.object({ file: z.string() });
  },

  async handle({ stepConfig, value, config, stepName }) {
    stepConfig.instructions; // string — typed by stepConfigValidator
    value.file;              // string — typed by getStepValueValidator
    config;                  // full resolved Barnum config
    stepName;                // "Analyze"
    return [{ kind: "Implement", value: { plan: "..." } }];
  },
} satisfies HandlerDefinition;
```

Types:

```typescript
interface HandlerDefinition<
  C = unknown,
  V = unknown,
> {
  /** Validates stepConfig from the envelope. */
  stepConfigValidator?: z.ZodType<C>;

  /** Returns a validator for the task value, given the validated step config. */
  getStepValueValidator?: (stepConfig: C) => z.ZodType<V>;

  /** Process the task. Returns follow-up tasks. */
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

interface HandlerContext<C = unknown, V = unknown> {
  /** The validated step configuration (action.params minus path/export). */
  stepConfig: C;
  /** The validated task value. */
  value: V;
  /** The full resolved Barnum config. */
  config: Config;
  /** The name of the step this handler is processing. */
  stepName: string;
}

/** A follow-up task to spawn. */
interface FollowUpTask {
  /** Step name — must be one of this step's `next` entries. Untyped: the
   *  handler doesn't know the config's `next` array at compile time. */
  kind: string;
  /** Task payload — opaque to the framework. */
  value: unknown;
}
```

**Type safety boundary:** Inputs (`stepConfig` and `value`) can be fully typed via Zod validators. The output (`FollowUpTask[]`) is necessarily untyped — which steps a handler can transition to is determined by the config's `next` array, and the handler has no compile-time knowledge of that. Invalid transitions are caught at runtime by Rust's response validator.

**Validation flow** (in `run-handler.ts`):

1. If `stepConfigValidator` exists, parse `envelope.stepConfig` → validated step config `C`. Otherwise, pass `envelope.stepConfig` as `unknown`.
2. If `getStepValueValidator` exists, call it with the validated step config to get the value schema, then parse `envelope.task.value` → validated value `V`. Otherwise, pass `envelope.task.value` as `unknown`.
3. Call `handle({ stepConfig, value, config, stepName })`.

A minimal handler can skip both validators:

```typescript
export default {
  async handle({ stepConfig, value }) {
    // stepConfig: unknown, value: unknown
    return [];
  },
} satisfies HandlerDefinition;
```

## run-handler.ts

**File:** `libs/barnum/actions/run-handler.ts` (new file)

```typescript
const [handlerPath, exportName = "default"] = process.argv.slice(2);

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope = JSON.parse(Buffer.concat(chunks).toString());

const mod = await import(handlerPath);
const definition = mod[exportName];

// 1. Validate step config
const stepConfig = definition.stepConfigValidator
  ? definition.stepConfigValidator.parse(envelope.stepConfig)
  : envelope.stepConfig;

// 2. Validate value, potentially dependent on step config
const value = definition.getStepValueValidator
  ? definition.getStepValueValidator(stepConfig).parse(envelope.task.value)
  : envelope.task.value;

// 3. Call handler
const results = await definition.handle({
  stepConfig,
  value,
  config: envelope.config,
  stepName: envelope.step.name,
});

process.stdout.write(JSON.stringify(results));
```

## run.ts changes

**File:** `libs/barnum/run.ts`

`.run()` needs to inject `--executor` with the TS runtime command:

```typescript
run(opts?: RunOptions): ChildProcess {
  const args = opts?.resumeFrom
    ? ["run", "--resume-from", opts.resumeFrom]
    : ["run", "--config", JSON.stringify(this.config)];

  // Inject executor for TypeScript handler dispatch
  const runHandlerPath = new URL("./actions/run-handler.ts", import.meta.url).pathname;
  args.push("--executor", "npx tsx");
  // run-handler.ts path could also be passed, or resolved by the executor

  // ... rest of opts handling
  return spawnBarnum(args);
}
```

The executor command is `npx tsx` by default. When invoked via `tsx barnum.config.ts`, the TS runtime is already available. The exact executor discovery logic (tsx vs bun vs node) is an implementation detail — start with `npx tsx` and refine later.

## Schemas

After all type changes, regenerate:
- `libs/barnum/barnum-config-schema.json`
- `libs/barnum/barnum-config-schema.zod.ts`
- `libs/barnum/barnum-resolved-schema.zod.ts`

The TypeScript variant appears in the generated schemas. `stepConfig` is typed as an arbitrary JSON value.

## Tests

### Config parsing tests

Test that TypeScript action configs parse correctly:

```rust
#[test]
fn action_typescript_with_step_config() {
    let json = r#"{
        "steps": [{
            "name": "Test",
            "action": {
                "kind": "TypeScript",
                "path": "./handler.ts",
                "stepConfig": {
                    "instructions": "Do stuff",
                    "pool": "demo"
                }
            },
            "next": []
        }]
    }"#;
    let config: ConfigFile = serde_json::from_str(json).expect("parse");
    match &config.steps[0].action {
        ActionFile::TypeScript(ts) => {
            assert_eq!(ts.path, "./handler.ts");
            assert_eq!(ts.export, None);
            assert_eq!(ts.step_config["instructions"], "Do stuff");
            assert_eq!(ts.step_config["pool"], "demo");
        }
        _ => panic!("expected TypeScript action"),
    }
}
```

### Resolution tests

Test that resolution resolves the handler path and defaults export:

```rust
#[test]
fn typescript_action_resolves_path() {
    // Write a temp handler file, resolve config, check canonical path
}

#[test]
fn typescript_action_missing_handler_errors() {
    // Config with path to nonexistent file, resolution should fail
}
```

### Integration tests

Integration tests for TypeScript dispatch require a TS runtime (tsx/node) and aren't suitable for the Rust test suite. They belong in the `crates/barnum_cli/tests/` integration tests or as a separate demo.

## Sequencing

This refactor has two independent parts that can land as separate branches:

1. **Rename Command → Bash**: Mechanical rename across Rust types, demo configs, tests, and schemas. No new functionality.
2. **Add TypeScript variant**: New types, dispatch logic, run-handler.ts, run.ts changes.

The rename lands first, then the TypeScript variant builds on top.

## What this does NOT do

- Does not implement step constructors (`createTroupeStep`, `createBashStep`) — those are JS convenience helpers, not Rust concerns
- Does not implement value_schema on TypeScript actions — value validation happens in the handler's Zod validator, not in Rust
- Does not change the Bash action stdin format — Bash actions still get `{"kind": ..., "value": ...}`
- Does not implement `.validate()` on `BarnumConfig` — that's a future JS-side feature
