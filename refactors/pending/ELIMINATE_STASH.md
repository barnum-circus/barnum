# Eliminate the Stash

**Depends on:** RESUME_VS_RESTART_HANDLERS.md (tasks 2 and 3 must land first)

## What the stash is

The stash is a `VecDeque<StashedItem>` on `WorkflowState` that holds deferred work items. Two kinds of items get stashed:

1. **Deliveries** (`StashedItem::Delivery`): A task completed, but its delivery path passes through a suspended Handle. The value is stashed until the Handle becomes free.
2. **Effects** (`StashedItem::Effect`): A Perform fired, but `find_blocking_ancestor` detected a suspended Handle between the Perform and its target Handle. The effect is stashed until the path clears.

When any `complete()` call finishes, `sweep_stash` runs in a loop, retrying every stashed item. Items that are still blocked stay in the stash. Items whose path is now clear get processed. The loop repeats until no progress is made.

## The full stash infrastructure

Across the engine, the stash involves:

| Component | File | Lines |
|-----------|------|-------|
| `StashedItem` enum | `lib.rs` | 150-169 |
| `AncestorCheck` enum | `lib.rs` | 171-180 |
| `TryDeliverResult` enum (Blocked variant) | `lib.rs` | 182-192 |
| `StashOutcome` enum | `lib.rs` | 194-202 |
| `SweepResult` enum | `lib.rs` | 204-213 |
| `stashed_items` field | `lib.rs` | 231 |
| `sweep_stash` function | `complete.rs` | 79-87 |
| `sweep_stash_once` function | `complete.rs` | 91-143 |
| `try_deliver` function | `complete.rs` | 61-74 |
| `find_blocking_ancestor` function | `ancestors.rs` | 42-52 |
| `is_blocked_by_handle` function | `ancestors.rs` | 55-65 |
| `HandleStatus::Suspended` variant | `frame.rs` | 103-106 |
| `bubble_effect` (Blocked path) | `effects.rs` | 17-32 |
| `resume_continuation` (Blocked path) | `effects.rs` | 158-189 |
| Stash assertions in tests | `effects.rs` | 569, 654, 667, 888 |

## Why the stash exists

The stash exists because the engine **suspends the Handle frame** whenever a Perform fires.

When a Perform fires during `advance`, the engine:

1. Walks the ancestor chain to find a matching Handle.
2. Marks the Handle as `HandleStatus::Suspended(perform_parent)`.
3. Advances the handler DAG as a child of the Handle frame (Handler side).

While the Handle is Suspended, any delivery or effect that needs to pass through that Handle is blocked. The item goes into the stash. When the handler completes, the Handle becomes Free again, and `sweep_stash` retries everything.

The suspension is the single reason the stash exists. If the Handle never suspended, deliveries and effects would always have a clear path, and there would be nothing to stash.

## Why suspension is unnecessary

Every Handle/Perform usage in the system falls into one of two categories:

| Category | Combinators | What the handler does |
|----------|------------|----------------------|
| Resume | `bind`, `counter` | Returns a value to the Perform site. Body continues. |
| Restart | `loop`, `tryCatch`, `race`, `earlyReturn`, `scope`/`jump` | Returns a new body input. Body is torn down and re-entered. |

Neither category needs suspension:

**Resume handlers** deliver a value back to the Perform site. The body continues running. There is no reason to freeze the body while the handler runs. The handler is logically a function call at the Perform site: the Perform's input goes in, a value comes out, and execution continues from the Perform's parent. If the handler DAG were advanced as a child of a frame at the Perform site (instead of as a child of the Handle frame), the Handle would never need to know about it. Siblings would continue running concurrently.

The `resume_handler_does_not_block_sibling_completion` and `concurrent_resume_performs_not_serialized` tests document this: they assert the correct non-suspending behavior and are currently `#[should_panic]` because the engine suspends on all Performs.

**Restart handlers** tear down the body and re-enter it. Tearing down the body kills all in-flight tasks and removes their frames. Any stashed items that reference those frames become stale. The engine then re-enters the body from scratch. There is no reason to keep body frames alive while the handler runs. Tearing down immediately at the Perform site (instead of waiting for the handler to complete) eliminates the window where stashing could occur.

The `throw_proceeds_while_resume_handler_in_flight` test documents this: a restart-style throw should proceed immediately even while a resume handler is in flight in a sibling branch.

## What "eliminate the stash" means

Delete every row in the table above. `complete()` calls `deliver` directly instead of going through `try_deliver` and `sweep_stash`. `bubble_effect` walks the ancestor chain and dispatches without checking for blocking ancestors.

The simplified `complete()`:

```rust
pub fn complete(workflow_state, task_id, value) -> Result<Option<Value>, CompleteError> {
    let Some(frame_id) = workflow_state.task_to_frame.remove(&task_id) else {
        return Ok(None);
    };
    let frame = workflow_state.frames.remove(frame_id).expect("invoke frame exists");
    match frame.parent {
        Some(parent_ref) => deliver(workflow_state, Some(parent_ref), value),
        None => Ok(Some(value)),
    }
}
```

No stash sweep. No try_deliver. Direct delivery.

## Prerequisites

The stash cannot be deleted until Performs stop suspending the Handle. This requires the Resume/Restart split from RESUME_VS_RESTART_HANDLERS.md:

1. **ResumePerform** runs the handler as a child of a ResumePerformFrame at the Perform site. The ResumeHandle frame is uninvolved. (RESUME_VS_RESTART_HANDLERS.md, Task 2)

2. **RestartPerform** tears down the body immediately and advances the handler as a child of the RestartHandle frame. No suspension window. (RESUME_VS_RESTART_HANDLERS.md, Task 3)

After both land, `HandleStatus::Suspended` has no remaining producers. The stash has no remaining consumers. Both can be deleted. (RESUME_VS_RESTART_HANDLERS.md, Task 4)

## What about STASH_KEYED_BY_BLOCKER.md?

That doc proposes optimizing the stash by keying items by their blocking Handle. It is obsolete once the stash is deleted. It should be moved to `refactors/past/` (or deleted) when this work lands.
