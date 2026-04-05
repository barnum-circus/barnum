# Frame Parent Cleanup

## Motivation

Every `Frame` in the arena carries `parent: Option<ParentRef>`. Exactly one frame (the root) ever has `None`. Every other frame has `Some(parent_ref)`. The `Option` is load-bearing in only a few places:

1. `deliver` (`lib.rs:730`) — `None` means the workflow is done.
2. `complete` (`lib.rs:333`) — `None` on the Invoke frame means workflow result.
3. `advance` (`lib.rs:814`) — `None` means this is the initial call (root frame).
4. `FlatAction::Perform` (`lib.rs:950`) — explicitly rejects `None` with `.ok_or(...)`.

The Option spreads to every site that reads `frame.parent`: ancestor iteration, teardown, Chain trampoline (reads `frame.parent` to pass to the next advance), All/ForEach collection (reads `frame.parent` to deliver the collected array).

Some frame kinds structurally require a parent. Perform (which currently doesn't even create a frame) errors on a missing parent. After the RESUME_VS_RESTART_HANDLERS refactor, ResumePerform will be a frame kind that always has a parent. Invoke is the workflow terminal — it can be the root, but that's the degenerate case of a single-handler workflow.

The `Option<ParentRef>` on every frame is correct but imprecise. It represents "exactly one frame lacks a parent" as "any frame might lack a parent."

## Current state

```rust
// frame.rs:124-131
pub struct Frame {
    pub parent: Option<ParentRef>,
    pub kind: FrameKind,
}
```

```rust
// lib.rs:814
pub fn advance(
    &mut self,
    action_id: ActionId,
    value: Value,
    parent: Option<ParentRef>,
) -> Result<(), AdvanceError> { ... }
```

`deliver` takes `Option<ParentRef>` and returns `Ok(Some(value))` when it's `None`:

```rust
// lib.rs:730-737
fn deliver(
    &mut self,
    parent: Option<ParentRef>,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let Some(parent_ref) = parent else {
        return Ok(Some(value));
    };
    // ... match on parent_ref variants
}
```

Chain trampoline reads `frame.parent` and passes it through:

```rust
// lib.rs:740-749
ParentRef::Chain { frame_id } => {
    let frame = self.frames.remove(frame_id).expect("parent frame exists");
    let FrameKind::Chain { rest } = frame.kind else { unreachable!(...) };
    self.advance(rest, value, frame.parent)?;  // propagates Option
    Ok(None)
}
```

## Options

### Option A: `ParentRef::WorkflowRoot` variant

Add a variant to `ParentRef` representing "no parent — deliver here completes the workflow." Replace `Option<ParentRef>` with `ParentRef` everywhere.

```rust
pub enum ParentRef {
    WorkflowRoot,
    Chain { frame_id: FrameId },
    All { frame_id: FrameId, child_index: usize },
    ForEach { frame_id: FrameId, child_index: usize },
    Handle { frame_id: FrameId, side: HandleSide },
}

pub struct Frame {
    pub parent: ParentRef,  // non-optional
    pub kind: FrameKind,
}
```

`deliver` becomes:

```rust
fn deliver(
    &mut self,
    parent_ref: ParentRef,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    match parent_ref {
        ParentRef::WorkflowRoot => Ok(Some(value)),
        ParentRef::Chain { frame_id } => { ... }
        // ...
    }
}
```

`advance` takes `ParentRef` instead of `Option<ParentRef>`. The public entry point passes `ParentRef::WorkflowRoot`.

`ParentRef::frame_id()` either returns `Option<FrameId>` (WorkflowRoot has no frame ID) or panics on WorkflowRoot (caller's responsibility to not call it on the root variant). The Option return is cleaner:

```rust
impl ParentRef {
    pub const fn frame_id(self) -> Option<FrameId> {
        match self {
            Self::WorkflowRoot => None,
            Self::Chain { frame_id }
            | Self::All { frame_id, .. }
            | Self::ForEach { frame_id, .. }
            | Self::Handle { frame_id, .. } => Some(frame_id),
        }
    }
}
```

**Tradeoff:** This is structurally isomorphic to the current `Option<ParentRef>`. The `Option` moves from `Frame.parent` to `ParentRef::frame_id()`. The win is that `ParentRef` now handles the root case explicitly in match arms (the `deliver` match becomes exhaustive over ParentRef variants instead of splitting into `let Some(parent_ref) = parent else { return }` + match). The loss is that `frame_id()` now returns `Option` where it was previously infallible.

### Option B: Arena stores a `FrameSlot` enum

Make root vs. child a first-class distinction in the arena entry type:

```rust
enum FrameSlot {
    Root { kind: FrameKind },
    Child { parent: ParentRef, kind: FrameKind },
}
```

`parent` is not a separate field on every frame — it exists only on `Child` entries. The root has no parent by construction.

Accessing `kind` requires a method or match:

```rust
impl FrameSlot {
    fn kind(&self) -> &FrameKind {
        match self {
            Self::Root { kind } | Self::Child { kind, .. } => kind,
        }
    }

    fn parent(&self) -> Option<&ParentRef> {
        match self {
            Self::Root { .. } => None,
            Self::Child { parent, .. } => Some(parent),
        }
    }
}
```

**Tradeoff:** This matches the user's original intuition ("parent is not a separate field — you get a frame that has either parent or a value"). The root/child distinction is encoded in the arena entry itself. But every access to `kind` goes through a method or match, which is worse ergonomics than the current `frame.kind` direct access. And `parent()` still returns `Option` — the Option hasn't been eliminated, it's moved from the struct field to the accessor.

### Option C: Split `advance` into public entry point + internal method

Don't change the Frame struct. Instead, split the advance API:

```rust
/// Public: start the workflow. Root frame gets parent: None.
pub fn start(
    &mut self,
    action_id: ActionId,
    value: Value,
) -> Result<(), AdvanceError> {
    self.advance_inner(action_id, value, None)
}

/// Internal: always has a parent.
fn advance_child(
    &mut self,
    action_id: ActionId,
    value: Value,
    parent: ParentRef,
) -> Result<(), AdvanceError> {
    self.advance_inner(action_id, value, Some(parent))
}
```

All internal call sites (Chain trampoline, All/ForEach child creation, Handle body/handler) call `advance_child` with a non-optional `ParentRef`. Only the external caller uses `start`.

**Tradeoff:** The Frame struct stays the same. The Option is still there. But internal code never constructs `Some(parent)` — it passes `ParentRef` directly. The Perform arm doesn't need `.ok_or()` because `advance_child` guarantees a parent. The downside: Chain trampoline reads `frame.parent: Option<ParentRef>` and needs to decide whether to call `advance_child` or `advance_inner`. If the chain was the root frame, its parent is `None`, and the trampoline needs the `None` path. So the trampoline can't use `advance_child` — it must propagate the Option. This means the split doesn't fully help; the Option still flows through trampoline.

## Assessment

None of these options produces a dramatic simplification. The fundamental issue is that the root frame exists in the same arena as child frames, and the single `None` parent propagates through Chain trampoline and `deliver`. The Option is structurally load-bearing.

Option A is the most practical. It replaces `Option<ParentRef>` with a richer enum, making `deliver`'s match arms exhaustive and self-documenting. The cost is `frame_id()` returning Option. If `frame_id()` callers always know they're not dealing with WorkflowRoot (which is true everywhere except `deliver` and `complete`), the Option is a minor tax.

Option C is a small API improvement that can land independently of A or B. Splitting advance into start/advance_child is a readability win even if the internal Option stays.

Option B is the most structurally faithful to "parent is not a separate field" but the ergonomic cost of accessing `kind` through a method on every frame read is significant. The engine reads `frame.kind` constantly.

## Interaction with RESUME_VS_RESTART_HANDLERS

The pending refactor adds `ResumePerform` as a frame kind. ResumePerform always has a parent — it's created at the Perform site, runs a handler as its child, and delivers to `perform_parent`. Under Option A, ResumePerform frames would have a non-Root `ParentRef`, which is correct by construction. Under the current design, they have `Some(parent_ref)`, same thing.

The refactor also adds `ParentRef::ResumePerform { frame_id }` — a new variant that always carries a `frame_id`. This variant would never be `WorkflowRoot` under Option A, which is consistent.

No blocking relationship. This cleanup can land before or after RESUME_VS_RESTART_HANDLERS.

## Open question

Is there a design where the root frame is not in the arena at all? If the root were stored separately (e.g., `root: Option<FrameKind>` on WorkflowState), all arena frames would have non-optional parents. But children of the root frame point up to it via `FrameId`, which requires the root to be in the arena. Separating the root would require a `ParentTarget` enum (`Root` vs `Frame(FrameId)`) inside every `ParentRef` variant, which proliferates complexity rather than reducing it.
