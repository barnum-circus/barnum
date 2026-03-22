# Pluggable Action Kinds

## Motivation

Barnum's runner is tightly coupled to the two action kinds it knows about: `Pool` and `Command`. The dispatch logic, response processing, and config types all have hardcoded `match` arms for each kind. This means:

1. **Adding a new action kind requires touching 5+ files.** Config enum, resolved enum, dispatch function, `SubmitResult` variant, response processing logic. Every new kind is a cross-cutting change.

2. **The runner knows how to execute things.** It shouldn't. The runner's job is task scheduling, concurrency control, retry logic, state logging, and finally-hook tracking. *How* a task gets executed is a separate concern.

3. **Pool and Command are privileged.** They're compiled into the binary. But there's nothing fundamentally special about them — Pool shells out to `troupe submit_task`, Command shells out to `bash`. Both are "run a subprocess, get JSON back." They should be pluggable like anything else.

The goal: **Barnum's runner should know nothing about how to execute actions.** It dispatches tasks to an executor, gets results back, and manages the workflow graph. Pool, Command, Claude, Git, TypeScript — they're all just executors registered under a name.

## Current Coupling Points

### 1. Config enum (closed)

`crates/barnum_config/src/config.rs:161-187`:
```rust
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<Instructions> },
    Command { script: String },
}
```

### 2. Resolved action enum (closed)

`crates/barnum_config/src/resolved.rs`:
```rust
pub enum Action {
    Pool { instructions: String },
    Command { script: String },
}
```

### 3. Dispatch (hardcoded match)

`crates/barnum_config/src/runner/mod.rs:489-521`:
```rust
match &step.action {
    Action::Pool { .. } => {
        // 8 lines of pool-specific setup
        thread::spawn(move || dispatch_pool_task(ctx, &docs, timeout, ...));
    }
    Action::Command { script } => {
        // 3 lines of command-specific setup
        thread::spawn(move || dispatch_command_task(ctx, &script, ...));
    }
}
```

### 4. Submit result (per-kind variants)

`crates/barnum_config/src/runner/dispatch.rs:33-48`:
```rust
pub enum SubmitResult {
    Pool { value: StepInputValue, response: io::Result<Response> },
    Command { value: StepInputValue, output: io::Result<String> },
    Finally { value: StepInputValue, output: Result<String, String> },
    PreHookError(String),
}
```

### 5. Response processing (per-kind match)

`crates/barnum_config/src/runner/response.rs:52-127`:
```rust
match result {
    SubmitResult::Pool { value, response } => { /* pool-specific response handling */ }
    SubmitResult::Command { value, output } => { /* command-specific response handling */ }
    SubmitResult::Finally { value, output } => { /* finally-specific handling */ }
    SubmitResult::PreHookError(e) => { /* pre-hook error handling */ }
}
```

Note: `Pool` response handling unwraps a `troupe::Response` enum (which has `Processed` and `NotProcessed` variants), while `Command` response handling just processes stdout directly. But both ultimately call the same `process_stdout()` function for the success case. The difference is just the error/timeout wrapping.

## Proposed Design

### The executor trait

Every action kind is an executor. The runner doesn't know what executors exist — it just calls them.

```rust
/// What the runner sends to an executor.
pub struct ExecutorInput {
    pub task: Task,
    pub task_id: LogTaskId,
    /// Pre-hook has already been applied. This is the post-pre-hook value.
    pub value: StepInputValue,
    /// Working directory for the executor.
    pub working_dir: PathBuf,
}

/// What the runner gets back.
pub enum ExecutorOutput {
    /// Stdout from the executor (JSON array of follow-up tasks).
    Success { value: StepInputValue, stdout: String },
    /// Executor-level timeout (e.g., agent didn't respond).
    Timeout { value: StepInputValue },
    /// Executor failed to run at all.
    Error { value: StepInputValue, error: String },
}
```

The key insight: **every executor returns the same thing** — a `StepInputValue` and either stdout (JSON to parse as follow-up tasks) or an error. The runner handles retry logic, schema validation, post-hooks, and state logging uniformly. The executor just runs the action and returns text.

This is already almost true today. Both `Pool` and `Command` ultimately feed into `process_stdout()`. The only divergence is that `Pool` has a `NotProcessed` variant (timeout from troupe), which maps to `ExecutorOutput::Timeout`.

### Executor registration

```rust
/// An action kind executor.
pub trait Executor: Send + Sync {
    /// The action kind name (e.g., "Pool", "Command", "Claude").
    fn kind(&self) -> &str;

    /// Execute a task. Called from a worker thread.
    fn execute(&self, input: ExecutorInput) -> ExecutorOutput;

    /// Validate action parameters at config load time.
    /// Returns an error message if the parameters are invalid.
    fn validate_params(&self, params: &serde_json::Value) -> Result<(), String> {
        let _ = params;
        Ok(()) // Default: no validation
    }
}
```

The runner holds a `HashMap<String, Box<dyn Executor>>`:

```rust
struct TaskRunner<'a> {
    executors: HashMap<String, Box<dyn Executor>>,
    // ... existing fields, minus pool-specific ones
}
```

