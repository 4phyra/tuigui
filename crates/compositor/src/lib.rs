//! The programs running in a compositor of ours, drawn into the pane as windows.
//!
//! The video path decodes a file; this one runs programs. Each program gets a
//! session of its own — a compositor in this process, one output — and the windows
//! it opens are placed here as boxes of cells.
//!
//! # What is whose
//!
//! The compositor holds the **session** ([`WindowingServer`]): the program and
//! its windows. It also holds the windows it has seen, in stacking order — the
//! session's root first and therefore at the bottom, the frontmost last — and the
//! one with focus.
//!
//! What a window *is* — where it sits, what its chrome looks like, what a mouse
//! event does to it — is [`widgets::decorated_window`]'s business. This module is
//! only the stack: which window is in front, which one has the keyboard, and the
//! pass that offers an event to each of them in turn.
//!
//! # What is not
//!
//! The pictures. A window is asked for one and takes its own: whatever a frame
//! becomes after the pixels exist — a codec, a message, a socket — belongs to
//! whoever implements [`AgnosticWindow`], because only that side has a link to
//! send it down. This crate names no codec and no wire.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;
use ratatui::widgets::Widget;

use crate::widgets::decorated_window::{DecoratedWindow, WindowState};
use crate::widgets::frame::Frame;

pub mod widgets;
pub mod windowing;

pub use windowing::{Compositable, Error, WindowingServer};

/// The pointer, as the pane draws it.
///
/// An arrow, because a pointer is looked for rather than read: it sits on top of whatever
/// window is under it, and the glyph only has to be findable at a glance.
const CURSOR: char = '➤';

/// A program, or several, running in a compositor of ours.
///
/// `T` is the window session: a real one (wayland, later macOS) or the dummy
/// one (`--dbg-dummy-windowing`), which draws windows nobody is running.
pub struct Compositor<T: WindowingServer> {
    /// The session the windows live in. Dropping it ends them.
    windowing: T,
    /// Every window, in stacking order, each with the windows its program placed inside it.
    ///
    /// The outer order is the stack: the session's root first, so it is at the bottom of the
    /// pane, and the windows above it in the order they were raised, so the frontmost is last.
    /// Each inner slice starts with the window itself and continues with its popups — a menu
    /// belongs to the window it hangs off, so it is next to it rather than beside it.
    ///
    /// That pairing is what makes everything else one rule. Flattened, the sequence is the draw
    /// order: a popup is over its parent and under the next window. Reversed, it is the hit order:
    /// the topmost thing is found first. Enumerated, the index is the rank. And a popup's place is
    /// its parent's box plus its own offset, read from the two entries side by side.
    ///
    /// Each one carries its own chrome and the drag it is in the middle of, so an entry outlives
    /// the frame that drew it.
    windows: Vec<Vec<DecoratedWindow>>,
    /// The window a key belongs to, as its place in the stack: which group, and which entry in
    /// that group. A position rather than a handle, because the stack is what holds the windows —
    /// a handle would have to be kept in step with it, and an index cannot go stale while the
    /// stack is what is being read.
    ///
    /// `(0, 0)` until something is clicked: the root, which is the program itself.
    focused: (usize, usize),
    /// The pane's rectangle as of the last render, in cells.
    pane: Rect,
    /// Where the pointer last was, in cells of the pane.
    ///
    /// Kept because it is asked for at a different time from the event that says it: a
    /// mouse event arrives between frames and the pane is drawn once per frame, so the
    /// cell has to outlive the event to be drawn. `None` until the pointer has moved,
    /// which is a pane with no pointer on it yet rather than one at the origin.
    pointer: Option<Position>,
}

impl<T: WindowingServer> Compositor<T> {
    /// Take a session, and start compositing it.
    pub fn new(windowing: T) -> Self {
        Self {
            windowing,
            windows: Vec::new(),
            focused: (0, 0),
            pane: Rect::ZERO,
            pointer: None,
        }
    }

