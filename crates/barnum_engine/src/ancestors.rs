use super::frame::{Frame, ParentRef};
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
