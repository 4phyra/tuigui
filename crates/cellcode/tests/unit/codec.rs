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
