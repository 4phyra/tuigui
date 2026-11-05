//! A window with its chrome: the window's own cells, and the frame around it.
//! The root has no frame — it is the pane, not a window over anything.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Clear, Widget};
use termina::event::MouseEvent;

use super::frame::Frame;
use crate::windowing::{Compositable, event_map};

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
    /// The window itself, owned: the stack is what holds the session's windows, so nothing else
    /// keeps them alive and nothing has to be pruned. `Box` rather than `Arc` because there is one
    /// owner — this — and a window that needs to be reachable from elsewhere is reached by its
    /// place here, never by a second handle.
    pub window: Box<dyn Compositable>,
    pub frame: Option<Frame>,
    /// Where this window sits, which cells are its own, and how its picture is encoded.
    ///
    /// A plain field, like the two above it: every caller that needs it holds the window
    /// mutably — the frame loop walks the stack with `iter_mut`, and the drag and resize paths
    /// take `&mut self` — so there is no second reader to synchronise with.
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
/// configured, and read back must give the same box. Multiplying here and dividing there is
/// not enough on its own — a grid does not divide pixels evenly, so dividing a program's
/// committed size can land a cell either side of the box that produced it, and adopting that
/// makes the window flip between two sizes forever.
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
    pub fn new(window: Box<dyn Compositable>, frame: Option<Frame>, state: WindowState) -> Self {
        Self {
            window,
            frame,
            state,
        }
    }

    /// Tell the window's program to draw at the size the window occupies.
    ///
    /// A window is a box of cells on the pane, and its program draws a picture: those
    /// two are the same fact only if someone converts one into the other, which is what
    /// this does. Without it a window keeps drawing at whatever size it chose when it
    /// was opened, and the pane stretches that picture over the box — a window that
    /// looks right only if the two sizes happen to agree, which for the root they do
    /// because it is told the pane's own size.
    ///
    /// The root has no chrome, so its box is the pane and it is measured in pixels
    /// directly by the compositor; every other window loses a row to its bar and its
    /// ring, and is measured here.
    pub fn maybe_set_size(
        &mut self,
        winsize: rustix::termios::Winsize,
    ) -> Result<(), crate::Error> {
        // A popup is sized by its own program's positioner, so there is nothing to ask: the box
        // it is drawn in is the size it said it would be.
        if self.frame.is_none() || self.window.is_popup() {
            return Ok(());
        }
        let area = self.state.area;
        // The ask goes out every frame regardless: a program that has not caught up yet needs to
        // keep hearing the size, and a configure restating what it already has is not news to it.
        self.window.set_size(pixels(area, winsize))
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
        // A window without chrome has nothing to hit-test first, so the event goes
        // straight on: a frame that claimed nothing would report the window as having claimed
        // it, and the root — which has none — would then deliver no pointer event at all.
        let mouse = match self.frame.as_mut() {
            Some(frame) => {
                match frame.handle_mouse(self.window.as_ref(), &mut self.state, mouse, pane, area)?
                {
                    Some(mouse) => mouse,
                    None => return Ok(None),
                }
            }
            None => mouse,
        };
        if !area.contains(Position::new(mouse.column, mouse.row)) {
            return Ok(Some(mouse));
        }
        // The terminal reports the pointer in cells of the pane; the window's program
        // thinks in pixels of its own picture. One cell is not one pixel, so the offset
        // inside the window is multiplied by the cell before it is sent — a pointer sent
        // in cells lands in the window's top-left corner and never reaches the widget the
        // user aimed at.
        let size = cell_size(winsize);
        if let Some(event) = event_map::pointer(
            &mouse,
            (
                f64::from(mouse.column - area.x) * f64::from(size.0),
                f64::from(mouse.row - area.y) * f64::from(size.1),
            ),
        ) {
            self.window.pointer_input(event)?;
        }
        Ok(None)
    }
}

impl Widget for &mut DecoratedWindow {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        ratatui::widgets::StatefulWidgetRef::render_ref(
            self.window.as_ref(),
            area,
            buf,
            &mut self.state,
        );
        if let Some(frame) = self.frame.as_mut() {
            // A popup has no bar: it is a menu its own program drew, and a title strip under it
            // would be this side painting furniture over something the client owns. The ring it
            // keeps, because a popup is still a box on the pane.
            //
            // The bar shows what the window calls itself, which only the window
            // knows: it is read once per frame, here, rather than kept in step.
            //
            // A window that is over has no name to give, and the bar it would be painted on is
            // the bar of a window `cleanup` is about to drop: the title is cleared rather than
            // reported, because there is nothing left that could act on the failure.
            frame.set_title(self.window.title().unwrap_or_default());
            frame.render(area, buf);
        }
    }
}
