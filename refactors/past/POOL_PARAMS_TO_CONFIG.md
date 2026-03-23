# Pool Parameters to Config

**Blocks:** ACTION_REGISTRY, JS_ACTION_RESOLUTION

## Motivation

Pool root and pool name are currently global CLI arguments (`--root`, `--pool`) passed to `barnum run`. Every Pool action step uses the same pool. This creates two problems:

1. Barnum (the engine) knows about pool infrastructure. It shouldn't. Barnum manages a task tree and dispatches actions. Where the pool lives is the Pool action's business.

2. Different steps can't use different pools. A workflow where "Analyze" goes to a fast-turnaround pool and "Review" goes to a senior-reviewer pool isn't expressible.

This refactor moves pool root and pool name into the Pool action's config, and removes the special timeout passthrough from barnum to troupe.

## Current State

### CLI args (`crates/barnum_cli/src/lib.rs:36-49`)

```rust
pub struct Cli {
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,  // troupe root directory
    // ...
    // In Command::Run:
    #[arg(long)]
    pub pool: Option<String>,   // pool ID, defaults to "default"
}
```

### RunnerConfig (`crates/barnum_config/src/runner/mod.rs:39-50`)

```rust
pub struct RunnerConfig<'a> {
    pub troupe_root: &'a Path,      // full pool path: <root>/pools/<pool_id>
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub invoker: &'a Invoker<TroupeCli>,
    pub state_log_path: &'a Path,
}
```

`troupe_root` is the resolved pool path (`<root>/pools/<pool_id>`). It flows into `PoolConnection.root`, which every `PoolAction` clones.

### Pool path resolution (`crates/barnum_cli/src/main.rs:271-283`)

```rust
fn resolve_pool_path(pool_id: &str, root: &Path) -> io::Result<PathBuf> {
    if pool_id.contains('/') {
        return Err(/* ... */);
    }
    Ok(troupe::resolve_pool(root, pool_id))
}
```

### Timeout passthrough (`crates/barnum_config/src/runner/submit.rs:17-26`)

```rust
pub fn build_agent_payload(/* ... */, timeout: Option<u64>) -> String {
    let mut payload = serde_json::json!({
        "task": { "kind": step_name, "value": value },
        "instructions": docs,
    });
    if let Some(t) = timeout {
        payload["timeout_seconds"] = serde_json::json!(t);
    }
    // ...
}
```

`step.options.timeout` serves double duty:
- Barnum worker timeout (how long barnum waits for the action to complete)
- Troupe payload `timeout_seconds` (how long the agent gets to work)

These are different concerns. Barnum's timeout controls barnum's behavior. The agent timeout is a pool-specific parameter that belongs in the pool action config.

### Hardcoded troupe CLI timeout (`submit.rs:64`)

```rust
invoker.run(["submit_task", /* ... */, "--timeout-secs", "86400", /* ... */])
```

24-hour placeholder. Should come from the pool action config or be omitted entirely.

### JS layer (`libs/barnum/run.ts:39-51`)

```typescript
run(opts?: RunOptions): ChildProcess {
    // ...
    if (opts?.pool) args.push("--pool", opts.pool);
    if (opts?.root) args.push("--root", opts.root);
    // ...
}
```

### Pool action config (`crates/barnum_config/src/config.rs:131-138`)

```rust
pub struct PoolActionFile {
    pub instructions: MaybeLinked<Instructions>,
    // no pool or root fields
}
```

## Proposed Changes

### 1. Add pool and root to PoolActionFile

**File:** `crates/barnum_config/src/config.rs`

All fields are optional. If pool/root aren't provided, they aren't passed to troupe — troupe uses its own defaults.

```rust
pub struct PoolActionFile {
    pub instructions: MaybeLinked<Instructions>,
    /// Pool name (e.g., "demo", "reviewers"). If omitted, troupe uses its default.
    #[serde(default)]
    pub pool: Option<String>,
    /// Troupe root directory. If omitted, troupe uses its default.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Agent lifecycle timeout in seconds. Passed to troupe as `timeout_seconds`
    /// in the payload. Controls how long the agent gets to work on the task.
    /// Separate from the step-level timeout which controls barnum's worker timeout.
    #[serde(default)]
    pub timeout: Option<u64>,
}
```

### 2. Add pool, root, and timeout to resolved PoolAction

**File:** `crates/barnum_config/src/resolved.rs`

```rust
pub struct PoolAction {
    pub instructions: String,
    pub pool: Option<String>,
    pub root: Option<PathBuf>,
    pub timeout: Option<u64>,
}
```

### 3. Update config resolution

**File:** `crates/barnum_config/src/config.rs`, in `ActionFile::resolve`

```rust
Self::Pool(PoolActionFile { instructions, pool, root, timeout }) => {
    let resolved: Instructions = instructions.resolve(base_path, |path| {
        let content = std::fs::read_to_string(path)?;
        Ok(Instructions(content))
    })?;
    Ok(ActionKind::Pool(PoolAction {
        instructions: resolved.0,
        pool,
        root,
        timeout,
    }))
}
```

### 4. Timeout comes from pool action, not step options

**File:** `crates/barnum_config/src/runner/submit.rs`

`build_agent_payload` takes the pool action's timeout instead of the step timeout. If `timeout` is set, it's included in the payload as `timeout_seconds`. Otherwise it's omitted.

