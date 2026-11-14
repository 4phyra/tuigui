//! What the component tests share: one widget's pane, and where it drew.

#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use termlens::Terminal;

/// The pane each one is given.
///
/// The size the layout is written for: the widget is centred, so a different
/// pane would move it and change every snapshot for a reason that is not the
/// behaviour under test.
pub const SIZE: (u16, u16) = (80, 24);

/// The path of `examples/pane`, the program that draws one widget.
///
/// Cargo sets no `CARGO_BIN_EXE_` for an example, so the path comes from the
/// test binary's own: it runs from `target/<profile>/deps/<test>`, and the
/// example is in `target/<profile>/examples/pane` beside that `deps`.
fn example() -> PathBuf {
    let mut path = std::env::current_exe().expect("the test binary has a path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("examples");
    path.push("pane");
    path
}

/// The pane that draws just `component`, in a pty.
///
/// One widget and one line under it, not the gallery: a snapshot of this pane
/// changes when this component changed, and nothing else on the screen can
/// change it.
pub fn pane(component: &str) -> termlens::Result<Terminal> {
    Terminal::builder()
        .size(SIZE.0, SIZE.1)
        .env_clear()
        .timeout(Duration::from_secs(5))
        .env("TERM", "xterm-256color")
        .args([component])
        .spawn(example())
}

/// Where `needle` is drawn, as `(row, column)` — the order `Screen::find`
/// reports.
///
/// A missing needle fails with the screen it was missing from rather than as
/// a click at a wrong place.
pub fn at(terminal: &Terminal, needle: &str) -> (u16, u16) {
    match terminal.screen().find(needle) {
        Some(at) => at,
        None => panic!(
            "the pane does not draw {needle:?}: {}",
            terminal.screen().text()
        ),
    }
}

/// Press the primary button down on `(row, column)` and leave it down.
///
/// `Terminal::click` is the whole gesture — a press and its release in one
/// write — so a widget that changes while it is held is back at rest before
/// the screen is read. A press with no release has no termlens call of its
/// own, which is why this one writes the report itself.
pub fn press(terminal: &mut Terminal, row: u16, column: u16) -> termlens::Result<()> {
    mouse(terminal, row, column, 0, false)
}

/// Let the primary button go at `(row, column)`: the cell it was pressed on.
///
/// The release belongs to the press still in flight, so it goes where that
/// press went — not necessarily where the needle it was aimed at is drawn
/// now.
pub fn let_go(terminal: &mut Terminal, row: u16, column: u16) -> termlens::Result<()> {
    mouse(terminal, row, column, 0, true)
}
/// A bare SGR mouse report at `(row, column)`.
///
/// SGR is what the pane asked for by enabling mouse capture, and a press with
/// no release or a motion with no button has no termlens call of its own. SGR
/// counts coordinates from one; the tests count from zero.
fn mouse(
    terminal: &mut Terminal,
    row: u16,
    column: u16,
    code: u8,
    release: bool,
) -> termlens::Result<()> {
    terminal.send_str(&format!(
        "\x1b[<{code};{};{}{}",
        u32::from(column) + 1,
        u32::from(row) + 1,
        match release {
            true => 'm',
            false => 'M',
        },
    ))
}
