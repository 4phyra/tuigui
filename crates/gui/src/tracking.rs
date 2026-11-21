//! A backend that answers where the cursor is without asking the terminal.
//!
//! `Terminal::draw` on an inline viewport asks the backend for the cursor's position every time the
//! size moved, and crossterm answers by writing `ESC [ 6n` and reading the reply off **stdin** with a
//! two-second timeout. This process's own draw loop is reading stdin too, so the reply is as likely
//! to be consumed there as by crossterm, and the call fails.
//!
//! The position the terminal holds after a draw is not a mystery to the writer: it is the last cell
//! the buffer diff printed, and where `set_cursor_position` last put it. Tracking that here answers
//! the question from the writer's own knowledge instead of the reader's race.

use std::io;

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// A `CrosstermBackend` that knows where it left the cursor.
///
/// The tracked position is one this writer produced, never the terminal's word for it, which is the
/// whole point: both sides of this process's stdin contest cannot consume the same reply.
pub struct Tracking {
    inner: CrosstermBackend<io::Stdout>,
    cursor: Position,
}

impl Tracking {
    /// Wrap a backend, taking the position the terminal is currently at.
    ///
    /// The one question worth asking the terminal: the inline viewport anchors to the cursor at
    /// construction, and asking then is safe because the input thread does not exist yet. After that
    /// this never asks again — the whole point.
    pub fn new(backend: CrosstermBackend<io::Stdout>) -> io::Result<Self> {
        let (x, y) = ratatui::crossterm::cursor::position()?;
        Ok(Self { inner: backend, cursor: Position { x, y } })
    }
}

impl io::Write for Tracking {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::Write::write(&mut self.inner, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut self.inner)
    }
}

impl Backend for Tracking {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut last = None;
        let content = content.inspect(|(x, y, _)| last = Some(Position { x: *x, y: *y }));
        self.inner.draw(content)?;
        if let Some(pos) = last {
            self.cursor = pos;
        }
        Ok(())
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)?;
        self.cursor.y += n;
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        let position = position.into();
        self.inner.set_cursor_position(position)?;
        self.cursor = position;
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }

    fn scroll_region_up(&mut self, region: core::ops::Range<u16>, line_count: u16) -> Result<(), Self::Error> {
        self.inner.scroll_region_up(region, line_count)
    }

    fn scroll_region_down(&mut self, region: core::ops::Range<u16>, line_count: u16) -> Result<(), Self::Error> {
        self.inner.scroll_region_down(region, line_count)
    }
}
