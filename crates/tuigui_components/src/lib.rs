//! The widgets a pane is built from, drawn as plain ratatui widgets.
//!
//! Each one implements [`StatefulWidget`](ratatui::widgets::StatefulWidget) with
//! [`Events`] as its state: it is handed the frame's input along with the box it is drawn
//! in, so the box it is drawn in is the box it is pressed in. Nothing here reads the
//! terminal, owns a screen, or tracks focus — who is in front, and which widget the next
//! event belongs to, are the caller's questions.

pub mod button;
pub mod cross;
pub mod events;
pub mod tabs;

pub use button::Button;
pub use cross::Cross;
pub use events::Events;
pub use tabs::Tabs;
