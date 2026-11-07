//! The windowing vocabulary: what a session is, what a window is, and the
//! input a window takes.
//!
//! A **session** runs one program and gives back what it draws. Its windows
//! form a tree: the root window, and the windows the program opened after it,
//! each opening more. This module is the traits a backend implements, plus the
//! state the compositor keeps about a window; the backends live with whoever
//! owns the way a picture travels, which is the client rather than this crate.
//!
//! Deliberately absent: keycodes and button codes. Those are per-platform
//! (evdev on Linux, a virtual keycode on macOS), so the vocabulary here is
//! logical — a key, a button, a wheel step — and the backend owns the mapping.
//!
//! Also absent: the link. What a window's picture is encoded with and where it
//! goes is the backend's business, so a window is asked for a frame and answers
//! in whatever way its own side can. Nothing here names a codec, a socket or a
//! message.

pub mod event_map;

use ratatui::widgets::StatefulWidgetRef;

use crate::widgets::WindowState;

/// The window session as a whole: what a program runs in.
///
/// A server owns one session: it starts the program, steps it forward, and
/// offers the windows it produced. The holder works through this trait, so a
/// backend with no real program behind it — a debug one that only draws — is
/// as good a session as a wayland one.
pub trait WindowingServer {
    /// The root window: the program's main one, owned. `None` while it has not been built.
    ///
    /// Asked for on every frame rather than kept: the session is what holds its windows, so
    /// answering here is a lookup, and a root that went away is an answer of `None` instead of a
    /// slot that has to be kept in step.
    fn root_window(&mut self) -> Option<Box<dyn Compositable>>;

    /// Start the session's program, and hand back what it says.
    ///
    /// One pipe carries both of the program's streams: a printed line and a reported failure
    /// arrive on it in the order they were written, so the order a reader sees is the order the
    /// program wrote rather than two orderings that cannot be put back together.
    ///
    /// A session whose program is not its own to start — the dummy's, or a session
    /// attached to one already running — starts nothing and gives back nothing.
    fn spawn(&mut self, _path: &std::path::Path) -> std::io::Result<Option<std::io::PipeReader>> {
        Ok(None)
    }

    /// The program's own output pipe, taken once.
    ///
    /// A session whose program is not its own to start has none. Taking it is what makes the
    /// pipe one reader's: whoever takes it drains the program's words, and the session keeps no
    /// copy of what a program said.
    fn take_stdout(&mut self) -> Option<std::io::PipeReader> {
        None
    }

    /// One turn of the session: accept what arrived, dispatch what the
    /// program did, and flush back down the sockets. A dummy session moves
    /// forward by being drawn instead, so its turn is nothing.
    fn advance(&mut self, _timeout: std::time::Duration) {}

    /// Whether the session's program has connected. False while nobody has
    /// said otherwise.
    fn is_ready(&self) -> bool {
        false
    }

    /// Whether the session's program is still running: the fact that ends a
    /// session.
    fn is_running(&self) -> bool {
        true
    }

    /// Resize the session: the size every window is measured against.
    fn set_size(&mut self, size: (u32, u32));
}

impl WindowingServer for Box<dyn WindowingServer> {
    fn root_window(&mut self) -> Option<Box<dyn Compositable>> {
        (**self).root_window()
    }

    fn spawn(&mut self, path: &std::path::Path) -> std::io::Result<Option<std::io::PipeReader>> {
        (**self).spawn(path)
    }

    fn take_stdout(&mut self) -> Option<std::io::PipeReader> {
        (**self).take_stdout()
    }

    fn advance(&mut self, timeout: std::time::Duration) {
        (**self).advance(timeout)
    }

    fn is_ready(&self) -> bool {
        (**self).is_ready()
    }

    fn is_running(&self) -> bool {
        (**self).is_running()
    }

    fn set_size(&mut self, size: (u32, u32)) {
        (**self).set_size(size);
    }
}

