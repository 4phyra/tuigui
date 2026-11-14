//! A window with its chrome: the window's own cells, and the frame around it.
//! The root has no frame — it is the pane, not a window over anything.

use std::sync::{Arc, Weak};

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Clear, Widget};
use termina::event::MouseEvent;

use super::frame::Frame;
use crate::window::CompositableWindow;

/// The smallest a window may be, in cells.
///
/// Below this the chrome alone would take more room than the window has: the bar needs a row
/// and the ring needs two columns, so a box smaller than this cannot be drawn at all. Also the
/// box a window is minted in, which is how "nobody has sized this window yet" is said: its
/// program's first picture is what replaces it.
pub const MIN: (u16, u16) = (10, 10);

/// Where a window is on the pane, and what its picture measures.
///
/// Both are the compositor's to decide: a window knows where its own program thinks it is, not
/// where the pane put it. The two travel together because they are the same question — the box
/// is what the pane draws, and the picture is the size of what goes in it.
pub struct WindowState {
    /// The window's rectangle on the pane, in cells.
    pub area: Rect,
}

/// One window, as the pane draws it and hands events to it.
pub struct DecoratedWindow {
    /// The window itself, held weakly: the side running the program owns it, and a placement
    /// whose window is gone is one that fails to upgrade rather than something to prune.
    pub window: Weak<CompositableWindow>,
    pub frame: Option<Frame>,
    /// Where this window sits, which cells are its own.
    pub state: WindowState,
}

/// One cell of the terminal, in pixels: the whole grid divided out.
///
/// A window is laid out in cells and its program draws in pixels, so this is the
/// conversion between the two, and the terminal's own report is the only place it
/// comes from. A grid of no cells would be a division by zero rather than a size.
pub fn cell_size(winsize: rustix::termios::Winsize) -> (u32, u32) {
    (
        (u32::from(winsize.ws_xpixel) / u32::from(winsize.ws_col).max(1)).max(1),
        (u32::from(winsize.ws_ypixel) / u32::from(winsize.ws_row).max(1)).max(1),
    )
}

/// A box of cells as pixels: what a program is asked to draw at.
///
/// The other half of [`cells`], and the pair has to round-trip: a box converted to pixels,
/// configured, and read back must give the same box.
pub fn pixels(area: Rect, winsize: rustix::termios::Winsize) -> (u32, u32) {
    let (width, height) = cell_size(winsize);
    (u32::from(area.width) * width, u32::from(area.height) * height)
}

/// Pixels as a box of cells: what a program's committed size occupies on the pane.
///
/// Rounded to the nearest cell rather than up or down, which is what makes it the inverse of
/// [`pixels`]: a size the program was asked for in whole cells lands back on exactly that many,
/// and only a program that drew something genuinely between two cells moves at all.
pub fn cells(px: (u32, u32), winsize: rustix::termios::Winsize) -> (u16, u16) {
    let (width, height) = cell_size(winsize);
    (
        ((px.0 + width / 2) / width).max(1) as u16,
        ((px.1 + height / 2) / height).max(1) as u16,
    )
}

impl DecoratedWindow {
    pub fn new(window: Weak<CompositableWindow>, frame: Option<Frame>, state: WindowState) -> Self {
        Self {
            window,
            frame,
            state,
        }
    }

    /// The window itself, for the calls that need its facts.
    pub fn get(&self) -> Option<Arc<CompositableWindow>> {
        self.window.upgrade()
    }

    /// Whether this placement is the one for `window`.
    pub fn is_about(&self, window: &Weak<CompositableWindow>) -> bool {
        Weak::ptr_eq(&self.window, window)
    }

    /// Whether the window behind this placement is still there.
    pub fn is_alive(&self) -> bool {
        self.window.strong_count() > 0
    }

