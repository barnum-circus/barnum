# Refactor: Barnum Uses task_queue

## Goal

Make `barnum_config` use `task_queue`'s execution engine. Barnum will have a single dynamic task type that implements `QueueItem`.

**Design principle:** The `QueueItem` trait is minimal. Pre-hooks, post-hooks, finally-hooks, retry logic — all of that is Barnum's business. Barnum "compiles" its rich config-driven behavior into simple queue operations. The trait knows nothing about hooks or finalization.

## Current State

### task_queue

```rust
pub trait QueueItem<Context>: Sized {
    type InProgress;
    type Response: DeserializeOwned;
    type NextTasks;

    fn start(self, ctx: &mut Context) -> (Self::InProgress, Command);
    fn process(in_progress: Self::InProgress, result: Result<Self::Response, _>, ctx: &mut Context) -> Self::NextTasks;
}
```

**Problem:** `start()` returns a `Command`, which is executed locally. Barnum needs to either:
- Submit to troupe and wait for response
- Run a local command with pre/post hooks

### barnum_config

Has its own `TaskRunner` that manages:
- Queue of tasks
- Concurrent execution via thread spawning
- Pre/post/finally hooks
- Retry logic
- Timeout handling

## Proposed Changes

### Change 1: Replace `Command` with `BoxFuture`

Instead of returning a `Command` to execute, return a future that resolves to the response:

```rust
// task_queue/src/lib.rs

pub trait QueueItem<Context>: Sized {
    type InProgress;
    type Response: DeserializeOwned;
    type NextTasks;

    /// Start executing the task. Returns in-progress state and a future that resolves to the response.
    fn start(self, ctx: &mut Context) -> (Self::InProgress, BoxFuture<'static, TaskOutput<Self::Response>>);

    /// Process the result and return follow-up tasks.
    fn process(
        in_progress: Self::InProgress,
        result: TaskOutput<Self::Response>,
        ctx: &mut Context,
    ) -> Self::NextTasks;
}

/// Output from executing a task.
pub enum TaskOutput<T> {
    /// Task completed successfully with deserialized response.
    Success(T),
    /// Task timed out.
    Timeout,
    /// Response failed to deserialize.
    InvalidResponse(serde_json::Error),
    /// I/O or execution error.
    Error(std::io::Error),
}
```

That's the entire trait. Two methods. No `finally()`, no `has_finally()`. task_queue is a dumb execution engine that runs tasks, collects results, and feeds them back.

**Why this works:**
- Barnum can return a future that submits to troupe, waits for response file, parses JSON
- Typed task_queue users can return a future that spawns a local command
- Both use the same `TaskRunner` execution engine
- task_queue doesn't need to know about hooks, retries, or finalization

### Change 2: Keep `Command` helper for simple cases

For users who just want to run a local command (the common case for typed task_queue):

```rust
// task_queue/src/lib.rs

/// Helper to create a future from a Command.
pub fn run_command<T: DeserializeOwned>(cmd: Command) -> BoxFuture<'static, TaskOutput<T>> {
    Box::pin(async move {
        let output = TokioCommand::from(cmd)
            .stdout(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                match serde_json::from_slice(&out.stdout) {
                    Ok(val) => TaskOutput::Success(val),
                    Err(e) => TaskOutput::InvalidResponse(e),
                }
            }
            Ok(_) => TaskOutput::Error(io::Error::other("command failed")),
            Err(e) => TaskOutput::Error(e),
        }
    })
}
```

Typed task_queue usage remains clean:

```rust
impl QueueItem<Ctx> for AnalyzeFile {
    // ...
    fn start(self, ctx: &mut Ctx) -> (Self::InProgress, BoxFuture<'static, TaskOutput<Self::Response>>) {
        let mut cmd = Command::new("./analyze.sh");
        cmd.arg(&self.path);
        (AnalyzeInProgress { path: self.path }, run_command(cmd))
    }
}
```

### Change 3: Barnum implements `QueueItem` dynamically

