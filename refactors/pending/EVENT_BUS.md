# Event Bus

## Motivation

Concurrent pipelines inside `all()` have no way to communicate with each other. One branch can produce values and another branch wants to consume them, but there's no channel between them. The only shared state primitive (`withState`) provides a mutable cell — it can't express "wait until a value is available."

The event bus fills this gap: a scoped, bounded channel between concurrent branches.

## The API

```ts
eventBus<TEvent, TIn, TOut>(
  body: (ctx: {
    send: TypedAction<TEvent, void>;
    receive: TypedAction<never, TEvent>;
  }) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut>
```

HOAS pattern — same as `withState`, `bind`, `loop`. The callback receives effect tokens scoped to this channel.

- **`send`** — enqueue a value. Input is `TEvent`. Output is `void`. Never blocks (the channel is unbounded).
- **`receive`** — dequeue the next value. Input is `never` (doesn't consume the pipeline value). Output is `TEvent`. Suspends until a value is available.

### Example: producer/consumer

```ts
eventBus<CiEvent, PrUrl, void>(({ send, receive }) =>
  all(
    // producer: polls CI, sends events
    loop((recur) =>
      pipe(
        pollCiStatus,
        send,
        sleep(30_000).then(recur),
      ),
    ),
    // consumer: processes events as they arrive
    loop((recur) =>
      pipe(
        receive,
        handleCiEvent,
        recur,
      ),
    ),
  ),
)
```

The producer and consumer run concurrently inside `all()`. The producer sends events whenever CI status changes. The consumer's `receive` suspends until the next event arrives — no polling, no sleep, no `Option` check.

### Example: fan-in from multiple sources

```ts
eventBus<PrEvent, PrUrl, PrResult>(({ send, receive }) =>
  all(
    // source 1: CI status
    loop((recur) => pipe(pollCi, send, sleep(30_000).then(recur))),
    // source 2: review comments
    loop((recur) => pipe(pollReviews, send, sleep(60_000).then(recur))),
    // single consumer
    loop<PrResult>((recur, done) =>
      pipe(
        receive,
        branch({
          CiCompleted: pipe(handleCi, recur),
          ReviewSubmitted: pipe(handleReview, recur),
          Closed: done,
        }),
      ),
    ),
  ),
)
```

Multiple producers send into the same bus. One consumer processes events sequentially in arrival order.

## Why this requires a new executor primitive

Current handler DAGs are pure data transformations. They receive `{ payload, state }` and immediately produce `{ kind: "Resume", value, state_update }`. A handler cannot say "don't resume yet — park this branch until something else happens."

`receive` needs exactly that: when the channel is empty, the branch suspends. It resumes only when another branch sends a value. This coordination between concurrent branches is not expressible with handler DAGs.

### Why not build on ResumeHandle?!

A channel *looks* like a ResumeHandle with two operations (send performs, receive performs, handler manages a buffer). Three engine limitations prevent this:

1. **Concurrent performs.** `all(branch_that_sends, branch_that_receives)` means both branches perform on the same handler simultaneously. The engine assumes one in-flight ResumePerform per handler — the second perform does not execute independently. (`defineRecursiveFunctions` documents this exact limitation.)

2. **Handler state.** A channel handler needs mutable state (the buffer, parked receivers) that persists across performs. Current handlers are stateless data transforms — they dispatch and resume, retaining nothing between invocations.

3. **Selective resumption.** When the buffer is empty, the handler must park the receiver and NOT resume it. Later, when a send arrives, the handler must wake the parked receiver. Current handlers always immediately produce a Resume response.

Fixing all three would make ResumeHandle powerful enough to express channels, but that's a larger engine overhaul. A dedicated primitive sidesteps all three: the executor manages the queue directly (like `sleep` manages a timer directly), with no handler DAG in the loop.

The closest analogy in the existing system is `sleep` — a built-in action that the Rust executor handles directly (it parks the task on a timer, not on a handler DAG). The event bus is the same idea: a built-in action that parks the task on a channel.

## How it compiles

### AST

Two new action variants:

```
EventBusHandle {
  event_bus_id: EventBusId,
  body: Action,
}

EventBusSend {
  event_bus_id: EventBusId,
}

EventBusReceive {
  event_bus_id: EventBusId,
}
```

`EventBusId` is a gensym'd identifier, allocated at AST construction time (same as `ResumeHandlerId`).

### TypeScript construction

```ts
function eventBus<TEvent, TIn, TOut>(
  body: (ctx: {
    send: TypedAction<TEvent, void>;
    receive: TypedAction<never, TEvent>;
  }) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut> {
  const event_bus_id = allocateEventBusId();
  const send = typedAction({ kind: "EventBusSend", event_bus_id });
  const receive = typedAction({ kind: "EventBusReceive", event_bus_id });
  const bodyAction = toAction(body({ send, receive }));
  return typedAction({ kind: "EventBusHandle", event_bus_id, body: bodyAction });
}
```

### Executor behavior

When the executor enters an `EventBusHandle` frame, it creates an unbounded FIFO queue (a `tokio::sync::mpsc::unbounded_channel` or equivalent).

**Send**: The executor enqueues the value and immediately resumes the branch with `void`. If any branches are parked on `receive`, one is woken with the value (FIFO wake order).

**Receive**: The executor checks the queue. If non-empty, it dequeues the front value and resumes immediately. If empty, it parks the branch — the branch does not advance until a send arrives.

**Teardown**: When the `EventBusHandle` frame completes (all branches of the body finish), the channel is dropped. If any branches are still parked on `receive` when the frame tears down (because another branch caused the `all` to complete via `race` semantics), the parked branches are cancelled as part of normal frame teardown.

## Semantics

**FIFO ordering.** Values dequeue in send order. If multiple branches send concurrently, their values interleave in executor scheduling order (nondeterministic, but each send's position is stable once enqueued).

**Unbounded buffer.** Send never blocks. The queue grows without limit. This matches the "fire-and-forget producer" pattern where the consumer may lag. A bounded variant (where send blocks when the buffer is full) is a possible future extension but adds backpressure complexity.

**Single-consumer receive.** Each `receive` dequeues exactly one value. If multiple branches call `receive` concurrently, each gets a different value (no broadcast). This is MPSC (multi-producer, single-consumer) semantics, though nothing prevents multiple consumers — they just compete for values.

**Scoped lifetime.** The channel exists only within the `EventBusHandle` frame. `send` and `receive` tokens cannot escape (they're lexically scoped by the HOAS pattern, same as `withState`).

## Relationship to existing primitives

| Primitive | What it provides | Blocking? |
|-----------|-----------------|-----------|
| `withState` | Shared mutable cell | No — get/set always resume immediately |
| `bind` | Immutable captured values | No — read always resumes immediately |
| `sleep` | Timer-based suspension | Yes — executor parks until timer fires |
| **`eventBus`** | **Channel between branches** | **Yes — receive parks until send delivers** |

The event bus fills the gap between "shared state" (immediate, racy) and "external handler" (leaves the engine entirely). It provides intra-workflow coordination with proper suspension semantics.

## Open questions

1. **Bounded vs unbounded.** The design above is unbounded (send never blocks). A bounded channel adds backpressure but introduces deadlock risk: if producer and consumer are in the same `all()` and the buffer fills, send blocks waiting for the consumer to drain, but the consumer can't drain because `all()` waits for all branches. Deadlock. Unbounded avoids this entirely. Worth adding a capacity parameter later, or is unbounded always correct for intra-`all()` communication?

2. **Multiple consumers.** The MPSC semantics mean multiple `receive` branches compete. Should there be a broadcast variant where every consumer sees every event? Or is that a separate primitive (`eventBroadcast`)?

3. **Ordering guarantees.** Within a single sequential pipeline, send ordering is deterministic. Across concurrent branches in `all()`, ordering depends on executor scheduling. Is this acceptable, or do users need a sequence number?

4. **Interaction with `race`.** If the body is wrapped in `race` and one branch finishes, parked `receive` branches are cancelled. This is correct (same as any cancelled branch), but worth documenting.

-----

- concurrent performs. More info on why we have this.
- I would rather fix resume handle than add another basic primitive, and instead build this on result handler.
- it seems fine to not have a deterministic order when dequeueing, and even preferable, so that no one accidentally relies on that behavior.
- selective resumption: if we can only support a maybeDequeue fn, that seems fine too, i.e. this might get rid of the need for state + suspense
- I don't think we need an actual channel, right? The actual impl of the channel is a vec of where to send data + a queue, right? Or, if we don't have suspense,  then we don't have to store "where to send the data" i.e. nothing parks
- let's rewrite with the goal of just maybeDequeue, and see what we need to add to support this in userland
- goal is no new queue-specific primitives and do impl queue in userland
- all the other details are over engineering, the priority is simple primitives and that's it.
- multiple consumers -> no, that's just state where you pop onto an array and read that state