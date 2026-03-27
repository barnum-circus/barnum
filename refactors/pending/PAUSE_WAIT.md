# Pause/Wait: Unified Suspension Primitive

## Motivation

MISSING_FEATURES.md identifies two gaps as separate items:

1. **Pause/Wait** (external signals): suspend execution until a human clicks "approve" or a webhook fires.
2. **Time-based triggers**: suspend execution for a duration (delay, exponential backoff, SLA escalation).

These are the same mechanism. In both cases, the VM:
- Serializes execution state (which Sequence step, Loop iteration, accumulated All results)
- Persists it to durable storage
- Halts
- Resumes when a trigger fires (external signal or timer)

The trigger source is the only difference. The algebra doesn't need two primitives.

## The Primitive

A single `Suspend` action node. The workflow's execution state is persisted, and a trigger is registered. When the trigger fires, execution resumes and the trigger's payload becomes the output.

### AST

```rust
pub struct SuspendAction {
    pub trigger: Trigger,
}

pub enum Trigger {
    /// Wait for an external signal delivered via API.
    Signal {
        /// Logical name the caller uses to deliver the signal (e.g., "approval_{task_id}").
        name: String,
    },
    /// Wait for a fixed duration.
    Delay {
        duration_ms: u64,
    },
}
```

Both `Signal` and `Delay` share the same VM codepath: persist state, halt, resume on trigger. `Signal` registers a listener keyed by name. `Delay` schedules a timer. Future trigger types (cron, composite AND/OR of triggers) extend the `Trigger` enum without changing the VM's suspend/resume machinery.

### TypeScript API

```typescript
// Wait for an external signal. Resumes with the signal's payload.
function waitForSignal<TPayload>(
  name: string,
): TypedAction<unknown, TPayload>

// Wait for a fixed duration. Input passes through unchanged.
function delay<T>(durationMs: number): TypedAction<T, T>
```

`waitForSignal` ignores its input and produces whatever payload the external caller provides. `delay` is a passthrough that just adds a pause.

### Timeout as composition

A "wait for signal OR time out" is not a new primitive. It composes from existing pieces:

```typescript
// Wait for approval, but escalate after 24 hours
sequence(
  all(
    waitForSignal<Approval>("approval"),
    delay(86_400_000),
  ),
  // First result wins; the other is cancelled
)
```

This does require `All` to support "first-to-complete" semantics (race), which it doesn't today. Two options:

1. Add a `Race` primitive (parallel, returns first result, cancels the rest).
2. Add a `mode` field to `All` (`{ mode: "all" | "race" }`).

A `Race` primitive is cleaner: it's a distinct operation with distinct semantics (one winner vs. all results). Putting a mode flag on `All` conflates two operations that share an implementation detail (parallel dispatch) but differ in their fundamental contract.

```rust
pub struct RaceAction {
    pub actions: Vec<Action>,
}
```

```typescript
function race<Out>(
  ...actions: TypedAction<unknown, Out>[]
): TypedAction<unknown, Out>
```

### Examples

**Human-in-the-loop approval:**
```typescript
sequence(
  prepareReport(),           // In -> Report
  submitForReview(),         // Report -> { reviewId: string }
  extractField("reviewId"),  // -> string
  // Workflow suspends here. External API delivers the approval payload.
  waitForSignal<ApprovalDecision>("review"),
  matchCases({
    Approved: publishReport(),
    Rejected: notifyAuthor(),
  }),
)
```

**Retry with exponential backoff:**
```typescript
loop(
  sequence(
    attempt(callExternalApi()),
    matchCases({
      Ok: done(),
      Err: sequence(
        extractField("retryCount"),
        computeBackoff(),     // number -> { delayMs: number }
        extractField("delayMs"),
        delay(/* dynamic — see open question below */),
        recur(),
      ),
    }),
  ),
)
```

The backoff example exposes a design question: `delay` takes a static duration in the AST, but backoff needs a dynamic duration computed at runtime. See open questions.

**SLA escalation:**
```typescript
race(
  sequence(
    waitForSignal<Result>("task_complete"),
    processResult(),
  ),
  sequence(
    delay(86_400_000), // 24 hours
    escalateToManager(),
  ),
)
```

## State Persistence

Suspend requires serializing the VM's execution continuation: where in the AST tree execution paused, and all intermediate state accumulated up to that point.

What needs to be serialized:
- **Program counter**: which Action node is currently executing, including position within Sequence (step index), Loop (iteration), All (which branches completed), Match (which case was taken).
- **Data stack**: the current value flowing through the pipeline, plus any partial results from All branches.
- **Step bindings**: if named Steps are in scope, their resolved Actions.

The VM's evaluator already walks the AST tree recursively. Serializing the continuation means capturing this call stack as data. Two approaches:

1. **Explicit continuation passing**: transform the recursive evaluator into a state machine where the "stack" is a serializable data structure (a list of frames, each describing which AST node and what position within it). Essentially, defunctionalize the continuation.

2. **Checkpoint at Suspend boundaries only**: restrict Suspend to appear at specific points (e.g., top-level Sequence steps only, not inside Traverse or All branches). This simplifies serialization at the cost of expressiveness.

Option 1 is the general solution. Option 2 is a pragmatic starting point that covers the common cases (human approval, delays between pipeline stages) without solving the hard problem of suspending mid-parallel-execution.

## Resume Infrastructure

When a trigger fires:
1. Look up the persisted state by trigger key (signal name or timer ID).
2. Deserialize the execution continuation.
3. Inject the trigger payload as the current value.
4. Resume the evaluator from the saved continuation.

For `Signal` triggers, the external API looks like:

```
POST /signals/{signal_name}
Content-Type: application/json

{ "payload": { "approved": true, "reviewer": "alice" } }
```

For `Delay` triggers, a background scheduler fires the timer. The scheduler is infrastructure — could be a Tokio timer for in-process, or a database-backed job queue for distributed deployments. The algebra doesn't care.

## Open Questions

**Dynamic delay durations.** The `delay` builtin takes a static `duration_ms` in the AST. But retry backoff needs a duration computed at runtime from handler output. Options:
- `delay` reads the duration from its input value (e.g., input must be `{ delayMs: number }`). This makes the API less explicit.
- A separate `dynamicDelay` that reads duration from input. Two delay variants feels unnecessary.
- `delay` always reads from input, and the "static" version is just `sequence(constant(5000), delay())`. This is consistent with how `range` works.

**Cancellation semantics.** If a workflow is suspended waiting for a signal that never comes, how is it cleaned up? Options: explicit TTL on all triggers, administrative cancellation API, or both.

**Suspend inside parallel branches.** If one branch of `All` hits a Suspend, do the other branches continue executing or are they all suspended? If one branch of `Race` hits Suspend and another completes, the Suspend should be cancelled. This interacts with the evaluator's concurrency model.

**Signal naming and scoping.** Signal names need to be unique per workflow run. Should the VM automatically scope them (e.g., prefix with run ID), or is that the workflow author's responsibility?

**Idempotent resume.** If a signal is delivered twice (network retry), the VM must handle it idempotently. The second delivery to an already-resumed workflow should be a no-op, not a crash.

**Persistence backend.** The design assumes durable storage exists but doesn't specify it. For local dev, a file on disk. For production, a database. The Applier pattern in `barnum_event_loop` could serve as the integration point: a `PersistenceApplier` that serializes state on Suspend events and loads it on resume.
