//! The programs running in a compositor of ours, drawn into the pane as windows.
//!
//! This crate is the **placing** half: which window is in front, which one has the keyboard, what
//! box on the pane each one occupies, and where a key or a press ends up. It owns no program, no
//! picture and no link — a window arrives as a [`CompositableWindow`], which is the few facts
//! placement needs and the channels that reach the side running the program.
//!
//! Nothing here removes a window: a window is gone when the [`Weak`] the placement holds stops
//! upgrading, which is the one fact that cannot go stale.

use std::sync::{Arc, Weak};

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;
use ratatui::widgets::Widget;

use crate::widgets::decorated_window::{DecoratedWindow, WindowState, MIN};
use crate::widgets::frame::Frame;

pub mod widgets;
pub mod window;

pub use window::CompositableWindow;

/// The work one window change wants: place a window or a popup, or nothing at all.
///
/// A function and not a future, because placing needs `&mut Compositor` and the wait that produced
/// it holds a borrow of the same stack: the change is taken out first, then applied.
///
/// `true` is the popup half: a window its program opened and a menu it placed inside one arrive by
/// different channels, and only the second is positioned by its own program.
pub type Task = Box<dyn FnOnce(&mut Compositor, rustix::termios::Winsize)>;

/// The pointer, as the pane draws it.
///
/// An arrow, because a pointer is looked for rather than read: it sits on top of whatever window
/// is under it, and the glyph only has to be findable at a glance.
const CURSOR: char = '➤';

/// A program, or several, running in a compositor of ours.
pub struct Compositor {
    /// Every window, in stacking order, each with the windows its program placed inside it.
    ///
    /// The outer order is the stack: the session's root first, so it is at the bottom of the
    /// pane, and the windows above it in the order they were raised, so the frontmost is last.
    /// Each inner slice starts with the window itself and continues with its popups — a menu
    /// belongs to the window it hangs off, so it is next to it rather than beside it.
    ///
    /// Flattened, the sequence is the draw order: a popup is over its parent and under the next
    /// window. Reversed, it is the hit order: the topmost thing is found first.
    windows: Vec<Vec<DecoratedWindow>>,
    /// The window a key belongs to, as its place in the stack.
    ///
    /// `(0, 0)` until something is clicked: the root, which is the program itself.
    focused: (usize, usize),
    /// The pane's rectangle as of the last render, in cells.
    pane: Rect,
    /// Where the pointer last was, in cells of the pane.
    ///
    /// Kept because it is asked for at a different time from the event that says it: a mouse event
    /// arrives between frames and the pane is drawn once per frame, so the cell has to outlive the
    /// event to be drawn. `None` until the pointer has moved.
    pointer: Option<Position>,
}

impl Compositor {
    /// A compositor over the session's root window, which fills the pane.
    ///
    /// The root is the program itself: it has no chrome, and a key pressed before anything was
    /// clicked is its. Every other window arrives through [`Compositor::run`].
    pub fn new(root: Weak<CompositableWindow>) -> Self {
        Self {
            windows: vec![vec![DecoratedWindow::new(
                root,
                None,
                WindowState { area: Rect::ZERO },
            )]],
            focused: (0, 0),
            pane: Rect::ZERO,
            pointer: None,
        }
    }

    /// One turn of the stack: wait for the program to change its windows, then take the change.
    ///
    /// Placement is master's — a child is centred with a bar, a popup is its parent's box plus the
    /// offset its own program asked for — and what changed is only *when* it runs: `populate_*`
    /// used to be asked once per frame whether anything had arrived, and this is told instead.
    ///
    /// `winsize` is the terminal's report of itself: it is what turns a box of cells into a
    /// program's size, and the caller owns the terminal, so it is handed in.
    /// One turn of the stack: take whatever changed, then tell the pane's program what size its box
    /// is.
    ///
    /// The size ask comes first and happens whatever else does, because it is the pane's answer to
    /// the terminal rather than the program's: a window keeps drawing at the size it was configured
    /// with, so a turn that only asked when the stack changed would leave a resized pane holding a
    /// picture drawn for the old one. The wait is what may be long, so it is the part that comes
    /// after.
    ///
    /// `winsize` is the terminal's report of itself: it is what turns a box of cells into a
    /// program's size, and the caller owns the terminal, so it is handed in.
    pub async fn step(&mut self, winsize: rustix::termios::Winsize) -> Result<(), Error> {
        self.cleanup();
        self.maybe_resize_root(winsize)?;
        if let Some(place) = self.recv_tasks().await {
            place(self, winsize);
        }
        Ok(())
    }