    /// Whether the session's program has ended.
    ///
    /// A program that has not connected yet has not ended: the session is
    /// still waiting, which is what `is_running` answers.
    pub fn finished(&self) -> bool {
        !self.windowing.is_running()
    }

    /// The session, for the side that owns more of it than this crate does.
    ///
    /// What a window's pictures are sent down is that side's business, so it is that
    /// side which hands the session its connection. This crate cannot do it: it does
    /// not know what a connection is.
    pub fn windowing_mut(&mut self) -> &mut T {
        &mut self.windowing
    }

    /// Start the session's program, so it connects to this compositor.
    ///
    /// Started after the link exists and not before: what a client is told in order to reach this
    /// compositor is the same thing that tells it where its frames go, so a program running before
    /// there is a link would draw into a session nothing is streaming.
    pub fn spawn(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.windowing.spawn(path).map(|_| ())
    }

    /// The program's own output pipe, if this session started one.
    ///
    /// Taken rather than lent: a pipe has one reader, and whoever takes it is what drains the
    /// program's words. The session keeps no copy — what a program says is not the compositor's
    /// business, which is why it is handed over instead of stored.
    pub fn take_stdout(&mut self) -> Option<std::io::PipeReader> {
        self.windowing.take_stdout()
    }

    /// Hand one event to the pane.
    ///
    /// Keys go to the focused window, or to the root until something is focused:
    /// the pane is the program, and a key reaches the program first. A mouse event
    /// is offered to the windows from the front, and the first one that takes it
    /// ends the walk — see [`DecoratedWindow::handle_mouse`].
    pub fn input(
        &mut self,
        event: &termina::Event,
        winsize: rustix::termios::Winsize,
    ) -> std::io::Result<()> {
        match event {
            termina::Event::Key(key) => {
                // The window being typed into, or the root when nothing has been
                // focused yet: the root is the program, so a key pressed before any
                // window was chosen is a key for the program itself.
                if let Some(window) = self.focused_window().or_else(|| self.root_window()) {
                    window.key_input(windowing::event_map::key(key))?;
                }
            }
            termina::Event::Mouse(mouse) => {
                let pane = self.pane;
                // Where the pointer is, in cells of the pane, kept for the draw that
                // comes after this event: the cursor is painted last, over everything.
                self.pointer = Some(Position::new(mouse.column, mouse.row));
                // Back to front, and innermost first within a group: a popup is over the window
                // that holds it, so a menu is offered the event before the window it hangs off,
                // and the root last because it fills the pane. The answer is the pair — which
                // group, and which entry in it — because the group is what gets raised.
                //
                // A group's first entry is the window, whose box is already on the pane; the
                // rest are its popups, whose boxes are offsets *inside* that box. So the parent's
                // corner is the origin for everything after it.
                let mut found = None;
                for (index, group) in self.windows.iter_mut().enumerate().rev() {
                    for inner in (0..group.len()).rev() {
                        let area = place(group, inner);
                        if group[inner]
                            .handle_mouse(*mouse, pane, area, winsize)?
                            .is_none()
                        {
                            found = Some((index, inner));
                            break;
                        }
                    }
                    if found.is_some() {
                        break;
                    }
                }
                let Some(focus) = found else {
                    return Ok(());
                };
                // What claimed the event is what the keyboard talks to, the root
                // included: the root is the pane, so over the pane is over the
                // program. This is set before the raise below because the two are
                // different decisions, and the root takes focus without ever coming up.
                self.focused = focus;
                // The first group is the root, which is the pane itself: raising it would
                // cover its own windows, so it stays where it is. Anything else comes
                // forward, with its popups: they are part of it.
                if focus.0 == 0 {
                    return Ok(());
                }

                let window = self.windows.remove(focus.0);
                self.windows.push(window);
            }
            _ => {}
        }
        Ok(())
    }

