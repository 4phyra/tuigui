//! The pointer is one glyph, wherever it has been.
//!
//! The whole session, as `e2e-unix` runs it: `tuigui` starts the server, the
//! server starts the client, and the client puts `foot` on the pane. Once foot
//! is streaming, the pointer is walked over every cell of the pane — left to
//! right, top to bottom — and the frame the last cell produces must carry one
//! cursor glyph, at that cell. The compositor paints the pointer by writing it
//! into the cell it is over, so a move that fails to repaint the cell it left
//! leaves the old arrow on screen. This is that frame, read from the terminal
//! rather than from the buffer that produced it.
//!
//! The moves are sent as fast as the reports can be written, so a sweep is one
//! gesture rather than one frame per cell.
//!
//! The move is an SGR motion report with no button held (code 35). `termlens`
//! has no call for a hover; the report is the one a terminal sends.

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;
use compositor::CURSOR;
use termlens::{Graphics, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (80, 24);

/// The rows the dock takes at the bottom of the screen, so the pane ends
/// above them. Matches `gui`'s `dock::HEIGHT`.
const DOCK_HEIGHT: u16 = 3;

#[test]
fn moving_over_every_cell_leaves_one_cursor() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("leaks-leaking-cursor");
    // Foot's shell waits for this file, so the session ends by the program
    // finishing rather than by a signal in the middle of a frame.
    let stop = runtime.join("stop");
    let runnable = common::script(
        "leaks-leaking-cursor",
        &format!(
            "exec foot -- sh -c 'printf \"streaming\\n\"; \
             while [ ! -e {} ]; do sleep 0.05; done'",
            stop.display()
        ),
    );

    let mut session = Terminal::builder()
        .size(SIZE.0, SIZE.1)
        .graphics(Graphics::Kitty)
        .cell_size(8, 16)
        .env("XDG_RUNTIME_DIR", &runtime)
        .timeout(Duration::from_secs(60))
        .arg("--dbg-anchor-greenscreen")
        .arg(&runnable)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;

    // Foot is on the pane when its first frame is: the picture reaches this
    // terminal only after the client captured and transmitted it.
    session.wait_until(|screen| screen.graphics().total() > 0)?;

    let cursor = CURSOR.to_string();
    let (cols, rows) = session.screen().size();
    let pane_rows = rows - DOCK_HEIGHT;
    for row in 0..pane_rows {
        for col in 0..cols {
            // SGR counts from one; this loop counts from zero.
            session.send_str(&format!(
                "\x1b[<35;{};{}M",
                u32::from(col) + 1,
                u32::from(row) + 1
            ))?;
            // The verdict is taken per move, not at the end of the sweep: an arrow left in the
            // cell the pointer came from is only visible in the frame that follows the move, and
            // a full repaint later would erase the evidence. The wait is what fails — a pointer
            // that never leaves the cell it left keeps this predicate false — so the timeout is
            // short enough that a leak reads as a failure rather than as a hung test.
            session
                .wait_until_for(
                    |screen| screen.find_all(&cursor) == vec![(row, col)],
                    Duration::from_secs(2),
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "after moving to ({row}, {col}) the pointer is not the only one: \
                         {error}\n{}",
                        session.screen()
                    )
                });
        }
    }

    let screen = session.screen();
    // The screen a reader compares when the count below disagrees with it: the
    // pointer is a cell's contents, and a stale one is a second arrow here. The
    // dock is masked because it carries the runnable's path, which is the temp
    // directory's rather than the pane's.
    termlens::insta::assert_snapshot!(
        "moving_over_every_cell_leaves_one_cursor",
        screen.mask_rect(0..cols, pane_rows..rows)
    );
    let cursors = screen.find_all(&cursor);
    assert_eq!(
        cursors,
        vec![(pane_rows - 1, cols - 1)],
        "the pointer leaves one glyph, on the cell it is over: {cursors:?}\n{screen}"
    );

    std::fs::write(&stop, b"").expect("the sentinel that ends foot");
    let status = session.wait_exit()?;
    assert!(
        status.success(),
        "the session exits cleanly: {status:?}\n{}",
        session.screen().full_text()
    );
    Ok(())
}
