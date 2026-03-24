# Add TypeScript Action Kind

**Parent:** TS_CONFIG.md
**Depends on:** FLATTEN_AND_RENAME_ACTION (Command renamed to Bash, params nesting removed, camelCase fields)

## Motivation

After FLATTEN_AND_RENAME_ACTION lands, Rust has one action kind: `Bash` (a shell script) with flat config shape (`{"kind": "Bash", "script": "..."}`). This refactor adds the TypeScript action kind.

TypeScript actions point to a handler file. Rust dispatches them as subprocesses — the same `ShellAction` infrastructure used by Bash, but with a richer stdin envelope and a different subprocess command. From Rust's perspective, a TypeScript action is just a Bash command that happens to invoke a TypeScript runtime.

## Current state (after FLATTEN_AND_RENAME_ACTION)

```rust
// config.rs
pub struct BashActionFile { pub script: String }

#[serde(tag = "kind")]
pub enum ActionFile {
    Bash(BashActionFile),
}

// resolved.rs
pub struct BashAction { pub script: String }

#[serde(tag = "kind")]
pub enum ActionKind {
    Bash(BashAction),
}
```

Config shape: `{"kind": "Bash", "script": "echo hello"}`. All structs use `rename_all = "camelCase"`. The `--executor` CLI flag exists but is unused.

## Add TypeScript action kind

### TypeScriptAction type

**File:** `crates/barnum_config/src/config.rs` (shared between config and resolved)

```rust
/// Run a TypeScript handler file as a subprocess.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TypeScriptAction {
    /// Path to the handler file.
    pub path: String,

    /// Named export to invoke from the handler module.
    #[serde(default = "default_exported_as")]
    pub exported_as: String,

    /// Step configuration passed through to the handler.
    /// Rust stores this as-is and includes it in the envelope.
    #[serde(default)]
    pub step_config: serde_json::Value,
}

fn default_exported_as() -> String { "default".to_string() }
```

One type used in both enums:

```rust
// config.rs
#[serde(tag = "kind")]
pub enum ActionFile {
    Bash(BashActionFile),
    TypeScript(TypeScriptAction),
}

// resolved.rs
#[serde(tag = "kind")]
pub enum ActionKind {
    Bash(BashAction),
    TypeScript(TypeScriptAction),
}
```

Config `{"kind": "TypeScript", "path": "./handlers/analyze.ts", "stepConfig": {...}}` deserializes with `exported_as = "default"` (serde default). Resolution canonicalizes `path` in place — no separate resolved type needed.

### Config resolution

**File:** `crates/barnum_config/src/config.rs` (in `ActionFile::resolve`)

```rust
Self::TypeScript(mut ts) => {
    // Resolve path relative to config directory
    let resolved_path = base_path.join(&ts.path);
    let canonical = resolved_path.canonicalize().map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("TypeScript handler not found: {}: {e}", resolved_path.display()),
        )
    })?;
    ts.path = canonical.to_string_lossy().into_owned();
    Ok(ActionKind::TypeScript(ts))
}
```

Resolution canonicalizes `path` in place on the same struct. The `exported_as` field is already defaulted to `"default"` by serde. Resolution validates the handler file exists at config load time — a typo in the path fails immediately, not at dispatch time.

### Dispatch

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
ActionKind::TypeScript(TypeScriptAction { path, exported_as, ref step_config }) => {
    let executor = self.executor.as_deref().unwrap_or("pnpm dlx tsx");
    let run_handler = self.run_handler_path.as_deref()
        .unwrap_or("node_modules/@barnum/barnum/actions/run-handler.ts");

    let script = format!(
        "{executor} {run_handler} {path} {exported_as}",
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
    /// Executor command for TypeScript handlers (e.g., "pnpm dlx tsx").
    /// Injected by cli.cjs via --executor. None means TypeScript actions are unavailable.
    pub executor: Option<&'a str>,
    /// Path to run-handler.ts. Defaults to looking in node_modules.
    pub run_handler_path: Option<&'a str>,
}
```

The `executor` is the TypeScript runtime command (e.g., `pnpm dlx tsx`, `bun`, `node --import tsx`). It's optional because JSON configs (without a JS entry point) may not have a TS runtime available. If a config uses TypeScript actions but no executor is provided, dispatch fails with a clear error.

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
  /** The validated step configuration. */
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
  args.push("--executor", "pnpm dlx tsx");
  // run-handler.ts path could also be passed, or resolved by the executor

  // ... rest of opts handling
  return spawnBarnum(args);
}
```

The executor command is `pnpm dlx tsx` by default. When invoked via `tsx barnum.config.ts`, the TS runtime is already available. The exact executor discovery logic (tsx vs bun vs node) is an implementation detail — start with `pnpm dlx tsx` and refine later.

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
            assert_eq!(ts.exported_as, "default");
            assert_eq!(ts.step_config["instructions"], "Do stuff");
            assert_eq!(ts.step_config["pool"], "demo");
        }
        _ => panic!("expected TypeScript action"),
    }
}
```

### Resolution tests

Test that resolution resolves the handler path and defaults exported_as:

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

FLATTEN_AND_RENAME_ACTION lands first (rename Command → Bash, remove params nesting, add camelCase). Then this refactor adds the TypeScript variant on top.

## What this does NOT do

- Does not implement step constructors (`createTroupeStep`, `createBashStep`) — those are JS convenience helpers, not Rust concerns
- Does not implement value_schema on TypeScript actions — value validation happens in the handler's Zod validator, not in Rust
- Does not change the Bash action stdin format — Bash actions still get `{"kind": ..., "value": ...}`
- Does not implement `.validate()` on `BarnumConfig` — that's a future JS-side feature
