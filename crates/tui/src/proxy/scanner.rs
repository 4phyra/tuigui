//! What is on the screen: what a client announced, and where every window is.
//!
//! ```text
//!   output ──▶ vt100 ──▶ walk ──┬──▶ barcode: Option<String>
//!                               └──▶ surfaces: Vec<(SurfaceId, Position)>
//! ```
//!
//! A surface is a window's identity: a symbol in one foreground and one background, drawn across
//! the cells the window occupies. Its position is where its first cell is, read off the grid
//! rather than sent — a multiplexer that moved the text moved the window without anyone saying so.
//!
//! A barcode is a contiguous run of braille: a client that cannot be dialed says so by drawing its
//! address, and this finds it.

use std::collections::HashMap;

use cellcode::{Cell as CodeCell, Rgb};
use wire::message::SurfaceId;
use wire::transport::{ANNOUNCE_SEPARATOR, Transport};
use wire::unix::UnixSocket;
use wire::p2p::P2p;

/// A window's top-left cell, in cells from the top left of the screen.
pub type Position = (u16, u16);

/// The grid the command has drawn, read as cells rather than parsed from bytes.
pub struct Scanner {
    /// This side's own terminal, fed the command's bytes.
    terminal: vt100::Parser,
}

/// How many braille cells in a row make a barcode.
const MIN_BARCODE_CELLS: usize = 2;

impl Scanner {
    /// A scanner over this terminal's grid.
    ///
    /// Both dimensions are already non-zero: they come from the terminal's own report, and a
    /// terminal with no rows or no columns has nothing to scan. Which is why this cannot fail.
    pub fn new(rows: std::num::NonZeroU16, cols: std::num::NonZeroU16) -> Self {
        Self {
            terminal: vt100::Parser::new(rows, cols, 0),
        }
    }

    /// Feed one chunk of the command's output.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.terminal.process(bytes);
    }

    /// The channel a client announced, if one drew its address.
    ///
    /// A barcode is a contiguous run of braille: two or more in a row, which is what a program
    /// drawing a graph does not produce. The longest run wins, because a graph and a record on the
    /// same screen is a screen with a record on it.
    pub fn scan_barcode(&self) -> Option<Box<dyn Transport>> {
        let screen = self.terminal.screen();
        let (rows, cols) = screen.size();
        let mut best: Option<Box<dyn Transport>> = None;

        for row in 0..rows.get() {
            let mut run: Vec<CodeCell> = Vec::new();
            for col in 0..cols.get() {
                match screen.cell(row, col).and_then(code_cell) {
                    Some(cell) => run.push(cell),
                    // Anything that is not braille ends the run.
                    None => consider(&mut best, &mut run),
                }
            }
            // A run that reaches the right edge has no cell after it to end it.
            consider(&mut best, &mut run);
        }
        best
    }

    /// Where each of `surfaces` first appears, in cells from the top left.
    ///
    /// A window's position is the first cell carrying its signature, so a surface already found is
    /// not looked for again. `None` for a surface with no cell on screen: it has scrolled away, or
    /// been covered entirely, or has not drawn yet, and all three mean the same thing to a
    /// placement — there is nothing to place.
    pub fn scan_positions(&self, surfaces: &[SurfaceId]) -> HashMap<SurfaceId, Position> {
        let screen = self.terminal.screen();
        let (rows, cols) = screen.size();
        let mut found: HashMap<SurfaceId, Position> = HashMap::with_capacity(surfaces.len());

        for row in 0..rows.get() {
            for col in 0..cols.get() {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                for surface in surfaces {
                    if found.contains_key(surface) {
                        continue;
                    }
                    if is_surface(cell, surface) {
                        found.insert(*surface, (col, row));
                    }
                }
                // Every surface placed, so the rest of the grid is not this question's business.
                if found.len() == surfaces.len() {
                    return found;
                }
            }
        }
        found
    }
}

/// Turn one run of braille into the channel it announces, if it announces one.
///
/// The text a barcode carries is `<transport> : <address>`, which is the announcement format the
/// transports already define. `Transport::from_magic` is the only place that understands it, so a
/// run that does not parse into a channel is not an announcement — a program drawing braille by
/// hand has no reason to spell an address in it.
fn consider(best: &mut Option<Box<dyn Transport>>, run: &mut Vec<CodeCell>) {
    if run.len() >= MIN_BARCODE_CELLS {
        let announced = cellcode::decode(run.drain(..))
            .ok()
            .and_then(|text| announcement(&text));
        if announced.is_some() {
            *best = announced;
        }
    }
    run.clear();
}

/// The channel an announcement names, if it names one this build can dial.
///
/// Every transport is offered the text, in the order [`wire::transport::names`] lists them,
/// and the one whose name matches builds itself. That is why a transport's parse is a
/// `from_magic` on the trait rather than a function here: a channel is added by
/// implementing it and naming it, not by teaching the scan about it.
fn announcement(text: &str) -> Option<Box<dyn Transport>> {
    let (name, address) = text.split_once(ANNOUNCE_SEPARATOR)?;
    let transport: Box<dyn Transport> = match name {
        wire::unix::NAME => Box::new(UnixSocket::from_magic(name, address)?),
        wire::p2p::NAME => Box::new(P2p::from_magic(name, address)?),
        _ => return None,
    };
    log::info!("a client announced itself over {}", transport.name());
    Some(transport)
}

/// The braille cell a terminal cell holds, if it holds one.
fn code_cell(cell: &vt100::Cell) -> Option<CodeCell> {
    let mut chars = cell.contents().chars();
    let symbol = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let vt100::Color::Rgb(r, g, b) = cell.fgcolor() else {
        return None;
    };
    let cell = CodeCell {
        symbol,
        fg: Rgb(r, g, b),
    };
    // `bits` is the only test of "is this braille", so it is what decides.
    cell.bits().map(|_| cell)
}

/// Whether `cell` carries `surface`'s signature.
///
/// True colour only: a palette index names whatever the terminal has in that slot, which is not a
/// fact a surface carries.
fn is_surface(cell: &vt100::Cell, surface: &SurfaceId) -> bool {
    let rgb = |colour: vt100::Color| match colour {
        vt100::Color::Rgb(r, g, b) => Some((r, g, b)),
        vt100::Color::Default | vt100::Color::Idx(_) => None,
    };
    cell.contents().chars().next() == Some(surface.symbol)
        && rgb(cell.fgcolor()) == Some(surface.fg)
        && rgb(cell.bgcolor()) == Some(surface.bg)
}
