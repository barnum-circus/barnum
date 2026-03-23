# Pluggable Action Kinds

## Motivation

Barnum's runner is tightly coupled to the two action kinds it knows about: `Pool` and `Command`. The config types have hardcoded enums for each kind. This means:

1. **Adding a new action kind still requires touching config types.** Config enum (`ActionFile`), resolved enum (`ActionKind`), and the `dispatch_task` match in `mod.rs`. Every new kind is a cross-cutting change in config, even though the runner dispatch is now generic.

2. **Pool and Command are privileged.** They're compiled into the binary. But there's nothing fundamentally special about them — Pool shells out to `troupe submit_task`, Command shells out to `sh -c`. Both are "run a subprocess, get JSON back." They should be pluggable like anything else.

The goal: **Barnum's config and runner should not need modification to add new action kinds.** Pool, Command, Claude, Git, TypeScript — they're all just action implementations registered under a name.

## Current State (post-UNIFIED_ACTION_DISPATCH)

The runner dispatch infrastructure is already trait-based. The remaining coupling is in the config types.

### What's already done (no longer coupling points)

**Action trait and dispatch (`runner/action.rs`):**
```rust
pub trait Action: Send {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle;
}
```

All three dispatch paths (Pool, Command, Finally) go through `Action::start` → `ActionHandle` → `spawn_worker`. The runner doesn't know how to execute actions — it constructs the right `Action` impl and hands it to `spawn_worker`.

**Unified result type (`ActionResult`):**
```rust
pub(super) struct ActionResult {
    pub value: StepInputValue,
    pub output: Result<String, ActionError>,
}
```

No per-kind variants. Every action returns `Result<String, String>` on its channel. `run_action` wraps `Err` into `ActionError::Failed` and timeout into `ActionError::TimedOut`. Response processing (`process_submit_result`) is completely uniform — it doesn't know what ran.

**Cancel-on-drop (`ActionHandle`):**
```rust
pub struct ActionHandle {
    pub rx: mpsc::Receiver<Result<String, String>>,
    drop_guard: Box<dyn Send>,
}
```

`ProcessGuard` uses `Child::kill()` (cross-platform). `PoolAction` uses a no-op guard (troupe manages its own lifecycle). New action kinds just provide their own guard.

**Finally hooks:** Reuse `ShellAction` with `WorkerKind::Finally { parent_id }`. No separate executor or dispatch function.

### What's still coupled

**1. Config enum (closed)**

`crates/barnum_config/src/config.rs`:
```rust
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool(PoolActionFile),
    Command(CommandActionFile),
}
```

**2. Resolved action enum (closed)**

`crates/barnum_config/src/resolved.rs`:
```rust
#[serde(tag = "kind")]
pub enum ActionKind {
    Pool(PoolAction),
    Command(CommandAction),
}
```

**3. Dispatch match (constructs Action impls per kind)**

`crates/barnum_config/src/runner/mod.rs`:
```rust
match &step.action {
    ActionKind::Pool(..) => {
        // Construct PoolAction, call spawn_worker
    }
    ActionKind::Command(CommandAction { script }) => {
        // Construct ShellAction, call spawn_worker
    }
}
```

This match is thin — it just constructs the right `Action` impl with the right fields. But it still needs a new arm for every new kind. The construction logic could be pushed into a factory method or into the config type itself.

## Proposed Design

### Action construction moves into config/resolved types

Instead of matching on `ActionKind` in the runner, the resolved type knows how to construct its own `Action` impl:

```rust
// resolved.rs
impl ActionKind {
    pub fn into_action(
        &self,
        step: &Step,
        config: &Config,
        pool: &PoolConnection,
    ) -> Box<dyn Action> {
        match self {
            Self::Pool(..) => Box::new(PoolAction { ... }),
            Self::Command(CommandAction { script }) => Box::new(ShellAction { ... }),
        }
    }
}
```

This moves the match out of the runner. The runner just calls:
```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("...");
    let action = step.action.into_action(step, self.config, &self.pool);
    let timeout = step.options.timeout.map(Duration::from_secs);
    spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
}
```

But the `match` still lives somewhere — just in the config layer instead of the runner. For true pluggability, the config needs to be open.

### Open config with executor registry

The action in config becomes a `kind` string plus opaque parameters:

```rust
// config.rs — preserve editor support for known kinds
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool(PoolActionFile),
    Command(CommandActionFile),
    #[serde(untagged)]
    Custom { kind: String, #[serde(flatten)] params: serde_json::Value },
}
```

