//! A cross, pressed to close or dismiss.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::StatefulWidget;

use crate::events::Events;

/// The cross, the same glyph the compositor draws on a window's close button.
const CROSS: char = '✕';

/// What a hovered cross turns.
const HOT: Color = Color::Red;

/// How many cells wide the cross is.
///
/// Three: the glyph alone is a target a pointer has to hit exactly, and a pressed cross is meant to
/// read as a block with the glyphs faded out of it, which needs the cells either side.
pub const WIDTH: u16 = 3;

/// A cross that reports being pressed and being let go.
///
/// Both callbacks are the caller's to use: a cross that acts on the press closes on the way down,
/// and one that acts on the release wants the press and the let-go to be one gesture. Handing over
/// both lets the caller decide, rather than this widget picking one and being wrong half the time.
///
/// Hover tints the glyphs red. A press fills the whole box red and draws the cross in black on it,
/// so the button reads as a solid block while it is down.
pub struct Cross {
    on_press: Box<dyn FnMut()>,
    on_release: Box<dyn FnMut()>,
    /// Whether the cross is being held down, which outlives the frame the press arrived on: a
    /// press and the release that ends it are two events, usually two frames.
    held: bool,
}

impl Cross {
    pub fn new(on_press: impl FnMut() + 'static, on_release: impl FnMut() + 'static) -> Self {
        Self {
            on_press: Box::new(on_press),
            on_release: Box::new(on_release),
            held: false,
        }
    }
}

impl StatefulWidget for &mut Cross {
    type State = Events;

    fn render(self, area: Rect, buf: &mut Buffer, events: &mut Self::State) {
        let hovered = events.hovering(area);
        // Only a held cross takes the release. A release is one event for the whole pane and every
        // widget is offered the same `Events`: a widget that took it without being held would eat
        // the release of whatever was really pressed, and the widget that was held would stay held
        // forever.
        if self.held && events.take_release().is_some() {
            self.held = false;
            (self.on_release)();
        }
        if events.take_press(area).is_some() {
            self.held = true;
            (self.on_press)();
        }

        // Held is the strongest state and wins over hover: a red block with the cross cut out of
        // it in black. The cells either side of the glyph are the same block, so the target reads
        // as three wide while it is down.
        let style = match (self.held, hovered) {
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
