# Phase 6: Durable Suspension

## Goal

A workflow can serialize its entire state to storage and go dormant. An external trigger (webhook, CLI command, human action) resumes it later. This transforms Barnum from a batch orchestrator into a durable workflow engine.

## Prerequisites

All previous phases. The entire effect mechanism must be stable. Serialization touches every frame kind and every effect type.

## The effect

```
Effect: Suspend
Payload: Value (prompt/context for what's being waited on)
Handler behavior:
  The scheduler does NOT resume immediately.
  It returns a top-level signal indicating the workflow is dormant.
  The external driver persists the WorkflowState.
  A future event triggers resume.
```

This is the first effect where the continuation is not resolved within a single scheduler tick. The continuation is persisted to durable storage and resumed in a different process invocation.

## How it works

### Suspending

```ts
// User writes:
pipe(
  createPR,
  pause({ event: "pr-approved", key: extractField("prUrl") }),
  deploy,
)

// pause compiles to:
Perform("Suspend")
```

When the scheduler encounters Perform("Suspend"):

1. `bubble_effect` walks up to a Handle("Suspend") or to the workflow root.
2. If a Handle catches it, the handler can customize the suspension behavior.
3. If it reaches the root, the scheduler returns a top-level `Suspended(prompt, continuation_state)` result.
4. The external driver serializes the entire `WorkflowState` to a database/file.
5. The process exits.

### Resuming

```bash
barnum resume --state <file> --event '{"kind": "pr-approved", "prUrl": "..."}'
```

1. The driver loads the persisted `WorkflowState`.
2. It calls `resume_continuation(cont_id, event_value)`.
3. The scheduler reconnects the continuation and delivers the event value.
4. The workflow continues from the pause point.

## Serialization requirements

The entire `WorkflowState` must be serializable:

- The slab of frames (each frame's kind, parent pointer, local state)
- The environment (Handle frame ReadVar bindings)
- Pending task state (which external tasks are in flight)
- Continuation state (which continuations are active, their root pointers)
- The flat action table (already serializable — it's the compiled config)

### What's hard

- **Handler closures**: If a Handle frame stores handler logic as a Rust closure or function pointer, it's not serializable. Handler logic must be stored as ActionIds (references into the flat action table), which are serializable.
- **Values**: All Values flowing through the system must be serializable (they already are — JSON).
- **External task state**: If an external task was in flight when the workflow suspended, the driver must decide: cancel it and re-dispatch on resume, or persist enough state to reconnect.

### Frame serialization

Each frame kind must implement `Serialize`/`Deserialize`:

```rust
#[derive(Serialize, Deserialize)]
pub enum FrameKind {
    Chain { stage: ChainStage, ... },
    Parallel { results: Vec<Option<Value>>, ... },
    Handle { bindings: HashMap<DeclareId, Value>, continuations: ..., ... },
    // etc.
}
```

This is straightforward if all local state is data (Values, ActionIds, integers). No function pointers, no closures, no Rust-heap references that can't serialize.

## Interaction with other effects

### Suspend inside tryCatch

If a pause point is inside a tryCatch body, and the process restarts, the tryCatch Handle must still be active to catch errors during the remaining execution.

This works naturally: the Handle frame is part of the serialized slab. On resume, the Handle frame is restored with its handler DAGs (ActionIds into the flat table). If the resumed body throws, bubble_effect finds the Handle frame and dispatches to the catch handler.

### Suspend inside loop

If a pause point is inside a loop body, resume must re-enter the loop correctly. The Loop Handle frame (from Phase 4) is in the slab. The continuation root is below the Loop Handle. Resuming delivers a value into the continuation, which completes the current iteration. If Continue fires, the Loop Handle re-enters the body.

### Suspend inside Parallel

If one parallel branch suspends and the other is still running, the driver must handle this. Options:
- Suspend the entire workflow (both branches pause). On resume, the other branch is re-advanced.
- Let the other branch continue executing, and only suspend when all branches are paused or complete.

Recommend: Suspend pauses the entire workflow. The driver serializes the full state. On resume, all branches resume from their last state.

## Test strategy

1. Simple suspend and resume: workflow pauses, state serialized, state loaded, workflow continues.
2. Suspend inside Chain: work before pause, work after pause.
3. Suspend inside loop: pause in loop body, resume, loop continues.
4. Suspend inside tryCatch: resume, body throws, catch handles.
5. Multiple suspend points: workflow pauses twice. Each resume advances to the next pause or completion.
6. Suspend with variables: declare bindings are preserved across suspend/resume.
7. Round-trip serialization: serialize → deserialize → verify identical state.

## Deliverables

1. `EffectType::Suspend` variant
2. Top-level `WorkflowResult::Suspended` return from scheduler
3. `WorkflowState` serialization (Serialize/Deserialize on all frame kinds)
4. `resume_continuation` method on WorkflowState
5. `pause()` TypeScript function
6. CLI: `barnum resume --state <file> --event <json>`
7. Tests per above
