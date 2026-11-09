//! Blank a window's cells on their way to the screen.
//!
//! A surface is identified by the cells it draws: a symbol and two colours, chosen at random so
//! they cannot collide with ordinary content. Those cells anchor the picture the other side
//! paints, and the picture is drawn *behind* the cells — so a cell carrying a surface's signature
//! would cover the very image it exists to place.


use cell_map::CellMap;
use wire::message::SurfaceId;

/// Rewrites the command's output, blanking the cells of a registered surface.
///
/// `better_default` rather than std's derive, because `CellMap`'s own `Default` is what this wants
/// and the tuple field should be reached through it rather than spelled out here.
#[derive(better_default::Default)]
pub struct Filter(CellMap);

impl Filter {
    /// Feed one chunk, which goes to `sink` with a window's cells blanked.
    ///
    /// A chunk is already in memory, so it is its own reader: `&[u8]` is an `AsyncRead` that is
    /// always ready. The `CellMap` is what carries the state between calls, so a character or a
    /// sequence the chunk ends in the middle of waits for the next one rather than being written
    /// half-formed.
    pub async fn feed(
        &mut self,
        chunk: impl tokio::io::AsyncRead + Unpin,
        terminal: &mut vt100::Parser,
        sink: &mut impl std::io::Write,
        surfaces: &[SurfaceId],
    ) -> std::io::Result<()> {
        self.0
            .map(terminal, chunk, sink, &mut |cell: cell_map::Cell| {
                // Blanked on the default background: the image is drawn behind cell backgrounds,
                // so a cell with a background would cover it.
                match surfaces.iter().any(|surface| surface.matches(&cell)) {
                    true => cell_map::Cell {
                        symbol: ' ',
                        fg: cell.fg,
                        bg: vt100::Color::Default,
                    },
                    false => cell,
                }
            })
            .await
    }
}
