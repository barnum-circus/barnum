# Event Bus

## Motivation

Concurrent pipelines inside `all()` have no way to communicate with each other. One branch can produce values and another branch wants to consume them, but there's no channel between them. The only shared state primitive (`withState`) provides a mutable cell — it can't express "wait until a value is available."

The event bus fills this gap: a scoped, bounded channel between concurrent branches.

## The API

```ts
eventBus<TEvent, TIn, TOut>(
  body: (ctx: {
    push: TypedAction<TEvent, void>;
    get: TypedAction<never, TEvent>;
  }) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut>
```

HOAS pattern — same as `withState`, `bind`, `loop`. The callback receives effect tokens scoped to this channel.

- **`push`** — enqueue a value. Input is `TEvent`. Output is `void`. Never blocks (the channel is unbounded).
- **`get`** — dequeue the next value. Input is `never` (doesn't consume the pipeline value). Output is `TEvent`. Suspends until a value is available.

### Example: producer/consumer

```ts
eventBus<CiEvent, PrUrl, void>(({ push, get }) =>
  all(
    // producer: polls CI, pushes events
    loop((recur) =>
      pipe(
        pollCiStatus,
        push,
        sleep(30_000).then(recur),
      ),
    ),
    // consumer: processes events as they arrive
    loop((recur) =>
      pipe(
        get,
        handleCiEvent,
        recur,
      ),
    ),
  ),
)
```

The producer and consumer run concurrently inside `all()`. The producer pushes events whenever CI status changes. The consumer's `get` suspends until the next event arrives — no polling, no sleep, no `Option` check.

### Example: fan-in from multiple sources

```ts
eventBus<PrEvent, PrUrl, PrResult>(({ push, get }) =>
  all(
    // source 1: CI status
    loop((recur) => pipe(pollCi, push, sleep(30_000).then(recur))),
    // source 2: review comments
    loop((recur) => pipe(pollReviews, push, sleep(60_000).then(recur))),
    // single consumer
    loop<PrResult>((recur, done) =>
      pipe(
        get,
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

Multiple producers push into the same bus. One consumer processes events sequentially in arrival order.

## Why this requires a new executor primitive

Current handler DAGs are pure data transformations. They receive `{ payload, state }` and immediately produce `{ kind: "Resume", value, state_update }`. A handler cannot say "don't resume yet — park this branch until something else happens."

`get` needs exactly that: when the channel is empty, the branch suspends. It resumes only when another branch pushes a value. This coordination between concurrent branches is not expressible with handler DAGs.

The closest analogy in the existing system is `sleep` — a built-in action that the Rust executor handles directly (it parks the task on a timer, not on a handler DAG). The event bus is a scoped version of the same idea: a built-in action that parks the task on a channel.

## How it compiles

### AST

Two new action variants:

```
EventBusHandle {
  event_bus_id: EventBusId,
  body: Action,
}

EventBusPush {
  event_bus_id: EventBusId,
}

EventBusGet {
  event_bus_id: EventBusId,
}
```

`EventBusId` is a gensym'd identifier, allocated at AST construction time (same as `ResumeHandlerId`).

### TypeScript construction

```ts
function eventBus<TEvent, TIn, TOut>(
  body: (ctx: {
    push: TypedAction<TEvent, void>;
    get: TypedAction<never, TEvent>;
  }) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut> {
  const event_bus_id = allocateEventBusId();
  const push = typedAction({ kind: "EventBusPush", event_bus_id });
  const get = typedAction({ kind: "EventBusGet", event_bus_id });
  const bodyAction = toAction(body({ push, get }));
  return typedAction({ kind: "EventBusHandle", event_bus_id, body: bodyAction });
}
```

### Executor behavior

When the executor enters an `EventBusHandle` frame, it creates an unbounded FIFO queue (a `tokio::sync::mpsc::unbounded_channel` or equivalent).

**Push**: The executor enqueues the value and immediately resumes the branch with `void`. If any branches are parked on `get`, one is woken with the value (FIFO wake order).

**Get**: The executor checks the queue. If non-empty, it dequeues the front value and resumes immediately. If empty, it parks the branch — the branch does not advance until a push arrives.

**Teardown**: When the `EventBusHandle` frame completes (all branches of the body finish), the channel is dropped. If any branches are still parked on `get` when the frame tears down (because another branch caused the `all` to complete via `race` semantics), the parked branches are cancelled as part of normal frame teardown.

## Semantics

**FIFO ordering.** Values dequeue in push order. If multiple branches push concurrently, their values interleave in executor scheduling order (nondeterministic, but each push's position is stable once enqueued).

**Unbounded buffer.** Push never blocks. The queue grows without limit. This matches the "fire-and-forget producer" pattern where the consumer may lag. A bounded variant (where push blocks when the buffer is full) is a possible future extension but adds backpressure complexity.

**Single-consumer get.** Each `get` dequeues exactly one value. If multiple branches call `get` concurrently, each gets a different value (no broadcast). This is MPSC (multi-producer, single-consumer) semantics, though nothing prevents multiple consumers — they just compete for values.

**Scoped lifetime.** The channel exists only within the `EventBusHandle` frame. `push` and `get` tokens cannot escape (they're lexically scoped by the HOAS pattern, same as `withState`).

## Relationship to existing primitives

| Primitive | What it provides | Blocking? |
|-----------|-----------------|-----------|
| `withState` | Shared mutable cell | No — get/set always resume immediately |
| `bind` | Immutable captured values | No — read always resumes immediately |
| `sleep` | Timer-based suspension | Yes — executor parks until timer fires |
| **`eventBus`** | **Channel between branches** | **Yes — get parks until push delivers** |

The event bus fills the gap between "shared state" (immediate, racy) and "external handler" (leaves the engine entirely). It provides intra-workflow coordination with proper suspension semantics.

## Open questions

1. **Bounded vs unbounded.** The design above is unbounded (push never blocks). A bounded channel adds backpressure but introduces deadlock risk if producer and consumer are in the same `all()` and the buffer fills. Unbounded is simpler and matches most use cases. Worth adding a capacity parameter later?

2. **Multiple consumers.** The MPSC semantics mean multiple `get` branches compete. Should there be a broadcast variant where every consumer sees every event? Or is that a separate primitive (`eventBroadcast`)?

3. **Ordering guarantees.** Within a single sequential pipeline, push ordering is deterministic. Across concurrent branches in `all()`, ordering depends on executor scheduling. Is this acceptable, or do users need a sequence number?

4. **Interaction with `race`.** If the body is wrapped in `race` and one branch finishes, parked `get` branches are cancelled. This is correct (same as any cancelled branch), but worth documenting.
