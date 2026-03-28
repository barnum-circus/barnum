# Scheduling and Runtime

How dispatches produced by the engine get executed and how results flow back in. This is the runtime layer that wraps the pure engine state machine.

**Depends on:** ENGINE.md (design), FRAME_STORAGE_AND_ADVANCE.md (advance milestone), COMPLETION.md (completion milestone)

**Scope:** The scheduling loop, handler execution, the bridge between `Engine` and the outside world. This is the third milestone — after advance and completion, this makes the engine actually *run*.

## The engine is not the runtime

The engine (`Engine`) is a pure state machine. It has no I/O, no async, no timers, no concurrency. It:
- Receives `start(input)` and produces dispatches
- Receives `on_task_completed(task_id, result)` and produces more dispatches (or terminates)

The runtime is everything around the engine:
- Taking dispatches and executing them (invoking handlers)
- Collecting results and feeding them back via `on_task_completed`
- Managing concurrency (multiple handlers executing in parallel)
- Handling timeouts, retries, logging, and other operational concerns

The engine runs synchronously within the runtime. The runtime runs asynchronously (tokio, event loop, etc.).

## The scheduling loop

```
1. engine.start(input)
2. dispatches = engine.take_pending_dispatches()
3. for each dispatch:
     spawn handler execution (async)
4. wait for any handler to complete
5. engine.on_task_completed(task_id, result)
6. dispatches = engine.take_pending_dispatches()
7. if engine.is_done(): return engine.result()
8. goto 3 (spawn new dispatches) + goto 4 (keep waiting)
```

This is cooperative scheduling. The engine produces work (dispatches). The runtime executes work (handlers). Results come back one at a time, each triggering another synchronous engine step that may produce more work.

Key property: **the engine is never called concurrently.** All engine methods (`start`, `on_task_completed`, `take_pending_dispatches`) are called sequentially from the scheduling loop. The concurrency is in handler execution, not in engine access. No locks needed on the engine.

## Handler execution

A `Dispatch` contains `handler_id` and `value`. The runtime resolves `handler_id` to a `HandlerKind` via `engine.handler(handler_id)`, then executes the handler based on its kind:

### TypeScript handlers

```
HandlerKind::TypeScript { module, func, .. }
```

The runtime invokes a TypeScript function in a Deno/Node subprocess. This is the current execution model — the CLI spawns a Deno process, sends the handler invocation as JSON over stdin, and reads the result from stdout.

The details of TypeScript handler execution (subprocess management, IPC protocol, sandboxing) are out of scope for this doc. The scheduling layer only cares about: "give me a `Future<TaskResult>` for this dispatch."

### Builtin handlers (future)

```
HandlerKind::Builtin(BuiltinKind::Tag { kind })
```

Executed inline by the runtime — no subprocess, no async. The runtime computes the result synchronously and immediately calls `on_task_completed`. From the engine's perspective, these complete instantly.

Builtins could be handled inside the engine itself (during advance, skip the dispatch entirely and inline the computation). This would eliminate the round trip through the runtime. But keeping them as dispatches preserves the uniform scheduling model and makes logging/tracing consistent.

## Connecting to the existing event system

The current `barnum_event_loop` has:
- `Event` enum: `TaskStarted`, `TaskCompleted`
- `Applier` trait: processes events
- `EngineApplier`: stub that will own the `Engine`
- `NdjsonApplier`: logs events to a file
- `run_event_loop`: receives events from a tokio channel, dispatches to appliers

### Option A: Engine inside the event loop (EngineApplier)

The `EngineApplier` owns the `Engine`. When a `TaskCompleted` event arrives, the applier calls `engine.on_task_completed(task_id, result)`, takes pending dispatches, and emits new `TaskStarted` events for each dispatch. Handler execution happens somewhere else (a task spawner that listens for `TaskStarted` events and sends `TaskCompleted` events when done).

```
Event::TaskCompleted arrives
  → EngineApplier.apply()
    → engine.on_task_completed(task_id, result)
    → dispatches = engine.take_pending_dispatches()
    → for each dispatch:
        emit Event::TaskStarted
          → handler executor picks it up
          → handler runs
          → Event::TaskCompleted emitted
            → cycle repeats
```

