//! Mouse / pointer state for the run loop.
//!
//! Today this only retains the last cursor position: Aurea button events carry
//! their own coordinates, but the wheel handler still needs the last move
//! position to pick which pane to scroll/focus.
//!
//! It is the deliberate home for the forthcoming unified pointer model in
//! `docs/aurea-pointer-roadmap.md`. When Aurea ships `PointerEvent` + element
//! capture, the pressed-button set, the drag-selection anchor, the capture
//! target, and the active cursor shape all belong here — so press-drag-release
//! selection, edge autoscroll, and double/triple-click land without
//! re-plumbing the event loop. The run loop owns exactly one `MouseState`.

/// Run-loop pointer state. See the module docs for the planned growth.
#[derive(Default)]
pub(crate) struct MouseState {
    /// Last cursor position in window coordinates, or `None` before the first
    /// move event. Currently consumed only by wheel pane targeting.
    pos: Option<(f32, f32)>,
}

impl MouseState {
    /// Record a move. Window coordinates.
    pub(crate) fn moved(&mut self, x: f32, y: f32) {
        self.pos = Some((x, y));
    }

    /// The last known cursor position, if any.
    pub(crate) fn pos(&self) -> Option<(f32, f32)> {
        self.pos
    }
}
