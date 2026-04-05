use super::AncestorCheck;
use super::frame::{Frame, FrameKind, HandleSide, HandleStatus, ParentRef};
use thunderdome::Arena;

/// Walks up the frame tree from a starting [`ParentRef`].
///
/// Yields `(ParentRef, &Frame)` for each ancestor. The [`ParentRef`] is
/// the edge from the child to this frame — the same value that was used to
/// look up the frame.
///
/// Iteration stops when:
/// - A frame's `parent` is `None` (reached the root). The root frame
///   itself IS yielded; iteration stops after it.
/// - A [`FrameId`] resolves to `None` in the arena (frame was removed).
///   The gone frame is NOT yielded.
pub struct Ancestors<'a> {
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

/// Walk ancestors starting from `parent_ref`.
pub const fn ancestors(frames: &Arena<Frame>, parent_ref: ParentRef) -> Ancestors<'_> {
    Ancestors {
        frames,
        next: Some(parent_ref),
    }
}

/// Check whether a `ParentRef`'s path to the root is blocked by a
/// suspended Handle, or whether the frame has been torn down.
pub fn find_blocking_ancestor(frames: &Arena<Frame>, parent_ref: ParentRef) -> AncestorCheck {
    if frames.get(parent_ref.frame_id()).is_none() {
        return AncestorCheck::FrameGone;
    }
    for (edge, frame) in ancestors(frames, parent_ref) {
        if is_blocked_by_handle(&edge, &frame.kind) {
            return AncestorCheck::Blocked;
        }
    }
    AncestorCheck::Clear
}

/// Does this parent edge cross from a body child into a suspended Handle?
const fn is_blocked_by_handle(parent_ref: &ParentRef, parent_kind: &FrameKind) -> bool {
    if let ParentRef::Handle {
        side: HandleSide::Body,
        ..
    } = parent_ref
        && let FrameKind::Handle(handle_frame) = parent_kind
    {
        return matches!(handle_frame.status, HandleStatus::Suspended(_));
    }
    false
}