/// One admitted window: what it draws, where it sits, and what may be pushed
/// into it.
///
/// A window keeps its own state, so a later read reads where the compositor
/// put it. Children are handed out as weak handles, so the compositor can
/// keep a registry of what it has seen and prune what died.
pub trait Compositable: StatefulWidgetRef<State = WindowState> + Send + Sync {
    /// Queue one key event for delivery to this window.
    ///
    /// Keys and pointer events arrive through separate methods because UI Events
    /// keeps them apart, and a seat depends on it: a modified key is a sequence
    /// of key events that only means something in order, while a pointer event
    /// carries a whole state and must not be interleaved into that sequence.
    fn key_input(&self, event: ui_events::keyboard::KeyboardEvent) -> Result<(), Error>;

    /// Queue one pointer event for delivery to this window.
    fn pointer_input(&self, event: ui_events::pointer::PointerEvent) -> Result<(), Error>;

    /// Take whatever this window drew since the last call, and do with it what this
    /// window's side does with a picture.
    ///
    /// What the pixels are, which codec they travel in and which socket they leave by are
    /// the backend's: a backend that streams is the only kind that has any of them, and a
    /// backend that does not sends nothing here. A window asked may stream a frame, save one
    /// or drop it.
    ///
    /// Taking is what lets the client draw again, so nothing here may hold a picture: every
    /// call hands the pixels on or lets them go.
    ///
    /// `rank` is where the window sits in the stack, bottom first, because nothing on the
    /// screen says which window is in front of another: that is this crate's answer, and it
    /// travels with the frame it applies to.
    ///
    /// `cells` is the box the pane gave it, in cells. A picture is placed by the far side on
    /// a grid of cells, and how many is not the window's to know: its own program draws at a
    /// pixel size the compositor asked for, which is a different number.
    ///
    /// Nothing is answered: how big the window is, is a separate question, asked of
    /// [`Compositable::size_cells`]. A picture and a size are not the same fact — a frame with no
    /// new pixels still has a size, and one taken here would have to say what a picture with
    /// nothing in it measures.
    fn frame(&self, rank: u32, cells: (u16, u16)) -> Result<(), Error>;

    /// How big this window is, in pixels: what its program last committed, or the size it was
    /// adopted at before it drew.
    ///
    /// This is what a backend can answer, because a surface is a picture and its own program drew
    /// it. Not the frame it just sent: taking a picture empties the offer of one, and the size
    /// outlives that.
    fn size_px(&self) -> Result<(u32, u32), Error>;

    /// How big this window is, in cells: the unit the pane lays out in.
    ///
    /// Derived from [`Compositable::size_px`] through the same conversion the ask uses, so a
    /// program that followed a configure exactly answers with the box it was given. A backend
    /// only answers pixels, so there is no way for one to convert twice or to disagree about
    /// what a cell is.
    fn size_cells(
        &self,
        winsize: rustix::termios::Winsize,
    ) -> Result<(u16, u16), Error> {
        Ok(crate::widgets::cells(self.size_px()?, winsize))
    }

    /// Ask this window to draw at `size` pixels instead of its current one.
    ///
    /// The client decides whether to follow: its next commit is the answer. Fails for a window
    /// that is over, which is the one thing an ask cannot do anything about.
    fn set_size(&self, size: (u32, u32)) -> Result<(), Error>;

    /// Whether this window is a popup: a menu, a tooltip, a dropdown.
    ///
    /// The distinction that decides whether it can be resized at all, because a popup's size is
    /// its client's positioner's, not this side's, and asking it for another is asking a question
    /// it has no way to answer. A dialog is a toplevel, however much it looks like a small window,
    /// and is placed and resized like one.
    fn is_popup(&self) -> bool;