```rust
pub fn build_agent_payload(
    step_name: &StepName,
    value: &serde_json::Value,
    docs: &str,
    pool_timeout: Option<u64>,
) -> String {
    let mut payload = serde_json::json!({
        "task": { "kind": step_name, "value": value },
        "instructions": docs,
    });
    if let Some(t) = pool_timeout {
        payload["timeout_seconds"] = serde_json::json!(t);
    }
    serde_json::to_string(&payload).unwrap_or_default()
}
```

The caller passes `pool_action.timeout` instead of `step.options.timeout`. The step-level timeout now exclusively controls barnum's worker timeout.

### 5. Use pool/root from PoolAction in dispatch

**File:** `crates/barnum_config/src/runner/action.rs`

`PoolAction` gets pool/root from the action config instead of from `PoolConnection`:

```rust
pub struct PoolAction {
    pub root: Option<PathBuf>,
    pub pool: Option<String>,
    pub timeout: Option<u64>,
    pub invoker: Invoker<TroupeCli>,
    pub docs: String,
    pub step_name: StepName,
}
```

**File:** `crates/barnum_config/src/runner/submit.rs`

`submit_via_cli` takes optional root and pool. Only passes them to troupe if present:

```rust
pub fn submit_via_cli(
    root: Option<&Path>,
    pool: Option<&str>,
    payload: &str,
    invoker: &Invoker<TroupeCli>,
) -> io::Result<Response> {
    let mut args = vec!["submit_task"];
    if let Some(root) = root {
        args.extend(["--root", root.to_str().unwrap_or(".")]);
    }
    if let Some(pool) = pool {
        args.extend(["--pool", pool]);
    }
    args.extend(["--notify", "file", "--data", payload]);
    let output = invoker.run(args)?;
    // ...
}
```

The `--timeout-secs 86400` is removed. If troupe needs a CLI timeout, it should be a troupe-side default.

### 6. Remove troupe_root from RunnerConfig

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub invoker: &'a Invoker<TroupeCli>,
    pub state_log_path: &'a Path,
    // troupe_root removed
}
```

`PoolConnection` loses its `root` field (or is deleted if ACTION_REGISTRY lands first). The invoker still needs to come from somewhere — it stays on RunnerConfig for now and gets passed to dispatch.

### 7. Remove --pool and --root from CLI

**File:** `crates/barnum_cli/src/lib.rs`

Remove `--root` (global arg) and `--pool` (run arg). Remove `resolve_pool_path`. The `has_pool_actions()` check that validates pool availability is no longer needed — if a Pool action has a bad root, it fails at dispatch time.

### 8. Update JS layer

**File:** `libs/barnum/run.ts`

Remove `pool` and `root` from `RunOptions`. They're no longer CLI args.

```typescript
export interface RunOptions {
    entrypointValue?: string;
    resumeFrom?: string;
    logLevel?: string;
    logFile?: string;
    stateLog?: string;
    wake?: string;
    // pool and root removed
}
```

### 9. Update demo configs

Each demo's `config.json` can optionally add `pool` and `root` fields on Pool actions. Since these now have defaults (`"default"` and `/tmp/troupe`), demos that use the standard pool can omit them entirely:

```json
{
    "action": {
        "kind": "Pool",
        "instructions": {"kind": "Link", "path": "instructions.md"},
        "pool": "demo"
    }
}
```

Demos that need a specific pool name add the `pool` field. Root is only needed if it's non-standard.

The `run-demo.ts` files drop the `ROOT` and `POOL` env vars:

```typescript
// Before
BarnumConfig.fromConfig(require("./config.json"))
  .run({ pool: process.env.POOL, root: process.env.ROOT })
  .on("exit", (code) => process.exit(code ?? 1));

// After
BarnumConfig.fromConfig(require("./config.json"))
  .run()
  .on("exit", (code) => process.exit(code ?? 1));
```

### 10. Update integration tests

**File:** `crates/barnum_cli/tests/common/mod.rs`

`BarnumCli::run()` currently takes `pool_root` and passes it as `--root`/`--pool` CLI args to barnum (lines 440-464). After this refactor, the pool root and pool name need to be in the config JSON instead.

The test helper changes: `BarnumCli::run()` drops the `pool_root` parameter. Test configs (constructed as inline JSON strings) gain `"pool"` and `"root"` fields on their Pool actions. The helper could inject these automatically, or each test can include them in its config string.

The troupe CLI invocations (`TroupeHandle::start`, `stop_stale_daemon`, `FileWriterAgent`) still use `--root`/`--pool` — those are troupe args, not barnum args, and don't change.

### 11. Regenerate schemas

`cargo run -p barnum_cli --bin build_schemas` to update:
- `libs/barnum/barnum-config-schema.json`
- `libs/barnum/barnum-config-schema.zod.ts`
- `libs/barnum/barnum-cli-schema.zod.ts`

## What stays the same

- `step.options.timeout` — still controls barnum's worker timeout (how long barnum waits before killing the action). This is barnum's concern and stays in step options.
- `Config::has_pool_actions()` — still works, just checks for ActionKind::Pool variant.
- Step doc generation — `generate_step_docs` unchanged.
- State log format — the resolved config (serialized into the state log) now includes pool/root per PoolAction. Resume deserializes these and uses them.

## Open questions

1. **Invoker.** The `Invoker<TroupeCli>` (which locates the troupe binary) currently comes from RunnerConfig. With pool params in config, it still needs to come from somewhere outside the config. It stays on RunnerConfig for now. In the JS_ACTION_RESOLUTION future, the JS pool executor script handles binary discovery.
