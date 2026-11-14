use super::*;

/// The cell as it was drawn: the callback's own view of the stream.
fn unchanged(cell: Cell) -> Cell {
    cell
}

/// A terminal to rewrite into, sized so a test's text fits on one row.
fn terminal() -> vt100::Parser {
    vt100::Parser::new(
        std::num::NonZeroU16::new(3).expect("rows"),
        std::num::NonZeroU16::new(20).expect("cols"),
        0,
    )
}

/// Rewrite one chunk, collecting what would have been written.
fn rewrite(map: &mut CellMap, bytes: &[u8], callback: &mut impl Map) -> Vec<u8> {
    map.rewrite(&mut terminal(), bytes, callback)
}

#[test]
fn an_untouched_stream_is_byte_identical() {
    let mut map = CellMap::new();
    let wrote = b"hello \x1b[31mred\x1b[0m and \x1b]0;a title\x07plain";
    assert_eq!(rewrite(&mut map, wrote, &mut unchanged), wrote.to_vec());
}

#[test]
fn a_rewritten_cell_reports_the_colours_in_effect() {
    let mut map = CellMap::new();
    let seen = std::cell::RefCell::new(Vec::new());
    rewrite(&mut map, b"\x1b[31;42mx", &mut |cell| {
        seen.borrow_mut().push(cell);
        cell
    });
    assert_eq!(
        *seen.borrow(),
        vec![Cell {
            symbol: 'x',
            fg: Color::Idx(1),
            bg: Color::Idx(2),
        }]
    );
}

#[test]
fn a_replacement_carries_its_own_colours_and_restores_the_streams() {
    let mut map = CellMap::new();
    let wrote = rewrite(&mut map, b"\x1b[31mx", &mut |_| Cell {
        symbol: ' ',
        fg: Color::Default,
        bg: Color::Default,
    });
    let text = String::from_utf8(wrote).expect("escapes are text");
    // The replacement's own SGR, its symbol, then the stream's SGR back.
    assert!(text.contains("\x1b[0m "), "{text:?}");
    assert!(text.ends_with("\x1b[0;31m"), "{text:?}");
}

#[test]
fn an_escape_is_never_split_by_a_rewrite() {
    let mut map = CellMap::new();
    let wrote = rewrite(&mut map, b"\x1b]0;title\x07a", &mut |_| Cell {
        symbol: 'b',
        fg: Color::Default,
        bg: Color::Default,
    });
    let text = String::from_utf8(wrote).expect("escapes are text");
    assert!(text.starts_with("\x1b]0;title\x07"), "{text:?}");
}

#[test]
fn a_character_split_across_reads_waits_for_its_bytes() {
    let mut map = CellMap::new();
    let mut terminal = terminal();
    // The first byte of a two-byte character, alone.
    let first = map.rewrite(&mut terminal, &[0xc3], &mut unchanged);
    assert!(first.is_empty(), "half a character is not written: {first:?}");
    // The second completes it, and it is written whole.
    let second = map.rewrite(&mut terminal, &[0xa9], &mut unchanged);
    assert_eq!(second, vec![0xc3, 0xa9]);
}
