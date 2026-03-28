# Scheduling and Runtime

How dispatches produced by the engine get executed and how results flow back in.

**Depends on:** COMPLETION.md (engine completion milestone — done)

**Scope:** The event-driven scheduling loop, the bridge between `Engine` and the outside world. After advance and completion, this makes the engine actually *run*.

## The engine is not the runtime

The engine is a pure state machine. No I/O, no async, no timers, no concurrency. It:
- Receives `advance(workflow_root(), input, None)` and produces dispatches
- Receives `complete(task_id, value)` and produces more dispatches (or terminates)

The runtime is everything around the engine:
- Taking dispatches and executing them
- Collecting results and feeding them back via `complete`
- Managing concurrency (multiple handlers executing in parallel)

## Architecture: event-driven with completion channel

The entire runtime is an event loop driven by a `tokio::sync::mpsc` channel. Two event kinds:

```rust
enum Event {
    /// A handler was triggered (dispatch produced by the engine).
    TaskTriggered { task_id: TaskId, handler_id: HandlerId, value: Value },
    /// A handler finished and produced a result.
    TaskCompleted { task_id: TaskId, value: Value },
}
```

The scheduler owns the engine and a channel `(tx, rx)`. The loop:

1. `engine.advance(engine.workflow_root(), input, None)`
2. For each pending dispatch: emit `TaskTriggered`, then actually execute the handler
3. `loop { event = rx.recv().await }` — process completions as they arrive
4. On `TaskCompleted`: call `engine.complete(task_id, value)`, take new dispatches, emit `TaskTriggered` for each, execute handlers
5. When `complete` returns `Some(value)`: workflow is done

### Why event-driven?

The event stream is the single source of truth. Every state transition flows through the channel. This makes **replay trivial**: instead of executing real handlers, feed canned `TaskCompleted` events into the channel. The engine replays identically.

The event stream is also the logging story. An NDJSON applier writes every event to disk. The log is a complete, replayable record.

### Why a channel (not FuturesUnordered)?

Handlers get `tx.clone()` and send `(TaskId, Value)` when done. The scheduler just does `rx.recv().await`. Benefits:

- Dead simple. No type erasure headaches.
- Handlers can be anything — async task, thread, subprocess, webhook callback. They just need a `Sender`.
- Naturally supports the event-driven model: the channel IS the event stream.
- Replay: feed events directly into the channel without spawning real handlers.

Unbounded channel — we don't want back-pressure on handler completions.

## Type-erased actors

Handlers are type-erased behind a trait. The scheduler holds a `Vec<Box<dyn Actor>>` (or similar). During setup, concrete actors (TypeScript executor, builtin handler, etc.) register themselves. From then on, the scheduler never knows their concrete types.

Each actor receives dispatches and has access to `tx` to send completions back. The actor trait and concrete implementations are a follow-up — for the scheduling milestone, the actor can be a no-op that immediately completes with an empty object.

## Key properties

**Single-threaded engine access.** The scheduler calls engine methods one at a time, never concurrently. Handlers execute concurrently on tokio tasks, but results arrive through the channel and are processed sequentially.

**Deterministic engine.** Given the same sequence of `(TaskId, Value)` completions, the engine always produces the same state transitions. Nondeterminism is only in completion ordering, which is captured by the event stream.

**Engine API:**
- `engine.advance(action_id, value, parent)` — expand an action
- `engine.complete(task_id, value) -> Result<Option<Value>, CompleteError>` — deliver a result
- `engine.take_pending_dispatches() -> Vec<Dispatch>` — drain pending work
- `engine.workflow_root() -> ActionId` — the root action
- `engine.handler(id) -> &HandlerKind` — resolve a handler ID

## Sketch

```rust
pub async fn run(engine: &mut Engine, input: Value) -> Value {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    // Initial advance
    let root = engine.workflow_root();
    engine.advance(root, input, None).unwrap();
    dispatch_pending(engine, &tx);

    // Event loop
    loop {
        let event = rx.recv().await.expect("channel closed");
        match event {
            Event::TaskCompleted { task_id, value } => {
                if let Some(result) = engine.complete(task_id, value).unwrap() {
                    return result;
                }
                dispatch_pending(engine, &tx);
            }
            Event::TaskTriggered { .. } => {
                // Logged/observed only. Actual execution was already spawned.
            }
        }
    }
}

fn dispatch_pending(engine: &mut Engine, tx: &UnboundedSender<Event>) {
    let dispatches = engine.take_pending_dispatches();
    for dispatch in dispatches {
        tx.send(Event::TaskTriggered {
            task_id: dispatch.task_id,
            handler_id: dispatch.handler_id,
            value: dispatch.value.clone(),
        }).unwrap();

        let completion_tx = tx.clone();
        let task_id = dispatch.task_id;
        // Actor execution — type-erased, details TBD
        tokio::spawn(async move {
            let result = Value::Object(Default::default()); // placeholder
            completion_tx.send(Event::TaskCompleted {
                task_id,
                value: result,
            }).unwrap();
        });
    }
}
```

## What happens to barnum_event_loop?

The existing `run_event_loop`, `Applier` trait, `NdjsonApplier`, and `EngineApplier` all stay. The `EngineApplier` stub gets filled in — it owns the `Engine`, and its `apply()` implementation calls `engine.complete()` on `TaskCompleted` events, takes pending dispatches, and spawns handler executions (via `tx`). `NdjsonApplier` writes events to disk. The event loop with its `Vec<Box<dyn Applier>>` is the scheduler.
