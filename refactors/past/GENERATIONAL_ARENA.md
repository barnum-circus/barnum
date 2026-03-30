# Generational Arena for Frame Store

## Motivation

The frame store (`WorkflowState.frames`) is currently a `slab::Slab<Frame>`. The slab reuses slot indices after removal. This creates an ABA problem: a stashed item holds a `ParentRef` containing a `FrameId`. If `teardown_body` removes the target frame and the slot is later reused by a new frame, the stashed `FrameId` silently matches the wrong frame. Delivering to the wrong frame is a correctness bug.

This is a prerequisite for the effects substrate (Handle/Perform), which introduces the stash mechanism. Without generational indices, the stash cannot safely reference frames that may have been torn down and replaced.

## Blocks

Effects Phase 1 Substrate (`EFFECTS_PHASE_1_SUBSTRATE.md`).

## Current state

**File:** `crates/barnum_engine/src/lib.rs`

```rust
pub struct WorkflowState {
    frames: slab::Slab<Frame>,
    // ...
}
```

FrameId is a newtype around `usize` (the slab's index type):

**File:** `crates/barnum_engine/src/frame.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId(pub usize);
```

All frame access uses raw indexing:

```rust
self.frames.get(frame_id.0)       // Option<&Frame>
self.frames.get_mut(frame_id.0)   // Option<&mut Frame>
self.frames[frame_id.0]           // panicking index
self.frames.remove(frame_id.0)    // remove and return
self.frames.insert(frame)         // returns usize
```

ParentRef stores FrameId:

```rust
pub enum ParentRef {
    SingleChild { frame_id: FrameId },
    IndexedChild { frame_id: FrameId, child_index: usize },
}
```

`task_to_parent` maps TaskId to `Option<ParentRef>`:

```rust
task_to_parent: HashMap<TaskId, Option<ParentRef>>,
```

## Proposed change

Replace `slab::Slab<Frame>` with a generational arena. FrameId becomes the arena's index type, which carries both a slot index and a generation counter. When a frame is removed, the slot's generation is bumped. If the slot is reused, any old FrameId referencing it has a stale generation — `.get(stale_id)` returns None.

### Crate choice

`thunderdome` is a good fit:
- `Arena<T>` with `Index` type that carries generation
- `.get(index)` returns None for stale generations
- `.insert(value)` returns the generational Index
- `.remove(index)` returns Option<T> (None if stale)
- `.iter()` yields `(Index, &T)` pairs
- No unsafe, well-maintained, minimal dependencies

`generational-arena` and `slotmap` are alternatives with similar APIs.

### FrameId

FrameId becomes an alias or newtype for the arena's index:

```rust
// Option A: type alias
pub type FrameId = thunderdome::Index;

// Option B: newtype (if we want to keep derives or add methods)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId(pub thunderdome::Index);
```

Option A is simpler. Option B preserves the existing newtype pattern. Either works — the key property is that FrameId is opaque and carries a generation.

### Frame access

All raw `.0` indexing becomes direct generational access:

```rust
// Before
self.frames.get(frame_id.0)
self.frames[frame_id.0]
self.frames.remove(frame_id.0)
let raw_id = self.frames.insert(frame);
let frame_id = FrameId(raw_id);

// After
self.frames.get(frame_id)
self.frames[frame_id]    // thunderdome supports Index for panicking access
self.frames.remove(frame_id)
let frame_id = self.frames.insert(frame);
```

### insert_frame

Currently returns FrameId by wrapping the slab's usize:

```rust
fn insert_frame(&mut self, frame: Frame) -> FrameId {
    FrameId(self.frames.insert(frame))
}
```

With a generational arena, the arena's insert returns the generational index directly:

```rust
fn insert_frame(&mut self, frame: Frame) -> FrameId {
    self.frames.insert(frame)  // returns thunderdome::Index
}
```

### Iteration

`slab::Slab::iter()` yields `(usize, &T)`. `thunderdome::Arena::iter()` yields `(Index, &T)`. The change is mechanical — replace `FrameId(id)` with the yielded Index directly.

### ParentRef, task_to_parent

No API changes. ParentRef stores FrameId, which now carries a generation. `task_to_parent` maps to `Option<ParentRef>`. All existing code that checks `self.frames.get(parent_ref.frame_id())` gets generational safety for free.

## What doesn't change

- Frame struct and FrameKind enum — unchanged
- ParentRef enum — unchanged (FrameId is still FrameId, just generational now)
- Advance logic, deliver logic, complete_task — unchanged beyond the mechanical `.0` removal
- Flat table, flattener, AST — completely unaffected
- TypeScript layer — completely unaffected

## Test strategy

All existing tests pass unchanged. The generational arena is a drop-in replacement for the slab with the same API shape. No new behavior to test — the generation checking is internal to the arena.

One new test worth adding: a regression test that creates a frame, removes it, creates another frame (which may reuse the slot), and verifies that the old FrameId returns None from `.get()`. This confirms the arena rejects stale references.

## Deliverables

1. Add `thunderdome` (or chosen arena crate) to `Cargo.toml`
2. Change FrameId to use the arena's index type
3. Replace `slab::Slab<Frame>` with `Arena<Frame>` in WorkflowState
4. Update all frame access to use generational FrameId (remove `.0` indexing)
5. Remove `slab` dependency
6. Regression test for stale FrameId
