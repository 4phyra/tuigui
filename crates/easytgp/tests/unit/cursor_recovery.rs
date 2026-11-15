use super::*;

/// A move to `(row, col)` followed by the escape and then the restore, which is what both
/// placement paths hand the writer.
#[test]
fn a_placement_brackets_its_move_with_a_save_and_a_restore() {
    let bytes = around_cursor(7, 11, b"\x1b_Gi=1;p\x1b\\");
    let text = String::from_utf8(bytes).expect("escapes are text");

    assert!(text.starts_with("\x1b7"), "{text:?}");
    assert!(text.contains("\x1b[8;12H"), "the move is to the cell: {text:?}");
    assert!(text.contains("\x1b_Gi=1;p\x1b\\"), "{text:?}");
    assert!(text.ends_with("\x1b8"), "{text:?}");
}

/// The move comes first and the escape second, in one buffer: split across two writes, another
/// writer's bytes could land between the move and the picture it positions.
#[test]
fn the_move_and_the_picture_are_one_write() {
    let bytes = around_cursor(3, 4, b"payload");
    let move_at = bytes
        .windows(2)
        .position(|pair| pair == b"\x1b[")
        .expect("a move");
    let payload_at = bytes
        .windows(7)
        .position(|pair| pair == b"payload")
        .expect("the picture");
    assert!(move_at < payload_at, "the move comes first");
}

/// Nothing is written when there is no cell to move to: a placement without `at` draws where the
/// cursor already is, so there is nothing to undo.
#[test]
fn a_placement_with_no_cell_writes_no_brackets() {
    let placement = PlacementRequest::default();
    assert_eq!(placement.at, None);
    assert_eq!(
        around_cursor(1, 1, b""),
        b"\x1b7\x1b[2;2H\x1b8".to_vec(),
        "the bracket is the whole of what an empty escape adds"
    );
}
