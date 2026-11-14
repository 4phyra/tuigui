//! The session this client composites, and how its pictures leave.
//!
//! The compositor places windows and hands them events; it does not know what a
//! frame becomes. This is the half that does: the session the program runs in —
//! one per platform — and the link its pictures are written to.
//!
//! The session is a type alias rather than a parameter because this client has
//! exactly one on any given machine. The choice lives here, so nothing above has
//! to be written again for a second one.

#[cfg(target_os = "linux")]
pub mod wayland;

/// The session a program on this machine runs in.
#[cfg(target_os = "linux")]
pub type PlatformWindowing = wayland::WaylandServer;

/// What a session is started with: the output's resolution, the size a window is drawn at before
/// its program has committed anything, the scale it advertises, and whether to carry X11.
///
/// Re-exported rather than named by each caller, because the one caller that builds a session
/// ([`crate::client`]) should not have to know which backend answered the question.
pub use self::wayland::Options;

pub mod event_map;

/// The window session as a whole: what a program runs in.
///
/// A server owns one session: it starts the program, steps it forward, and offers the window the
/// program drew. The holder works through this trait, so a backend with no real program behind it —
/// a dummy one that only draws — is as good a session as a wayland one.
pub trait WindowingServer {
    /// The window the program drew, waited for once.
    ///
    /// A wait rather than a lookup, because a program has to connect and commit before it has a
    /// window at all. The session only moves when it is stepped, so a backend that is asked is a
    /// backend that steps itself until there is something to answer with.
    async fn recv_root(&mut self) -> std::io::Result<Box<dyn Compositable>>;

    /// Start the session's program, and hand back what it says.
    ///
    /// A session whose program is not its own to start — a dummy's, or one attached to a session
    /// already running — starts nothing and gives back nothing.
    fn spawn(&mut self, _path: &std::path::Path) -> std::io::Result<Option<std::io::PipeReader>> {
        Ok(None)
    }

    /// The program's own output pipe, taken once.
    ///
    /// Taking it is what makes the pipe one reader's: whoever takes it drains the program's words,
    /// and the session keeps no copy of what a program said.
    fn take_stdout(&mut self) -> Option<std::io::PipeReader> {
        None
    }

    /// One turn of the session: accept what arrived, dispatch what the program did, and flush back
    /// down the sockets. A dummy session moves forward by being drawn instead, so its turn is
    /// nothing.
    fn advance(&mut self, _timeout: std::time::Duration) {}

    /// Whether the session's program is still running: the fact that ends a session.
    fn is_running(&self) -> bool {
        true
    }

    /// Resize the session: the size every window is measured against.
    fn set_size(&mut self, size: (u32, u32));
}

/// One admitted window: what it draws, where it sits, and what may be pushed into it.
///
/// The compositor holds it weakly, so a window that is over is one that fails to upgrade.
///
/// `async_trait` rather than the language's own `async fn`: an async method takes a trait out of
/// `dyn` compatibility, and the compositor holds its windows as `Box<dyn Compositable>`. The macro
/// desugars each such method into a future the vtable can point at.
#[async_trait::async_trait]
pub trait Compositable: Send + Sync {
    /// Queue one key event for delivery to this window.
    ///
    /// Keys and pointer events arrive through separate methods because UI Events keeps them apart,
    /// and a seat depends on it: a modified key is a sequence of key events that only means
    /// something in order, while a pointer event carries a whole state and must not be interleaved
    /// into that sequence.
    fn key_input(&self, event: termina::event::KeyEvent) -> std::io::Result<()>;

    /// Queue one pointer event for delivery to this window.
    fn pointer_input(&self, event: termina::event::MouseEvent) -> std::io::Result<()>;

    /// Take whatever this window drew since the last call, and send it down the link.
    ///
    /// `rank` is where the window sits in the stack, bottom first, because nothing on the screen
    /// says which window is in front of another: that is the compositor's answer, and it travels
    /// with the frame it applies to.
    ///
    /// `cells` is the box the pane gave it, in cells: a picture is placed by the far side on a grid
    /// of cells, and how many is not the window's to know.
    ///
    /// `connection` is where the picture goes, handed in rather than held: the link arrives late and
    /// from outside, so a window that stored one would have to be told twice.
    async fn frame(
        &self,
        connection: &wire::transport::Connection,
        rank: u32,
        cells: (u16, u16),
    ) -> std::io::Result<()>;

    /// The signature this window's cells carry, and the name it is placed under.
    ///
    /// The far side has to be told it exists before any picture arrives: the cells carrying this
    /// surface are the anchor a picture is placed on, and a frame for a surface with no anchor is
    /// refused rather than drawn.
    fn surface(&self) -> wire::message::SurfaceId;

    /// The codec this window's frames are in, which the far side needs to decode them.
    fn codec(&self) -> codecs::CodecId;

    /// How big this window is, in pixels: what its program last committed.
    fn size_px(&self) -> std::io::Result<(u32, u32)>;

    /// How big this window is, in cells: the unit the pane lays out in.
    ///
    /// Derived from [`Compositable::size_px`] through the same conversion the ask uses, so a
    /// program that followed a configure exactly answers with the box it was given.
    fn size_cells(&self, winsize: rustix::termios::Winsize) -> std::io::Result<(u16, u16)> {
        Ok(compositor::widgets::cells(self.size_px()?, winsize))
    }

    /// Ask this window to draw at `size` pixels instead of its current one.
    ///
    /// The client decides whether to follow: its next commit is the answer.
    fn set_size(&self, size: (u32, u32)) -> std::io::Result<()>;

    /// Whether this window is a popup: a menu, a tooltip, a dropdown.
    ///
    /// The distinction that decides whether it can be resized at all, because a popup's size is its
    /// client's positioner's rather than this side's.
    fn is_popup(&self) -> bool;

    /// Where this window sits inside the one that opened it, in that parent's pixels.
    ///
    /// `Ok(None)` means the window was not placed by its own client, which is every window the
    /// compositor puts somewhere: a toplevel asks to be placed rather than saying where.
    fn offset_px(&self) -> std::io::Result<Option<(i32, i32)>> {
        Ok(None)
    }

    /// What the program calls this window: the title it set, or the name it goes by.
    fn title(&self) -> std::io::Result<String>;

    /// The windows this one's program opens from now on.
    ///
    /// A wait, because a program opens a window when it has something to show, which is not on any
    /// clock this side keeps. `None` is that program being over: there is nothing more to wait for,
    /// and a window that will never open another is not an error to report.
    async fn recv_new_child(&self) -> Option<Box<dyn Compositable>>;

    /// The menus this one's program places inside it from now on.
    async fn recv_new_popup(&self) -> Option<Box<dyn Compositable>>;

    /// Ask the program to close this window.
    ///
    /// A request and not a verdict: the client is told and decides, which is why the window may
    /// still be there afterwards.
    fn terminate(&self) -> std::io::Result<()>;
}

/// How long one message is given to leave this side.
///
/// A message is a write to another process, which may be busy with the one before it. Bounded so a
/// link that stopped draining is reported rather than waited on: without it the frame stalls with
/// the message in hand and says nothing, which reads as a frozen pane.
pub const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