### Pool becomes an executor

```rust
pub struct PoolExecutor {
    root: PathBuf,
    invoker: Invoker<TroupeCli>,
}

impl Executor for PoolExecutor {
    fn kind(&self) -> &str { "Pool" }

    fn execute(&self, input: ExecutorInput) -> ExecutorOutput {
        let payload = build_agent_payload(&input.task.step, &input.value.0, ...);
        match submit_via_cli(&self.root, &payload, &self.invoker) {
            Ok(Response::Processed { stdout, .. }) => {
                ExecutorOutput::Success { value: input.value, stdout }
            }
            Ok(Response::NotProcessed { .. }) => {
                ExecutorOutput::Timeout { value: input.value }
            }
            Err(e) => {
                ExecutorOutput::Error { value: input.value, error: e.to_string() }
            }
        }
    }
}
```

### Command becomes an executor

```rust
pub struct CommandExecutor;

impl Executor for CommandExecutor {
    fn kind(&self) -> &str { "Command" }

    fn execute(&self, input: ExecutorInput) -> ExecutorOutput {
        let task_json = serde_json::json!({
            "kind": &input.task.step,
            "value": &input.value.0,
        }).to_string();

        match run_command_action(&input.script, &task_json, &input.working_dir) {
            Ok(stdout) => ExecutorOutput::Success { value: input.value, stdout },
            Err(e) => ExecutorOutput::Error { value: input.value, error: e.to_string() },
        }
    }
}
```

### The runner dispatch becomes generic

```rust
fn dispatch(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("unknown step");
    let kind = &step.action_kind; // e.g., "Pool", "Command", "Claude"
    let executor = self.executors.get(kind).expect("unknown action kind");

    // Pre-hook runs here (same for all kinds)
    let value = run_pre_hook_if_present(...);

    let input = ExecutorInput { task, task_id, value, working_dir };
    let executor = Arc::clone(executor);
    let tx = self.tx.clone();

    thread::spawn(move || {
        let output = executor.execute(input);
        tx.send(InFlightResult { identity, result: output });
    });
}
```

### Response processing becomes uniform

```rust
fn process_result(&self, output: ExecutorOutput, task: &Task, step: &Step) -> ProcessedSubmit {
    match output {
        ExecutorOutput::Success { value, stdout } => {
            // Parse stdout as JSON, validate against schema, build post-hook input
            // This is the existing process_stdout() logic — works for ALL kinds
            process_stdout(&stdout, task, &value, step, schemas)
        }
        ExecutorOutput::Timeout { value } => {
            process_retry(task, &step.options, FailureKind::Timeout)
        }
        ExecutorOutput::Error { value, error } => {
            process_retry(task, &step.options, FailureKind::SubmitError)
        }
    }
}
```

No more `match` on action kind in response processing. The runner doesn't care what ran — it just processes the output.

### Config becomes open

The action in config is now just a `kind` string plus opaque parameters:

```rust
// config.rs — the action is no longer a closed enum
pub struct ActionFile {
    pub kind: String,
    #[serde(flatten)]
    pub params: serde_json::Value,
}
```

Or, to preserve editor support for known kinds while allowing unknown ones:

```rust
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<Instructions> },
    Command { script: String },
    Claude { prompt: MaybeLinked<String>, model: Option<String>, ... },
    #[serde(untagged)]
    Custom { kind: String, #[serde(flatten)] params: serde_json::Value },
}
```

The `Custom` variant catches anything not matched by the known variants. At runtime, all variants are normalized to `(kind_name: String, params: Value)` and dispatched through the executor registry.

### Resolved config becomes uniform

```rust
// resolved.rs
pub struct Action {
    pub kind: String,
    pub params: serde_json::Value,
}
```

No more per-kind variants in the resolved config. The executor trait handles interpretation of params.

## What this means for hooks

Hooks (pre, post, finally) are currently Command-only: `{"kind": "Command", "script": "..."}`. With pluggable kinds, hooks could also be pluggable:

```jsonc
{
  "pre": {"kind": "Typescript", "module": "./hooks/enrich.ts"},
  "action": {"kind": "Pool", "instructions": {"kind": "Link", "path": "analyze.md"}},
  "post": {"kind": "Command", "script": "jq '.next'"}
}
```

But this is a stretch goal. The immediate value is making *actions* pluggable. Hooks can stay Command-only for now and get the same treatment later.

## How executors get registered

### Default executors (built-in)

Barnum ships with executors for `Pool` and `Command`. They're registered automatically:

```rust
fn default_executors(pool_config: Option<PoolConfig>) -> HashMap<String, Box<dyn Executor>> {
    let mut map = HashMap::new();
    map.insert("Command".into(), Box::new(CommandExecutor));
    if let Some(pc) = pool_config {
        map.insert("Pool".into(), Box::new(PoolExecutor::new(pc)));
    }
    map
}
```

Note: `Pool` executor is only registered when a pool root is configured. If your workflow is all `Command` and `Claude` steps, no troupe daemon is needed.

### User-registered executors

Users declare custom executors in config:

