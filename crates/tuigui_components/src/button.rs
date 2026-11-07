//! A label that is pressed.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::StatefulWidget;

use crate::events::Events;

/// How many blank columns sit on each side of the label.
const PADDING: u16 = 1;

/// The block drawn just outside the box while the pointer is on it: an eighth of a cell.
///
/// Unicode has blocks for eighths, so the nearest eighth is what an eighth is drawn with. The
/// glyphs sit *outside* the box, in the columns either side of it, so the button appears to
/// reach a little wider than the cells it owns without any letter being covered.
const LEAD: char = '▕';
const TRAIL: char = '▏';

/// A label, in a colour, that calls `on_click` when it is pressed.
///
/// One row high and as wide as its label plus the padding either side, so a caller lays it out
/// against the string it holds rather than against a box it has to size.
pub struct Button {
    label: String,
    colour: Color,
    on_click: Box<dyn FnMut()>,
    /// Whether the button is being held down, which outlives the frame the press arrived on:
    /// a press and the release that ends it are two events, usually two frames, always in pairs.
    held: bool,
}

impl Button {
    pub fn new(label: impl Into<String>, colour: Color, on_click: impl FnMut() + 'static) -> Self {
        Self {
            label: label.into(),
            colour,
            on_click: Box::new(on_click),
            held: false,
        }
    }

    /// The columns the button needs: the label, and its padding.
    pub fn width(&self) -> u16 {
        let label = u16::try_from(self.label.chars().count()).unwrap_or(u16::MAX);
        label.saturating_add(PADDING * 2)
    }
}

/// A colour, darker.
///
/// Each channel is taken toward black by a fixed fraction, so a held button is its own colour with
/// the light turned down rather than some other colour: two buttons of different colours stay
/// distinguishable while they are pressed.
fn darker(colour: Color) -> Color {
    let Color::Rgb(r, g, b) = colour else {
        // A named colour has no channels to dim. There is nothing between `Red` and a darker
        // `Red` to name, so it is left as it is.
        return colour;
    };
    // Four fifths of the way up the channel, which is a step down you can see and not a blackout.
    // In `u16` and then narrowed: `channel * 4` alone is past `u8::MAX` for any channel over 63.
    let dim = |channel: u8| u8::try_from(u16::from(channel) * 4 / 5).unwrap_or(channel);
    Color::Rgb(dim(r), dim(g), dim(b))
}

/// A colour, brighter.
///
/// Each channel is lifted toward white by a fixed fraction rather than scaled: scaling leaves a
/// dark colour dark, which is exactly the case a hover has to be visible in.
fn brighter(colour: Color) -> Color {
    let Color::Rgb(r, g, b) = colour else {
        // A named colour has no channels to lift, so the pointer's hint is left as the only
        // answer a hover can give on it: there is nothing between `Red` and a brighter `Red`
        // to name.
        return colour;
    };
    let lift = |channel: u8| {
        // Toward 255 by a fifth of the distance, which is a step you can see and not a wash.
        channel + (255 - channel) / 5
    };
    Color::Rgb(lift(r), lift(g), lift(b))
}

impl StatefulWidget for &mut Button {
    type State = Events;

    fn render(self, area: Rect, buf: &mut Buffer, events: &mut Self::State) {
        let hovered = events.hovering(area);
        // Only a held button takes the release. A release is one event for the whole pane and
        // every widget is offered the same `Events`: a widget that took it without being held
        // would eat the release of whatever was really pressed, and that widget would stay held
        // forever.
        if self.held && events.take_release().is_some() {
            self.held = false;
        }
        if events.take_press(area).is_some() {
            self.held = true;
            (self.on_click)();
        }

        // Dressed in the order the events came: a held button is dark whatever the pointer is
        // doing, and a hover on a button that is not held is the bright one.
        let (colour, weight) = match (self.held, hovered) {
            (true, _) => (darker(self.colour), Modifier::empty()),
            (false, true) => (brighter(self.colour), Modifier::BOLD),
            (false, false) => (self.colour, Modifier::empty()),
        };

        // The row is filled first, so the button is drawn over whatever was there rather than
        // over the cells it happens to change.
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(' ').set_bg(colour);
            }
        }
        // The label is drawn from its own padding in, so the padding is the room the hint at each
        // end has to itself.
        buf.set_string(
            area.x.saturating_add(PADDING),
            area.y,
            &self.label,
            Style::new().bg(colour).add_modifier(weight),
        );

        if hovered && !self.held {
            // An eighth on each side, outside the box and in its colour: the button answers the
            // pointer by reaching beyond the cells it owns, which reads as the box growing rather
            // than as a decoration appearing inside it. Not while it is held: a held button has
            // stopped reaching and gone dark, and the two do not belong together.
            if let Some(x) = area.left().checked_sub(1) {
                if let Some(cell) = buf.cell_mut((x, area.y)) {
                    cell.set_char(LEAD).set_fg(colour);
                }
            }
            if let Some(cell) = buf.cell_mut((area.right(), area.y)) {
                cell.set_char(TRAIL).set_fg(colour);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every channel, including the ends, where the arithmetic has the least room.
    const CHANNELS: [u8; 4] = [0, 1, 128, 255];

    #[test]
    fn dimming_never_takes_a_channel_above_where_it_started() {
        for channel in CHANNELS {
            let Color::Rgb(dim, _, _) = darker(Color::Rgb(channel, channel, channel)) else {
                panic!("truecolour stays truecolour");
            };
            assert!(dim <= channel, "{channel} dimmed to {dim}");
        }
    }

    #[test]
    fn brightening_never_takes_a_channel_below_where_it_started() {
        for channel in CHANNELS {
            let Color::Rgb(lit, _, _) = brighter(Color::Rgb(channel, channel, channel)) else {
                panic!("truecolour stays truecolour");
            };
            assert!(lit >= channel, "{channel} brightened to {lit}");
        }
    }

    #[test]
    fn the_ends_of_a_channel_are_unchanged() {
        // Neither direction can move past the end of the range, which is where an overflow or a
        // wrap would show up.
        assert_eq!(darker(Color::Rgb(0, 0, 0)), Color::Rgb(0, 0, 0));
        assert_eq!(brighter(Color::Rgb(255, 255, 255)), Color::Rgb(255, 255, 255));
    }

    #[test]
    fn a_named_colour_is_left_alone() {
        // There is nothing between `Red` and a darker `Red` to name.
        assert_eq!(darker(Color::Red), Color::Red);
        assert_eq!(brighter(Color::Red), Color::Red);
    }
}
