//! A window, as the half that owns it hands it to the compositor.
//!
//! The compositor places windows and hands them events. What a window *is* behind that — a
//! program, a link, a capture, a task waiting on the next commit — belongs to whoever started it,
//! so it arrives here as the few facts placement needs and the channels that reach it.
//!
//! Nothing here names removal: a window is gone when the compositor's `Weak` to it goes stale.

use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

/// One window the compositor may place.
pub struct CompositableWindow {
    /// Where a size ask goes, in pixels.
    pub set_size: flume::Sender<(u32, u32)>,
    /// The size this window is, in pixels: what its program last committed.
    pub size: Arc<Mutex<(u32, u32)>>,
    /// What the program calls this window.
    pub title: Arc<Mutex<String>>,
    /// A menu, a tooltip, a dropdown: sized by its own program rather than by this side.
    pub is_popup: bool,
    /// Where it asked to sit inside the window that opened it, in that window's pixels.
    pub offset_px: Option<(i32, i32)>,
    /// Where this window's input goes, in the pane's own event vocabulary.
    pub input: flume::Sender<termina::Event>,
    /// The windows this one's program opens from now on, and the menus it places inside it.
    ///
    /// The compositor waits on both, so a window that opens another is placed the moment the
    /// program says so: nothing between the two has to notice and forward it.
    pub new_children: flume::Receiver<Arc<CompositableWindow>>,
    pub new_popups: flume::Receiver<Arc<CompositableWindow>>,
    /// A request that this window close.
    ///
    /// A request and not a verdict: the program is told and decides, and only a client that
    /// answers stops existing. One-shot because closing is said once — a second ask for a window
    /// already on its way out has nothing to say to it.
    pub close: tokio::sync::oneshot::Sender<()>,
    /// Where this window sits in the stack, bottom first.
    ///
    /// The order is the rank and the rank is the order, so this is written wherever the stack is
    /// reordered and read by the side that captures: a picture carries its place because that is
    /// where it belongs.
    ///
    /// An atomic and not a `Cell`, because it is shared across threads: `Arc<Cell<_>>` is not
    /// `Sync`, so a window holding one could not be sent to a task.
    pub rank: Arc<AtomicU32>,
    /// How this window draws itself into the pane.
    ///
    /// A closure rather than a trait object, because only the side that made the window knows how
    /// to paint it: the wayland one fills the box with its surface signature, the dummy one paints a
    /// picture. It captures whatever that needs, so nothing about it leaks into this struct.
    ///
    /// The signature is a ratatui widget's: the box it occupies, the buffer, and the placement.
    pub render: Box<
        dyn Fn(ratatui::layout::Rect, &mut ratatui::buffer::Buffer, &mut crate::widgets::WindowState)
            + Send
            + Sync,
    >,
}
