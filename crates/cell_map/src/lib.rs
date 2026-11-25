//! A terminal stream whose cells a callback may replace on the way through.
//!
//! ```text
//!   reader ──▶ parse ──▶ Cell { symbol, fg, bg } ──▶ callback ──▶ writer
//! ```
//!
//! A cell is the three facts that decide how it looks and nothing else. The callback is asked
//! about each one as the stream draws it and answers with what should be drawn instead;
//! answering with the cell it was given means "draw it as written", which is passed through
//! untouched.
//!
//! # What is not touched
//!
//! Every escape, control sequence and mode set is copied through exactly as it arrived. Only text
//! is rewritten, and only where the callback answered with a different cell. A sequence is never
//! split, and never re-encoded.

use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};
use vt100::Color;

/// A colour, as a cell carries it and as a caller describes one.
///
/// Re-exported so a caller never has to name the engine that supplied it.
pub use vt100::Color as CellColor;

/// One cell, as a callback sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The character drawn in it.
    pub symbol: char,
    /// The foreground it is drawn in.
    pub fg: Color,
    /// The background behind it.
    pub bg: Color,
}

/// Asked about each cell the stream draws, and answers with what to draw instead.
///
/// A function rather than a type alias with a lifetime, so a caller may hand in a closure that
/// borrows: the borrow lasts for the call and no longer.
pub trait Map {
    fn map(&mut self, cell: Cell) -> Cell;
}

impl<F: FnMut(Cell) -> Cell> Map for F {
    #[tracing::instrument(skip_all)]
    fn map(&mut self, cell: Cell) -> Cell {
        self(cell)
    }
}

/// The start of a character or sequence that the last read cut in half. It is held until the
/// bytes that finish it arrive, because writing half of one would put the terminal
/// mid-character.
#[derive(Default)]
pub struct CellMap {
    /// A sequence the last chunk ended in the middle of.
    ///
    /// An escape is only obeyed when its final byte arrives, so half of one is neither written
    /// nor fed to the terminal: it waits here and is prepended to the next chunk. Without this a
    /// chunk that split `ESC [ 3 1 m` would emit the prefix, and the `m` of the next chunk would
    /// be read as an ordinary character and rewritten as a cell.
    escape: Vec<u8>,
    /// The bytes of a character the last chunk ended in the middle of. The mirror of `escape` for
    /// text: a two-byte character split across a chunk boundary is held whole.
    carry: Vec<u8>,
}

impl CellMap {
    #[tracing::instrument(skip_all)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read one chunk up to its end, rewriting every cell into `to`.
    ///
    /// The terminal is fed as the bytes are walked, so the colours it reports are the ones in
    /// effect at that exact point in the stream: a rewrite never has to interpret an SGR sequence
    /// itself. What the chunk ends inside waits in `self` for the next call, which is what makes a
    /// chunk boundary invisible to the screen.
    #[tracing::instrument(skip_all)]
    pub async fn map(
        &mut self,
        terminal: &mut vt100::Parser,
        from: impl AsyncRead + Unpin,
        to: &mut impl std::io::Write,
        map: &mut impl Map,
    ) -> std::io::Result<()> {
        let mut from = BufReader::new(from);
        loop {
            let bytes = match from.fill_buf().await {
                Ok([]) => return Ok(()),
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            let len = bytes.len();
            let out = self.rewrite(terminal, bytes, map);
            from.consume(len);
            to.write_all(&out)?;
            to.flush()?;
        }
    }

    /// One chunk: parse it into `terminal` and return what to write to the screen instead.
    #[tracing::instrument(skip_all)]
    fn rewrite(
        &mut self,
        terminal: &mut vt100::Parser,
        bytes: &[u8],
        map: &mut impl Map,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len() + self.carry.len() + self.escape.len());
        // Anything held from the last call is the beginning of this one, and is only walked once
        // it has been joined by the bytes that complete it. The escape comes first: it was the
        // head of the stream, and a character cannot have been started before it.
        let mut input = std::mem::take(&mut self.escape);
        input.extend_from_slice(&std::mem::take(&mut self.carry));
        input.extend_from_slice(bytes);
        let mut rest = input.as_slice();

        while !rest.is_empty() {
            // An escape owns everything up to its final byte, and goes through in one piece so a
            // rewrite can never land inside a sequence.
            if rest[0] == 0x1b {
                let (escape, tail) = split_escape(rest);
                match complete(escape) {
                    true => {
                        terminal.process(escape);
                        out.extend_from_slice(escape);
                        rest = tail;
                        continue;
                    }
                    // The chunk ended inside this sequence. It is not obeyed and not written until
                    // its final byte arrives: half of one is not a shorter sequence, it is a
                    // prefix the terminal would wait on forever.
                    false => {
                        self.escape.extend_from_slice(escape);
                        break;
                    }
                }
            }

            let len = utf8_len(rest[0]);
            let Some(chunk) = rest.get(..len) else {
                // A character cut off by the end of the read. It waits for the bytes that
                // complete it rather than being written half-decoded.
                self.carry.extend_from_slice(rest);
                break;
            };
            let Some(ch) = std::str::from_utf8(chunk)
                .ok()
                .and_then(|s| s.chars().next())
            else {
                out.extend_from_slice(chunk);
                rest = &rest[len..];
                continue;
            };

            // Where the character will land, asked before it is fed: afterwards the cursor has
            // already moved past it.
            let (row, col) = terminal.screen().cursor_position();
            terminal.process(chunk);

            let (fg, bg) = match terminal.screen().cell(row, col) {
                Some(cell) => (cell.fgcolor(), cell.bgcolor()),
                None => (Color::Default, Color::Default),
            };
            let drawn = Cell {
                symbol: ch,
                fg,
                bg,
            };
            let wanted = Map::map(map, drawn);

            if wanted == drawn {
                out.extend_from_slice(chunk);
            } else {
                // The replacement's own colours, then its symbol, then the colours the stream
                // has in effect so the next cell is not affected by it.
                push_sgr(&mut out, wanted.fg, wanted.bg);
                let mut buf = [0u8; 4];
                out.extend_from_slice(wanted.symbol.encode_utf8(&mut buf).as_bytes());
                push_sgr(&mut out, fg, bg);
            }
            rest = &rest[len..];
        }

        out
    }
}

