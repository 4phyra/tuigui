//! The pieces a window is drawn and pressed as: the decorated window, its
//! frame, and the bar under it.

pub mod bar;
pub mod decorated_window;
pub mod frame;

pub use decorated_window::{DecoratedWindow, WindowState, cell_size, cells, pixels};
