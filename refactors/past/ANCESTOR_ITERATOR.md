# Ancestor Frame Iterator

Pre-refactor: extract the frame tree walk-up pattern into a reusable iterator.

## Motivation

The engine has two methods that walk up the frame tree from a `ParentRef`:

1. **`find_blocking_ancestor`** (`lib.rs:329`): Walks up checking each edge for a suspended Handle body. Returns `FrameGone`, `Blocked`, or `Clear`.

2. **`find_and_dispatch_handler`** (`lib.rs:404`): Walks up looking for a `Handle` frame with a matching `effect_id`. Dispatches when found.

Both follow the same structural pattern:

```rust
let mut current = starting_parent;
loop {
    let frame = self.frames.get(current.frame_id())?;
    // ... check frame ...
    let Some(next) = frame.parent else { break; };
    current = next;
}
```

After the Resume/Restart split, the Handle-finding walk exists in two places: one for `ResumePerform` (looking for `ResumeHandle`) and one for `RestartPerform` (looking for `RestartHandle`). Extracting the iterator now means the split doesn't triple the loop boilerplate.

## Current code

### `find_blocking_ancestor` (`lib.rs:329`)

```rust
fn find_blocking_ancestor(&self, parent_ref: ParentRef) -> AncestorCheck {
    let mut current_ref = parent_ref;
    loop {
        let Some(frame) = self.frames.get(current_ref.frame_id()) else {
            return AncestorCheck::FrameGone;
        };
        if Self::is_blocked_by_handle(&current_ref, &frame.kind) {
            return AncestorCheck::Blocked;
        }
        let Some(next_ref) = frame.parent else {
            return AncestorCheck::Clear;
        };
        current_ref = next_ref;
    }
}
```

Needs: the `ParentRef` edge (to check `HandleSide::Body`) and the `Frame` (to check `HandleStatus::Suspended`).

### `find_and_dispatch_handler` (`lib.rs:404`)

```rust
fn find_and_dispatch_handler(
    &mut self,
    starting_parent: ParentRef,
    effect_id: EffectId,
    payload: Value,
) -> Result<StashOutcome, AdvanceError> {
    let mut current_frame_id = starting_parent.frame_id();
    loop {
        let frame = self.frames.get(current_frame_id)
            .expect("ancestor guaranteed present by find_blocking_ancestor");
        if let FrameKind::Handle(handle_frame) = &frame.kind
            && handle_frame.effect_id == effect_id
        {
            let perform_parent = starting_parent;
            self.dispatch_to_handler(current_frame_id, perform_parent, payload)?;
            return Ok(StashOutcome::Consumed);
        }
        let Some(next_parent) = frame.parent else {
            return Err(AdvanceError::UnhandledEffect { effect_id });
        };
        current_frame_id = next_parent.frame_id();
    }
}
```

Needs: the `FrameId` (to pass to `dispatch_to_handler`) and the `Frame` (to match `FrameKind::Handle`).

## Design

### Iterator type

```rust
/// Walks up the frame tree from a starting `ParentRef`.
///
/// Yields `(ParentRef, &Frame)` for each ancestor. The `ParentRef` is
/// the edge from the child to this frame — same value that was used to
/// look up the frame. `FrameId` is extractable via `parent_ref.frame_id()`.
///
/// Iteration stops when:
/// - A frame's `parent` is `None` (reached the root). The root frame
///   itself IS yielded; iteration stops after it.
/// - A `FrameId` resolves to `None` in the arena (frame was removed).
///   The gone frame is NOT yielded. Callers that need to distinguish
///   "reached root" from "frame gone" can check whether the last
///   yielded frame had `parent: None`.
struct Ancestors<'a> {
    frames: &'a Arena<Frame>,
    next: Option<ParentRef>,
}

impl<'a> Iterator for Ancestors<'a> {
    type Item = (ParentRef, &'a Frame);

    fn next(&mut self) -> Option<Self::Item> {
        let parent_ref = self.next.take()?;
        let frame = self.frames.get(parent_ref.frame_id())?;
        self.next = frame.parent;
        Some((parent_ref, frame))
    }
}
```

Key decisions:

- **Yields `(ParentRef, &Frame)`**, not `(FrameId, &Frame)`. `find_blocking_ancestor` needs the `ParentRef` to check the edge variant (`HandleSide::Body`). `find_and_dispatch_handler` extracts `FrameId` via `.frame_id()`.
- **Stops silently on gone frames.** When a `FrameId` doesn't resolve, the iterator returns `None`. This matches `find_blocking_ancestor`'s `FrameGone` path — the caller sees iteration ended without reaching root.
- **Borrows `&Arena<Frame>`, not `&self`.** The iterator only needs the frame arena, not the full `WorkflowState`. This is important: `find_and_dispatch_handler` needs `&mut self` after the walk to call `dispatch_to_handler`. If the iterator borrowed `&self`, the mutable call would conflict. Borrowing only `&self.frames` lets the caller hold the iterator (or collect results) while still having mutable access to other fields.

### Constructor

```rust
impl WorkflowState {
    /// Walk ancestors starting from `parent_ref`.
    fn ancestors(&self, parent_ref: ParentRef) -> Ancestors<'_> {
        Ancestors {
            frames: &self.frames,
            next: Some(parent_ref),
        }
    }
}
```

