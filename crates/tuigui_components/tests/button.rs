//! The button, as a user meets it: pressed with the pointer, then let go.
//!
//! The pane draws one button and the number of clicks it has reported. The two
//! snapshots are its two states, in colour: darker while it is held, its own
//! colour once the release has reached it, with the count still one — a
//! release is not a second press.

mod common;

#[test]
fn the_button_reports_a_press_and_a_release() -> termlens::Result<()> {
    let mut t = common::pane("button")?;
    t.wait_until(|s| s.contains("clicks: 0"))?;

    let (row, column) = common::at(&t, "Press me");
    common::press(&mut t, row, column)?;
    t.wait_until(|s| s.contains("clicks: 1"))?;
    termlens::assert_screen_snapshot!(t);

    let held = t.screen();
    common::let_go(&mut t, row, column)?;
    t.wait_until(|s| s != &held)?;
    termlens::assert_screen_snapshot!(t);

    Ok(())
}