Problem: the Applier trait's `apply` method is `&mut self` and synchronous. It can't spawn async work or emit events back into the channel without a handle. The current architecture assumes appliers are passive observers, not active participants.

### Option B: Engine drives the loop

The engine scheduling loop is the top-level orchestrator. It owns the engine directly (not through an Applier). Events (`TaskStarted`, `TaskCompleted`) are emitted as side effects for logging/observability, but the scheduling loop doesn't *receive* events — it *produces* them.

```
scheduler owns Engine
scheduler owns handler executor
scheduler owns event channel (for logging)

loop:
  dispatches = engine.take_pending_dispatches()
  for dispatch in dispatches:
    emit TaskStarted event (for logging)
    spawn handler execution
  wait for any handler to complete
  emit TaskCompleted event (for logging)
  engine.on_task_completed(task_id, result)
  if engine.is_done(): break
```

This is simpler. The scheduler is a single async function that drives the engine and spawns handlers. Events are a byproduct, not the control mechanism. The `NdjsonApplier` can still observe events for logging, but the engine doesn't depend on the event loop.

**Option B is the right approach.** The event loop was designed before the engine existed. The engine's synchronous, pull-based model doesn't fit the event loop's push-based architecture. The scheduler should own the engine directly.

### What happens to EngineApplier?

It becomes unnecessary. The `EngineApplier` stub can be removed. The scheduler replaces it — it's a standalone async function, not an Applier implementation.

The `NdjsonApplier` and the event channel still exist for logging. The scheduler emits events into the channel as a side effect. Appliers observe them passively.

## Scheduler implementation sketch

```rust
pub struct Scheduler {
    engine: Engine,
    /// In-flight handler executions. Maps TaskId → JoinHandle.
    in_flight: HashMap<TaskId, tokio::task::JoinHandle<TaskResult>>,
    /// Channel for emitting events (logging, observability).
    event_tx: Option<tokio::sync::mpsc::Sender<Event>>,
}

impl Scheduler {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            in_flight: HashMap::new(),
            event_tx: None,
        }
    }

    pub fn with_event_channel(mut self, tx: tokio::sync::mpsc::Sender<Event>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Run the workflow to completion.
    pub async fn run(&mut self, input: Value) -> EngineResult {
        self.engine.start(input);
        self.spawn_pending_dispatches();

        loop {
            // Wait for any in-flight handler to complete.
            let (task_id, task_result) = self.wait_for_completion().await;

            self.emit_task_completed(task_id, &task_result);
            self.engine.on_task_completed(task_id, task_result);

            if self.engine.is_done() {
                return self.engine.result().unwrap().clone();
            }

            self.spawn_pending_dispatches();
        }
    }

    fn spawn_pending_dispatches(&mut self) {
        let dispatches = self.engine.take_pending_dispatches();
        for dispatch in dispatches {
            self.emit_task_started(&dispatch);
            let handler = self.engine.handler(dispatch.handler_id).clone();
            let handle = tokio::spawn(execute_handler(handler, dispatch.value));
            self.in_flight.insert(dispatch.task_id, handle);
        }
    }

    async fn wait_for_completion(&mut self) -> (TaskId, TaskResult) {
        // Wait for any one of the in-flight tasks to finish.
        // tokio::select! over all JoinHandles, or use a FuturesUnordered.
        todo!("select over in_flight handles")
    }
}

/// Execute a handler and return the result.
async fn execute_handler(handler: HandlerKind, value: Value) -> TaskResult {
    match handler {
        HandlerKind::TypeScript(ts_handler) => {
            // Spawn Deno subprocess, send value, read result.
            todo!("TypeScript handler execution")
        }
    }
}
```

### wait_for_completion

The scheduler needs to wait for *any* of N in-flight handlers to complete. Options:

1. **`FuturesUnordered`**: Collect all JoinHandles into a `FuturesUnordered<JoinHandle<TaskResult>>`, poll it. Returns results in completion order. Problem: we need to know *which* TaskId completed, so we'd need `FuturesUnordered<(TaskId, JoinHandle<TaskResult>)>` or similar.

