//! The row under a window: what it is called, the grip that moves it, and the
//! button that asks it to close.

use ratatui::buffer::Buffer;
use ratatui::layout::{Offset, Position, Rect, Size};
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;
use termina::event::{MouseButton, MouseEvent, MouseEventKind};
use tuigui_components::{cross, Cross};

use crate::widgets::WindowState;
use crate::windowing::Compositable;

/// Eight dots, the standard grip: the bar is what a window is moved by.
const GRIP: char = '⣿';

#[derive(Default)]
pub struct Bar {
    drag: Option<Position>,
    /// The close button: the same cross a pane's furniture uses, so the two cannot drift apart.
    close: Cross,
    /// What the window calls itself, as of the frame being drawn. Held here rather
    /// than read in `render`, because a `Widget` is handed an area and nothing
    /// else.
    title: String,
}

impl Bar {
    /// Take the window's name for the frame about to be drawn.
    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    /// Where the close button sits, when the bar has the room for one.
    ///
    /// One cell in from the right edge, and as wide as a cross is: the glyph alone is
    /// a target a pointer has to hit exactly.
    fn close_area(area: Rect) -> Option<Rect> {
        (area.width > cross::WIDTH + 2)
            .then(|| Rect::new(area.right() - 1 - cross::WIDTH, area.y, cross::WIDTH, 1))
    }

    pub fn handle_mouse(
        &mut self,
        window: &dyn Compositable,
        state: &mut WindowState,
        mouse: MouseEvent,
        pane: Rect,
        area: Rect,
    ) -> Result<Option<MouseEvent>, crate::Error> {
        let at = Position::new(mouse.column, mouse.row);
        // The cross is offered this frame's one event, which is what `handle_events` is for: the
        // same state machine and the same drawing as a pane's cross, driven by whatever event
        // model the caller has.
        if let Some(close) = Self::close_area(area) {
            let mut events = vec![termina::Event::Mouse(mouse)];
            self.close.handle_events(close, &mut events);
            // A click — pressed and let go inside the box — is what closes. The cross answers that
            // once, so this does not have to remember the last frame to notice the edge.
            if self.close.was_released() {
                window.terminate()?;
            }
            // The button is not a grip: a press on it, and anything at all while it is held, are
            // the cross's rather than the bar's.
            if close.contains(at) || self.close.is_held() {
                return Ok(None);
            }
        }

        let Some(last) = self.drag else {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) && area.contains(at) {
                self.drag = Some(at);
                return Ok(None);
            }
            return Ok(Some(mouse));
        };
        let delta = Offset::new(
            i32::from(at.x) - i32::from(last.x),
            i32::from(at.y) - i32::from(last.y),
        );
        let room = pane.resize(Size::new(pane.width, pane.height.saturating_sub(1)));
        // Moved, not resized: the box keeps its size, so the program has nothing to answer and its
        // next picture is still about this box, which is why no countdown is armed here.
        state.area = (state.area + delta).clamp(room);
        self.drag = (!matches!(mouse.kind, MouseEventKind::Up(_))).then_some(at);
        Ok(None)
    }
}

impl Widget for &Bar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Grey on a darker grey: the bar is chrome, and its own colour is what tells it apart
        // from the picture behind it. Named explicitly rather than left alone — a style that omits
        // a background patches the cell, and the cell already holds the surface's colour.
        let style = Style::new().fg(Color::Gray).bg(Color::DarkGray);
        // Every cell is written, not just styled: the bar is drawn over whatever
        // was there, and a styled cell keeps the character it already had.
        for column in area.x..area.right() {
            if let Some(cell) = buf.cell_mut((column, area.y)) {
                cell.set_char(' ').set_style(style);
            }
        }

        let mut x = area.x;
        let close = Bar::close_area(area);
        // The grip, a space, then the name — clipped so it never reaches the
        // close button.
        let text_end = close.map_or(area.right(), |close| close.x);
        let text = format!("{GRIP} {}", self.title);
        for grapheme in text.chars() {
            if x >= text_end {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(grapheme).set_style(style);
            }
            x += 1;
        }

        if let Some(close) = close {
            Widget::render(&self.close, close, buf);
        }
    }
}
