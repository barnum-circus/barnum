# Runtime

The workflow execution runtime. Connects WorkflowState to handler execution via a simple dispatch/complete loop.

**Depends on:** COMPLETION.md (done)

**Scope:** The Scheduler struct, the `run_workflow` async function, no-op handler execution. Does NOT cover TypeScript handler invocation (see TYPESCRIPT_HANDLER_INVOCATION.md) or NDJSON logging.

**File:** `crates/barnum_event_loop/src/lib.rs` (to be simplified — most of the current content is replaced)

## Architecture

A direct loop. No event-driven architecture, no Applier trait, no event enum.

```
Initial advance → produce dispatches
loop {
    dispatch all pending work to Scheduler
    recv one result from Scheduler
    complete(task_id, value) on WorkflowState
    if workflow done: break
    // new dispatches may have been produced; loop back
}
```

WorkflowState is the pure state machine. The Scheduler is a dumb executor. The `run_workflow` function is the glue.

## Scheduler

```rust
pub struct Scheduler {
    result_tx: UnboundedSender<(TaskId, Value)>,
    result_rx: UnboundedReceiver<(TaskId, Value)>,
}

impl Scheduler {
    pub fn new() -> Self {
        let (result_tx, result_rx) = tokio::sync::mpsc::unbounded_channel();
        Self { result_tx, result_rx }
    }

    pub fn dispatch(&self, task_id: TaskId, _handler: HandlerKind, _value: Value) {
        let result_tx = self.result_tx.clone();
        tokio::spawn(async move {
            let value = Value::Object(Default::default()); // no-op
            let _ = result_tx.send((task_id, value));
        });
    }

    pub async fn recv(&mut self) -> (TaskId, Value) {
        self.result_rx.recv().await.expect("scheduler channel closed")
    }
}
```

Each `dispatch()` call:
- Clones the result sender (cheap — `Arc` internally)
- Spawns a tokio task (lightweight — not a thread; multiplexed on the tokio thread pool)
- The task executes the handler (currently a no-op returning `{}`), sends the result, and completes

`recv()` returns the next completed result. Results may arrive in any order when multiple tasks are in flight (Parallel, ForEach).

### Handler execution (first milestone)

All handlers return an empty JSON object:

```rust
// Inside the tokio::spawn:
let value = Value::Object(Default::default());
```

This exercises the full loop: WorkflowState advances, produces dispatches, Scheduler "runs" them, returns results, WorkflowState processes completions, cycle repeats until the workflow terminates.

Real TypeScript handler execution replaces this later. See TYPESCRIPT_HANDLER_INVOCATION.md.

### Actor trait (future, not this milestone)

When multiple handler types exist (TypeScript, builtin, etc.), handler execution becomes a dispatch through type-erased actors:

```rust
trait Actor: Send + Sync {
    fn execute(&self, value: Value) -> Pin<Box<dyn Future<Output = Value> + Send>>;
}
```

The Scheduler holds `Vec<Box<dyn Actor>>` and routes dispatches based on `HandlerKind`. Each actor returns a future; the Scheduler wraps it in a `tokio::spawn` with the result channel wiring.

For this milestone, no Actor trait — just the inline no-op. The trait is introduced when we have a second handler type.

## run_workflow

```rust
pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
) -> Result<Value, CompleteError> {
    let root = workflow_state.workflow_root();
    workflow_state
        .advance(root, Value::Null, None)
        .expect("initial advance failed");

    loop {
        let dispatches = workflow_state.take_pending_dispatches();
        for dispatch in dispatches {
            let handler = workflow_state.handler(dispatch.handler_id).clone();
            scheduler.dispatch(dispatch.task_id, handler, dispatch.value);
        }

        let (task_id, value) = scheduler.recv().await;
        if let Some(terminal_value) = workflow_state.complete(task_id, value)? {
            return Ok(terminal_value);
        }
    }
}
```

The loop:
1. Takes all pending dispatches from WorkflowState
2. Sends each to the Scheduler
3. Waits for one result
4. Calls `complete()` — if the workflow terminates, returns the terminal value
5. Otherwise loops back — `take_pending_dispatches()` picks up any new work produced by the completion

This handles:
- **Chain**: completion of step N produces a dispatch for step N+1 (trampoline)
- **Parallel**: all children dispatched at once, completions arrive one at a time, Parallel frame collects results
- **ForEach**: same as Parallel but over array elements
- **Loop**: body completion produces either Break (terminal) or Continue (re-dispatch)
- **Nesting**: arbitrary combinations of the above

## Replay

The only non-deterministic input to WorkflowState is the sequence of `(TaskId, Value)` pairs from handler completions. Everything else — dispatches, frame creation, value routing — is deterministic given the config.

To replay: record the completion sequence, feed the same `(task_id, value)` pairs into `complete()` on a fresh WorkflowState with the same config. The engine reproduces the same behavior. No special event types or logging infrastructure needed — just the completions.

When replay or observability is needed, completions can be logged at the `run_workflow` level — one line per `complete()` call — without any architectural changes.

## What this replaces

The current `barnum_event_loop` crate has:
- `Event` enum (`TaskStarted`, `TaskCompleted`) — removed
- `TaskResult` enum (`Success`, `Failure`) — removed
- `Applier` trait — removed
- `NdjsonApplier` — deferred
- `EngineApplier` stub — replaced by `run_workflow`
- `run_event_loop` — replaced by `run_workflow`

The Applier/event-loop pattern can be reintroduced later if multiple independent reactors are needed. The current architecture doesn't foreclose on it — `run_workflow` is the single point where completions flow, so wrapping it in an event dispatch is straightforward.

## Future: error handling is an AST concern

Handlers cross a process boundary (Rust → Node.js). They can always fail. Every handler naturally returns a Result — not as a special engine concept, but as the natural return type of any cross-boundary call. A Result is just a Value with a particular shape.

Error handling — retries, unwrap, propagation — is expressible via existing AST primitives:

**Retry as a Loop:**

```
Loop(
  Chain(
    Invoke(handler),
    Switch(
      "ok"  => Break(value),
      "err" => Continue
    )
  )
)
```

Add a max iteration count to Loop for bounded retries. No special engine or scheduler machinery needed.

The Scheduler doesn't handle retries. WorkflowState doesn't distinguish success from failure. The AST determines what to do with Result values. See also DEFERRED_FEATURES.md § "Remove Attempt."
