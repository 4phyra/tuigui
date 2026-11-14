//! The cross, as a user meets it: held down, then let go.
//!
//! While it is held the glyph's cell and the two either side of it flood with
//! the hot colour, so the snapshot is where that block is visible; the release
//! is where the block is gone and the line under it says so.
//!
//! The two snapshots are the two steps, in order.

mod common;

#[test]
fn the_cross_reports_being_held_and_let_go() -> termlens::Result<()> {
    let mut t = common::pane("cross")?;
    t.wait_until(|s| s.contains("cross: up"))?;

    let (row, column) = common::at(&t, "✖");
    common::press(&mut t, row, column)?;
    t.wait_until(|s| s.contains("cross: down"))?;
    termlens::assert_screen_snapshot!(t);

    // Let go on the cross: it is the one being held, so the release is its
    // own, and the block goes with it.
    common::let_go(&mut t, row, column)?;
    t.wait_until(|s| s.contains("cross: up"))?;
    termlens::assert_screen_snapshot!(t);

    Ok(())
}
