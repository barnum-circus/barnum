//! Frame types for the engine's frame tree.

use barnum_ast::flat::ActionId;
use serde_json::Value;

/// Key into the engine's frame slab. Wraps the `usize` returned by
/// [`Slab::insert`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId(pub usize);

/// How a child frame refers to its parent.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Fields read by complete/error (completion milestone).
pub enum ParentRef {
    /// Parent has one active child (Chain, Loop, Attempt).
    SingleChild { frame_id: FrameId },
    /// Parent has N children; this child occupies `child_index` (Parallel,
    /// `ForEach`).
    IndexedChild {
        frame_id: FrameId,
        child_index: usize,
    },
}

impl ParentRef {
    /// Extract the parent's [`FrameId`] regardless of variant.
    #[allow(dead_code)] // Used by complete/error (completion milestone).
    pub const fn frame_id(self) -> FrameId {
        match self {
            ParentRef::SingleChild { frame_id } | ParentRef::IndexedChild { frame_id, .. } => {
                frame_id
            }
        }
    }
}

/// The kind-specific state stored in each frame.
#[derive(Debug)]
#[allow(dead_code)] // Fields read by complete/error (completion milestone).
pub enum FrameKind {
    /// Leaf: handler dispatched, waiting for result.
    Invoke,
    /// Sequential: first child active, then trampoline to `rest`.
    Chain { rest: ActionId },
    /// Fan-out: collecting results from N parallel branches.
    Parallel { results: Vec<Option<Value>> },
    /// Fan-out: collecting results from N array elements.
    ForEach { results: Vec<Option<Value>> },
    /// Fixed-point: re-enter body on Continue, complete on Break.
    Loop { body: ActionId },
    /// Error boundary: wraps child result in Ok/Err.
    Attempt,
}

/// A single frame in the engine's frame tree.
#[derive(Debug)]
#[allow(dead_code)] // Fields read by complete/error (completion milestone).
pub struct Frame {
    /// Parent reference. `None` for the top-level action.
    pub parent: Option<ParentRef>,
    /// Kind-specific state.
    pub kind: FrameKind,
}
