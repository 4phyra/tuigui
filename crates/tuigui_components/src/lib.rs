//! The widgets a pane is built from, drawn as plain ratatui widgets.
//!
//! Each one draws itself from a shared reference and takes its input through a `handle_events`
//! that is handed the box it was drawn in and the frame's events, so the box it is drawn in is
//! the box it is pressed in. Nothing here reads the terminal, owns a screen, or tracks focus —
//! who is in front, and which widget the next event belongs to, are the caller's questions.
//!
//! The events are the terminal's own, not a vocabulary of this crate's: the caller drains its
//! reader once into a `Vec<termina::Event>` and offers that to each widget in turn. A widget
//! takes the events it acts on and leaves the rest, which is [`events`](crate::events)'s whole
//! job.

pub mod button;
pub mod cross;
pub mod events;
pub mod tabs;
pub mod theme;

pub use button::Button;
pub use cross::Cross;
pub use tabs::Tabs;
pub use theme::{CORAL, PINK, VIOLET};
