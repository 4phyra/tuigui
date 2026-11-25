//! A program's window inside a program's window leaves no phantom cells behind.
//!
//! The pane draws a window as a field of anchor cells — one random glyph, two
//! random colours — and the picture is placed under them. The other side scans
//! those cells to learn where the window is; the picture is what a user is meant
//! to see. So every anchor cell the pane does not paint over is a phantom: a
//! cell showing the window's scaffolding instead of the program.
//!
//! One window is not the hard case: the root program's cells are blanked by the
//! same frame that places its picture. A *nested* window is. `foot` started from
//! inside `foot` is admitted as a sub-client's window, which arrives in the
//! middle of a running session rather than before the first frame, and it
//! arrives while the outer picture is already on screen.
//!
//! # How a phantom is seen
//!
//! The session is started with `--dbg-anchor-greenscreen`, which gives every
//! surface the one background a program here never draws: green. The anchor
//! glyph and foreground stay random, so the picture under them cannot be read
//! through the cells — but the background can, and a green cell on this terminal
//! is an anchor nobody blanked.
//!
//! The test also waits for *two* distinct kitty image ids before it looks. A
//! picture is drawn behind the cells, so an inner picture that was placed but
//! hidden by unblanked anchor cells still reports its id; that makes the wait
//! independent of whether the bug being tested is fixed, and it is the fixed
//! state that decides the verdict.

#![cfg(target_os = "linux")]

mod common;

use std::collections::HashSet;
use std::time::Duration;
use termlens::{Color, Graphics, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (80, 24);

/// The green the client gives every surface under `--dbg-anchor-greenscreen`.
const ANCHOR: Color = Color::Rgb(0, 255, 0);

#[test]
fn a_window_inside_a_window_leaves_no_phantom_cells() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("phantom-cells-nested-foot");
    // The inner program waits for this file, so the session ends by the programs
    // finishing rather than by a signal in the middle of a frame.
    let stop = runtime.join("stop");
    // The inner foot: a second client of the session, opened by the first one's
    // program. It inherits `WAYLAND_DISPLAY` from the client that spawned it, so
    // it is admitted into the outer window's tree without being told anything.
    let inner = common::script(
        "phantom-cells-nested-foot-inner",
        &format!(
            "exec foot -- sh -c 'printf \"nested\\n\"; \
             while [ ! -e {} ]; do sleep 0.05; done'",
            stop.display()
        ),
    );
    let runnable = common::script(
        "phantom-cells-nested-foot",
        &format!("exec foot -- {}", inner.display()),
    );

    let mut session = Terminal::builder()
        .size(SIZE.0, SIZE.1)
        .graphics(Graphics::Kitty)
        .cell_size(8, 16)
        .env("XDG_RUNTIME_DIR", &runtime)
        .timeout(Duration::from_secs(60))
        // The session's plumbing, not the codec: JPEG keeps this test on every
        // machine, hardware AV1 or not.
        .arg("--jpeg")
        .arg("--dbg-anchor-greenscreen")
        .arg(&runnable)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;

    // Two windows are two pictures, and a window's id is derived from its own
    // surface: a second id is the inner foot, drawn inside the outer one.
    session.wait_until(|screen| {
        let ids: HashSet<u32> = screen
            .graphics()
            .payloads()
            .iter()
            .filter_map(|picture| picture.id())
            .collect();
        ids.len() >= 2
    })?;

    let screen = session.screen();
    let (cols, rows) = screen.size();
    // The grid, and not `with_styles()`.
    //
    // Every cell a window's picture is placed over carries that window's anchor signature, and
    // the signature's foreground is chosen at random per surface so no window's cells can be
    // mistaken for another's. The cells themselves are blanked by the rewrite, so the grid is the
    // same on every run — but their foreground is not, and a styles block would report a colour
    // that differs each time while saying nothing a reader can judge.
    //
    // The one style fact this test is about, a green background left on a cell nobody blanked, is
    // asserted cell by cell just below, where it can be named exactly.
    termlens::insta::assert_snapshot!(
        "a_window_inside_a_window_leaves_no_phantom_cells",
        screen
    );
    let phantoms: Vec<(u16, u16)> = (0..rows)
        .flat_map(|row| (0..cols).map(move |col| (row, col)))
        .filter(|&(row, col)| {
            screen
                .cell(row, col)
                .is_some_and(|cell| cell.style().bg == ANCHOR)
        })
        .collect();
    assert!(
        phantoms.is_empty(),
        "no cell is left showing a window's scaffolding, but {} are: {:?}\n{screen}",
        phantoms.len(),
        phantoms
    );

    std::fs::write(&stop, b"").expect("the sentinel that ends the inner foot");
    let status = session.wait_exit()?;
    assert!(
        status.success(),
        "the session exits cleanly: {status:?}\n{}",
        session.screen().full_text()
    );
    Ok(())
}
