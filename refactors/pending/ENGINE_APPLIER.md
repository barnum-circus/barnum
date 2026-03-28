# Engine Applier

Filling in the EngineApplier stub: connecting WorkflowState to the event loop.

**Depends on:** COMPLETION.md (done)

**Scope:** The EngineApplier struct, its Applier implementation, event type changes, channel changes, termination.

**File:** `crates/barnum_event_loop/src/lib.rs`

## Current state

EngineApplier is an empty stub (`crates/barnum_event_loop/src/lib.rs:159`):

```rust
pub struct EngineApplier;

impl Applier for EngineApplier {
    fn apply(&mut self, _event: &Event) {
        // Stub: will drive the AST evaluator based on task completions.
    }
}
```

The event loop uses a bounded channel (`crates/barnum_event_loop/src/lib.rs:175`):

```rust
pub async fn run_event_loop(
    mut receiver: tokio::sync::mpsc::Receiver<Event>,
    appliers: &mut [Box<dyn Applier>],
) {
    while let Some(event) = receiver.recv().await {
        for applier in appliers.iter_mut() {
            applier.apply(&event);
        }
    }
}
```

Event types use `String` for `task_id` and wrap results in a `TaskResult` enum with `Success`/`Failure` variants.

## Two event types, one that matters

Both `TaskStarted` and `TaskCompleted` exist in the `Event` enum. Both flow through the channel. Both hit every applier. But they serve different roles:

- **`TaskCompleted` is control flow.** It carries non-deterministic information (what a handler returned). The EngineApplier processes it: calls `workflow_state.complete()`, takes new dispatches, sends them to the scheduler.
- **`TaskStarted` is observability.** It's deterministic — given the WorkflowState and a completion, the resulting dispatches are fully determined. `NdjsonApplier` logs it. EngineApplier ignores it.

For replay, you only need the `TaskCompleted` stream. The EngineApplier will deterministically reproduce the same `TaskStarted` events and dispatches.

## Event flow

```
TaskCompleted arrives in event loop
  → NdjsonApplier.apply(): logs it
  → EngineApplier.apply():
      → workflow_state.complete(task_id, value)
      → dispatches = workflow_state.take_pending_dispatches()
      → for each dispatch:
          send Event::TaskStarted via self.event_tx  (for logging)
          scheduler.dispatch(task_id, handler, value) (for execution)
      → if complete() returned Some(result): workflow is done

TaskStarted arrives in event loop (sent by EngineApplier in the previous step)
  → NdjsonApplier.apply(): logs it
  → EngineApplier.apply(): does nothing
```

## Changes

### 1. Unbounded channel

EngineApplier.apply() sends events into the same channel the event loop reads from. `UnboundedSender::send()` is sync — works inside the sync `apply()` method. A bounded channel would require async send, which `apply()` can't do.

Also prevents deadlock: if the channel were bounded and full, `apply()` would block trying to send, but the channel can't drain because the event loop is blocked in `apply()`.

```rust
// Before (crates/barnum_event_loop/src/lib.rs:175)
pub async fn run_event_loop(
    mut receiver: tokio::sync::mpsc::Receiver<Event>,
    appliers: &mut [Box<dyn Applier>],
)

// After
pub async fn run_event_loop(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<Event>,
    appliers: &mut [Box<dyn Applier>],
)
```

The body stays the same — `while let Some(event) = receiver.recv().await { ... }`.

### 2. Event type updates

Align with WorkflowState's types. Remove the `TaskResult` wrapper — error handling is deferred.

```rust
// Before (crates/barnum_event_loop/src/lib.rs:38)
pub struct TaskStartedEvent {
    pub task_id: String,
    pub handler: HandlerKind,
    pub value: Value,
}

pub struct TaskCompletedEvent {
    pub task_id: String,
    pub result: TaskResult,
}

pub enum TaskResult {
    Success { value: Value },
    Failure { error: String },
}

// After
pub struct TaskStartedEvent {
    pub task_id: TaskId,
    pub handler_id: HandlerId,
    pub handler: HandlerKind,  // redundant with handler_id, but useful for NDJSON logging
    pub value: Value,
}

pub struct TaskCompletedEvent {
    pub task_id: TaskId,
    pub value: Value,
}
```

