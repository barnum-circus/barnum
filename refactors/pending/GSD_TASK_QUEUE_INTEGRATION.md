# Refactor: GSD and Task Queue Integration

## Background

We have two crates that do conceptually the same thing:

| Aspect | `task_queue` | `gsd_config` |
|--------|--------------|--------------|
| **Validation** | Compile-time (types) | Runtime (JSON Schema) |
| **Step names** | Enum variants | Strings |
| **Transitions** | `NextTasks` associated type | `next: Vec<StepName>` |
| **Value schemas** | Rust types | JSON Schema |
| **State machine** | Enforced by type system | Enforced by runtime checks |

Neither uses the other. This is a missed opportunity.

## Current State

### task_queue

Defines the `QueueItem` trait:

```rust
pub trait QueueItem<Context>: Sized {
    type InProgress;           // State held while command runs
    type Response: DeserializeOwned;  // Deserialized from stdout
    type NextTasks;            // Follow-up tasks to enqueue

    fn start(self, ctx: &mut Context) -> (Self::InProgress, Command);
    fn process(in_progress: Self::InProgress, result: Result<Self::Response, _>, ctx: &mut Context) -> Self::NextTasks;
}
```

And the `GsdTask` derive macro for enum dispatch:

```rust
#[derive(GsdTask)]
enum Task {
    Analyze(AnalyzeTask),
    Implement(ImplementTask),
}
```

The type system enforces valid transitions: `NextTasks` must be convertible to `Vec<Task>`.

### gsd_config

Uses JSON config to define steps:

```json
{
  "steps": [
    {
      "name": "Analyze",
      "value_schema": {"type": "object", "properties": {"path": {"type": "string"}}},
      "next": ["Implement", "Done"]
    }
  ]
}
```

Runtime validates:
- Task values against JSON Schema (`CompiledSchemas::validate`)
- Transitions via `validate_response` checking `step.next`

## The Problem

1. **task_queue isn't used** - GSD reinvents the queue execution logic
2. **No shared abstractions** - Concepts like "queue item" exist twice
3. **Typed workflows can't use GSD** - If you want compile-time safety, you bypass GSD entirely

## Possible Approaches

### Option A: GSD Generates task_queue Types (Codegen)

A `gsd-codegen` tool reads config JSON and generates Rust:

```rust
// Generated from config.json
#[derive(GsdTask)]
pub enum Task {
    Analyze(AnalyzeTask),
    Implement(ImplementTask),
}

pub struct AnalyzeTask {
    pub path: String,  // From JSON Schema
}

impl QueueItem<Ctx> for AnalyzeTask {
    type NextTasks = Vec<Task>;  // From "next": ["Implement", "Done"]
    // ...
}
```

**Pros:**
- Compile-time safety from JSON config
- Best of both worlds

**Cons:**
- Build complexity (codegen step)
- Schema-to-type mapping is lossy (JSON Schema is more expressive)
- Two sources of truth

### Option B: Runtime QueueItem Implementation

Add a `DynamicTask` type that implements `QueueItem` with runtime validation:

```rust
pub struct DynamicTask {
    pub step: StepName,
    pub value: serde_json::Value,
}

impl QueueItem<GsdContext> for DynamicTask {
    type InProgress = DynamicInProgress;
    type Response = serde_json::Value;
    type NextTasks = Vec<DynamicTask>;

    fn start(self, ctx: &mut GsdContext) -> (Self::InProgress, Command) {
        // Build command based on step config
        let step = ctx.config.step_map().get(&self.step);
        // ...
    }

    fn process(in_progress: Self::InProgress, result: Result<Value, _>, ctx: &mut GsdContext) -> Vec<DynamicTask> {
        // Runtime validate transitions
        // ...
    }
}
```

**Pros:**
- GSD uses task_queue's execution engine
- Single source of truth (JSON config)
- Gradual migration possible

**Cons:**
- Loses compile-time guarantees (but GSD never had them)
- task_queue becomes aware of "dynamic" use case

### Option C: Abstract Core Traits

Extract shared traits that both can implement:

```rust
// In a new `task_queue_core` crate
pub trait TaskProcessor {
    type Task;
    type Context;
    type Error;

    fn process_one(&mut self, task: Self::Task) -> Result<Vec<Self::Task>, Self::Error>;
}

pub trait TaskQueue {
    type Task;

    fn push(&mut self, task: Self::Task);
    fn pop(&mut self) -> Option<Self::Task>;
    fn is_empty(&self) -> bool;
}
```

**Pros:**
- Clean separation of concerns
- Both crates can evolve independently

**Cons:**
- More abstraction without clear benefit
- Still two implementations of queue execution

### Option D: task_queue as "Typed GSD"

Position task_queue as the typed alternative:
- Use GSD for dynamic/JSON-configured workflows
- Use task_queue for compile-time-safe workflows
- Don't try to merge them

**Pros:**
- Simplest approach
- Clear use cases for each

