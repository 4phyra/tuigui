//! The ring around a window and the bar under it: drawing the chrome, and the
//! resize a press on the ring starts.

use ratatui::buffer::Buffer;
use ratatui::layout::{Margin, Offset, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use termina::event::{MouseButton, MouseEvent, MouseEventKind};

use super::bar::Bar;
use crate::widgets::WindowState;
use crate::windowing::Compositable;

#[derive(Default)]
pub struct Frame {
    /// The title strip under the window, or `None` for a window that does not get one.
    ///
    /// The root fills the pane and a popup is a menu its own program drew, so neither has one: a
    /// bar over either is this side painting furniture on something that is not a framed window.
    /// The ring is drawn either way.
    bar: Option<Bar>,
    resize: Option<(Position, Rect)>,
}

impl Frame {
    /// A frame with no bar: ring only.
    pub fn bare() -> Self {
        Self::default()
    }

    /// A frame with the bar under it, which is what a placed window gets.
    pub fn with_bar() -> Self {
        Self {
            bar: Some(Bar::default()),
            resize: None,
        }
    }

    /// Take the window's name for the frame about to be drawn, for the bar to show.
    pub fn set_title(&mut self, title: String) {
        if let Some(bar) = self.bar.as_mut() {
            bar.set_title(title);
        }
    }

    /// Offer a mouse event to the frame: the bar first, then the resize grips.
    ///
    /// The placement is passed in mutably rather than read back from the window, because a
    /// window's box on the pane is the compositor's fact: the program behind it knows where it
    /// drew, not where it was put.
    pub fn handle_mouse(
        &mut self,
        window: &dyn Compositable,
        state: &mut WindowState,
        mouse: MouseEvent,
        pane: Rect,
        content: Rect,
    ) -> Result<Option<MouseEvent>, crate::Error> {
        let mouse = match self.bar.as_mut() {
            Some(bar) => match bar.handle_mouse(window, state, mouse, pane, bar_area(content))? {
                Some(mouse) => mouse,
                None => return Ok(None),
            },
            None => mouse,
        };
        // A popup's size is its own program's to state, so the ring is not a grip on one: the
        // event goes through to the window like any other cell of it.
        if !window.is_popup() {
            return self.handle_resize(window, state, mouse, pane, content);
        }
        Ok(Some(mouse))
    }

    fn handle_resize(
        &mut self,
        _window: &dyn Compositable,
        state: &mut WindowState,
        mouse: MouseEvent,
        pane: Rect,
        content: Rect,
    ) -> Result<Option<MouseEvent>, crate::Error> {
        let at = Position::new(mouse.column, mouse.row);
        let ring = content.outer(Margin::new(1, 1));
        let started = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && ring.contains(at)
            && !ring.inner(Margin::new(1, 1)).contains(at);
        let Some((pressed, from)) = self.resize.or(started.then_some((at, content))) else {
            return Ok(Some(mouse));
        };

        let d = Offset::new(
            i32::from(at.x) - i32::from(pressed.x),
            i32::from(at.y) - i32::from(pressed.y),
        );
        let moved = from.offset(d);
        let (left, right, top) = (
            pressed.x < from.x,
            pressed.x >= from.right(),
            pressed.y < from.y,
        );

        #[rustfmt::skip]
        let area = {
            let x = if left { moved.x.clamp(pane.x, from.right().saturating_sub(2).max(pane.x)) } else { from.x };
            let y = if top { moved.y.clamp(pane.y, from.bottom().saturating_sub(1).max(pane.y)) } else { from.y };
            let w = if right { moved.right().clamp(from.x + 2, pane.right()) - from.x } else { from.right() - x };
            Rect::new(x, y, w, from.bottom() - y)
        };

        state.area = area;
        self.resize = (!matches!(mouse.kind, MouseEventKind::Up(_))).then_some((pressed, from));
        Ok(None)
    }
}

impl Widget for &mut Frame {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // `area` is the window's own cells; the chrome is the ring around them.
        //
        // Rounded, to match the dock's border: both are furniture on one pane, and a square
        // corner against a rounded one reads as a different kind of thing rather than a
        // different window.
        //
        // The background is reset rather than left alone. The border is drawn over cells that
        // already carry the window's surface, and a style naming only a foreground patches them
        // — so the ring and its corner come out in the window's own colour.
        Block::bordered()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(Color::DarkGray).bg(Color::Reset))
            .render(area.outer(Margin::new(1, 1)), buf);
        if let Some(bar) = self.bar.as_ref() {
            Widget::render(bar, bar_area(area), buf);
        }
    }
}

/// The row under a window's cells that its bar occupies: within the ring's columns.
///
/// Used by the draw and by the hit test, so the bar is pressed where it is seen.
fn bar_area(area: Rect) -> Rect {
    let inside = area.inner(Margin::new(1, 0));
    Rect::new(inside.x, area.bottom(), inside.width, 1)
}
