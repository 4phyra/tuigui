//! Carry a string in a run of terminal cells.
//!
//! ```text
//!   ⠺ ⡊ ⣿ ⠀      one run, no header, no footer
//! ```
//!
//! A cell holds a character and a foreground colour, and both are values. This crate spends
//! them on payload: the two hundred and fifty-six braille symbols are one code point per byte
//! pattern, so a symbol is eight bits with no table to keep in step, and the foreground adds
//! twenty-four more. One cell is four bytes.
//!
//! # Why there is no header
//!
//! A record needs no marker, length or terminator, because the payload is printable ASCII.
//! That single fact does the work all three would:
//!
//! - **No marker.** Nothing in ASCII is a braille code point, so a run of braille on a screen
//!   is either this crate's output or a program drawing braille by hand — and a program draws
//!   scattered cells, not a contiguous run of them.
//! - **No length.** A record is `ceil(len / 4)` cells, so the count follows from the payload:
//!   there is nothing to agree on and no width to ask for.
//! - **No terminator.** The run ends where the braille stops.
//!
//! # The padding
//!
//! The last cell is filled out with zero bytes, and a zero byte is padding and nothing else:
//! printable ASCII starts at `0x20`, so nothing [`encode`] accepts can contain one. A decoder
//! strips trailing zeros and stops, which is why the payload needs no length to say where it
//! ends.
//!
//! A caller placing the record in a pane pads in front of it with blank cells — braille index
//! zero, `⠀` — and this crate never hears about it: those cells are theirs to place, and the
//! record decodes the same wherever it sits.
//!
//! # What does not survive
//!
//! The colour is the fragile half. A terminal or multiplexer without truecolor turns `Rgb` into
//! a palette index, which is a different value rather than a rounded one, so those bits are
//! lost. The symbols have no such problem: braille is single width in every width table and
//! present in every font that has it at all.

use std::fmt;

/// The first code point of the braille block.
///
/// One code point per byte pattern of the cell's dots, so the character *is* the eight bits it
/// carries and neither side needs a table.
const SYMBOL_BASE: u32 = 0x2800;

const SYMBOL_BITS: u32 = 8;
const COLOR_BITS: u32 = 24;

/// Bits one cell carries: eight for the symbol and twenty-four for the colour.
pub const CELL_BITS: u32 = SYMBOL_BITS + COLOR_BITS;

/// Bytes one cell holds.
///
/// Whole bytes, deliberately: the payload is cut into cells, and a cut that landed mid-byte
/// would leave the last cell's padding indistinguishable from data.
pub const BYTES_PER_CELL: usize = (CELL_BITS / 8) as usize;

const _: () = assert!(CELL_BITS == BYTES_PER_CELL as u32 * 8);
const _: () = assert!(CELL_BITS == 32);

/// A foreground colour, as a cell carries one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// One cell of a record: the character drawn in it, and the colour it is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub symbol: char,
    pub fg: Rgb,
}

impl Cell {
    /// The bits this cell carries.
    /// `None` for a symbol outside the braille block, which is how a decoder finds the end of
    /// a run: any other character on the row is not part of this record.
    pub fn bits(self) -> Option<u64> {
        let symbol = u32::from(self.symbol).checked_sub(SYMBOL_BASE)?;
        if symbol >= 1 << SYMBOL_BITS {
            return None;
        }
        let Rgb(r, g, b) = self.fg;
        let fg = u64::from(r) << 16 | u64::from(g) << 8 | u64::from(b);
        Some(u64::from(symbol) | fg << SYMBOL_BITS)
    }

    /// The cell the bits describe.
    ///
    /// Infallible: the symbol is the low eight bits, and the braille block is exactly two
    /// hundred and fifty-six code points starting at [`SYMBOL_BASE`].
    pub fn from_bits(bits: u64) -> Self {
        let fg = (bits >> SYMBOL_BITS) & 0xFF_FFFF;
        Self {
            symbol: char::from_u32(SYMBOL_BASE + (bits & 0xFF) as u32)
                .expect("the braille block is all assigned code points"),
            fg: Rgb((fg >> 16) as u8, (fg >> 8) as u8, fg as u8),
        }
    }
}