Alternatively, `ancestors` could be a free function or a method on the arena wrapper. Keeping it on `WorkflowState` is simplest.

## After: `find_blocking_ancestor`

```rust
fn find_blocking_ancestor(&self, parent_ref: ParentRef) -> AncestorCheck {
    for (edge, frame) in self.ancestors(parent_ref) {
        if Self::is_blocked_by_handle(&edge, &frame.kind) {
            return AncestorCheck::Blocked;
        }
    }
    // Iterator exhausted. Did we reach root or hit a gone frame?
    // If the starting parent_ref's frame is gone, that's FrameGone.
    // If we iterated to the root, that's Clear.
    //
    // Distinguishing these: if ancestors() yields nothing (first
    // frame_id is gone), it's FrameGone. If it yields at least one
    // frame, the last frame has parent: None (root), so it's Clear.
    //
    // We need to track whether we yielded anything:
    let mut found_any = false;
    for (edge, frame) in self.ancestors(parent_ref) {
        found_any = true;
        if Self::is_blocked_by_handle(&edge, &frame.kind) {
            return AncestorCheck::Blocked;
        }
    }
    if found_any {
        AncestorCheck::Clear
    } else {
        AncestorCheck::FrameGone
    }
}
```

Hmm — the "did we yield anything" check is ugly. Two options:

The iterator stops on gone frames (returns `None`). `find_blocking_ancestor` needs to detect FrameGone vs root, so it does a preliminary `self.frames.get(parent_ref.frame_id())` check before iterating:

```rust
fn find_blocking_ancestor(&self, parent_ref: ParentRef) -> AncestorCheck {
    // Quick check: is the immediate parent even alive?
    if self.frames.get(parent_ref.frame_id()).is_none() {
        return AncestorCheck::FrameGone;
    }
    for (edge, frame) in self.ancestors(parent_ref) {
        if Self::is_blocked_by_handle(&edge, &frame.kind) {
            return AncestorCheck::Blocked;
        }
    }
    AncestorCheck::Clear
}
```

This works because:
- If the first frame is gone → `FrameGone` before iteration starts.
- If a later frame is gone → impossible. If the first frame is present but its parent is gone, that means a parent was removed while its child still exists. The engine maintains the invariant that removing a frame removes all descendants first (teardown is top-down). So if a frame is present, its entire ancestor chain up to root is present.
- Therefore the only "gone" case is the very first frame. After that, the iterator is guaranteed to reach root.

**Future: single-pass walk.** `find_blocking_ancestor` and `find_and_dispatch_handler` could be combined into one iteration that checks for blocking AND finds the matching Handle in a single walk. At that point the iterator would return `Result` items so the combined pass can handle FrameGone. For now, two separate iterations is fine — the second walk can unwrap freely since `find_blocking_ancestor` already confirmed all frames are present.

## After: `find_and_dispatch_handler`

```rust
fn find_and_dispatch_handler(
    &mut self,
    starting_parent: ParentRef,
    effect_id: EffectId,
    payload: Value,
) -> Result<StashOutcome, AdvanceError> {
    // Walk ancestors to find matching Handle. Collect the FrameId
    // so we can release the immutable borrow before dispatching.
    let handle_frame_id = self
        .ancestors(starting_parent)
        .find_map(|(edge, frame)| {
            if let FrameKind::Handle(handle) = &frame.kind
                && handle.effect_id == effect_id
            {
                Some(edge.frame_id())
            } else {
                None
            }
        })
        .ok_or(AdvanceError::UnhandledEffect { effect_id })?;

    self.dispatch_to_handler(handle_frame_id, starting_parent, payload)?;
    Ok(StashOutcome::Consumed)
}
```

The iterator borrows `&self.frames`. Once `find_map` returns, the borrow is released. `dispatch_to_handler` can then take `&mut self`.

## Borrow checker consideration

The iterator borrows `&self.frames`. As long as the iterator is dropped before any `&mut self` call, there's no conflict. Both call sites above achieve this:

- `find_blocking_ancestor`: iterator consumed in `for` loop, no mutation needed.
- `find_and_dispatch_handler`: `find_map` consumes the iterator and returns a `FrameId` (a `Copy` type). The iterator is dropped. Then `dispatch_to_handler` takes `&mut self`.

If we ever need to hold the iterator while mutating, we'd need to restructure (e.g., collect FrameIds first). But both current and future call sites (ResumePerform, RestartPerform) follow the "find then act" pattern, so this isn't a concern.

## Tests

No new tests needed. The iterator is a pure refactor of existing logic. Existing tests cover both `find_blocking_ancestor` and `find_and_dispatch_handler` paths:

- `handle_routes_to_correct_handler` — verifies effect routing
- `nested_handle_routes_inner_first` — verifies walk-up finds innermost match
- `stash_dropped_after_discard` (now `teardown_cleans_up_concurrent_tasks`) — exercises `find_blocking_ancestor` Blocked path
- `throw_proceeds_while_resume_handler_in_flight` — exercises `find_blocking_ancestor` with nested Handles

## Sequencing

1. Add `Ancestors` struct and `ancestors()` method.
2. Rewrite `find_blocking_ancestor` with the one-line FrameGone guard + iterator.
3. Rewrite `find_and_dispatch_handler` with `find_map`.
4. `cargo test` — all engine tests pass unchanged.
