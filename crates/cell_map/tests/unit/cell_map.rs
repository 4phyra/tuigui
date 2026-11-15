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
fn an_untouched_stream_is_byte_identical_however_it_is_cut() {
    // The same screen drawn again in two reads, cut at every position: an escape split from its
    // final byte, a character split from its tail. What is held across the cut must come back out
    // in front of the rest, so the terminal reads the stream the command wrote, not one byte less.
    let wrote = b"\x1b[2J\x1b[H\x1b[1;1Htop\x1b[31mred\x1b[0m\xe4\xb8\x80wide\x1b]0;t\x07\
                  \x1b[1K\n\r\ttab";
    let mut map = CellMap::new();
    let mut terminal = terminal();
    for cut in 0..=wrote.len() {
        let mut got = map.rewrite(&mut terminal, &wrote[..cut], &mut unchanged);
        got.extend(map.rewrite(&mut terminal, &wrote[cut..], &mut unchanged));
        assert_eq!(got, wrote.to_vec(), "cut at {cut}");
    }
}

/// The terminal as the rewrite measures it must be the terminal the stream describes: the rewrite
/// asks its model where each character lands, so a model that lost track of an escape reports a cell
/// the screen does not have.
fn agrees(bytes: &[u8]) {
    let mut map = CellMap::new();
    let mut model = terminal();
    let mut reference = terminal();
    map.rewrite(&mut model, bytes, &mut unchanged);
    reference.process(bytes);
    assert_eq!(
        model.screen().cursor_position(),
        reference.screen().cursor_position(),
        "cursor after {bytes:?}"
    );
    for row in 0..3 {
        for col in 0..20 {
            assert_eq!(
                model.screen().cell(row, col).map(|c| c.contents()),
                reference.screen().cell(row, col).map(|c| c.contents()),
                "cell {row},{col} after {bytes:?}"
            );
        }
    }
}

#[test]
fn the_model_follows_a_character_set_designation() {
    // `ESC ( B` and `ESC ) 0` are three bytes: introducer, intermediate, final. The final byte is
    // not text, and a model that draws it puts every following cell one column to the right.
    agrees(b"\x1b(Bab\x1b)0cd");
}

#[test]
fn the_model_follows_a_line_attribute_escape() {
    agrees(b"a\x1b#8b\x1b#5c");
}

#[test]
fn a_replacement_leaves_the_rest_of_the_pen_alone() {
    // Reverse video and red, an anchor cell blanked, then more text in the same run. A run is
    // drawn with one style, so a rewrite that reset the pen would undo reverse for everything
    // after the replacement while the stream never said so.
    let mut map = CellMap::new();
    let wrote = rewrite(&mut map, b"\x1b[7;31mA B", &mut |cell: Cell| match cell.symbol {
        'A' => Cell {
            symbol: ' ',
            fg: cell.fg,
            bg: Color::Default,
        },
        _ => cell,
    });

    let mut screen = terminal();
    screen.process(&wrote);
    let after = screen
        .screen()
        .cell(0, 2)
        .expect("the cell after the replacement");
    assert!(after.inverse(), "reverse survived: {wrote:?}");
    assert_eq!(after.fgcolor(), Color::Idx(1), "red survived: {wrote:?}");
}

#[test]
fn a_chunk_that_ends_mid_sequence_is_never_emitted_mid_sequence() {
    // What the rewrite promises: whatever comes out is made of whole sequences, so a second writer
    // cutting in between two chunks cannot land inside an escape. Every split of this stream must
    // leave the emitted bytes complete — the head waits in `escape`, the tail completes it.
    let wrote = b"\x1b[38;2;255;245;9m\x1b]0;title\x07\x1b_Gi=31;AAAA\x1b\\x\xf0\x9f\x99\x82";
    for cut in 0..=wrote.len() {
        let mut map = CellMap::new();
        let mut terminal = terminal();
        for part in [&wrote[..cut], &wrote[cut..]] {
            let out = map.rewrite(&mut terminal, part, &mut unchanged);
            assert!(whole_sequences(&out), "cut {cut} emitted {out:?}");
        }
    }
}

/// Whether every escape in `bytes` is complete, so nothing here waits on a byte that has not
/// been written yet.
fn whole_sequences(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while !rest.is_empty() {
        if rest[0] != 0x1b {
            let width = utf8_len(rest[0]);
            let Some(character) = rest.get(..width) else {
                return false;
            };
            if std::str::from_utf8(character).is_err() {
                return false;
            }
            rest = &rest[width..];
            continue;
        }
        let (escape, tail) = split_escape(rest);
        if !complete(escape) {
            return false;
        }
        rest = tail;
    }
    true
}

#[test]
fn an_untouched_screen_is_byte_identical() {
    // The edits a full-screen program makes: erase in display, erase in line, cursor addressing,
    // scrolling regions, and the control characters that move the cursor without drawing. None of
    // it may be re-encoded when the callback changes nothing, which is what makes an empty mask a
    // true control: the stream reaches the screen as the command wrote it.
    let mut map = CellMap::new();
    let wrote = b"\x1b[2J\x1b[H\x1b[1;1Htop left\x1b[1K\x1b[2;5H\x1b[3L\x1b[1;4r\n\r\n\ttab\
                  \x1b[0m\xe4\xb8\x80wide\x1b[7mreverse\x1b[0m\x1b[?25l\x1b[K\x1b[999;999Hz";
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
    // The replacement's own colours, its symbol, then the stream's colours back. Only the two
    // colours move: a reset here would drop a reverse or a bold the stream still has in effect.
    assert!(text.contains("\x1b[39;49m "), "{text:?}");
    assert!(text.ends_with("\x1b[31;49m"), "{text:?}");
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
