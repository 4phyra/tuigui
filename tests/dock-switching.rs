//! Switching the dock's screens must not end the session.
//!
//! Both screens are reached from the dock: a press on a tab, and a press of the key the tab
//! advertises. The mouse press and the program's own input arrive on the same descriptor, so a
//! press the dock reads as the close — or a press it takes and then wedges on — is a session that
//! stops answering while the user is still switching. That is what this test is for: five times
//! over, it walks the two screens through both routes and checks, after every press, that the
//! session is both alive and still drawing.
//!
//! # Why the screen alone is not the assertion
//!
//! Leaving the alternate screen is what a session does when it *ends*, and also what it does when
//! the user asks for the logs: a test that waited for the logs screen and stopped there would be
//! satisfied by the failure it exists to catch. A frozen session is the same trap one step on — it
//! shows the screen it froze on, so any predicate about what is drawn still holds.
//!
//! So each press is followed by two questions, in this order. First: has the screen done what the
//! press asked for, or has the session gone? Second: is the process still there, and still
//! drawing? The order matters — a session that died has left the alternate screen too, so the
//! first question alone cannot tell the two apart, and the second is what does.

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use termlens::{Graphics, Key, Screen, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (100, 40);

/// How many times the pair of screens is walked.
///
/// More than once because the first press is not the one that breaks: a session that wedges on the
/// second visit is one whose state was left behind by the first, and a single round would miss it.
const ROUNDS: usize = 5;

/// How long a press is given to reach the screen it asked for.
///
/// A press is read on the frame it arrives on and the crossing is two frames after it, so anything
/// still running has done it well inside this. A session that wedged never will.
const SWITCH: Duration = Duration::from_secs(10);

/// How long the session is given to draw another frame before it is called frozen.
///
/// The dock's indicator tick is a quarter of a second, so a running session has drawn several
/// frames well inside this.
const STILL: Duration = Duration::from_secs(5);

/// The glyph the dock's light is drawn as.
///
/// The only cell on the dock's row whose colour changes from frame to frame, which is what makes
/// it the evidence that frames are still being drawn.
const LIGHT: &str = "•";

/// Press the tab named `label`, as a terminal reports a click on it.
///
/// The two reports a terminal sends, rather than `Terminal::click`: `click` refuses to send one at
/// all unless it has seen the application ask for mouse tracking, and what is under test is the
/// client, not whether that request reached this terminal.
fn press(session: &mut Terminal, label: &str) -> termlens::Result<()> {
    let (row, col) = screen_find(session, label);
    // SGR counts from one; the grid counts from zero.
    let (col, row) = (u32::from(col) + 1, u32::from(row) + 1);
    session.send_str(&format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m"))
}

/// The row and column of `label` on the screen the session is showing.
fn screen_find(session: &Terminal, label: &str) -> (u16, u16) {
    session
        .screen()
        .find(label)
        .unwrap_or_else(|| panic!("the {label} tab is on the dock's row: {}", session.screen()))
}

/// The dock's light: the cell its dot is drawn in, as a colour.
///
/// Found by the glyph rather than by column, because the dock is anchored to the bottom on one
/// screen and to the cursor on the other: neither its row nor its column is a constant.
fn light(screen: &Screen) -> Option<String> {
    let (row, col) = screen.find(LIGHT)?;
    screen
        .cell(row, col)
        .map(|cell| format!("{:?}", cell.style()))
}

/// Assert the session is alive and still drawing, and return the screen it is on.
///
/// Alive first, because it is the question the screen cannot answer: a session that died has left
/// the alternate screen and blanked the dock, which is also what a session that switched screens
/// looks like. Then drawing, because a client wedged on a terminal query is alive and holds the
/// screen it wedged on — its light stops breathing, and that is the only trace it leaves.
fn alive_and_drawing(session: &mut Terminal, after: &str) -> termlens::Result<Screen> {
    assert!(
        session.wait_exit_for(Duration::from_millis(1)).is_err(),
        "the session is up {after}: {}",
        session.screen()
    );

    let before = light(&session.screen());
    assert!(
        session
            .wait_until_for(|screen| light(screen) != before, STILL)
            .is_ok(),
        "the session is still drawing {after}; its light has stopped breathing: {}",
        session.screen()
    );
    Ok(session.screen())
}

#[test]
fn switching_screens_never_ends_the_session() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("dock-switching");
    // Foot's shell waits for this file, so the session outlives every press below: without it the
    // program would finish on its own and the test could not tell an exit it caused from one the
    // program did.
    let stop = runtime.join("stop");
    let runnable = common::script(
        "dock-switching",
        &format!(
            "exec foot -- sh -c 'printf \"streaming\\n\"; \
             while [ ! -e {} ]; do sleep 0.05; done'",
            stop.display()
        ),
    );

    let mut session = Terminal::builder()
        .size(SIZE.0, SIZE.1)
        // The session hands this terminal the graphics protocol; without it the client's first
        // transmit waits for a reply that never comes.
        .graphics(Graphics::Kitty)
        .cell_size(8, 16)
        .env("XDG_RUNTIME_DIR", &runtime)
        .timeout(Duration::from_secs(60))
        .arg(&runnable)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;

    // Foot is on the pane when its first frame is, and both buttons are on the dock from the
    // first frame: this is the moment they can be pressed.
    session.wait_until(|screen| screen.graphics().total() > 0)?;
    session.wait_until(|screen| screen.contains("Logs") && screen.contains("Windows"))?;

    // The stream first, and not for its own sake: the client asks for the pointer only while it is
    // on the alternate screen, so a press is one the terminal reports only from there.
    session.send(Key::Char('w'))?;
    session.wait_until(|screen| screen.alternate_screen())?;
    alive_and_drawing(&mut session, "after w put the stream up")?;

    for round in 1..=ROUNDS {
        // The logs screen, by the pointer: it is the tab that is pressed, which is the route a
        // user takes with the mouse the dock asks for.
        press(&mut session, "Logs")?;
        session.wait_until_for(|screen| !screen.alternate_screen(), SWITCH)?;
        alive_and_drawing(&mut session, &format!("after the press on Logs, round {round}"))?;
        assert!(
            session.screen().contains("Logs"),
            "it is on the logs screen, round {round}: {}",
            session.screen()
        );

        // The program's screen, by the shortcut the Windows tab advertises.
        session.send(Key::Char('w'))?;
        session.wait_until_for(|screen| screen.alternate_screen(), SWITCH)?;
        alive_and_drawing(&mut session, &format!("after w put the stream up, round {round}"))?;

        // And the logs again, by the tab: a press the dock reads as the close is a press that ends
        // the session, and this is where that shows up.
        press(&mut session, "Logs")?;
        session.wait_until_for(|screen| !screen.alternate_screen(), SWITCH)?;
        alive_and_drawing(&mut session, &format!("after the press on Logs again, round {round}"))?;
        assert!(
            session.screen().contains("Logs"),
            "it is on the logs screen, round {round}: {}",
            session.screen()
        );

        // Back to the stream for the next round, which is the screen a press needs.
        session.send(Key::Char('w'))?;
        session.wait_until_for(|screen| screen.alternate_screen(), SWITCH)?;
        alive_and_drawing(&mut session, &format!("after w put the stream up, round {round}"))?;
    }

    // The program ends, and the session with it.
    std::fs::write(&stop, b"").expect("the sentinel that ends foot");
    let status = session.wait_exit()?;
    assert!(
        status.success(),
        "the session exits cleanly: {status:?}\n{}",
        session.screen().full_text()
    );
    // The terminal is given back on the way out: the alternate screen is left, and the banner
    // written above the pane before the stream started is still on the screen it returns to.
    assert!(
        !session.screen().alternate_screen(),
        "the session leaves the alternate screen: {}",
        session.screen()
    );

    Ok(())
}