The `Custom` variant catches anything not matched by known variants. At runtime, all variants are normalized to `(kind: String, params: Value)` and dispatched through an executor registry.

### Executor registry

```rust
/// Factory that constructs an Action from config parameters.
pub trait ActionFactory: Send + Sync {
    fn build(
        &self,
        params: &serde_json::Value,
        step_name: &StepName,
        working_dir: &Path,
    ) -> Result<Box<dyn Action>, String>;

    /// Validate action parameters at config load time.
    fn validate_params(&self, params: &serde_json::Value) -> Result<(), String> {
        let _ = params;
        Ok(())
    }
}

struct Engine<'a> {
    factories: HashMap<String, Box<dyn ActionFactory>>,
    // ... existing fields
}
```

The runner holds factories, not action-specific logic. `dispatch_task` looks up the factory by kind string, calls `build`, and passes the result to `spawn_worker`.

### Built-in factories

```rust
fn default_factories(pool: &PoolConnection) -> HashMap<String, Box<dyn ActionFactory>> {
    let mut map = HashMap::new();
    map.insert("Pool".into(), Box::new(PoolActionFactory { ... }));
    map.insert("Command".into(), Box::new(CommandActionFactory { ... }));
    map
}
```

### User-registered executors

Users declare custom action kinds in config:

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

Each custom executor is a command that receives `{"kind": "StepName", "value": <payload>}` on stdin and writes a JSON array of follow-up tasks on stdout — same contract as `Command` actions. The factory constructs a `ShellAction` with the executor's command template.

### npm packages (future)

```jsonc
{
  "plugins": ["@barnum/executor-claude", "@barnum/executor-git"],
  "steps": [...]
}
```

Deferred until there's demand.

## Config schema generation

### Current pipeline

```
Rust ActionFile enum (schemars derive)
  → cargo run --bin build_schemas
    → libs/barnum/barnum-config-schema.json
      → editors use for validation/completion
      → CI verifies it matches committed version
```

### With pluggable kinds

**Built-in kinds** (Pool, Command, and any first-party kinds like Claude) remain as enum variants in Rust. They get automatic `schemars` schema generation and full editor support.

**User-defined kinds** use the `Custom` catch-all variant. The generated schema allows any object with a `kind` string, but provides no field-level validation for custom kinds. Users get:
- Editor completion for built-in kinds (Pool, Command, Claude, etc.)
- No editor completion for custom kinds (but runtime validation via the factory's `validate_params`)

## Phased approach

- **Phase 0 (done):** `Action` trait with `start() -> ActionHandle`, `run_action` with timeout, `spawn_worker`, `PoolAction`, `ShellAction`, `ProcessGuard`. All dispatch goes through the trait. `dispatch.rs` and `shell.rs` deleted. Result processing is uniform.
- **Phase 1:** Move action construction out of the runner — `ActionKind::into_action()` or equivalent. Runner's `dispatch_task` becomes a single generic path with no match.
- **Phase 2:** Open the config to accept custom kind strings. Add `ActionFactory` trait and registry. Add `Custom` catch-all variant and `"executors"` config section.
- **Phase 3:** Add first-party action kinds (Claude, etc.) as named enum variants with full schema support.

## Relationship to other refactor docs

- **`CLAUDE_CLI_ACTION_KIND.md`** — Claude becomes an `Action` impl: `ClaudeAction` that spawns `claude` subprocesses with `ProcessGuard` for kill-on-drop. Uses the same `ActionHandle` pattern as `ShellAction`.
- **`EXTERNAL_VISUALIZATION.md`** — Action kinds don't affect visualization. The state log captures task submission/completion regardless of which action ran.

## Open Questions

1. **Should `ActionFactory` receive the full step config?** `PoolAction` needs `generate_step_docs(step, config)` which requires the full config. The factory interface needs to be wide enough for this without leaking runner internals.

2. **How do executor-specific config fields get passed?** In the current model, `Pool` needs `instructions` and `Command` needs `script`. In the pluggable model, these become opaque params (`serde_json::Value`). The factory interprets them. The question is whether the runner should do any preprocessing (like resolving `MaybeLinked` references) or if that's the factory's job.

3. **Should factories be stateful?** `PoolAction` needs a root path and invoker. `ShellAction` needs a working directory. The factory allows both stateful and stateless construction. But stateful factories complicate the registration model.
