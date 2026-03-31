# Stash Keyed by Blocking Frame

## Problem

The current stash is a `VecDeque<StashedItem>`. After each `complete()`, the engine sweeps the entire stash repeatedly until no progress is made (fixed point). This is O(stash_size) per sweep, and most items won't make progress because they're blocked by a Handle that's still suspended.

The engine already knows which Handle frame caused the stash — `bubble_effect` and `try_deliver` detect the blocking suspended Handle during their upward walk. But this information is discarded. The stashed item goes into a flat queue with no record of what blocked it.

## Design

Replace the flat `VecDeque` with a `HashMap<FrameId, Vec<StashedItem>>` keyed by the blocking Handle's FrameId.

```rust
pub struct WorkflowState {
    // ...
    stashed_items: HashMap<FrameId, Vec<StashedItem>>,
    // ...
}
```

### Stashing

When `bubble_effect` or `try_deliver` encounters a suspended Handle, it records the blocking frame:

```rust
// Current:
self.stashed_items.push_back(StashedItem::Effect { starting_parent, effect_id, payload });

// New:
self.stashed_items
    .entry(blocking_frame_id)
    .or_default()
    .push(StashedItem::Effect { starting_parent, effect_id, payload });
```

The blocking frame is the suspended Handle that caused the stash. The engine already identifies this frame during the upward walk — it's the Handle with `status == Suspended` that the walk hits.

### Draining

When a Handle frame unsuspends (handler completes, `status` goes from `Suspended` to `Free`), the engine drains only the items keyed by that frame:

```rust
fn on_handle_unsuspend(&mut self, frame_id: FrameId) {
    if let Some(items) = self.stashed_items.remove(&frame_id) {
        for item in items {
            match item {
                StashedItem::Delivery { parent_ref, value } => {
                    self.try_deliver(parent_ref, value);
                }
                StashedItem::Effect { starting_parent, effect_id, payload } => {
                    self.bubble_effect(starting_parent, effect_id, payload);
                }
            }
        }
    }
}
```

This replaces the sweep loop. Instead of scanning the entire stash looking for items that might make progress, we directly drain the items that were blocked by the frame that just unsuspended.

### When unsuspend happens

The Handle unsuspends in two places:

1. **Resume**: handler completes with Resume. Handle status → Free. Drain stash for this frame.
2. **Discard/RestartBody**: handler completes with Discard or RestartBody. The Handle frame is torn down or restarted. Items stashed against this frame are either:
   - **Discard**: the body is torn down. Stashed items targeting body-side frames are now stale (their parent frames are gone). These items should be discarded, not retried.
   - **RestartBody**: the body is torn down and re-entered. Same as Discard — stashed items targeting old body frames are stale.

So: on Resume, drain and retry. On Discard/RestartBody, drain and discard (the items' parent frames no longer exist).

### Frame teardown cascading

When a Handle frame is torn down (via Discard from an outer handler, or workflow completion), items stashed against it should be discarded. The teardown code should `self.stashed_items.remove(&frame_id)` for the torn-down frame.

More subtly: if frame A is torn down, and items stashed against A reference frames that are descendants of A, those items are already invalid (their parent frames are gone). Removing the stash entry for A handles this correctly — the items are dropped.

### Nested blocking

An item might be blocked by multiple suspended Handles (if the upward walk hits two suspended Handles). In this case, which frame do we key by?

Key by the **first** (innermost) blocking Handle encountered during the walk. When that Handle unsuspends and the item is retried, the retry walk may hit the next suspended Handle — and the item gets re-stashed against that frame. This is correct: each retry peels off one layer of blocking.

### Re-stashing on retry

When a stashed item is retried (drained from the map and re-delivered/re-bubbled), the retry may encounter another suspended Handle. In that case, the item gets stashed again under the new blocking frame. This is the same behavior as the current sweep loop, but targeted: we only retry items that have a chance of progressing.

## Complexity

**Current**: O(stash_size × sweep_iterations) per `complete()`. Sweep iterates until fixed point. In the worst case (deeply nested handlers), this is O(n^2).

**Proposed**: O(items_blocked_by_this_frame) per unsuspend. Each unsuspend drains only its own items. Items that are blocked by other frames are untouched. Total work across all unsuspends is O(total_stashed_items) — each item is processed once per unsuspend of its blocking frame.

## Implementation

1. Change `stashed_items` from `VecDeque<StashedItem>` to `HashMap<FrameId, Vec<StashedItem>>`
2. Update stashing code to record the blocking FrameId
3. Add `on_handle_unsuspend(frame_id)` that drains the keyed items
4. On Discard/RestartBody/teardown, remove the stash entry without retrying
5. Remove the sweep loop (the `loop { match self.sweep_stash() { ... } }` fixed-point loop)
6. Call `on_handle_unsuspend` after Resume in `complete()`

## Files to change

| File | Change |
|------|--------|
| `barnum_engine/src/lib.rs` | Replace `VecDeque<StashedItem>` with `HashMap<FrameId, Vec<StashedItem>>`. Update stash/drain logic. Remove sweep loop. |
| `barnum_engine/src/frame.rs` | No changes — FrameId already exists. |
