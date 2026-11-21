//! Resizing the pane must not end the session.
//!
//! The logs screen is drawn on an inline viewport, three rows anchored to the cursor. Keeping
//! that viewport in place is measured, and the measurement starts by asking the backend where the
//! cursor is: `Terminal::draw` calls `autoresize`, `autoresize` calls `resize` when the terminal's
//! size moved, and `resize` calls `compute_inline_size`, whose first act is
//! `backend.get_cursor_position()`.
//!
//! `CrosstermBackend` answers that by writing `ESC [ 6n` and reading the reply off **stdin**,
//! with a two-second timeout. This process's own draw loop is reading stdin too — that is where
//! its keys come from — so the reply is as likely to be consumed there as by crossterm, and the
//! call fails with "The cursor position could not be read within a normal duration". The session
//! ends on a terminal query, for a user who only resized their window.
//!
//! # Why a resize rather than anything else
//!
//! The query only happens when the size changed, so a session that is merely drawn, or merely
//! printed into, never asks. A resize is what makes the question get asked at all, which is why
//! every other test in this suite passes while a real user resizing their terminal does not.

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use termlens::{Graphics, Key, Terminal};

/// The pane the session starts in.
const SIZE: (u16, u16) = (100, 40);

/// How long a crossing to the stream is given.
const SWITCH: Duration = Duration::from_secs(10);

/// How long the session is given to draw again before it is called frozen.
const STILL: Duration = Duration::from_secs(5);

/// The glyph the dock's light is drawn as: the only cell whose colour changes per frame, so it is
/// what proves frames are still being drawn.
const LIGHT: &str = "•";

/// The dock's light, as a colour.
fn light(screen: &termlens::Screen) -> Option<String> {
    let (row, col) = screen.find(LIGHT)?;
    screen
        .cell(row, col)
        .map(|cell| format!("{:?}", cell.style()))
}

/// Press the tab named `label`, as a terminal reports a click on it.
///
/// The two reports a terminal sends, rather than `Terminal::click`: `click` refuses to send one
/// at all unless it has seen the application ask for mouse tracking, and what is under test is
/// the client, not whether that request reached this terminal.
fn press(session: &mut Terminal, label: &str) -> termlens::Result<()> {
    session.wait_until_for(|screen| screen.contains(label), SWITCH)?;
    let (row, col) = session
        .screen()
        .find(label)
        .unwrap_or_else(|| panic!("the {label} tab is on the dock's row: {}", session.screen()));
    let (col, row) = (u32::from(col) + 1, u32::from(row) + 1);
    session.send_str(&format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m"))
}

#[test]
fn resizing_the_pane_keeps_the_session() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("resize-keeps-session");
    let stop = runtime.join("stop");
    let runnable = common::script(
        "resize-keeps-session",
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
        .arg(&runnable)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;

    // The logs screen first: the inline viewport is the one that measures itself.
    session.wait_until(|screen| screen.contains("Windows"))?;
    // And the stream, so both terminals have drawn at least once before the size moves.
    session.send(Key::Char('w'))?;
    session.wait_until_for(|screen| screen.alternate_screen(), SWITCH)?;
    // Back to the logs, which is the inline one, and the screen a resize is measured on. The
    // Logs tab has no shortcut — `w` only ever selects the Windows one — so this is a press on
    // the tab itself, as a user would make it.
    press(&mut session, "Logs")?;
    session.wait_until_for(|screen| !screen.alternate_screen(), SWITCH)?;

    // The whole point: the terminal changes size under the session.
    session.resize(SIZE.0 - 10, SIZE.1)?;

    // A session that survived is still up and still drawing. Alive first, because a session that
    // died has left the alternate screen and blanked the dock, which a frozen session also does.
    assert!(
        session.wait_exit_for(Duration::from_millis(1)).is_err(),
        "the session is up after the resize: {}",
        session.screen()
    );
    let before = light(&session.screen());
    assert!(
        session
            .wait_until_for(|screen| light(screen) != before, STILL)
            .is_ok(),
        "the session is still drawing after the resize; its light has stopped breathing: {}",
        session.screen()
    );
    let screen = session.screen();
    assert!(
        !screen.contains("cursor position could not be read"),
        "the pane in place is not asking stdin for a cursor it cannot get: {screen}"
    );

    std::fs::write(&stop, b"").expect("the sentinel that ends the script");
    let status = session.wait_exit()?;
    assert!(
        status.success(),
        "the session exits cleanly: {status:?}\n{}",
        session.screen().full_text()
    );

    Ok(())
}
