//! Frame types for the engine's frame tree.

use barnum_ast::flat::ActionId;
use serde_json::Value;

/// Key into the engine's frame arena. Carries a generational index —
/// accessing a removed-and-reused slot with a stale `FrameId` returns `None`.
pub type FrameId = thunderdome::Index;

/// How a child frame refers to its parent.
///
/// One variant per frame kind — the variant determines the code path in
/// `deliver`, eliminating nested `FrameKind` dispatch.
#[derive(Debug, Clone, Copy)]
pub enum ParentRef {
    /// Parent is a Chain frame — sequential, single child.
    Chain {
        /// The parent frame's ID.
        frame_id: FrameId,
    },
    /// Parent is a Loop frame — single child, Continue/Break dispatch.
    Loop {
        /// The parent frame's ID.
        frame_id: FrameId,
    },
    /// Parent is an All frame — indexed child in fan-out.
    All {
        /// The parent frame's ID.
        frame_id: FrameId,
        /// This child's index in the parent's results vector.
        child_index: usize,
    },
    /// Parent is a `ForEach` frame — indexed child per array element.
    ForEach {
        /// The parent frame's ID.
        frame_id: FrameId,
        /// This child's index in the parent's results vector.
        child_index: usize,
    },
}

impl ParentRef {
    /// Extract the parent's [`FrameId`] regardless of variant.
    #[must_use]
    pub const fn frame_id(self) -> FrameId {
        match self {
            Self::Chain { frame_id }
            | Self::Loop { frame_id }
            | Self::All { frame_id, .. }
            | Self::ForEach { frame_id, .. } => frame_id,
        }
    }
}

/// The kind-specific state stored in each frame.
///
/// Only structural combinators have frames. Invoke actions are leaf
/// dispatches — they don't create frames.
#[derive(Debug)]
pub enum FrameKind {
    /// Sequential: first child active, then trampoline to `rest`.
    Chain {
        /// The remaining action to advance after the first child completes.
        rest: ActionId,
    },
    /// Fan-out: collecting results from N parallel branches.
    All {
        /// Slot per child; `None` until the child completes.
        results: Vec<Option<Value>>,
    },
    /// Fan-out: collecting results from N array elements.
    ForEach {
        /// Slot per element; `None` until the child completes.
        results: Vec<Option<Value>>,
    },
    /// Fixed-point: re-enter body on Continue, complete on Break.
    Loop {
        /// The body action to re-enter on each iteration.
        body: ActionId,
    },
}

/// A single frame in the engine's frame tree.
#[derive(Debug)]
pub struct Frame {
    /// Parent reference. `None` for the top-level action.
    pub parent: Option<ParentRef>,
    /// Kind-specific state.
    pub kind: FrameKind,
}