    /// One frame of the session: step it forward, catch the stack up with what
    /// it offers now, and ask every window for its picture.
    ///
    /// Stepping first is what makes the frames worth taking: what a window hands back
    /// here is what its program drew during this step.
    ///
    /// `winsize` is the terminal's own report of itself, read once per frame by the
    /// caller: it is the only place the pane's size in **pixels** comes from, and
    /// a terminal may change it without changing its grid — a font that grew is a
    /// bigger picture of the same number of cells.
    #[tracing::instrument(name = "compositor.frame", skip_all)]
    pub fn frame(
        &mut self,
        timeout: std::time::Duration,
        winsize: rustix::termios::Winsize,
    ) -> Result<(), Error> {
        self.windowing.advance(timeout);

        // Synced first, so the root has a placement to resize: the stack is what holds where a
        // window sits, and the root is a window like any other until it is minted.
        //
        // No root is not a failure and not a reason to stop: a session whose program has not
        // connected has nothing to draw, and one whose windows are already on the pane still has
        // to be stepped and sent.
        if self.windows.is_empty() {
            if let Some(root) = self.windowing.root_window() {
                self.populate_root(root);
            }
        }
        self.cleanup();
        self.populate_windows(winsize)?;
        self.populate_popups(winsize)?;
        self.maybe_resize_root(winsize)?;

        // Each window takes its own picture: the pixels are that window's, and so is
        // whatever it does with them — a window whose side streams encodes and sends, and
        // one whose side does not has nothing to do. A window that has not drawn has
        // nothing to take, which is not a failure: the next frame that commits carries a
        // picture.
        //
        // Its place in the stack goes with the frame: the outer order is the stack, and each
        // window is followed by its popups, so flattening the two is the draw order and one
        // count over it ranks everything. A menu ends up in front of the window it hangs off
        // and behind the next window, with no arithmetic of its own.
        // The ask first, then the picture: a resize happened between frames — a drag, a pane that
        // changed — and the window has to be told before its picture is taken, or the picture
        // taken is the old size and writing it back into the box undoes the drag.
        for window in self.windows.iter_mut().flatten() {
            window.maybe_set_size(winsize)?;
        }

        for (rank, entry) in self.windows.iter_mut().flatten().enumerate() {
            // The box is this side's decision, and it stays it: what the program answers about
            // its own size is never read back into the box. The two do not agree, and cannot —
            // the terminal reports a cell as whole pixels and the program lays out from its own
            // font metrics — so a size fed back is always a little under or over the box that
            // produced it, and asking for it again moves the window further every frame. The
            // program is told the box, every frame, and the box is what the pane draws.
            entry
                .window
                .frame(rank as u32, (entry.state.area.width, entry.state.area.height))?;
        }
        Ok(())
    }

