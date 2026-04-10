# Pause/Wait: Unified Suspension Primitive

## Motivation

Two features that are actually the same mechanism:

1. **External signals**: suspend until a human clicks "approve" or a webhook fires.
2. **Time-based triggers**: suspend for a duration (delay, backoff, SLA escalation).

In both cases: persist state, halt, resume when a trigger fires. The trigger source is the only difference.

## The key insight: Suspend is just a slow Invoke

From WorkflowState's perspective, every Invoke dispatches a task and waits for `complete(task_id, value)`. Whether the task is a TypeScript handler that takes 100ms or a human approval that takes 3 days — WorkflowState doesn't know or care. It dispatched work. It's waiting for a result.

This means Suspend doesn't need special engine support. It's an Invoke with a handler that the **runtime** knows to treat differently. WorkflowState dispatches it like any other task. The runtime sees the handler kind, persists state, and waits for the trigger instead of running code.

```
WorkflowState: advance() → Dispatch { task_id: 5, handler_id: 3, value: ... }
                                       ↓
Runtime sees handler_id 3 is HandlerKind::Suspend(Signal { name: "approval" })
                                       ↓
Runtime persists WorkflowState, registers trigger, halts
                                       ↓
         ... days pass ...
                                       ↓
Trigger fires with payload → Runtime deserializes WorkflowState
                                       ↓
WorkflowState: complete(TaskId(5), payload) → continues execution
```

## AST

```rust
// In the Action enum / FlatAction:
Suspend { trigger: Trigger }

pub enum Trigger {
    /// Wait for an external signal delivered via API.
    Signal { name: String },
    /// Wait for a fixed duration.
    Delay { duration_ms: u64 },
}
```

This extends `HandlerKind`:

```rust
pub enum HandlerKind {
    TypeScript(TypeScriptHandler),
    Suspend(Trigger),  // new
}
```

Or alternatively, Suspend is its own `FlatAction` variant (not an Invoke). Both work — the runtime just needs to recognize it in the dispatch. The `FlatAction` variant is cleaner because Suspend isn't really "invoking a handler" — it's a control flow primitive that happens to use the same dispatch/complete machinery.

### TypeScript API

```typescript
// Wait for an external signal. Resumes with the signal's payload.
function waitForSignal<TPayload>(name: string): TypedAction<unknown, TPayload>

// Wait for a fixed duration. Input passes through unchanged.
function delay<T>(durationMs: number): TypedAction<T, T>
```

`waitForSignal` ignores its input; its output is the signal payload. `delay` is a passthrough — input flows through unchanged after the pause.

## State persistence: already solved

WorkflowState is already almost snapshotable. All fields are fully owned with no lifetimes, function pointers, or channels:

| Field | Type | Serializable? |
|-------|------|---------------|
| `flat_config` | `FlatConfig` | ✅ All u32 newtypes + `Vec<HandlerKind>` |
| `frames` | `Slab<Frame>` | ✅ With `slab` crate's `serde` feature |
| `task_to_parent` | `BTreeMap<TaskId, Option<ParentRef>>` | ✅ All Copy types |
| `pending_dispatches` | `Vec<Dispatch>` | ✅ Contains `Value` (serde_json) |
| `next_task_id` | `u32` | ✅ Primitive |

The frame tree IS the defunctionalized continuation. Each `Frame` captures where execution paused and what intermediate state has accumulated (e.g., `Parallel { results: Vec<Option<Value>> }` tracks which branches have completed). This is exactly the "explicit continuation passing" approach — it's already built.

**Work needed:**
1. Add `Serialize, Deserialize` derives to `WorkflowState`, `Frame`, `FrameKind`, `ParentRef`, `FrameId`
2. Enable `serde` feature on the `slab` crate dependency
3. Add `Serialize, Deserialize` to the `u32_newtype!` macro
4. Add `Serialize, Deserialize` to `FlatConfig`, `FlatAction`, `FlatEntry`

No structural changes. Just derives.

## Snapshot and resume

### Snapshot

The runtime can snapshot WorkflowState at any point. The natural point is **before dispatching** — when `take_pending_dispatches()` hasn't been called yet, so pending work is still in WorkflowState. On resume, the runtime calls `take_pending_dispatches()` and re-dispatches everything.

```rust
// In run_workflow, after each completion:
let snapshot = serde_json::to_string(&workflow_state)?;
persist(run_id, &snapshot);

// Then proceed to take_pending_dispatches and dispatch
```