/// Append the SGR that selects `fg` and `bg`.
///
/// Only the two colours are named. A leading `0` would reset the whole pen, and the stream may
/// have a reverse, a bold or an underline in effect that a replacement must not quietly drop: the
/// stream will not repeat it, because from its side nothing changed.
#[tracing::instrument(skip_all)]
fn push_sgr(out: &mut Vec<u8>, fg: Color, bg: Color) {
    use std::io::Write as _;
    let _ = write!(out, "\x1b[");
    let mut first = true;
    push_colour(out, &mut first, 30, 38, fg);
    push_colour(out, &mut first, 40, 48, bg);
    let _ = write!(out, "m");
}

#[tracing::instrument(skip_all)]
fn push_colour(out: &mut Vec<u8>, first: &mut bool, basic: u8, extended: u8, colour: Color) {
    use std::io::Write as _;
    let separator = match std::mem::replace(first, false) {
        true => "",
        false => ";",
    };
    match colour {
        // `39` and `49` unset one colour and leave the rest of the pen alone.
        Color::Default => {
            let _ = write!(out, "{separator}{}", basic + 9);
        }
        Color::Idx(index) if index < 8 => {
            let _ = write!(out, "{separator}{}", basic + index);
        }
        Color::Idx(index) if index < 16 => {
            // The bright eight sit above the basic ones.
            let _ = write!(out, "{separator}{}", basic + 60 + (index - 8));
        }
        Color::Idx(index) => {
            let _ = write!(out, "{separator}{};5;{}", extended, index);
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(out, "{separator}{};2;{};{};{}", extended, r, g, b);
        }
    }
}

/// Whether `escape` is a whole sequence rather than a prefix of one.
///
/// A sequence is only obeyed on its final byte, so a prefix is not a shorter sequence: the terminal
/// would sit waiting for the rest, and the rewrite would map a final byte of the next chunk as if it
/// were an ordinary character.
#[tracing::instrument(skip_all)]
fn complete(escape: &[u8]) -> bool {
    let Some(&introducer) = escape.get(1) else {
        // `ESC` alone: the introducer has not arrived yet.
        return false;
    };
    match introducer {
        // `CSI` and `SS3`: a final byte in `@`..=`~` ends it.
        b'[' | b'O' => escape[2..]
            .iter()
            .any(|byte| (0x40..=0x7e).contains(byte)),
        // A string sequence: `BEL`, or `ESC \`.
        b']' | b'_' | b'P' | b'^' | b'X' => {
            escape[2..].windows(2).any(|pair| pair == b"\x1b\\") || escape[2..].contains(&0x07)
        }
        // Two bytes: `ESC` and its final.
        _ => true,
    }
}

/// The escape sequence starting at `bytes[0]`, and everything after it.
#[tracing::instrument(skip_all)]
fn split_escape(bytes: &[u8]) -> (&[u8], &[u8]) {
    let mut i = 1;
    match bytes.get(i) {
        // `CSI` and `SS3`: parameters and intermediates, then a final byte in `@`..=`~`.
        Some(b'[') | Some(b'O') => {
            i += 1;
            while let Some(&byte) = bytes.get(i) {
                i += 1;
                if (0x40..=0x7e).contains(&byte) {
                    break;
                }
            }
        }
        // A string sequence: `OSC`, `DCS`, `APC`, `PM`, `SOS`. Runs to `BEL` or to `ESC \`.
        Some(b']') | Some(b'_') | Some(b'P') | Some(b'^') | Some(b'X') => {
            i += 1;
            while let Some(&byte) = bytes.get(i) {
                if byte == 0x07 {
                    i += 1;
                    break;
                }
                if byte == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    break;
                }
                i += 1;
            }
        }
        // Two bytes: `ESC` and its final.
        Some(_) => i += 1,
        None => {}
    }
    bytes.split_at(i.min(bytes.len()))
}

/// How many bytes the UTF-8 character starting with `lead` occupies.
#[tracing::instrument(skip_all)]
fn utf8_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
#[path = "../tests/unit/cell_map.rs"]
mod tests;
