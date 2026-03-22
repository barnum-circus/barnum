# Engine Unit Testing: Separate State Transitions from Dispatch

**Status:** In progress

**Depends on:** APPLY_PATTERN (done)

## Motivation

The Engine in `crates/barnum_config/src/runner/mod.rs` mixes two concerns:

1. **State transitions** — given an event, mutate the task tree and decide what work is pending
2. **Dispatch** — spawn threads, submit to pools, run shell commands

These are tangled inside Engine: `apply_entry` mutates `self.state` and pushes to `self.pending_dispatches`, then `flush_dispatches` reads from `self.pending_dispatches` and spawns threads. Both live on the same struct, so testing `apply_entry` requires constructing a full Engine with a pool connection, channel, schemas, etc.

The state transition logic is the hardest code to get right (parent chain walks, child count arithmetic, finally detection, retry replacement) and the easiest to unit test — if it were accessible without I/O dependencies.

## Change

Move `pending_dispatches` from Engine to RunState. Add `apply_entry` on RunState that takes a `&StateLogEntry` and `&Config`, mutates the task tree, and accumulates pending dispatches — but never dispatches anything. Engine calls this method, then drains the pending dispatches into actual thread spawns.

No `apply_entries` batch method — callers just loop.

`in_flight` stays on Engine (it tracks actual workers, not state). Engine decrements `in_flight` when it receives a `WorkerResult`, before calling `state.apply_entry`.

## Unit Tests

### Test helpers

```rust
fn test_step(name: &str) -> Step {
    Step {
        name: StepName::new(name),
        value_schema: None,
        pre: None,
        action: Action::Command { script: "true".into() },
        post: None,
        next: vec![],
        finally_hook: None,
        options: Options::default(),
    }
}

fn test_step_with_finally(name: &str) -> Step {
    Step {
        finally_hook: Some(HookScript::new("echo done")),
        ..test_step(name)
    }
}

fn config(steps: Vec<Step>) -> Config {
    Config { max_concurrency: None, steps }
}
```

### TaskSubmitted

1. **`seed_queues_task_dispatch`** — Apply a seed TaskSubmitted. Assert: task in map as Pending, `PendingDispatch::Task` queued, `next_task_id` advanced.

2. **`spawned_child_queues_dispatch`** — Parent in WaitingForChildren, apply spawned TaskSubmitted. Assert: child in map with correct parent_id, dispatch queued.

3. **`retry_replaces_failed_task`** — Task in Failed state, apply retry TaskSubmitted. Assert: old task removed from map, retry in map with inherited parent_id, dispatch queued.

4. **`multiple_seeds_all_queued`** — Apply three seed entries. Assert: three tasks in map, three dispatches queued, `next_task_id` is 3.

5. **`id_advancement_handles_gaps`** — Apply seed with task_id 5. Assert: `next_task_id` is 6 (not 1).

### TaskCompleted — Success

6. **`leaf_success_removes_task`** — Seed task, complete with empty children. Assert: task removed from map, no dispatches queued (no parent to walk).

7. **`success_with_children_transitions_to_waiting`** — Complete with 2 children. Assert: parent in WaitingForChildren with count 2, children in map as Pending, 2 child dispatches queued.

8. **`success_with_children_advances_ids`** — Complete with children at task_ids 5 and 10. Assert: `next_task_id` is 11.

### TaskCompleted — Failure

9. **`failed_with_retry_inserts_retry`** — Failed with retry. Assert: original task removed (via retry's apply_submitted), retry in map as Pending, retry dispatch queued.

10. **`failed_permanent_removes_task`** — Failed without retry. Assert: task removed from map, no dispatch queued (no parent).

11. **`failed_permanent_under_parent_walks_up`** — Child fails permanently under parent with finally. Assert: child removed, `PendingDispatch::Finally` queued for parent.

### FinallyRun

12. **`finally_no_children_removes_parent`** — FinallyRun with empty children for a parent under a grandparent. Assert: parent removed, grandparent child count decremented.

13. **`finally_with_children_adds_children`** — FinallyRun with 2 children. Assert: parent removed, children in map as Pending, child dispatches queued, grandparent child count adjusted (+2 children, -1 parent = net +1).

14. **`finally_no_children_under_grandparent_triggers_grandparent_finally`** — Grandparent has finally hook, parent is its only child. FinallyRun with no children for parent. Assert: grandparent child count reaches 0, `PendingDispatch::Finally` queued for grandparent.

### Finally detection (walk_up_for_finally)

15. **`child_complete_triggers_parent_finally`** — Parent (with finally hook) has one child. Child completes as leaf. Assert: `PendingDispatch::Finally { parent_id }` queued.

16. **`child_complete_parent_no_finally_removes_parent`** — Parent (no finally hook) has one child. Child completes as leaf. Assert: parent removed from map, walks up to grandparent.

17. **`child_complete_skips_no_finally_ancestors`** — Grandparent (has finally), parent (no finally), child. Child completes. Assert: parent removed, `PendingDispatch::Finally` queued for grandparent.

18. **`child_complete_parent_still_has_siblings`** — Parent has 2 children. One child completes. Assert: parent count decremented to 1, no finally dispatch yet.

19. **`both_children_complete_then_finally`** — Parent (with finally) has 2 children. Complete both. Assert: after first, no finally; after second, `PendingDispatch::Finally` queued.

20. **`no_finally_at_any_level_just_removes`** — Three-level tree, no finally hooks anywhere. Leaf completes. Assert: leaf removed, parent removed, grandparent removed, no finally dispatch.

### Replay behavior

21. **`replay_completed_removes_stale_task_dispatch`** — Apply seed (queues dispatch), then immediately apply completed for same task. Assert: the seed's `PendingDispatch::Task` was removed by the completed entry.

22. **`replay_finally_removes_stale_finally_dispatch`** — Set up a parent whose child completed (queuing a finally dispatch), then apply FinallyRun. Assert: the `PendingDispatch::Finally` was removed.

### Complex scenarios

23. **`retry_under_parent_preserves_parent_waiting`** — Parent has child. Child fails with retry. Assert: parent still in WaitingForChildren (retry inherits parent_id, child count unchanged).

24. **`deeply_nested_finally_chain`** — Four levels: great-grandparent (finally) → grandparent (no finally) → parent (finally) → child. Child completes. Assert: parent's finally fires first (not great-grandparent's).

25. **`finally_spawns_children_that_complete`** — Parent (under grandparent) has finally. Finally runs, spawns 2 children. Both children complete. Assert: grandparent count adjustments are correct throughout, grandparent eventually reaches zero children.