`TaskResult` is deleted. `TaskStartedEvent` includes both `handler_id` (structural) and `handler: HandlerKind` (for human-readable NDJSON output — `NdjsonApplier` can't resolve `HandlerId` on its own).

### 3. Add barnum_engine dependency

`barnum_event_loop` needs `WorkflowState`, `TaskId`, `HandlerId`, `Dispatch`, `CompleteError`.

```toml
# crates/barnum_event_loop/Cargo.toml
[dependencies]
barnum_engine = { path = "../barnum_engine" }
```

### 4. EngineApplier struct

```rust
pub struct EngineApplier {
    workflow_state: WorkflowState,
    scheduler: Scheduler,
    event_tx: UnboundedSender<Event>,
    result_tx: Option<oneshot::Sender<Value>>,
}
```

- **`workflow_state`**: the pure state machine.
- **`scheduler`**: dispatches handler execution (see SCHEDULER.md).
- **`event_tx`**: sends `TaskStarted` events into the channel for logging. The `Scheduler`'s spawned tasks also hold clones to send `TaskCompleted` events.
- **`result_tx`**: oneshot sender for the terminal value. Fires when the workflow completes. The caller holds the receiver.

### 5. EngineApplier implementation

```rust
impl EngineApplier {
    pub fn new(
        workflow_state: WorkflowState,
        scheduler: Scheduler,
        event_tx: UnboundedSender<Event>,
        result_tx: oneshot::Sender<Value>,
    ) -> Self {
        Self {
            workflow_state,
            scheduler,
            event_tx,
            result_tx: Some(result_tx),
        }
    }

    /// Perform the initial advance and dispatch the first batch of work.
    ///
    /// Call this before starting the event loop. The initial TaskStarted
    /// events are buffered in the unbounded channel and processed once
    /// the event loop begins.
    pub fn start(&mut self, input: Value) {
        let root = self.workflow_state.workflow_root();
        self.workflow_state
            .advance(root, input, None)
            .expect("initial advance failed");
        self.flush_dispatches();
    }

    /// Take all pending dispatches from WorkflowState, emit TaskStarted
    /// events for logging, and send each dispatch to the Scheduler for
    /// execution.
    fn flush_dispatches(&mut self) {
        let dispatches = self.workflow_state.take_pending_dispatches();
        for dispatch in dispatches {
            let handler = self.workflow_state.handler(dispatch.handler_id).clone();

            // TaskStarted for logging/observability
            let _ = self.event_tx.send(Event::TaskStarted(TaskStartedEvent {
                task_id: dispatch.task_id,
                handler_id: dispatch.handler_id,
                handler: handler.clone(),
                value: dispatch.value.clone(),
            }));

            // Actual handler execution
            self.scheduler
                .dispatch(dispatch.task_id, handler, dispatch.value);
        }
    }
}

impl Applier for EngineApplier {
    fn apply(&mut self, event: &Event) {
        match event {
            Event::TaskCompleted(completed) => {
                let result = self
                    .workflow_state
                    .complete(completed.task_id, completed.value.clone())
                    .expect("completion failed");

                match result {
                    Some(value) => {
                        // Workflow done. Send the result to the caller.
                        if let Some(result_tx) = self.result_tx.take() {
                            let _ = result_tx.send(value);
                        }
                    }
                    None => {
                        // Workflow still running. Dispatch new work.
                        self.flush_dispatches();
                    }
                }
            }
            Event::TaskStarted(_) => {
                // Decorative. Logged by NdjsonApplier.
            }
        }
    }
}
```

### 6. Termination

When `workflow_state.complete()` returns `Some(value)`, the EngineApplier sends the result through a `oneshot::Sender<Value>`. The caller holds the `oneshot::Receiver` and selects on it alongside the event loop:

```rust
let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
let (result_tx, result_rx) = tokio::sync::oneshot::channel();

let scheduler = Scheduler::new(event_tx.clone());
let mut engine_applier = EngineApplier::new(workflow_state, scheduler, event_tx, result_tx);
engine_applier.start(input);

let ndjson_applier = NdjsonApplier::new()?;
let mut appliers: Vec<Box<dyn Applier>> = vec![
    Box::new(ndjson_applier),
    Box::new(engine_applier),
];

let result = tokio::select! {
    _ = run_event_loop(event_rx, &mut appliers) => {
        panic!("event loop ended before workflow completed")
    }
    result = result_rx => result.expect("result channel closed"),
};
```

The oneshot fires the moment the workflow completes, even if handler tasks are still in flight (they became irrelevant). The `select!` returns immediately with the result.

The Applier trait is unchanged. `run_event_loop` is unchanged (except the channel type). The termination mechanism is external to both.

## Walkthrough: Chain(Invoke(a), Invoke(b)) with input "start"

**1. Caller creates EngineApplier, calls `start("start")`:**

- `workflow_state.advance(root, "start", None)` → creates Chain frame, advances to Invoke(a), produces `Dispatch { task_id: 0, handler_id: 1, value: "start" }`
- `flush_dispatches()`:
  - Sends `Event::TaskStarted { task_id: 0, handler_id: 1, handler: TS("./a.ts", "a"), value: "start" }` into channel
  - Calls `scheduler.dispatch(0, TS("./a.ts", "a"), "start")` → spawns handler task

**2. Event loop starts. Picks up buffered `TaskStarted { task_id: 0 }`.**

- `NdjsonApplier` logs: `{"kind":"TaskStarted","taskId":0,"handlerId":1,"handler":{...},"value":"start"}`
- `EngineApplier` ignores it.

**3. Handler "a" finishes. Spawned task sends `TaskCompleted { task_id: 0, value: "a_result" }` via its `event_tx` clone.**

**4. Event loop picks up `TaskCompleted { task_id: 0, value: "a_result" }`.**

- `NdjsonApplier` logs it.
- `EngineApplier.apply()`:
  - `workflow_state.complete(TaskId(0), "a_result")` → `Ok(None)`. Chain frame consumed, advances to Invoke(b), produces `Dispatch { task_id: 1, handler_id: 0, value: "a_result" }`.
  - `flush_dispatches()`:
    - Sends `Event::TaskStarted { task_id: 1, ... }` into channel
    - Calls `scheduler.dispatch(1, TS("./b.ts", "b"), "a_result")`

**5. Event loop picks up `TaskStarted { task_id: 1 }`.**

- `NdjsonApplier` logs it.
- `EngineApplier` ignores it.

**6. Handler "b" finishes. Sends `TaskCompleted { task_id: 1, value: "b_result" }`.**

**7. Event loop picks up `TaskCompleted { task_id: 1, value: "b_result" }`.**

- `NdjsonApplier` logs it.
- `EngineApplier.apply()`:
  - `workflow_state.complete(TaskId(1), "b_result")` → `Ok(Some("b_result"))`.
  - `self.result_tx.take().send("b_result")` → fires the oneshot.

**8. Caller's `select!` fires on the oneshot. Returns `"b_result"`.**

## NDJSON log for this run

```json
{"kind":"TaskStarted","taskId":0,"handlerId":1,"handler":{"kind":"TypeScript","module":"./a.ts","func":"a"},"value":"start"}
{"kind":"TaskCompleted","taskId":0,"value":"a_result"}
{"kind":"TaskStarted","taskId":1,"handlerId":0,"handler":{"kind":"TypeScript","module":"./b.ts","func":"b"},"value":"a_result"}
{"kind":"TaskCompleted","taskId":1,"value":"b_result"}
```

For replay: filter to `TaskCompleted` lines. Feed them into a fresh EngineApplier. The same `TaskStarted` events are produced deterministically.

## What doesn't change

- **Applier trait.** `fn apply(&mut self, event: &Event)` — unchanged.
- **NdjsonApplier.** Still logs every event. Updated to handle new field types (`TaskId` serializes as a number, no `TaskResult` wrapper).
- **run_event_loop.** Same structure. Only the receiver type changes (`UnboundedReceiver` instead of `Receiver`).

## Future: error handling is an AST concern

Handlers cross a process boundary (Rust → Node.js). They can always fail. Every handler returns a Result — not as a special engine concept, but as the natural return type of any cross-boundary call. A Result is just a Value with a particular shape (`{"ok": ...}` or `{"err": ...}`).

Error handling — retries, unwrap, propagation — is expressible via existing AST primitives. No engine or scheduler machinery needed.

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

Add a max iteration count to Loop for bounded retries (e.g., retry 3 times). This composes naturally: `retry(3, handler)` is sugar for the above.

**Other combinators** (future AST nodes or convenience wrappers):
- `unwrap`: extract Ok value or panic the workflow
- `map` / `and_then`: transform Result values
- `?` (early return): short-circuit a Chain on Err

**What this means for the engine:** WorkflowState doesn't distinguish success from failure. It routes opaque Values through the AST. The AST nodes (Switch, Loop) interpret Result shapes. The Scheduler runs handlers and returns whatever happened. Neither layer needs special error/retry logic.

For the current milestone, handlers are no-ops that always succeed. Results, retries, and error combinators are future AST work. Nothing in the current architecture forecloses on them.
