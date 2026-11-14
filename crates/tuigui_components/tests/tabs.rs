//! The tab strip, as a user meets it: one tab marked, and the band walking to
//! the one that is clicked.
//!
//! Every frame of a move is a snapshot: the band walks, overshoots, and
//! settles, and the line under the strip says whether it is still moving. The
//! first snapshot is the strip at rest; each move after it contributes the
//! click's own frame and then one snapshot per frame until the strip is still
//! again.
//!
//! The screen is snapshotted as the `Screen` the test just read, not as the
//! terminal: `assert_screen_snapshot!(t)` settles first, and a settle longer
//! than a frame would leave every snapshot showing the end of the move.
//!
//! The frames are named after their number, zero-padded, because the review
//! lists snapshots by file name rather than by the order they were drawn:
//! `-2` sorts after `-19`, and a review read in that order shows a move that
//! jumps. `-02` sorts where it was drawn.

mod common;

#[test]
fn the_strip_moves_the_band_to_the_tab_that_was_clicked() -> termlens::Result<()> {
    let mut t = common::pane("tabs")?;
    let mut drawn = 0;
    let mut snapshot = |frame: &termlens::Screen| {
        drawn += 1;
        termlens::insta::assert_snapshot!(
            format!("the_strip_moves_the_band_to_the_tab_that_was_clicked-{drawn:02}"),
            frame.with_styles()
        );
    };
    t.wait_until(|s| s.contains("tab: 0 still"))?;
    let frame = t.screen();
    snapshot(&frame);

    let (row, column) = common::at(&t, "Chrome");
    common::press(&mut t, row, column)?;
    t.wait_until(|s| s.contains("tab: 1 moving"))?;
    loop {
        let frame = t.screen();
        snapshot(&frame);
        if !frame.contains("moving") {
            break;
        }
        t.wait_until(|s| s != &frame)?;
    }

    let (row, column) = common::at(&t, "About");
    common::press(&mut t, row, column)?;
    t.wait_until(|s| s.contains("tab: 2 moving"))?;
    loop {
        let frame = t.screen();
        snapshot(&frame);
        if !frame.contains("moving") {
            break;
        }
        t.wait_until(|s| s != &frame)?;
    }

    Ok(())
}
