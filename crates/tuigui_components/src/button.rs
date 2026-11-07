//! A labelled box that is pressed once.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;

use crate::events::Events;

/// The colour a button changes into when the pointer is on it, or it was just pressed.
const HOT: Color = Color::Rgb(0xff, 0x69, 0xb4);

/// How many columns of padding sit on each side of the label.
const PADDING: u16 = 1;

/// A box with a label in it, which the pointer presses.
///
/// Drawn with the frame's [`Events`], so the box it is drawn in is the box it is pressed in.
/// No state of its own beyond that answer: whether it is hot is read from the pointer every
/// frame, and whether it was pressed is this frame's event and nothing else.
pub struct Button<'a> {
    label: Line<'a>,
    /// Whether the draw that just ran took a press. Read by the caller after the frame.
    activated: bool,
}

impl<'a> Button<'a> {
    pub fn new(label: impl Into<Line<'a>>) -> Self {
        Self {
            label: label.into(),
            activated: false,
        }
    }

    /// Whether the press this frame's draw consumed landed on this button.
    pub fn activated(&self) -> bool {
        self.activated
    }

    /// The columns the label needs, with the padding that keeps it off the edges.
    ///
    /// What a caller laying buttons out in a column centres them against: a box narrower
    /// than this clips its own label.
    pub fn width(&self) -> u16 {
        let label = u16::try_from(self.label.width()).unwrap_or(u16::MAX);
        label.saturating_add(PADDING * 2)
    }

    /// Where the label starts, so the box and its label are centred by one rule.
    fn label_at(&self, area: Rect) -> Position {
        let width = u16::try_from(self.label.width()).unwrap_or(u16::MAX);
        Position::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(1) / 2,
        )
    }
}

impl StatefulWidget for &mut Button<'_> {
    type State = Events;

    fn render(self, area: Rect, buf: &mut Buffer, events: &mut Self::State) {
        // Read before taking the press: taking it clears the pointer event, and a button
        // that stopped being hot on the frame it was pressed would flicker as it is used.
        let hot = events.hovering(area);
        self.activated = events.take_press(area).is_some();

        let style = if hot || self.activated {
            Style::new().fg(Color::Black).bg(HOT)
        } else {
            Style::new().fg(Color::Gray).bg(Color::DarkGray)
        };
        // Every cell is written, not just styled: the button is drawn over whatever was
        // there, and a styled cell keeps the character it already had.
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_style(style);
                }
            }
        }

        let at = self.label_at(area);
        buf.set_line(at.x, at.y, &self.label, area.width);
    }
}
