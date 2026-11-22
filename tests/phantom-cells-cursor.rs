//! The pointer is one glyph, wherever it has been.
//!
//! The whole session, as `e2e-unix` runs it: `tuigui` starts the server, the
//! server starts the client, and the client puts `foot` on the pane. Once foot
//! is streaming, the pointer is walked over every cell of the pane — left to
//! right, top to bottom — and the screen the last move produces must carry one
//! cursor glyph, at that cell. The compositor paints the pointer by writing it
//! into the cell it is over, so a move that fails to repaint the cell it left
//! leaves the old arrow on screen: a phantom cell. This is that screen, read
//! from the terminal rather than from the buffer that produced it.
//!
//! Each move is followed by a millisecond, so every cell is drawn before the next
//! move overwrites it. The pane redraws on a turn of its own loop, so a gesture
//! written faster than that is coalesced, and a cell that was never painted
//! cannot be caught leaving its arrow behind.
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
        // The session's plumbing, not the codec: JPEG keeps this test on every
        // machine, hardware H.264 or not.
        .arg("--jpeg")
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
            // SGR counts from one; this loop counts from zero. A motion report is what a
            // terminal sends for a hover: `35` is "no button" plus the motion bit, and
            // `termlens` has no call for one — its `drag` is a press and a release around the
            // path, and this gesture has neither.
            session.send_str(&format!(
                "\x1b[<35;{};{}M",
                u32::from(col) + 1,
                u32::from(row) + 1
            ))?;
            // Brief, so each move lands on a frame of its own. The pane redraws on a turn of its
            // own loop, so moves written faster than that are coalesced: every cell but the last
            // is overwritten before it was ever drawn, and a cell that was never painted cannot
            // be caught leaving its arrow behind.
            std::thread::sleep(Duration::from_micros(500));
        }
    }

    // Read once, at the end: the sweep is over when the pane has drawn the last cell it was asked
    // for, which is the cell the last move put the pointer on.
    session.wait_until(|screen| {
        screen
            .cell(pane_rows - 1, cols - 1)
            .is_some_and(|cell| cell.contents() == cursor)
    })?;

    let screen = session.screen();
    // The screen a reader compares when the count below disagrees with it: the pointer is a
    // cell's contents, and a stale one is a second arrow here. The dock is masked because it
    // carries the runnable's path, which is the temp directory's rather than the pane's.
    termlens::insta::assert_snapshot!(
        "moving_over_every_cell_leaves_one_cursor",
        screen.mask_rect(0..cols, pane_rows..rows)
    );
    // One read, one count: the pane paints the pointer into the cell it is over and nothing
    // else, so every cell that still carries the glyph is a phantom the sweep left behind.
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
