//! The input a widget is handed for one frame: the keys it may act on, and the click that
//! landed somewhere on the pane.
//!
//! The caller decides which widget gets what. A widget takes only the parts it acts on and
//! leaves the rest, so one `Events` can be offered to several widgets in turn and whatever
//! is left over afterwards is what nobody wanted.

use ratatui::layout::{Position, Rect};
use termina::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

/// One frame's input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Events {
    /// Every key pressed since the last frame, in the order they arrived.
    pub keypresses: Vec<KeyEvent>,
    /// The pointer's last event, if there was one.
    pub mouse_hits: Option<MouseEvent>,
}

impl Events {
    /// Whether the pointer is over `area`.
    ///
    /// Read rather than taken: pointing at a widget is not the widget's to consume, and a
    /// button that swallowed the motion would not know it was under the pointer next frame.
    pub fn hovering(&self, area: Rect) -> bool {
        self.mouse_hits
            .is_some_and(|mouse| area.contains(Position::new(mouse.column, mouse.row)))
    }

    /// Take the left click inside `area`, if this frame's event is one.
    ///
    /// Taking it is what stops the click reaching a widget further down: the widget that
    /// was pressed is the one that acted.
    pub fn take_press(&mut self, area: Rect) -> Option<Position> {
        let mouse = self.mouse_hits?;
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return None;
        }
        let at = Position::new(mouse.column, mouse.row);
        if !area.contains(at) {
            return None;
        }
        self.mouse_hits = None;
        Some(at)
    }

    /// Take the first keypress `want` accepts, leaving the others for whoever looks next.
    pub fn take_key(&mut self, want: impl Fn(&KeyEvent) -> bool) -> Option<KeyEvent> {
        let at = self.keypresses.iter().position(want)?;
        Some(self.keypresses.remove(at))
    }
}