    /// Where this window sits inside the one that opened it, in that parent's **pixels**.
    ///
    /// This is what a backend can answer, because it is what a window's own program asks: a popup
    /// is positioned against its parent's surface, and a surface is a picture. `Ok(None)` means
    /// the window was not placed by its own client, which is every window the compositor puts
    /// somewhere — a toplevel asks to be, rather than saying where. `Err` is the window being
    /// over, which is a different answer and must not be read as "not a popup".
    fn offset_px(&self) -> Result<Option<(i32, i32)>, Error> {
        Ok(None)
    }

    /// Where this window sits inside its parent, in cells.
    ///
    /// The unit the pane lays out in, derived from [`Compositable::offset_px`] and the terminal's
    /// own report of how big a cell is. A backend only answers the pixel half, so there is no way
    /// for one to convert twice or to disagree about what a cell is.
    fn offset_cells(
        &self,
        winsize: rustix::termios::Winsize,
    ) -> Result<Option<(u16, u16)>, Error> {
        let Some((x, y)) = self.offset_px()? else {
            return Ok(None);
        };
        let (width, height) = crate::widgets::cell_size(winsize);
        Ok(Some((
            (x.max(0) as u32 / width) as u16,
            (y.max(0) as u32 / height) as u16,
        )))
    }

    /// The windows this one has gained since it was last asked.
    ///
    /// Only the new ones: the holder keeps every window it has already been told about, so a list
    /// of all of them would make it work out which are new on every frame. Reading is what empties
    /// this, which is why it is a call and not a field.
    ///
    /// Owned, because the caller keeps them: a window exists here only as the handle the
    /// compositor was handed.
    ///
    /// A popup is **not** here: it is one of [`Compositable::get_new_popups`], because where it
    /// sits is part of this window rather than a place the compositor chose. Two lists and not one
    /// with a flag, so neither reader has to filter.
    ///
    /// `&mut` because reading is what drains it: a backend keeps the queue that reports these, and
    /// a second read of the same window answers with what has arrived since the first.
    fn get_new_children(&mut self) -> Vec<Box<dyn Compositable>> {
        Vec::new()
    }

    /// The windows this one's program has placed inside it since it was last asked.
    ///
    /// A popup belongs to the window that opened it: it is drawn over that window, moves when it
    /// moves, and its box is measured from the window's own corner. Nothing here says where it
    /// sits — that is [`Compositable::offset_cells`] — so the pairing is all this list is.
    fn get_new_popups(&mut self) -> Vec<Box<dyn Compositable>> {
        Vec::new()
    }

    /// Whether this window is still there to be drawn.
    ///
    /// A handle to a window can outlive it — the compositor keeps one for what it has already
    /// placed — so the one thing that says a window is over is the window itself, answering.
    /// False means the holder drops it, which is the window closing rather than a failure.
    fn is_alive(&self) -> bool;

    /// What the window calls itself: the title its program set, or the name of
    /// the program when it set none. Shown on the bar, and nowhere else.
    fn title(&self) -> Result<String, Error>;

    /// Ask the window to close, the way a program's own quit is asked for: the
    /// client is told, and it decides. A window whose program ignores it stays.
    fn terminate(&self) -> Result<(), Error>;
}

/// Why a window could not do what was asked of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The client that owned the window is gone, so nothing is listening.
    Gone,
    /// The window's own side could not get the frame out: a codec that refused it, a link
    /// that would not take it. The window is the only thing that can say which, so the
    /// reason arrives as text rather than as variants this crate would have to know about.
    Frame(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Gone => f.write_str("the window is gone"),
            Error::Frame(e) => write!(f, "the frame could not be sent: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Delivering input is the only failure this module hands back as an I/O error:
/// the caller writes it to the pane's log beside the reads and writes it sits
/// between, so a window that took an event and could not pass it on becomes one
/// error rather than a stdout-shaped one at every call site.
impl From<Error> for std::io::Error {
    fn from(error: Error) -> Self {
        std::io::Error::other(error)
    }
}