2. **`tokio::select!` macro**: Select over all handles. Doesn't scale — select is O(N) in the number of branches and requires static arms.

3. **Completion channel**: Each spawned task sends its result back through a shared `mpsc` channel: `(TaskId, TaskResult)`. The scheduler receives from this channel. Simple, no polling, works for any N.

Option 3 is the cleanest:

```rust
struct Scheduler {
    engine: Engine,
    completion_tx: tokio::sync::mpsc::UnboundedSender<(TaskId, TaskResult)>,
    completion_rx: tokio::sync::mpsc::UnboundedReceiver<(TaskId, TaskResult)>,
    // ...
}

fn spawn_pending_dispatches(&mut self) {
    let dispatches = self.engine.take_pending_dispatches();
    for dispatch in dispatches {
        let handler = self.engine.handler(dispatch.handler_id).clone();
        let tx = self.completion_tx.clone();
        let task_id = dispatch.task_id;
        tokio::spawn(async move {
            let result = execute_handler(handler, dispatch.value).await;
            let _ = tx.send((task_id, result));
        });
    }
}

async fn wait_for_completion(&mut self) -> (TaskId, TaskResult) {
    self.completion_rx.recv().await.expect("completion channel closed")
}
```

Unbounded channel because we don't want back-pressure on handler completions — if 100 handlers complete simultaneously, we want to process them all without blocking the completing tasks.

## Ordering and determinism

### Handler completion order is nondeterministic

Parallel children are dispatched simultaneously. They complete in whatever order the handlers finish. The engine handles this correctly — `complete_indexed` fills slots by index regardless of completion order. Results are collected in *declaration order*, not completion order.

### Engine steps are deterministic

Given the same sequence of `(TaskId, TaskResult)` completions, the engine always produces the same state transitions. Nondeterminism is only in the *ordering* of completions, not in the engine's response to each one.

### Single-threaded engine access

The scheduler calls engine methods one at a time, never concurrently. Even though handlers execute concurrently (on tokio tasks), results arrive through the completion channel and are processed sequentially by the scheduler's main loop.

## What about the existing event loop?

The existing `run_event_loop` function and `Applier` trait remain for logging. The scheduler optionally emits events into the event channel. `NdjsonApplier` writes them to disk. The event loop runs as a separate tokio task, consuming events passively.

The `EngineApplier` stub is removed — the scheduler replaces its function.

## Open questions

### Cancellation

When the engine calls `cancel_descendants` (error in Parallel), it removes frames and task-to-frame mappings. But the corresponding handler tasks are still running. The scheduler needs to:
1. Cancel the tokio tasks (via `JoinHandle::abort()`)
2. Ignore results from cancelled tasks that arrive after cancellation

Option: maintain a `cancelled: HashSet<TaskId>` in the scheduler. When a completion arrives for a cancelled TaskId, silently drop it. Alternatively, `engine.on_task_completed` could return a `Result` or be lenient about unknown TaskIds.

### Timeouts

Handler timeouts are a scheduler concern, not an engine concern. The scheduler wraps each handler execution in `tokio::time::timeout()`. On timeout, it sends `TaskResult::Failure { error: "timeout" }` to the engine. The engine treats this like any other failure.

### Retry

Retry policies (if/when implemented) could live in the scheduler or the engine:
- **Scheduler-level retry**: The scheduler intercepts a failure, checks a retry policy, and re-dispatches the same handler. The engine never sees the failure. Simple but opaque — the engine's frame tree doesn't reflect retries.
- **Engine-level retry**: The engine has a `Retry` frame type (like Attempt but re-dispatches instead of catching). The frame tree accurately reflects retry state. More complex but debuggable.

For now, no retry. When needed, start with scheduler-level retry (simpler).

### Multiple workflows

Each workflow gets its own `Engine` instance. The scheduler could manage multiple engines, dispatching handlers from all of them. Or each workflow gets its own scheduler. The simpler model (one scheduler per workflow) is correct to start. Multiplexing is an optimization for when many workflows run concurrently.
