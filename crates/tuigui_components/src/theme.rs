//! The palette the widgets are drawn in.
//!
//! Here rather than in each caller, because a colour is the one thing a widget's look cannot
//! decide alone: the strip is told its tabs' colours and the light is told its own, and both came
//! from whatever crate happened to draw them. Naming them once keeps a dock in one program looking
//! like the dock in another.

use ratatui::style::Color;

/// Waiting for the other side: pairing, and the tabs that switch screens.
pub const PINK: Color = Color::Rgb(0xff, 0x69, 0xb4);

/// The program running: a stream that is up and being drawn.
pub const CORAL: Color = Color::Rgb(0xee, 0xc3, 0x9a);

/// The stream's own tab, where it is not the one that is up.
pub const VIOLET: Color = Color::Rgb(0xd7, 0x7b, 0xba);
