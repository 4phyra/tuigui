//! Blank a window's cells on their way to the screen.
//!
//! A surface is identified by the cells it draws: a symbol and two colours, chosen at random so
//! they cannot collide with ordinary content. Those cells anchor the picture the other side
//! paints, and the picture is drawn *behind* the cells — so a cell carrying a surface's signature
//! would cover the very image it exists to place.

use cell_map::CellMap;
use wire::message::SurfaceId;

/// Rewrites the command's output, blanking the cells of a registered surface.
pub struct Filter {
    /// The map that does the rewriting, and carries the state between chunks: a character or a
    /// sequence a chunk ends in the middle of waits here rather than being written half-formed.
    map: CellMap,
    /// The terminal the rewrite is measured against.
    ///
    /// `CellMap::map` asks it for the colour in effect at each cell, so it is fed the same bytes
    /// the rewrite walks. Owned here rather than handed in, because the two are one object: a
    /// terminal that saw other bytes would report colours that belong to a different screen.
    terminal: vt100::Parser,
}

impl Filter {
    /// A filter over this terminal's grid.
    ///
    /// The size is read here rather than passed, because it is the terminal's own answer and not
    /// something a caller decides: a rewrite measured against a made-up grid would blank cells the
    /// screen does not have. A terminal that will not report one is a failure, not a guess.
    pub fn new() -> std::io::Result<Self> {
        let ws = shared_utils::resize::size()?;
        Ok(Self {
            map: CellMap::new(),
            terminal: vt100::Parser::new(
                std::num::NonZeroU16::new(ws.ws_row)
                    .ok_or_else(|| std::io::Error::other("this terminal reports no rows"))?,
                std::num::NonZeroU16::new(ws.ws_col)
                    .ok_or_else(|| std::io::Error::other("this terminal reports no columns"))?,
                0,
            ),
        })
    }

    /// Feed one chunk, which goes to `sink` with a window's cells blanked.
    ///
    /// A chunk is already in memory, so it is its own reader: `&[u8]` is an `AsyncRead` that is
    /// always ready.
    pub async fn feed(
        &mut self,
        chunk: impl tokio::io::AsyncRead + Unpin,
        sink: &mut impl std::io::Write,
        surfaces: &[SurfaceId],
    ) -> std::io::Result<()> {
        self.map
            .map(
                &mut self.terminal,
                chunk,
                sink,
                &mut |cell: cell_map::Cell| {
                    // Blanked on the default background: the image is drawn behind cell
                    // backgrounds, so a cell with a background would cover it.
                    match surfaces.iter().any(|surface| surface.matches(&cell)) {
                        true => cell_map::Cell {
                            symbol: ' ',
                            fg: cell.fg,
                            bg: vt100::Color::Default,
                        },
                        false => cell,
                    }
                },
            )
            .await
    }

    /// Follow this terminal's own grid when it changes.
    pub fn resize(&mut self, rows: std::num::NonZeroU16, cols: std::num::NonZeroU16) {
        self.terminal.set_size(rows, cols);
    }
}
