# Remove Pre and Post Hooks

**Lands before:** UNIFIED_ACTION_DISPATCH (simplifies Phase 0 significantly by eliminating `PreHookError`, `PostHookInput`, `process_and_finalize`).

## Motivation

Pre and post hooks are unused outside a single demo config (`crates/barnum_cli/demos/hooks/config.jsonc`). They add substantial complexity: `PreHookError` variant on `SubmitResult`, `PostHookInput` enum with four variants, `process_and_finalize` wrapping the result pipeline, main-thread blocking for post-hooks, and the entire `hooks.rs` module (aside from `run_command_action` which is used by command actions).

Nobody uses them. Delete them.

## Current Code

### Config types (`config.rs`)

```rust
// config.rs:109-130
pub pre: Option<PreHook>,
pub post: Option<PostHook>,

// config.rs:212-228
pub enum PreHook {
    Command(HookCommand),
}

pub enum PostHook {
    Command(HookCommand),
}
```

Resolved in `config.rs:444-451`:
```rust
pre: self.pre.map(|h| {
    let PreHook::Command(HookCommand { script }) = h;
    HookScript::new(script)
}),
post: self.post.map(|h| {
    let PostHook::Command(HookCommand { script }) = h;
    HookScript::new(script)
}),
```

### Resolved types (`resolved.rs`)

```rust
// resolved.rs:50-57
pub pre: Option<HookScript>,
pub post: Option<HookScript>,
```

### Dispatch (`dispatch.rs`)

`SubmitResult::PreHookError(String)` variant exists solely because pre-hooks can fail before the action runs.

`process_and_finalize` (`dispatch.rs:77-110`) wraps `process_submit_result` with post-hook execution. The post-hook runs on the main thread, blocking the event loop.

`run_pre_hook_or_error` (`dispatch.rs:63-72`) is called at the top of both `dispatch_pool_task` and `dispatch_command_task`.

`extract_next_tasks` (`dispatch.rs:113-120`) extracts tasks from `PostHookInput::Success`.

### Hooks module (`hooks.rs`)

- `PostHookInput` enum (4 variants: `Success`, `Timeout`, `Error`, `PreHookError`)
- `PostHookSuccess`, `PostHookTimeout`, `PostHookError`, `PostHookPreHookError` structs
- `run_pre_hook` function
- `run_post_hook` function
- `run_command_action` function (used by command tasks, NOT a hook — stays)

### Response processing (`response.rs`)

Every function returns `(TaskOutcome, PostHookInput)` tuple — the `PostHookInput` exists solely to feed the post-hook. Without post-hooks, these functions can return just `TaskOutcome`.

- `process_submit_result` returns `ProcessedSubmit { outcome, post_input }`
- `process_pool_response` returns `(TaskOutcome, PostHookInput)`
- `process_command_response` returns `(TaskOutcome, PostHookInput)`
- `process_stdout` returns `(TaskOutcome, PostHookInput)`
- `process_finally_response` returns `(TaskOutcome, PostHookInput)`

### Runner (`mod.rs`)

`dispatch_task` passes `step.pre.clone()` to dispatch functions (`mod.rs:724, 743`).

`convert_task_result` calls `process_and_finalize` which runs post-hooks (`mod.rs:550-556`).

### CLI (`main.rs`)

Validation prints warnings if steps have pre/post hooks (`main.rs:483-486`).

### Schema

`pre` and `post` fields in the JSON schema and Zod types.

## Proposed Changes

### Phase 1: Delete hook types and functions

**File: `hooks.rs`**

Before:
```rust
pub struct PostHookSuccess { pub input: StepInputValue, pub output: serde_json::Value, pub next: Vec<Task> }
pub struct PostHookTimeout { pub input: StepInputValue }
pub struct PostHookError { pub input: StepInputValue, pub error: String }
pub struct PostHookPreHookError { pub input: StepInputValue, pub error: String }

pub enum PostHookInput {
    Success(PostHookSuccess),
    Timeout(PostHookTimeout),
    Error(PostHookError),
    PreHookError(PostHookPreHookError),
}

pub fn run_pre_hook(...) -> Result<serde_json::Value, String> { ... }
pub fn run_post_hook(...) -> Result<PostHookInput, String> { ... }
```