    /// The root is the pane, so a change of pane is a change of the root window's
    /// rectangle on the pane, and a change of the terminal's shape is a change of
    /// the session's output. The root takes the pane as its own, top left: it is
    /// the one window that fills the screen, and everything else is somewhere on
    /// it.
    ///
    /// The program is configured in **pixels**, which is what the terminal reports
    /// and not what the grid says: `ws_xpixel`/`ws_ypixel` are the whole pane in
    /// pixels, and the cells merely say where that picture goes. Configuring the
    /// program at the cell count instead gives it a window of `80x24` pixels for
    /// the pane to stretch over `80x24` cells.
    #[tracing::instrument(name = "compositor.resize", skip_all)]
    fn maybe_resize_root(&mut self, winsize: rustix::termios::Winsize) -> Result<(), Error> {
        let pixels = (u32::from(winsize.ws_xpixel), u32::from(winsize.ws_ypixel));
        // A terminal that reports no pixels is one that cannot say, and it is not
        // an error: the grid is still known, so the program is configured at one
        // pixel a cell. That is the smallest picture that keeps the layout
        // honest, and the pane stretches it to whatever the terminal really is.
        let pixels = match pixels {
            (0, _) | (_, 0) => (
                u32::from(winsize.ws_col).max(1),
                u32::from(winsize.ws_row).max(1),
            ),
            _ => pixels,
        };

        // The root is the bottom of the stack, so it is the first group's first entry: it is
        // the one window whose box is the pane rather than something put on it.
        let Some(root) = self.windows.first_mut().and_then(|group| group.first_mut()) else {
            return Ok(());
        };
        let was = root.state.area;
        if was != self.pane {
            // Worth a line every time it happens: the root is what the session
            // hears about its own size, and a root that disagrees with the pane is
            // a window drawn in the wrong place with nothing to show for it.
            //
            // The countdown is armed too: the program has just been told a new size, so the
            // picture it sends next answers the pane as it was.
            log::info!(
                "compositor: the pane is {:?} cells ({pixels:?} pixels), so the root goes from {:?} to it",
                self.pane,
                was
            );
            root.state.area = self.pane;
        }

        // The output is told the size the pane is, every frame: a backend that is
        // already that size does nothing, so an untouched terminal costs no
        // configure, and a terminal that changed without changing its grid is
        // still caught.
        root.window.set_size(pixels)?;
        self.windowing.set_size(pixels);
        Ok(())
    }

    /// The window a key belongs to when nothing is focused: the root, which is
    /// the program itself and sits at the bottom of the stack.
    pub(crate) fn root_window(&self) -> Option<&dyn Compositable> {
        let root = self.windows.first()?.first()?;
        Some(root.window.as_ref())
    }

    /// The window a key belongs to, by its place in the stack. `None` when that place is empty,
    /// which is a window that has been closed since it was clicked.
    pub(crate) fn focused_window(&self) -> Option<&dyn Compositable> {
        let (group, inner) = self.focused;
        Some(self.windows.get(group)?.get(inner)?.window.as_ref())
    }

    /// Bring the stack up to date with the session: drop what is over.
    ///
    /// One question per window, asked of the window itself: a handle outlives the window it names,
    /// so nothing else can say whether it is still there. A group goes with its window — its popups
    /// are part of it — and its own entries are checked too, because a popup can be dismissed while
    /// the window that opened it stays.
    #[tracing::instrument(name = "compositor.cleanup", skip_all)]
    fn cleanup(&mut self) {
        for group in self.windows.iter_mut() {
            group.retain(|entry| entry.window.is_alive());
        }
        self.windows.retain(|group| !group.is_empty());
    }

    /// Attach the popups every window has gained since the last frame.
    ///
    /// A popup goes into the group of the window that opened it, because it is part of that window:
    /// it is drawn over it, and its box is an offset inside it, so moving the window moves the
    /// popup with no bookkeeping. Where it sits is what its own program asked for, in cells.
    #[tracing::instrument(name = "compositor.popups", skip_all)]
    fn populate_popups(&mut self, winsize: rustix::termios::Winsize) -> Result<(), Error> {
        for group in self.windows.iter_mut() {
            // Every entry can open popups, not only the window: a menu can open a submenu. Walking
            // the group in order is what places them, because a popup's parent is always an entry
            // already in it — the window first, then the popup it hangs off.
            let mut inner = 0;
            while inner < group.len() {
                let popups = group[inner].window.get_new_popups();
                for popup in popups {
                    // The offset is inside the parent's own cells, and the box stored is the
                    // parent's corner plus it, so a submenu chains through its parent.
                    let (x, y) = popup.offset_cells(winsize)?.ok_or(Error::Gone)?;
                    let (width, height) = popup.size_cells(winsize)?;
                    let parent = group[inner].state.area;
                    group.push(DecoratedWindow::new(
                        popup,
                        Some(Frame::bare()),
                        WindowState {
                            area: Rect::new(
                                parent.x.saturating_add(x),
                                parent.y.saturating_add(y),
                                width,
                                height,
                            ),
                        },
                    ));
                }
                inner += 1;
            }
        }
        Ok(())
    }

