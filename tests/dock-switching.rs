//! Switching the dock's screens must not end the session.
//!
//! Both screens are reached from the dock: a press on a tab, and a press of the key the tab
//! advertises. The mouse press and the program's own input arrive on the same descriptor, so a
//! press the dock reads as the close — or a press it takes and then fails to draw — is a session
//! that ends while the user is still switching. That is what this test is for: five times over,
//! it walks Logs then Windows through both routes and asserts the session is still up after every
//! single press.
//!
//! Being still up is what has to be asserted, and a session that died is only visible from
//! outside it: `wait_exit_for` answers with the status the moment the process is gone, so a few
//! milliseconds is the whole window a dead session needs to give itself away.

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use termlens::{Graphics, Key, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (100, 40);

/// How many times the pair of screens is walked.
///
/// More than once because the first press is not the one that breaks: a session that dies on the
/// second visit is one whose state was left behind by the first, and a single round would miss it.
const ROUNDS: usize = 5;

/// Assert the session is still running.
///
/// A press that ends it ends it on the frame it is read on, so the status is available by the
/// time the press has been written. `wait_exit_for` is asked with the shortest wait it accepts:
/// this is a question about now, not a wait for the future.
fn still_up(session: &mut Terminal, after: &str) -> termlens::Result<()> {
    assert!(
        session.wait_exit_for(Duration::from_millis(1)).is_err(),
        "the session is still up {after}: {}",
        session.screen()
    );
    Ok(())
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

    // Foot is on the pane when its first frame is, and the two buttons are on the dock from the
    // first frame: this is the moment both can be pressed.
    session.wait_until(|screen| screen.graphics().total() > 0)?;
    session.wait_until(|screen| screen.contains("Logs") && screen.contains("Windows"))?;

    // Where the logs tab is, read once. The dock is the bottom three rows of both screens, so the
    // tab is at the same cell whichever one is up.
    let (tab_row, tab_col) = session
        .screen()
        .find("Logs")
        .expect("the logs tab is on the dock's row");

    // The stream first, and not for its own sake: the client asks for the pointer only while it is
    // on the alternate screen, so a press is one the terminal reports only from there.
    session.send(Key::Char('w'))?;
    session.wait_until(|screen| screen.alternate_screen())?;
    still_up(&mut session, "after w put the stream up")?;

    for round in 1..=ROUNDS {
        // The logs screen, by the pointer: it is the tab that is pressed, which is the route a
        // user takes with the mouse the dock asks for. Written as the two reports a terminal
        // sends, press and release, rather than through `click`: `click` refuses to send one at
        // all unless it has seen the application ask for tracking, and what is being tested here
        // is the client, not whether the request reached this terminal.
        session.send_str(&format!(
            "\x1b[<0;{};{}M\x1b[<0;{};{}m",
            u32::from(tab_col) + 1,
            u32::from(tab_row) + 1,
            u32::from(tab_col) + 1,
            u32::from(tab_row) + 1
        ))?;
        session.wait_until(|screen| !screen.alternate_screen())?;
        still_up(&mut session, &format!("after the press on Logs, round {round}"))?;

        // The program's screen, by the shortcut the Windows tab advertises.
        session.send(Key::Char('w'))?;
        session.wait_until(|screen| screen.alternate_screen())?;
        still_up(&mut session, &format!("after w put the stream up, round {round}"))?;

        // And the logs again, by the tab: a press the dock reads as the close is a press that ends
        // the session, and this is where that shows up.
        session.send_str(&format!(
            "\x1b[<0;{};{}M\x1b[<0;{};{}m",
            u32::from(tab_col) + 1,
            u32::from(tab_row) + 1,
            u32::from(tab_col) + 1,
            u32::from(tab_row) + 1
        ))?;
        session.wait_until(|screen| !screen.alternate_screen())?;
        assert!(
            session.screen().contains("Logs"),
            "the dock is drawn on the screen the session crossed to, round {round}: {}",
            session.screen()
        );
        still_up(&mut session, &format!("after the press on Logs again, round {round}"))?;

        // Back to the stream for the next round, which is the screen a click needs.
        session.send(Key::Char('w'))?;
        session.wait_until(|screen| screen.alternate_screen())?;
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