After: all deleted. `run_command_action` stays (it's used by command task dispatch, not hooks).

### Phase 2: Delete hook infrastructure from dispatch

**File: `dispatch.rs`**

Delete `run_pre_hook_or_error` (`dispatch.rs:63-72`):
```rust
// DELETED
fn run_pre_hook_or_error(
    pre_hook: Option<&HookScript>,
    original_value: &StepInputValue,
    working_dir: &Path,
) -> Result<StepInputValue, String> { ... }
```

Delete `SubmitResult::PreHookError`:

Before:
```rust
pub(super) enum SubmitResult {
    Pool(PoolResult),
    Command(CommandResult),
    Finally(FinallyResult),
    PreHookError(String),
}
```

After:
```rust
pub(super) enum SubmitResult {
    Pool(PoolResult),
    Command(CommandResult),
    Finally(FinallyResult),
}
```

Delete `process_and_finalize` entirely (`dispatch.rs:77-110`). Delete `extract_next_tasks` (`dispatch.rs:113-120`).

Remove pre-hook from `dispatch_pool_task`:

Before (`dispatch.rs:126-156`):
```rust
pub fn dispatch_pool_task(
    task_id: LogTaskId,
    task: Task,
    pre_hook: Option<&HookScript>,
    docs: &str,
    timeout: Option<u64>,
    pool: &super::PoolConnection,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = match run_pre_hook_or_error(pre_hook, &task.value, &pool.working_dir) {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(WorkerResult { task_id, task, result: SubmitResult::PreHookError(e) });
            return;
        }
    };
    let payload = build_agent_payload(&task.step, &value.0, docs, timeout);
    // ...
}
```

After:
```rust
pub fn dispatch_pool_task(
    task_id: LogTaskId,
    task: Task,
    docs: &str,
    timeout: Option<u64>,
    pool: &super::PoolConnection,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let payload = build_agent_payload(&task.step, &task.value.0, docs, timeout);
    // ...
}
```

Same for `dispatch_command_task` — remove `pre_hook` parameter, remove `run_pre_hook_or_error` call, use `task.value` directly.

### Phase 3: Simplify response processing

**File: `response.rs`**

Remove `PostHookInput` from return types. Every function that returned `(TaskOutcome, PostHookInput)` now returns just `TaskOutcome`.

Before (`response.rs:44-48`):
```rust
pub struct ProcessedSubmit {
    pub outcome: TaskOutcome,
    pub post_input: PostHookInput,
}
```

After: delete `ProcessedSubmit`. `process_submit_result` returns `TaskOutcome` directly.

Before (`response.rs:51-132`, `process_submit_result`):
```rust
pub fn process_submit_result(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
) -> ProcessedSubmit {
    match result {
        SubmitResult::Pool(PoolResult { value, response }) => match response {
            Ok(response) => {
                let (outcome, post_input) =
                    process_pool_response(response, task, &value, step, schemas);
                ProcessedSubmit { outcome, post_input }
            }
            Err(e) => {
                error!(...);
                let outcome = process_retry(task, &step.options, FailureKind::SubmitError);
                ProcessedSubmit {
                    outcome,
                    post_input: PostHookInput::Error(PostHookError { input: value, error: e.to_string() }),
                }
            }
        },
        SubmitResult::Command(CommandResult { value, output }) => match output {
            Ok(stdout) => {
                let (outcome, post_input) =
                    process_command_response(&stdout, task, &value, step, schemas);
                ProcessedSubmit { outcome, post_input }
            }
            Err(e) => { ... }
        },
        SubmitResult::PreHookError(e) => { ... },
        SubmitResult::Finally(FinallyResult { value, output }) => { ... },
    }
}
```

After:
```rust
pub fn process_submit_result(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
) -> TaskOutcome {
    match result {
        SubmitResult::Pool(PoolResult { value, response }) => match response {
            Ok(response) => process_pool_response(response, task, &value, step, schemas),
            Err(e) => {
                error!(...);
                process_retry(task, &step.options, FailureKind::SubmitError)
            }
        },
        SubmitResult::Command(CommandResult { value, output }) => match output {
            Ok(stdout) => process_command_response(&stdout, task, &value, step, schemas),
            Err(e) => {
                error!(...);
                process_retry(task, &step.options, FailureKind::SubmitError)
            }
        },
        SubmitResult::Finally(FinallyResult { value, output }) => match output {
            Ok(stdout) => process_finally_response(&stdout, task, &value),
            Err(e) => {
                error!(...);
                process_retry(task, &step.options, FailureKind::SubmitError)
            }
        },
    }
}
```

All internal functions (`process_pool_response`, `process_command_response`, `process_stdout`, `process_finally_response`) change from returning `(TaskOutcome, PostHookInput)` to returning just `TaskOutcome`. Delete all `PostHookInput` construction.

### Phase 4: Simplify engine

**File: `mod.rs`**

`convert_task_result` currently calls `process_and_finalize`. Replace with direct call to `process_submit_result`.

Before (`mod.rs:550-556`):
```rust
let outcome = process_and_finalize(
    submit_result,
    task,
    step,
    self.schemas,
    &self.pool.working_dir,
);
```

After:
```rust
let outcome = process_submit_result(
    submit_result,
    task,
    step,
    self.schemas,
);
```

Remove `pre_hook` from `dispatch_task`:

Before (`mod.rs:722-758`):
```rust
Action::Pool(..) => {
    let pre_hook = step.pre.clone();
    let docs = generate_step_docs(step, self.config);
    let timeout = step.options.timeout;
    let pool = self.pool.clone();

    thread::spawn(move || {
        dispatch_pool_task(
            task_id, task, pre_hook.as_ref(),
            &docs, timeout, &pool, &tx,
        );
    });
}
Action::Command(CommandAction { script }) => {
    let pre_hook = step.pre.clone();
    let script = script.clone();
    let working_dir = self.pool.working_dir.clone();

    thread::spawn(move || {
        dispatch_command_task(
            task_id, task, pre_hook.as_ref(),
            &script, &working_dir, &tx,
        );
    });
}
```

After:
```rust
Action::Pool(..) => {
    let docs = generate_step_docs(step, self.config);
    let timeout = step.options.timeout;
    let pool = self.pool.clone();

    thread::spawn(move || {
        dispatch_pool_task(
            task_id, task,
            &docs, timeout, &pool, &tx,
        );
    });
}
Action::Command(CommandAction { script }) => {
    let script = script.clone();
    let working_dir = self.pool.working_dir.clone();

    thread::spawn(move || {
        dispatch_command_task(
            task_id, task,
            &script, &working_dir, &tx,
        );
    });
}
```

### Phase 5: Delete from config and schema

**File: `config.rs`**

Delete `PreHook` enum, `PostHook` enum, `HookCommand` struct.

Delete `pre` and `post` fields from `StepFile`:

Before (`config.rs:109, 130`):
```rust
pub pre: Option<PreHook>,
pub post: Option<PostHook>,
```

After: fields deleted.

Delete resolution code (`config.rs:444-451`):
```rust
// DELETED
pre: self.pre.map(|h| { ... }),
post: self.post.map(|h| { ... }),
```

**File: `resolved.rs`**

Delete `pre` and `post` from `Step`:

Before (`resolved.rs:50-57`):
```rust
pub pre: Option<HookScript>,
pub post: Option<HookScript>,
```

After: fields deleted. `finally_hook` stays.

**File: `main.rs`**

Delete hook validation warnings (`main.rs:483-486`).

**Schema regeneration:** Run `cargo run -p barnum_cli --bin build_schemas` to update JSON schema and Zod types.

**Demo config:** Delete `crates/barnum_cli/demos/hooks/` directory entirely (the only consumer of pre/post hooks).

### Phase 6: Cleanup

- Delete `HookScript` import from `dispatch.rs` (only used for pre-hook parameter type)
- Remove unused imports from `hooks.rs`, `response.rs`
- If `hooks.rs` only has `run_command_action` left, consider inlining it into `dispatch.rs` and deleting the module
- Run `cargo clippy` to catch any remaining dead code

## What stays

- **`finally_hook`**: Stays on `Step`. Finally hooks are first-class, logged in the state log, and actively used.
- **`HookScript` type**: Stays in `barnum_types`. Used by `finally_hook`.
- **`run_shell_command`**: Stays. Used by command actions and finally hooks.
- **`run_command_action`**: Stays (or gets inlined). Used by command action dispatch.