    /// Put the session's root on the stack, the first frame there is one.
    ///
    /// The root is nobody's child, so nothing announces it: it is the window the session itself
    /// hands over, and the one whose box is the pane rather than a place on it.
    #[tracing::instrument(name = "compositor.root", skip_all)]
    fn populate_root(&mut self, root: Box<dyn Compositable>) {
        self.focused = (0, 0);
        let area = self.pane;
        self.windows.push(vec![DecoratedWindow::new(
            root,
            None,
            WindowState { area },
        )]);
    }

    /// Add the windows the session has gained since the last frame, each as a group of its own.
    ///
    /// A child is a window the compositor places, so it starts at the smallest box the chrome can
    /// be drawn in rather than at an offset its program chose.
    #[tracing::instrument(name = "compositor.windows", skip_all)]
    fn populate_windows(&mut self, winsize: rustix::termios::Winsize) -> Result<(), Error> {
        for index in 0..self.windows.len() {
            // Read first, then extend: `extend` holds `self.windows` mutably for its whole
            // argument, so the window that answers has to be asked before that borrow is taken.
            let children = self.windows[index][0].window.get_new_children();
            let (pane, focused) = (self.pane, (index, self.windows[index].len()));
            let placed = children
                .into_iter()
                .map(|child| {
                    // Centred in the pane: its size is its own answer — the session configured it
                    // at one when it was admitted — so the corner that puts it in the middle is
                    // known before it has drawn anything.
                    //
                    // A window that just appeared is the one the user asked for by opening it, and
                    // so the one they are about to type into.
                    self.focused = focused;
                    log::info!("compositor: a window arrived");
                    let (width, height) = child.size_cells(winsize)?;
                    Ok(DecoratedWindow::new(
                        child,
                        Some(Frame::with_bar()),
                        WindowState {
                            area: Rect::new(
                                pane.x + pane.width.saturating_sub(width) / 2,
                                pane.y + pane.height.saturating_sub(height) / 2,
                                width,
                                height,
                            ),
                        },
                    ))
                })
                .collect::<Result<Vec<_>, Error>>()?;
            self.windows.extend(placed.into_iter().map(|window| vec![window]));
        }
        Ok(())
    }
}

/// Where one entry of a group sits on the pane.
///
/// Its own box, always: a popup's stored box is already the pane's cells, because whoever placed
/// it added the corner of the window it hangs off. Chaining that here would add a parent's corner
/// twice — and for a window, which hangs off nothing, add its own corner to itself.
fn place(group: &[DecoratedWindow], inner: usize) -> Rect {
    group[inner].state.area
}

/// The compositor as the pane draws it: every window, bottom of the stack first.
impl<T: WindowingServer> Widget for &mut Compositor<T> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // The pane is what every window is placed against, so a change of pane is
        // worth a line: a frame drawn at a size nobody expected looks like a window
        // in the wrong place, and this says whether the pane itself moved.
        if self.pane != area {
            log::info!("compositor: the pane is {area:?}, was {:?}", self.pane);
        }
        self.pane = area;
        for group in self.windows.iter_mut() {
            for inner in 0..group.len() {
                let place = place(group, inner);
                group[inner].render(place, buf);
            }
        }
        // The pointer last, over every window: it belongs to the pane rather than to any
        // one of them, and a window drawn after it would cover it.
        //
        // The glyph carries the pane's own anchor colours nowhere: the other side finds
        // a window's picture by scanning for its surface's exact glyph and colours, so a
        // pointer that matched one would be read as a stray cell of that window.
        if let Some(pointer) = self.pointer {
            if area.contains(pointer) {
                if let Some(cell) = buf.cell_mut(pointer) {
                    cell.set_char(CURSOR)
                        .set_fg(Color::Rgb(0xff, 0xff, 0xff))
                        .set_bg(Color::Rgb(0, 0, 0));
                }
            }
        }
    }
}