    /// Tell the window's program to draw at the size the window occupies.
    ///
    /// A window is a box of cells on the pane, and its program draws a picture: those two are the
    /// same fact only if someone converts one into the other, which is what this does. Without it a
    /// window keeps drawing at whatever size it chose when it was opened, and the pane stretches
    /// that picture over the box — a window that looks right only if the two sizes happen to agree,
    /// which for the root they do because it is told the pane's own size.
    ///
    /// The root has no chrome, so its box is the pane and it is measured in pixels directly by the
    /// compositor; every other window loses a row to its bar and its ring.
    pub fn maybe_set_size(&mut self, winsize: rustix::termios::Winsize) {
        let Some(window) = self.get() else {
            return;
        };
        // A popup is sized by its own program's positioner, so there is nothing to ask: the box it
        // is drawn in is the size it said it would be.
        if window.is_popup {
            return;
        }
        // Asked only when the box and the picture disagree. A configure is not free on the
        // program's side — it stops, lays itself out again and commits — so asking on every turn is
        // a program that never finishes drawing one frame, which shows as a window that never
        // appears. The size the program reports is what it committed at, so the two agreeing means
        // there is nothing to ask for.
        let size = pixels(self.state.area, winsize);
        let committed = *window.size.lock().unwrap_or_else(|e| e.into_inner());
        if committed == size {
            return;
        }
        // A window whose program is over is a window about to be dropped, so a send that fails is
        // that rather than a failure of the pane's.
        let _ = window.set_size.send(size);
    }

    /// Offer a mouse event to this window, whose box on the pane is `area`.
    ///
    /// `area` is handed in rather than read from the state, because a popup's state holds only
    /// where it sits *inside* its parent: the pane-wide box is the parent's corner plus that
    /// offset, and only the compositor, which walks the two together, can say what it is.
    pub fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        pane: Rect,
        area: Rect,
        winsize: rustix::termios::Winsize,
    ) -> Result<Option<MouseEvent>, crate::Error> {
        let Some(window) = self.get() else {
            return Ok(None);
        };
        // A window without chrome has nothing to hit-test first, so the event goes straight on: a
        // frame that claimed nothing would report the window as having claimed it, and the root —
        // which has none — would then deliver no pointer event at all.
        let mouse = match self.frame.as_mut() {
            Some(frame) => match frame.handle_mouse(&window, &mut self.state, mouse, pane, area)? {
                Some(mouse) => mouse,
                None => return Ok(None),
            },
            None => mouse,
        };
        if !area.contains(Position::new(mouse.column, mouse.row)) {
            return Ok(Some(mouse));
        }
        // The pane reports the pointer in cells of itself; what the program is told is the pixel
        // inside its own picture, which is where the user aimed. One cell is not one pixel, so the
        // offset is multiplied by the cell before it is sent — a pointer sent in cells lands in the
        // window's top-left corner and never reaches the widget the user aimed at.
        let (cell_width, cell_height) = cell_size(winsize);
        let at = (
            f64::from(mouse.column - area.x) * f64::from(cell_width),
            f64::from(mouse.row - area.y) * f64::from(cell_height),
        );
        let _ = window.input.send(termina::Event::Mouse(MouseEvent {
            column: at.0 as u16,
            row: at.1 as u16,
            ..mouse
        }));
        Ok(None)
    }
}

impl Widget for &mut DecoratedWindow {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        // The window's own cells, which is what the far side scans for: they carry its surface
        // signature, and a window drawn without them is an anchor nothing can be placed on.
        //
        // A call and not a description, because a window is not only cells: the dummy session's
        // windows paint a picture, and only they know how.
        if let Some(window) = self.get() {
            (window.render)(area, buf, &mut self.state);
        }
        // Read before the frame is borrowed: the title is the program's fact, and the frame is
        // this side's.
        let title = self
            .get()
            .map(|window| window.title.lock().unwrap_or_else(|e| e.into_inner()).clone());
        if let Some(frame) = self.frame.as_mut() {
            // A popup has no bar: it is a menu its own program drew, and a title strip under it
            // would be this side painting furniture over something the client owns. The ring it
            // keeps, because a popup is still a box on the pane.
            //
            // A window that is over has no name to give, and the bar it would be painted on is the
            // bar of a window `cleanup` is about to drop: the title is cleared rather than
            // reported, because there is nothing left that could act on the failure.
            if let Some(title) = title {
                frame.set_title(title);
            }
            frame.render(area, buf);
        }
    }
}