Barnum compiles all its complexity (pre-hooks, post-hooks, finally tracking, retries) into plain `QueueItem` operations. The trait sees none of it.

```rust
// barnum_config/src/queue_item.rs

impl QueueItem<BarnumContext> for Task {
    type InProgress = TaskInProgress;
    type Response = serde_json::Value;
    type NextTasks = Vec<Task>;

    fn start(self, ctx: &mut BarnumContext) -> (Self::InProgress, BoxFuture<'static, TaskOutput<Value>>) {
        let step = ctx.step_for(&self.step).clone();

        // Run pre-hook (synchronously, before spawning)
        let effective_value = match &step.pre {
            Some(pre) => match run_pre_hook(pre, &self.value) {
                Ok(v) => v,
                Err(e) => {
                    // Pre-hook failed - return immediate error
                    return (
                        TaskInProgress::pre_hook_failed(self, e.clone()),
                        Box::pin(async move { TaskOutput::Error(io::Error::other(e)) }),
                    );
                }
            },
            None => self.value.clone(),
        };

        // Build the future based on action type
        let future: BoxFuture<'static, TaskOutput<Value>> = match &step.action {
            Action::Command { script } => {
                run_local_command(script.clone(), effective_value.clone())
            }
            Action::Pool { .. } => {
                submit_to_troupe(
                    ctx.pool_path.clone(),
                    self.step.clone(),
                    effective_value.clone(),
                    &step,
                    ctx.config_base_path.clone(),
                )
            }
        };

        (
            TaskInProgress {
                task: self,
                effective_value,
                step,
            },
            future,
        )
    }

    fn process(
        ip: Self::InProgress,
        result: TaskOutput<Value>,
        ctx: &mut BarnumContext,
    ) -> Vec<Task> {
        // Handle the result
        let (post_input, raw_tasks) = match result {
            TaskOutput::Success(response) => {
                match validate_response(&response, &ip.step, &ctx.schemas) {
                    Ok(tasks) => (
                        PostHookInput::Success {
                            input: ip.effective_value.clone(),
                            output: response,
                            next: tasks.clone(),
                        },
                        tasks,
                    ),
                    Err(e) => {
                        // Invalid transition or schema violation
                        (
                            PostHookInput::Error {
                                input: ip.effective_value.clone(),
                                error: e.to_string(),
                            },
                            vec![],
                        )
                    }
                }
            }
            TaskOutput::Timeout => (
                PostHookInput::Timeout {
                    input: ip.effective_value.clone(),
                },
                handle_retry_or_drop(&ip.task, ctx, FailureKind::Timeout),
            ),
            TaskOutput::InvalidResponse(e) => (
                PostHookInput::Error {
                    input: ip.effective_value.clone(),
                    error: e.to_string(),
                },
                handle_retry_or_drop(&ip.task, ctx, FailureKind::InvalidResponse),
            ),
            TaskOutput::Error(e) => (
                PostHookInput::Error {
                    input: ip.effective_value.clone(),
                    error: e.to_string(),
                },
                vec![],
            ),
        };

        // Run post-hook if configured
        let mut final_tasks = match &ip.step.post {
            Some(post) => match run_post_hook(post, &post_input) {
                Ok(modified) => extract_next_tasks(&modified),
                Err(_) => raw_tasks, // Post hook failed, use original tasks
            },
            None => raw_tasks,
        };

        // Finally tracking: register descendants if this step has a finally hook
        if let Some(finally_hook) = &ip.step.finally_hook {
            if !final_tasks.is_empty() {
                ctx.finally_tracker.register(
                    ip.task.id,
                    final_tasks.len(),
                    finally_hook.clone(),
                    ip.effective_value.clone(),
                );
                // Tag children so we can track when they complete
                for task in &mut final_tasks {
                    task.finally_origin = Some(ip.task.id);
                }
            }
        }

        // Check if any ancestor's finally is now ready to fire
        if let Some(origin_id) = ip.task.finally_origin {
            if let Some(finally_tasks) = ctx.finally_tracker.descendant_completed(origin_id) {
                final_tasks.extend(finally_tasks);
            }
        }

        final_tasks
    }
}
```