**Cons:**
- Duplicated concepts
- Users must choose upfront

## Analysis

The core question: **What value would integration provide?**

### What GSD gains from task_queue

1. **Proven execution engine** - task_queue's `TaskRunner` is simpler and async
2. **The `QueueItem` trait** - Could be useful abstraction even for dynamic tasks

### What task_queue gains from GSD

1. **JSON config parsing** - Not much, task_queue is designed for Rust-native definitions
2. **Runtime validation** - Could be added, but goes against task_queue's philosophy

### The Real Overlap

Both have:
- A queue of tasks
- Concurrent execution with limits
- Processing that produces more tasks
- A "done" state when queue drains

The difference is *when* validation happens (compile vs runtime) and *how* tasks are defined (Rust vs JSON).

## Recommendation

**Option B: Runtime QueueItem Implementation** seems most promising.

Rationale:
1. GSD's runtime validation is a feature, not a bug - it enables config-driven workflows
2. task_queue's execution engine is cleaner (async, simpler state)
3. Implementing `QueueItem` for GSD's dynamic tasks unifies the concepts without forcing either to change philosophy

### Concrete Changes

1. **Add `DynamicQueueItem`** in gsd_config that wraps `Task` and implements `QueueItem`
2. **Replace gsd_config's TaskRunner** with task_queue's `TaskRunner`
3. **Keep validation in `process()`** - runtime checks still happen, just inside the trait impl

## Open Questions

1. **Async vs sync execution** - task_queue is async (tokio), gsd_config is sync (threads). Do we make gsd_config async?

2. **Agent pool integration** - task_queue uses local `Command`, GSD uses agent_pool CLI. The `start()` method would need to handle this differently.

3. **Hooks** - task_queue has no concept of pre/post/finally hooks. Where do these go?

4. **Context** - task_queue passes `&mut Ctx` through everything. GSD's context is spread across `Config`, `CompiledSchemas`, paths, etc.

5. **Error handling** - task_queue's `process()` receives `Result<Response, serde_json::Error>`. GSD has richer error types (timeout, validation failure, etc.).

## Next Steps

Before implementing, need to resolve:

1. Should gsd_config become async?
2. How do hooks fit into the `QueueItem` model?
3. What's the migration path for existing GSD users (if any)?

---

## Appendix: Code Comparison

### task_queue Task Definition

```rust
struct AnalyzeFile { path: String }

impl QueueItem<Ctx> for AnalyzeFile {
    type InProgress = AnalyzeInProgress;
    type Response = AnalyzeResponse;
    type NextTasks = Vec<Task>;

    fn start(self, ctx: &mut Ctx) -> (Self::InProgress, Command) {
        let mut cmd = Command::new("./analyze.sh");
        cmd.arg(&self.path);
        (AnalyzeInProgress { path: self.path }, cmd)
    }

    fn process(ip: Self::InProgress, result: Result<Self::Response, _>, ctx: &mut Ctx) -> Vec<Task> {
        match result {
            Ok(resp) => resp.issues.into_iter().map(|i| Task::Fix(FixTask { issue: i })).collect(),
            Err(_) => vec![],
        }
    }
}
```

### gsd_config Task Definition (JSON)

```json
{
  "name": "AnalyzeFile",
  "value_schema": {
    "type": "object",
    "properties": { "path": { "type": "string" } },
    "required": ["path"]
  },
  "action": {
    "kind": "Pool",
    "instructions": "Analyze the file at the given path..."
  },
  "next": ["FixIssue", "Done"]
}
```

### Hypothetical Unified Model

```rust
// GSD's Task implements QueueItem dynamically
impl QueueItem<GsdContext> for Task {
    type InProgress = TaskInProgress;
    type Response = serde_json::Value;
    type NextTasks = Vec<Task>;

    fn start(self, ctx: &mut GsdContext) -> (Self::InProgress, Command) {
        let step = ctx.step_for(&self.step);

        // Run pre-hook if configured
        let value = if let Some(pre) = &step.pre {
            run_hook(pre, &self.value)?
        } else {
            self.value.clone()
        };

        // Build command (either local or agent_pool CLI)
        let cmd = match &step.action {
            Action::Command { script } => build_local_command(script, &value),
            Action::Pool { .. } => build_agent_pool_command(ctx.pool_path, &value, step),
        };

        (TaskInProgress { task: self, effective_value: value, step: step.clone() }, cmd)
    }

    fn process(ip: Self::InProgress, result: Result<Value, _>, ctx: &mut GsdContext) -> Vec<Task> {
        // Validate response against schema
        let tasks = match result {
            Ok(response) => validate_response(&response, &ip.step, &ctx.schemas),
            Err(e) => handle_parse_error(e, &ip, ctx),
        };

        // Run post-hook if configured
        if let Some(post) = &ip.step.post {
            run_post_hook(post, &ip, &tasks)
        } else {
            tasks
        }
    }
}
```

This keeps GSD's runtime validation while using task_queue's execution model.