/// What a string is not, when it cannot be carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A byte outside printable ASCII.
    ///
    /// `0x00` is the one that matters: it is what padding looks like, so a payload holding one
    /// would be cut short where the padding was stripped. Control characters are refused for
    /// the same reason — the record is read back as text, and a byte that means something to a
    /// terminal is not that.
    NotPrintable { at: usize },
    /// A cell whose symbol is not braille, so no record runs through it.
    Symbol,
    /// A payload byte that is not printable, which means the cells were not written by
    /// [`encode`] or were damaged on the way.
    Corrupt { at: usize },
    /// The record does not fit the number of cells it was given.
    NoRoom { needed: usize, given: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPrintable { at } => write!(f, "byte {at} is not printable ascii"),
            Self::Symbol => write!(f, "a cell's symbol is not braille"),
            Self::Corrupt { at } => write!(f, "byte {at} is not printable ascii"),
            Self::NoRoom { needed, given } => {
                write!(f, "the record needs {needed} cells and was given {given}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Write `text` as cells, handing each one to `take` in order.
///
/// No capacity, no count and no header: the number of cells follows from the number of bytes,
/// which is `ceil(len / 4)`. A caller that wants the record flush against the right edge of a
/// pane pads in front of it when drawing, and this side never hears about it.
pub fn encode(text: &str, mut take: impl FnMut(Cell)) -> Result<(), Error> {
    let bytes = text.as_bytes();
    for (at, byte) in bytes.iter().enumerate() {
        if !(0x20..=0x7E).contains(byte) {
            return Err(Error::NotPrintable { at });
        }
    }

    for chunk in bytes.chunks(BYTES_PER_CELL) {
        let mut bits = 0u64;
        let mut shifts = 0u32;
        for byte in chunk {
            shifts += 8;
            bits = bits << 8 | u64::from(*byte);
        }
        // The logical end is filled with zero bytes. Nothing this accepts is zero, so the fill
        // is the one thing a decoder may strip without asking how long the payload was.
        bits <<= u64::from(CELL_BITS - shifts);
        take(Cell::from_bits(bits));
    }
    Ok(())
}

/// Write `text` as exactly `cells` cells, left-padded with blank ones.
///
/// The padding goes in front, not behind, and it is made of cells rather than bytes: a blank
/// cell is braille index zero, `⠀`, which decodes as four zero bytes that [`decode`] strips
/// from the end. That works because nothing here encodes a zero byte, so the record is
/// unambiguous wherever it sits — a caller places it flush right in whatever room it has, and
/// this side never hears about the room.
///
/// `cells` is a count, not a width: the caller has already decided how many columns the record
/// may occupy. Requesting fewer cells than the payload needs is [`Error::NoRoom`] rather than a
/// silent truncation, because a shortened record is a wrong record.
pub fn encode_padded(
    text: &str,
    cells: usize,
    mut take: impl FnMut(Cell),
) -> Result<(), Error> {
    let needed = text.len().div_ceil(BYTES_PER_CELL);
    if needed > cells {
        return Err(Error::NoRoom {
            needed,
            given: cells,
        });
    }
    for _ in 0..cells - needed {
        take(Cell::from_bits(0));
    }
    encode(text, take)
}

/// Read back the string a run of cells carries.
/// Consumes the run and nothing else: an iterator over a single record, or over a whole row
/// whose every cell is braille. A cell that is not braille is an error rather than the end of
/// the record, because only a caller reading a row knows where one record ends and another
/// thing begins — see [`decode_run`].
pub fn decode(cells: impl Iterator<Item = Cell>) -> Result<String, Error> {
    let mut bytes = Vec::new();
    for cell in cells {
        let bits = cell.bits().ok_or(Error::Symbol)?;
        for shift in (0..BYTES_PER_CELL).rev() {
            bytes.push((bits >> (shift * 8)) as u8);
        }
    }

    // Padding, and only padding: nothing this crate encodes is zero.
    while bytes.last() == Some(&0) {
        bytes.pop();
    }

    let mut text = String::with_capacity(bytes.len());
    for (at, byte) in bytes.iter().enumerate() {
        if !(0x20..=0x7E).contains(byte) {
            return Err(Error::Corrupt { at });
        }
        text.push(char::from(*byte));
    }
    Ok(text)
}

/// Read one run off a row, and hand back whatever followed it.
///
/// The run is the cells up to the first one that is not braille. That is the whole of the
/// scan: a slot of one or two cells in a row of otherwise-scattered braille is a program
/// drawing a graph, while a contiguous run is a record.
pub fn decode_run(cells: impl Iterator<Item = Cell>) -> (Result<String, Error>, Vec<Cell>) {
    let mut run = Vec::new();
    let mut rest = Vec::new();
    let mut ended = false;
    for cell in cells {
        if ended {
            rest.push(cell);
        } else if cell.bits().is_some() {
            run.push(cell);
        } else {
            ended = true;
            rest.push(cell);
        }
    }
    (decode(run.into_iter()), rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells_of(text: &str) -> Vec<Cell> {
        let mut cells = Vec::new();
        encode(text, |cell| cells.push(cell)).expect("the test strings are printable");
        cells
    }

    /// A cell no record contains: a symbol outside braille, so a run ends there.
    fn other() -> Cell {
        Cell {
            symbol: 'x',
            fg: Rgb(0, 0, 0),
        }
    }

    #[test]
    fn round_trips_without_a_header() {
        for text in [
            "",
            "a",
            "unix:/tmp/run/unix-192312.sock",
            "0123456789 ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz",
        ] {
            assert_eq!(decode(cells_of(text).into_iter()), Ok(text.to_string()), "{text:?}");
        }
    }

    #[test]
    fn the_cell_count_follows_from_the_payload() {
        for (text, cells) in [("a", 1), ("1234", 1), ("12345", 2), ("12345678", 2)] {
            assert_eq!(cells_of(text).len(), cells, "{text:?}");
        }
    }

    #[test]
    fn a_run_ends_where_the_braille_does() {
        let mut row = cells_of("unix:/tmp/x.sock");
        row.push(other());
        let (decoded, rest) = decode_run(row.into_iter());
        assert_eq!(decoded, Ok("unix:/tmp/x.sock".to_string()));
        assert_eq!(rest.len(), 1);
    }

    #[test]
    fn a_payload_byte_that_is_not_printable_is_refused() {
        assert_eq!(encode("a\u{0}b", |_| {}), Err(Error::NotPrintable { at: 1 }));
        assert_eq!(encode("a\nb", |_| {}), Err(Error::NotPrintable { at: 1 }));
        assert_eq!(encode("ab\u{7F}", |_| {}), Err(Error::NotPrintable { at: 2 }));
        assert_eq!(encode("é", |_| {}), Err(Error::NotPrintable { at: 0 }));
    }

    #[test]
    fn a_cell_outside_braille_is_not_a_record() {
        assert_eq!(decode(vec![other()].into_iter()), Err(Error::Symbol));
    }

    #[test]
    fn a_cell_round_trips_through_its_bits() {
        let cell = Cell {
            symbol: '\u{28FF}',
            fg: Rgb(0x12, 0x34, 0x56),
        };
        assert_eq!(Cell::from_bits(cell.bits().expect("a braille symbol")), cell);
    }
}
