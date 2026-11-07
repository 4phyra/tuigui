//! A cross, pressed to close or dismiss.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;
use termina::event::Event;

use crate::events::{pointer_at, take_press, take_release};

/// The cross. Heavy rather than the thin `✕`, which sits on the cap height and reads as though it
/// were printed on the line above.
const CROSS: char = '✖';

/// What a hovered cross turns.
const HOT: Color = Color::Red;

/// How many cells wide a cross is.
///
/// Three: the glyph alone is a target a pointer has to hit exactly, and a pressed cross is a block
/// with the glyph in it, which needs the cells either side.
pub const WIDTH: u16 = 3;

/// A cross that answers a pointer.
///
/// Hover tints the glyph red. A press fills the box red and draws the cross in black on it, so the
/// button reads as a solid block while it is down, and the release puts it back.
///
/// What a press *means* is the caller's, but whether one happened is not: the two edges are
/// recorded here and answered once each, by [`was_pressed`](Self::was_pressed) and
/// [`was_released`](Self::was_released). Without that, every caller that wanted to act on a click
/// would keep the previous frame's `is_held` itself and detect the edge — the same small machine,
/// written again in each of them.
#[derive(Default)]
pub struct Cross {
    /// Whether the cross is being held down, which outlives the frame the press arrived on: a
    /// press and the release that ends it are two events, usually two frames.
    held: bool,
    /// Whether the pointer was over it in the events last taken.
    ///
    /// Kept rather than read in the draw, so the drawing takes no input: a window's close button is
    /// a cross too, and it is driven by one mouse event at a time rather than by a frame's
    /// [`Events`]. Both take events the same way and draw from the same fields.
    hovered: bool,
    /// A press taken since the caller last asked.
    pressed: bool,
    /// A release inside the cross since the caller last asked.
    released: bool,
}

impl Cross {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the cross is being held down right now.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Whether it was pressed since the caller last asked. Answered once.
    pub fn was_pressed(&mut self) -> bool {
        std::mem::take(&mut self.pressed)
    }

    /// Whether it was pressed and let go inside the box since the caller last asked.
    ///
    /// A click is a press and the release that ends it in the same box: a press dragged off and
    /// let go elsewhere is a press the user changed their mind about. Answered once.
    pub fn was_released(&mut self) -> bool {
        std::mem::take(&mut self.released)
    }

    /// Take this frame's input for a cross drawn in `area`.
    pub fn handle_events(&mut self, area: Rect, events: &mut Vec<Event>) {
        // A frame with no pointer event leaves the last answer alone: the pointer did not move, it
        // did not leave.
        if let Some(at) = pointer_at(events) {
            self.hovered = area.contains(at);
        }
        // Only a held cross takes the release. A release is one event for the whole pane and every
        // widget is offered the same list: a widget that took it without being held would eat the
        // release of whatever was really pressed, and the widget that was held would stay held
        // forever.
        if self.held && take_release(events).is_some() {
            self.held = false;
            self.released = self.hovered;
        }
        if take_press(events, area).is_some() {
            self.held = true;
            self.pressed = true;
        }
    }
}

impl Widget for &Cross {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Held is the strongest state and wins over hover: a red block with the cross cut out of
        // it in black. The cells either side of the glyph are the same block, so the target reads
        // as three wide while it is down.
        let style = match (self.held, self.hovered) {
            (true, _) => Style::new().fg(Color::Black).bg(HOT),
            (false, true) => Style::new().fg(HOT),
            (false, false) => Style::new().fg(Color::Gray),
        };
        // One glyph in the middle of the box. The cells either side are painted but left blank:
        // they are what a held cross floods with red, so the block is three wide while the cross
        // is one.
        let middle = area.x.saturating_add(area.width / 2);
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                match x == middle {
                    true => cell.set_char(CROSS).set_style(style),
                    false => cell.set_char(' ').set_style(style),
                };
            }
        }
    }
}
