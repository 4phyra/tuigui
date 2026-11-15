//! Grabbing a nested window's corner pulls its picture wider.
//!
//! A window inside a window is the case the ring exists for: the outer `foot` is
//! the pane itself, while the `foot` it started is a window the compositor drew a
//! rounded ring around. The ring is a resize grip — a press on its corner starts a
//! drag, and every cell the pointer crosses moves the window's box on the pane.
//!
//! The box is not the picture. The pane places the picture on the cells carrying
//! the window's surface, and the picture's own extent is the placement's `c=`/`r=`,
//! which the pane sends in every frame. So a drag that only moved the cells and
//! never the placement would leave a window that looks resized and an image that is
//! not. What this reads is the wire: the escape that carried the picture, before
//! and after the drag, and the box it named.
//!
//! The corner is found by glyph rather than by arithmetic: the ring is rounded, so
//! the top right of the innermost window is a `╮` above the dock's own corner, and
//! the test picks the highest one on screen.

#![cfg(target_os = "linux")]

mod common;

use std::collections::HashMap;
use std::time::Duration;
use termlens::{Graphics, GraphicsPayload, MouseButton, Screen, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (80, 24);

/// How far the corner is pulled, in cells.
const PULL: u16 = 5;

#[test]
fn dragging_a_nested_windows_corner_widens_its_picture() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("frame-drag-resizes-graphics");
    // The inner program waits for this file, so the session ends by the programs
    // finishing rather than by a signal in the middle of a frame.
    let stop = runtime.join("stop");
    let inner = common::script(
        "frame-drag-resizes-graphics-inner",
        &format!(
            "exec foot -- sh -c 'printf \"nested\\n\"; \
             while [ ! -e {} ]; do sleep 0.05; done'",
            stop.display()
        ),
    );
    let runnable = common::script(
        "frame-drag-resizes-graphics",
        &format!("exec foot -- {}", inner.display()),
    );

    let mut session = Terminal::builder()
        .size(SIZE.0, SIZE.1)
        .graphics(Graphics::Kitty)
        .cell_size(8, 16)
        .env("XDG_RUNTIME_DIR", &runtime)
        .timeout(Duration::from_secs(20))
        .arg(&runnable)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;

    // Two windows are two pictures: the pane's own, and the one the nested foot
    // draws inside it.
    session.wait_until_for(|screen| pictures(screen).len() >= 2, Duration::from_secs(2))?;

    let before =
        inner_cells(&session.screen()).expect("the nested window has a picture with a box of cells");
    let (row, col) =
        corner(&session.screen()).expect("the nested window has a ring corner to grab");
    // A press on the ring is what starts a resize; the path between the two ends
    // is reported a cell at a time under the tracking a terminal asks for.
    session.drag(MouseButton::Left, col, row, col + PULL, row)?;

    session.wait_until_for(
        |screen| inner_cells(screen).is_some_and(|after| after.0 > before.0),
        Duration::from_secs(2),
    )?;
    let after = inner_cells(&session.screen()).expect("the picture is still placed");
    assert!(
        after.0 > before.0,
        "the drag widened the window's box, so the placement is wider: {before:?} -> {after:?}\n{}",
        session.screen()
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

/// The newest payload of each image, by id.
///
/// A frame re-places the same image under the same id, so the newest payload is
/// the box the picture currently occupies; older ones are what it used to.
fn pictures(screen: &Screen) -> HashMap<u32, GraphicsPayload> {
    let mut latest = HashMap::new();
    for payload in screen.graphics().payloads() {
        if let Some(id) = payload.id() {
            latest.insert(id, payload.clone());
        }
    }
    latest
}

/// The nested window's placement box, in cells.
///
/// The pane is the root, and the root is placed at the pane's own corner, so the
/// picture that is not at the origin is the window inside it.
fn inner_cells(screen: &Screen) -> Option<(u16, u16)> {
    pictures(screen)
        .into_values()
        .filter(|payload| payload.at() != (0, 0))
        .max_by_key(|payload| payload.at())
        .and_then(|payload| payload.cells())
}

/// The top right corner of the nested window's ring: the highest `╮` on screen.
///
/// The dock draws a corner of its own at the bottom of the pane, and it is below
/// every window, so the topmost one belongs to the ring.
fn corner(screen: &Screen) -> Option<(u16, u16)> {
    screen
        .find_all("╮")
        .into_iter()
        .min_by_key(|&(row, _)| row)
}