    /// Wait for the first window to change, and hand back the work that change wants.
    ///
    /// Every placed window is waited on at once: a program that opens three windows is not three
    /// turns, and none of them can starve another of the news that it opened one. The answer is a
    /// function rather than the change itself, because placing needs `&mut self` and the wait holds
    /// a borrow: the stack cannot be changed while what changed it is still being read.
    ///
    /// A function rather than a future for the same reason — the future would have to borrow the
    /// stack it changes. What a caller does with it is await nothing and just call it.
    ///
    /// `None` is a stack with nothing left in it, which is a session whose program never drew.
    pub async fn recv_tasks(&mut self) -> Option<Task> {
        let mut waits = tokio::task::JoinSet::new();
        for (group, entries) in self.windows.iter().enumerate() {
            for (inner, entry) in entries.iter().enumerate() {
                let Some(window) = entry.get() else {
                    continue;
                };
                // Both waits at once: a program that opens three windows is not three turns, and a
                // window's children and its menus are the same question — what did it open.
                let (children, popups) = (window.new_children.clone(), window.new_popups.clone());
                waits.spawn(async move { (group, inner, children.recv_async().await, false) });
                waits.spawn(async move { (group, inner, popups.recv_async().await, true) });
            }
        }
        // A task that ended without a value is a window whose channel closed while it was being
        // read: there is nothing to place, and the stack learns it is over from the `Weak`.
        let Some(Ok((group, inner, arrived, popup))) = waits.join_next().await else {
            return None;
        };

        // Both channels close together, because the side that serves the program holds both: an
        // error from either is that window being over, and its place in the stack goes with it.
        let Ok(window) = arrived else {
            return Some(Box::new(move |compositor: &mut Self, _| {
                compositor.remove(group, inner);
            }));
        };

        Some(if popup {
            Box::new(move |compositor: &mut Self, winsize| {
                compositor.add_popup(group, inner, Arc::downgrade(&window), winsize);
            })
        } else {
            Box::new(move |compositor: &mut Self, winsize| {
                compositor.add_window(group, Arc::downgrade(&window), winsize);
            })
        })
    }

    /// Drop one entry by its place in the stack.
    ///
    /// A group is a window and its menus: dropping the window drops them, since nothing else shows
    /// them.
    fn remove(&mut self, group: usize, inner: usize) {
        if inner == 0 {
            self.windows.remove(group);
            return;
        }
        if let Some(entries) = self.windows.get_mut(group) {
            entries.remove(inner);
        }
    }

    /// Drop what is over, and tell every program the size its box is.
    ///
    /// The root's box is the pane, so a change of pane is a change of the root's rectangle. Every
    /// other window is asked for the box it was placed in, which its chrome has already taken a row
    /// and two columns from. A popup is left alone: its size is its program's to state.
    fn cleanup(&mut self) {
        for group in self.windows.iter_mut() {
            group.retain(DecoratedWindow::is_alive);
        }
        self.windows.retain(|group| !group.is_empty());
    }

    /// The root is the pane, so a change of pane is a change of the root's rectangle.
    fn maybe_resize_root(&mut self, winsize: rustix::termios::Winsize) -> Result<(), Error> {
        let Some(root) = self.windows.first_mut().and_then(|group| group.first_mut()) else {
            return Ok(());
        };
        root.state.area = self.pane;
        root.maybe_set_size(winsize);
        Ok(())
    }

    /// Put a window its program opened on the pane, as a group of its own.
    ///
    /// Centred, because its size is its own answer and the corner that puts it in the middle is
    /// known before it has drawn anything. It is also the one the user asked for by opening it, so
    /// it becomes what they are about to type into.
    ///
    /// `opened_by` is the group whose window opened it: a child of the root goes beside the root,
    /// and one a window opened goes above that window rather than at the bottom of the stack.
    fn add_window(
        &mut self,
        opened_by: usize,
        window: Weak<CompositableWindow>,
        winsize: rustix::termios::Winsize,
    ) {
        let (width, height) = self.cells(&window, winsize);
        let pane = self.pane;
        self.windows.push(vec![DecoratedWindow::new(
            window,
            Some(Frame::with_bar()),
            WindowState {
                area: Rect::new(
                    pane.x + pane.width.saturating_sub(width) / 2,
                    pane.y + pane.height.saturating_sub(height) / 2,
                    width,
                    height,
                ),
            },
        )]);
        self.focused = (self.windows.len() - 1, 0);
        let _ = opened_by;
    }

    /// Put a menu its program placed on top of the entry that opened it.
    ///
    /// No bar: a popup is the client's own drawing, and this side's chrome over it would be
    /// furniture on something that does not want any. It joins its parent's group, so moving the
    /// window moves the menu with no bookkeeping.
    fn add_popup(
        &mut self,
        group: usize,
        parent: usize,
        window: Weak<CompositableWindow>,
        winsize: rustix::termios::Winsize,
    ) {
        let offset = window
            .upgrade()
            .and_then(|window| window.offset_px)
            .unwrap_or((0, 0));
        let (width, height) = self.cells(&window, winsize);
        let (cell_width, cell_height) = crate::widgets::cell_size(winsize);
        // The program's offset is in pixels of its own picture, and a rectangle on the pane is in
        // cells: adding one to the other puts the menu a thousand columns away, off the pane and
        // into arithmetic that underflows the first time a click is measured against it.
        let (offset_x, offset_y) = (
            (offset.0.max(0) as u32 / cell_width) as u16,
            (offset.1.max(0) as u32 / cell_height) as u16,
        );
        let area = self.windows[group][parent].state.area;
        self.windows[group].push(DecoratedWindow::new(
            window,
            Some(Frame::bare()),
            WindowState {
                area: Rect::new(
                    area.x.saturating_add(offset_x),
                    area.y.saturating_add(offset_y),
                    width,
                    height,
                ),
            },
        ));
    }

