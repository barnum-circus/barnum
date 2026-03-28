# Scheduler

The handler execution layer. Receives dispatches from EngineApplier, executes handlers, sends completions back through the event channel.

**Depends on:** Nothing (can be built independently of ENGINE_APPLIER.md)

**Scope:** The Scheduler struct, handler dispatch, how completions flow back. Does NOT cover the Actor trait or multiple handler types — those are follow-ups.

**File:** `crates/barnum_event_loop/src/lib.rs` (alongside EngineApplier)

## What the Scheduler does

The Scheduler receives dispatches and executes them. For each dispatch:

1. Spawn a tokio task
2. Run the handler to completion
3. Send a `TaskCompleted` event through the event channel

The Scheduler has no knowledge of WorkflowState, frames, events, or the event loop. It receives work, runs it, sends completions. It is a dumb executor.

## Struct

```rust
pub struct Scheduler {
    event_tx: UnboundedSender<Event>,
}

impl Scheduler {
    pub fn new(event_tx: UnboundedSender<Event>) -> Self {
        Self { event_tx }
    }

    pub fn dispatch(&self, task_id: TaskId, handler: HandlerKind, value: Value) {
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let result = execute_handler(&handler, value).await;
            let _ = event_tx.send(Event::TaskCompleted(TaskCompletedEvent {
                task_id,
                value: result,
            }));
        });
    }
}
```

Each `dispatch()` call:
- Clones the `event_tx` sender (cheap — it's an `Arc` internally)
- Spawns a tokio task that owns `task_id`, `handler`, `value`, and the cloned sender
- The task runs the handler, sends the completion, and drops the sender clone

## Handler execution (first milestone)

For the first milestone, handlers are no-ops:

```rust
async fn execute_handler(_handler: &HandlerKind, _value: Value) -> Value {
    Value::Object(Default::default())
}
```

Returns an empty JSON object for every handler. This is enough to exercise the full event loop: EngineApplier advances WorkflowState, flushes dispatches to Scheduler, Scheduler "runs" them and sends completions, EngineApplier processes completions, cycle repeats until the workflow terminates.

Real TypeScript handler execution (subprocess management, IPC protocol) replaces this later. See TYPESCRIPT_HANDLER_INVOCATION.md.

## How completions flow back

```
EngineApplier calls scheduler.dispatch(task_id, handler, value)
  ↓
Scheduler spawns a tokio task
  ↓
Task runs execute_handler() → produces a Value
  ↓
Task sends Event::TaskCompleted { task_id, value } via event_tx
  ↓
Event arrives in the unbounded channel
  ↓
run_event_loop picks it up, calls apply() on all appliers
  ↓
EngineApplier.apply() processes the TaskCompleted
  ↓
workflow_state.complete() → may produce new dispatches
  ↓
EngineApplier calls scheduler.dispatch() again
  ↓
...cycle continues until workflow terminates
```

## Actor trait (future, not this milestone)

When multiple handler types exist (TypeScript, builtin, etc.), `execute_handler` becomes a dispatch through type-erased actors:

```rust
trait Actor: Send + Sync {
    fn execute(&self, value: Value) -> Pin<Box<dyn Future<Output = Value> + Send>>;
}
```

The Scheduler holds `Vec<Box<dyn Actor>>` and routes dispatches based on `HandlerKind`. Each actor returns a future; the Scheduler wraps it in a tokio::spawn with the event_tx wiring.

For this milestone, no Actor trait — just the inline `execute_handler` no-op. The trait is introduced when we have a second handler type to dispatch to.

## No retry logic in the Scheduler

The Scheduler is a dumb executor: run handler, send result. It has no knowledge of retries, error handling, or Result semantics. Retries are an AST-level concern — expressible as `Loop(Chain(Invoke(handler), Switch(...)))`. See ENGINE_APPLIER.md § "Future: error handling is an AST concern."