```jsonc
{
  "executors": {
    "Claude": {
      "command": "claude -p --model {{params.model}} --output-format json",
      "schema": {"link": "schemas/claude-action.json"}
    },
    "Git": {
      "command": "node executors/git.js",
      "schema": {"link": "schemas/git-action.json"}
    }
  },
  "steps": [...]
}
```

Each custom executor is a command that receives `ExecutorInput` as JSON on stdin and writes `ExecutorOutput`-compatible JSON on stdout. This is the same subprocess model as `Command`, but with:
- A parameter schema validated at config load time
- A named kind for config clarity
- Separation of the executor definition from its invocation

### npm packages (future)

```jsonc
{
  "plugins": ["@barnum/executor-claude", "@barnum/executor-git"],
  "steps": [...]
}
```

Each package exports an executor command and schema. Barnum resolves them from `node_modules`. This is the ecosystem play — deferred until there's demand.

## Config schema generation (`barnum-config-schema.json`)

### Current pipeline

```
Rust ActionFile enum (schemars derive)
  → cargo run --bin build_barnum_schema
    → libs/barnum/barnum-config-schema.json
      → editors use for validation/completion
      → CI verifies it matches committed version
```

The `ActionFile` enum uses `#[serde(tag = "kind")]`, so the schema emits a `oneOf` with one variant per enum arm.

### With pluggable kinds

**Built-in kinds** (Pool, Command, and any first-party kinds like Claude) remain as enum variants in Rust. They get automatic `schemars` schema generation and full editor support.

**User-defined kinds** use the `Custom` catch-all variant. The generated schema allows any object with a `kind` string, but provides no field-level validation for custom kinds. Users get:
- Editor completion for built-in kinds (Pool, Command, Claude, etc.)
- No editor completion for custom kinds (but runtime validation via the executor's schema)

**Config-driven schema generation** (future): `barnum build-schema --config workflow.jsonc` reads the config's `"executors"` section, merges their schemas into the base schema, and outputs a project-specific schema file:

```bash
barnum build-schema --config workflow.jsonc > .barnum-schema.json
```

This gives editor support for custom kinds at the cost of a build step.

### Phased approach

- **Phase 1:** Introduce the executor trait. Refactor Pool and Command into executors. No config changes yet — the closed enum still works, but the runner dispatches through the trait instead of hardcoded `match` arms.
- **Phase 2:** Open the config to accept custom kind strings. Add the `"executors"` config section. Add `Custom` catch-all variant.
- **Phase 3:** Add first-party executors (Claude, etc.) as named enum variants with full schema support.
- **Phase 4 (if needed):** `barnum build-schema` for projects that need editor support for custom kinds.

## Finally tasks

Finally tasks are currently dispatched differently from regular tasks — they bypass the pre-hook and use a separate `dispatch_finally_task` function. In the pluggable model, finally tasks should also go through an executor:

```rust
pub struct FinallyExecutor;

impl Executor for FinallyExecutor {
    fn kind(&self) -> &str { "Finally" }

    fn execute(&self, input: ExecutorInput) -> ExecutorOutput {
        let input_json = serde_json::to_string(&input.value.0).unwrap_or_default();
        match run_shell_command(&input.script, &input_json, Some(&input.working_dir)) {
            Ok(stdout) => ExecutorOutput::Success { value: input.value, stdout },
            Err(e) => ExecutorOutput::Error { value: input.value, error: e },
        }
    }
}
```

This removes the special-case `dispatch_finally_task` path and the `SubmitResult::Finally` variant. Finally tasks are just tasks dispatched to the `Finally` executor.

## Relationship to other refactor docs

- **`TYPESCRIPT_API.md`** — TypeScript becomes an executor: `TypescriptExecutor` that runs `.ts` modules via `tsx`.
- **`CLAUDE_CLI_ACTION_KIND.md`** — Claude becomes an executor: `ClaudeExecutor` that spawns `claude` subprocesses.
- **`EXTERNAL_VISUALIZATION.md`** — Executors don't affect visualization. The state log captures task submission/completion regardless of which executor ran.

## Open Questions

1. **Should the executor trait be sync or async?** Currently dispatch uses `thread::spawn`. The `Executor::execute` method is synchronous, called from a worker thread. This is simple and matches the current model. An async trait would add complexity for no clear benefit since barnum already manages concurrency via `max_concurrency`.

2. **How do executor-specific config fields get passed?** In the current model, `Pool` needs `instructions` and `Command` needs `script`. In the pluggable model, these become opaque params (`serde_json::Value`). The executor interprets them. The question is whether the runner should do any preprocessing (like resolving `MaybeLinked` references) or if that's the executor's job.

3. **Should executors be stateful?** `PoolExecutor` holds a troupe root path and invoker. `CommandExecutor` is stateless. The trait allows both. But stateful executors complicate the registration model — you need to construct them with runtime config.

4. **How does this interact with the instruction generation?** Today the runner calls `generate_step_docs(step, config)` to build the agent instructions for Pool tasks. This is runner logic that's Pool-specific. In the pluggable model, this moves into `PoolExecutor::execute`, which would need access to the full step config and the overall workflow config. The `ExecutorInput` would need to carry this.