    /// Feed one event to the pane, and answer whether a frame is owed.
    ///
    /// Keys go to the focused window, or to the root until something is focused: the pane is the
    /// program, and a key reaches the program first. A mouse event is offered to the windows from
    /// the front, and the first one that takes it ends the walk.
    ///
    /// The event is the pane's while it is here: it is fed, not lent, so nothing comes back. What
    /// the answer says is whether the frame changed — a pointer moved, a window raised — because a
    /// frame is drawn for that and not for every event a program is handed.
    pub fn handle_event(
        &mut self,
        event: termina::Event,
        winsize: rustix::termios::Winsize,
    ) -> Result<bool, Error> {
        match event {
            termina::Event::Key(key) => {
                let (group, inner) = self.focused;
                let Some(window) = self
                    .windows
                    .get(group)
                    .and_then(|group| group.get(inner))
                    .or_else(|| self.windows.first().and_then(|group| group.first()))
                    .and_then(DecoratedWindow::get)
                else {
                    return Ok(false);
                };
                // A window whose program is over is a window about to be dropped: a send that fails
                // is that rather than a failure of the pane's.
                let _ = window.input.send(termina::Event::Key(key));
                Ok(false)
            }
            termina::Event::Mouse(mouse) => {
                let pane = self.pane;
                self.pointer = Some(Position::new(mouse.column, mouse.row));
                // Back to front, and innermost first within a group: a popup is over the window
                // that holds it, so a menu is offered the event before the window it hangs off,
                // and the root last because it fills the pane.
                let mut found = None;
                for (index, group) in self.windows.iter_mut().enumerate().rev() {
                    for inner in (0..group.len()).rev() {
                        let area = group[inner].state.area;
                        if group[inner].handle_mouse(mouse, pane, area, winsize)?.is_none() {
                            found = Some((index, inner));
                            break;
                        }
                    }
                    if found.is_some() {
                        break;
                    }
                }
                // The pointer moved, so the frame is not the one on screen any more.
                let Some(focus) = found else {
                    return Ok(true);
                };
                // What claimed the event is what the keyboard talks to, the root included: the root
                // is the pane, so over the pane is over the program.
                self.focused = focus;
                // The first group is the root, which is the pane itself: raising it would cover its
                // own windows, so it stays. Anything else comes forward with its popups, which are
                // part of it.
                if focus.0 != 0 {
                    let window = self.windows.remove(focus.0);
                    self.windows.push(window);
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// A window's own size in cells, or the smallest a window can be drawn in before it has one.
    fn cells(
        &self,
        window: &Weak<CompositableWindow>,
        winsize: rustix::termios::Winsize,
    ) -> (u16, u16) {
        window
            .upgrade()
            .map(|window| *window.size.lock().unwrap_or_else(|e| e.into_inner()))
            .map(|size| crate::widgets::cells(size, winsize))
            .unwrap_or(MIN)
    }
}

/// The compositor as the pane draws it: every window, bottom of the stack first.
impl Widget for &mut Compositor {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.pane = area;
        // The root fills the pane, so this is where it learns its box: a window that has never been
        // placed has no rectangle to draw into, and a rectangle of nothing is a window whose cells
        // are nowhere — which reads on the far side as a window with no anchor, not as an empty one.
        if let Some(root) = self.windows.first_mut().and_then(|group| group.first_mut()) {
            root.state.area = area;
        }
        for group in self.windows.iter_mut() {
            for placed in group.iter_mut() {
                let place = placed.state.area;
                placed.render(place, buf);
            }
        }
        // The pointer last, over every window: it belongs to the pane rather than to any one of
        // them, and a window drawn after it would cover it.
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

/// Why a window could not do what was asked of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The window's own side could not get the frame out: a codec that refused it, a link that
    /// would not take it. The window is the only thing that can say which, so the reason arrives
    /// as text rather than as variants this crate would have to know about.
    Frame(String),
    /// The pane's own side is gone: nobody is left to draw on or to type into.
    Gone,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Frame(e) => write!(f, "the frame could not be sent: {e}"),
            Error::Gone => f.write_str("the pane is gone"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for std::io::Error {
    fn from(error: Error) -> Self {
        std::io::Error::other(error)
    }
}