This gives crash recovery for free: if the process dies, deserialize the last snapshot, re-take dispatches, re-run handlers. Handlers must be idempotent (or at-least-once is acceptable).

### Resume

```rust
// Later, when a trigger fires or the process restarts:
let snapshot = load(run_id);
let mut workflow_state: WorkflowState = serde_json::from_str(&snapshot)?;
let mut scheduler = Scheduler::new();

// If resuming from a trigger:
if let Some((task_id, payload)) = trigger_completion {
    workflow_state.complete(task_id, payload)?;
}

// Continue the normal run loop
run_workflow_loop(&mut workflow_state, &mut scheduler).await
```

The resume path is the same as the normal run path. WorkflowState doesn't know it was serialized and deserialized. It just keeps processing dispatches and completions.

### Replay (alternative to snapshots)

Instead of snapshotting WorkflowState directly, record the completion log: `Vec<(TaskId, Value)>`. To resume, create a fresh WorkflowState from the config, replay all completions. This is simpler and more correct (no snapshot versioning issues) but requires storing the full log.

Both approaches work. Replay is better for durability guarantees. Snapshots are better for large workflows where replaying from scratch is expensive.

## Suspend inside Parallel

If one branch of Parallel hits Suspend and other branches have active handlers:

1. Active handlers continue running and completing normally
2. Parallel frame accumulates their results as usual
3. When only suspended tasks remain, the runtime persists state
4. When a trigger fires, the runtime resumes and the Parallel frame continues accumulating

This works naturally because Suspend uses the same dispatch/complete machinery. The Parallel frame doesn't know or care that one of its children is a signal wait and another is a TypeScript handler.

## Timeout as composition: Race

"Wait for signal OR time out" composes from a Race primitive:

```typescript
race(
  waitForSignal<Approval>("approval"),
  pipe(
    delay(86_400_000), // 24 hours
    escalateToManager(),
  ),
)
```

Race is like Parallel but returns the first result and cancels the rest. It's a distinct AST node:

```rust
pub struct RaceAction {
    pub actions: Vec<Action>,
}
```

Cancellation of the losing branches means: when one branch completes, the runtime drops (doesn't dispatch or complete) any pending work from other branches. This requires tracking which tasks belong to which Race branch — a runtime concern, not a WorkflowState concern.

## Retry with backoff (updated — no Attempt)

> **Convention**: All discriminated unions use `TaggedUnion<Def>` — every variant carries `{ kind: K; value: T; __def?: Def }`. All union constructors require the full variant map so output carries `__def`. Branch auto-unwraps `value`.

```typescript
loop(
  pipe(
    callExternalApi(),
    // Handler returns a Result: { kind: "Ok"; value: T } | { kind: "Err"; value: E }
    // Branch auto-unwraps: Ok handler receives T, Err handler receives E
    branch({
      Ok: done(),          // receives success value, breaks the loop
      Err: pipe(
        computeBackoff(),  // receives error value, produces { delayMs: number }
        getField("delayMs"),
        delay(???),        // dynamic duration — see open question
        recur(),           // Continue the loop
      ),
    }),
  ),
)
```

No Attempt node needed. The handler returns a Result (it always does — cross-boundary calls can fail). Branch (Switch) dispatches on the Result kind. See DEFERRED_FEATURES.md § "Remove Attempt" and ENGINE_APPLIER.md § "Future: error handling is an AST concern."

## Open questions

**Dynamic delay durations.** `delay` takes a static `duration_ms` in the AST. Backoff needs a runtime-computed duration. Cleanest option: `delay` always reads duration from its input value, and the "static" version is `pipe(constant(5000), delay())`. Consistent with how other builtins work.

**Cancellation semantics.** Suspended workflows waiting for signals that never come. Options: TTL on triggers, administrative cancellation API, or both.

**Signal naming and scoping.** Signal names need to be unique per workflow run. Auto-scope with run ID, or leave to the workflow author?

**Idempotent resume.** Duplicate signal delivery (network retry) must be a no-op for already-resumed workflows.

**FlatConfig in the snapshot.** FlatConfig is static — it never changes during execution. Two options: (1) include it in every snapshot (simple, self-contained), (2) store it separately and only snapshot the dynamic state (frames, task_to_parent, pending_dispatches, next_task_id). Option 1 is simpler. Option 2 is more efficient for large configs with many snapshots.

**Snapshot versioning.** If WorkflowState's internal representation changes between releases, old snapshots become unloadable. Mitigation: version field in the snapshot format, or always use replay (which depends on the stable public API, not internal representation).