## Hooks: All User Land

| Hook | Where it runs | Responsibility |
|------|---------------|----------------|
| **pre** | In `start()`, before returning future | Barnum only |
| **post** | In `process()`, after getting result | Barnum only |
| **finally** | In `process()`, via `FinallyTracker` in context | Barnum only |

**All three hooks are Barnum-specific implementation details inside the `QueueItem` impl.** task_queue knows nothing about any of them.

## Finally: Compiled Into User Land

### The Problem

When a task spawns children, sometimes you want to run cleanup/aggregation after *all* descendants complete. This is `finally`. It's a Barnum workflow concept, not a generic queue concept.

### Why it doesn't belong in the trait

Previous design had `finally()` as a method on `QueueItem`. This is wrong because:

1. **It infects the trait with domain logic.** `finally` is a Barnum concept — "run a shell script after all descendants finish." A generic task queue shouldn't know about this.
2. **It forces task_queue to track parent-child relationships.** That's Barnum's concern. task_queue should be a flat list of tasks, each executing independently.
3. **It makes the trait harder to implement.** Every `QueueItem` implementor has to think about `finally` even if they don't use it.

### How Barnum handles it instead

Barnum tracks finally state in `BarnumContext`, not in the trait:

```rust
/// Lives in BarnumContext — invisible to task_queue.
pub struct FinallyTracker {
    /// Tasks waiting for all descendants to complete.
    pending: HashMap<TaskId, FinallyState>,
}

struct FinallyState {
    /// Number of direct children still pending.
    remaining_children: usize,
    /// The finally hook command to run.
    hook: FinallyHook,
    /// The original task's value (input to finally hook).
    original_value: StepInputValue,
}

impl FinallyTracker {
    /// Register a task that has a finally hook and spawned children.
    fn register(&mut self, task_id: TaskId, child_count: usize, hook: FinallyHook, value: StepInputValue) {
        self.pending.insert(task_id, FinallyState {
            remaining_children: child_count,
            hook,
            original_value: value,
        });
    }

    /// Called when a descendant of a tracked task completes.
    /// Returns Some(tasks) if the finally hook should fire now.
    fn descendant_completed(&mut self, origin_id: TaskId) -> Option<Vec<Task>> {
        let state = self.pending.get_mut(&origin_id)?;
        state.remaining_children -= 1;

        if state.remaining_children == 0 {
            let state = self.pending.remove(&origin_id).unwrap();
            match run_finally_hook(&state.hook, &state.original_value) {
                Ok(tasks) => Some(tasks),
                Err(e) => {
                    warn!(error = %e, "finally hook failed");
                    Some(vec![])
                }
            }
        } else {
            None
        }
    }
}
```

The flow:

1. Task completes, `process()` returns child tasks
2. If step has `finally` hook AND children were spawned → register in `ctx.finally_tracker`
3. Tag each child task with `finally_origin = Some(parent_id)`
4. When any child completes, `process()` checks `ctx.finally_tracker.descendant_completed()`
5. When count reaches 0, finally hook fires, returned tasks are appended to `process()`'s output
6. Finally-spawned tasks do NOT inherit `finally_origin` (prevents infinite tracking)

**task_queue sees none of this.** It just sees tasks going in and tasks coming out. The finally logic is entirely within Barnum's `process()` implementation and its `BarnumContext`.

### What about typed task_queue users who want finally?

They implement the same pattern in their own `process()` + context:

```rust
struct MyContext {
    finally_tracker: FinallyTracker<MyTask>,
    // ...
}

impl QueueItem<MyContext> for MyTask {
    fn process(ip: Self::InProgress, result: TaskOutput<Value>, ctx: &mut MyContext) -> Vec<MyTask> {
        let children = compute_children(&result);

        // Register for aggregation after all children complete
        if !children.is_empty() {
            ctx.finally_tracker.register(ip.id, children.len(), AggregateTask { source: ip.id });
        }

        // Check if a parent's finally is ready
        if let Some(origin) = ip.finally_origin {
            if let Some(tasks) = ctx.finally_tracker.descendant_completed(origin) {
                children.extend(tasks);
            }
        }

        children
    }
}
```

If this pattern proves common, we could publish `FinallyTracker` as a utility in task_queue, but it's not part of the trait.

## BarnumContext

Consolidate Barnum's scattered state into a single context:

```rust
pub struct BarnumContext {
    pub config: Config,
    pub schemas: CompiledSchemas,
    pub pool_path: PathBuf,
    pub config_base_path: PathBuf,
    pub finally_tracker: FinallyTracker,
}

impl BarnumContext {
    pub fn step_for(&self, name: &StepName) -> Option<&Step> {
        self.config.step_map().get(name.as_str()).copied()
    }

    pub fn effective_options(&self, step: &Step) -> EffectiveOptions {
        EffectiveOptions::resolve(&self.config.options, &step.options)
    }
}
```

## Implementation Plan

### Task 1: Modify task_queue's QueueItem trait

- Change `start()` to return `BoxFuture` instead of `Command`
- Add `TaskOutput` enum with `Success`, `Timeout`, `InvalidResponse`, `Error` variants
- Add `run_command()` helper for backward compatibility
- **No `finally()` method.** The trait stays at two methods.

### Task 2: Update task_queue's TaskRunner

- Modify execution loop to `.await` the future instead of spawning a `Command`
- **No parent-child tracking in TaskRunner.** It's a flat queue that runs tasks and collects results.

### Task 3: Create BarnumContext

Consolidate config, schemas, paths, and `FinallyTracker` into single struct.

### Task 4: Implement QueueItem for Task in barnum_config

- `start()`: run pre-hook, return future for either pool submission or local command
- `process()`: validate response, run post-hook, track finally via `FinallyTracker`, return next tasks

### Task 5: Replace barnum_config's TaskRunner

Delete the current `runner/` module, use task_queue's runner directly.

### Task 6: Make barnum_config async

Add tokio dependency, make `run()` async.

## Current `finally` Implementation (for reference)

From `barnum_config/src/runner/finally.rs`:

```rust
/// State for tracking when a `finally` hook should run.
struct FinallyState {
    /// Number of descendants still pending (in queue or in flight).
    pending_count: usize,
    /// The original task's value (input to finally hook).
    original_value: serde_json::Value,
    /// The finally hook command.
    finally_command: String,
}
```

When a task completes:
1. If it spawned children and has `finally`, register in `finally_tracking` with count = num_children
2. Children inherit `origin_id` pointing to parent
3. On each descendant completion, decrement `finally_tracking[origin_id].pending_count`
4. When count == 0, run `finally_command` with original value on stdin
5. Finally output (JSON array) spawns new tasks (without origin tracking)

**Edge cases:**
- Finally runs even if descendants failed
- Finally failures are logged but ignored
- Finally-spawned tasks don't inherit origin (prevents infinite tracking)

This logic moves from Barnum's runner into `FinallyTracker` in `BarnumContext`. Same algorithm, different home.

## Summary

| Component | Responsibility |
|-----------|----------------|
| **task_queue** | Queue execution, concurrency, async futures. **Nothing else.** |
| **QueueItem trait** | Two methods: `start()` and `process()`. No hooks, no finally. |
| **barnum_config** | Config parsing, validation, hooks (pre/post/finally), retry logic, state logging |
| **BarnumContext** | Barnum's runtime state including `FinallyTracker` |
| **QueueItem impl** | Bridge — compiles Barnum's rich behavior into simple queue operations |

The key insight: task_queue provides the execution engine, Barnum provides the dynamic/config-driven behavior. They compose via the `QueueItem` trait, with Barnum returning futures that do whatever Barnum needs (pool submission, hooks, etc.). Pre, post, and finally are all "user land" — they live in Barnum's `QueueItem` implementation and `BarnumContext`, invisible to the trait.
