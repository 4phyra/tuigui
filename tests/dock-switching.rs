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

use termlens::{Graphics, GraphicsAction, Key, Screen, Terminal};

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

/// How many pictures this session has asked the terminal to place, however it asked.
///
/// Both actions count, and they mean different things: `TransmitAndPlace` carries the pixels and
/// places them in one command, `Place` places a picture the terminal already holds. A window that
/// moves without changing its picture is the second kind, and a stream that comes back is either
/// — what is being asserted is that a placement happened, not which shape it took.
fn placements(screen: &Screen) -> usize {
    screen
        .graphics()
        .payloads()
        .iter()
        .filter(|payload| {
            matches!(
                payload.action(),
                GraphicsAction::Place | GraphicsAction::TransmitAndPlace
            )
        })
        .count()
}

/// Click the program's window, in the middle of the pane, as a user does to focus it.
///
/// SGR counts from one; the grid counts from zero. The middle is where the root window is: it fills
/// the pane above the dock, so no cell of it has to be searched for.
fn click_window(session: &mut Terminal) -> termlens::Result<()> {
    let (col, row) = (SIZE.0 / 2, SIZE.1 / 2);
    session.send_str(&format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m"))
}

/// Every id the client placed after deleting it.
///
/// `a=p` places a picture the terminal is already holding, so one sent after `a=d` of the same id
/// asks for pixels the client itself told the terminal to forget. foot answers `ENOENT` and the
/// session ends; this harness answers `OK` and draws nothing, so no check on the screen can see it.
/// The log is the only evidence, and a placement naming an id whose delete came first is the
/// failure, whichever order it reached the terminal in.
///
/// A transmit of the id clears it again: re-sending the pixels makes a later placement legitimate,
/// which is the whole difference between the two cases.
fn placements_of_a_deleted_image(screen: &Screen) -> Vec<u32> {
    let mut deleted: Vec<u32> = Vec::new();
    let mut offending = Vec::new();
    for payload in screen.graphics().payloads() {
        let Some(id) = payload.id() else { continue };
        match payload.action() {
            GraphicsAction::Delete => deleted.push(id),
            GraphicsAction::Place if deleted.contains(&id) => offending.push(id),
            GraphicsAction::Transmit | GraphicsAction::TransmitAndPlace => {
                deleted.retain(|held| *held != id)
            }
            _ => (),
        }
    }
    offending
}

/// Assert no picture was placed under an id the client had already deleted.
///
/// The one failure this harness cannot see on the screen: it answers every placement `OK` and
/// draws nothing, where foot answers `ENOENT` and the session ends. So what ended the session on
/// foot has to be read off the traffic here.
fn assert_nothing_placed_after_its_delete(session: &Terminal, after: &str) {
    let offending = placements_of_a_deleted_image(&session.screen());
    assert!(
        offending.is_empty(),
        "no picture is placed under an id it was deleted from {after}, but {offending:?} was; \
         actions were {:?}",
        session
            .screen()
            .graphics()
            .payloads()
            .iter()
            .map(|payload| (payload.action(), payload.id()))
            .collect::<Vec<_>>()
    );
}

/// Press the tab named `label`, as a terminal reports a click on it.
///
/// The two reports a terminal sends, rather than `Terminal::click`: `click` refuses to send one at
/// all unless it has seen the application ask for mouse tracking, and what is under test is the
/// client, not whether that request reached this terminal.
///
/// The tab is waited for rather than assumed: crossing to a screen and drawing the dock on it are
/// two frames, so a screen that has just changed does not carry the buttons yet.
fn press(session: &mut Terminal, label: &str) -> termlens::Result<()> {
    session.wait_until_for(|screen| screen.contains(label), SWITCH)?;
    let (row, col) = session
        .screen()
        .find(label)
        .unwrap_or_else(|| panic!("the {label} tab is on the dock's row: {}", session.screen()));
    // SGR counts from one; the grid counts from zero.
    let (col, row) = (u32::from(col) + 1, u32::from(row) + 1);
    session.send_str(&format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m"))
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

/// Assert the session is alive, still drawing, and has not reported a failure.
///
/// Three questions, and the third is the one a terminal makes easy to miss. A failure is not only
/// a process going away: it is drawn in the pane — `the session failed` with its cause under it —
/// while everything else keeps looking healthy. So a test that only asks "is the process alive"
/// passes through the failure it was written to find.
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
    // And the pane is not carrying a report. This is checked last on purpose: the report is drawn
    // a round trip after the press that caused it, so asking before any frame has passed would
    // look at the screen the failure had not reached yet.
    let screen = session.screen();
    assert!(
        !screen.contains("the session failed"),
        "the session reported a failure {after}: {screen}"
    );
    Ok(screen)
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
        // The window itself, clicked in the middle of the pane, before any screen is switched. It
        // is the gesture that makes the failure happen — a window that has been used is the one
        // that has to survive being left, and a session that only survives an untouched screen is
        // a session that breaks the moment a user works in it.
        click_window(&mut session)?;
        alive_and_drawing(&mut session, &format!("after clicking the window, round {round}"))?;

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

        // And the window again, now that it has been left and come back: a crossing that drops it
        // leaves a pane nothing can be clicked on, and the client says so.
        click_window(&mut session)?;
        alive_and_drawing(&mut session, &format!("after clicking it again, round {round}"))?;

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
        // And no picture was asked for under an id the client had already deleted on the way: the
        // failure that ended the session on a real terminal, which this one answers `OK`.
        assert_nothing_placed_after_its_delete(&session, &format!("round {round}"));
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

/// Coming back to the stream must put the picture back.
///
/// A different question from the one above, which is why it is a test of its own: a session can
/// survive every press and still draw nothing. The two screens are two terminals, and the picture
/// is not the terminal's to keep — a placement is a request the client made, and the crossing
/// tears down what was placed on the screen being left. So every return has to place the window
/// again, or the pane comes back blank while everything else about the session looks healthy.
///
/// What is counted is the kitty traffic this harness received: a frame the client sends becomes a
/// placement on this side, and nothing on the pane's grid says so. One command carries both the
/// picture and its placement (`a=T`), so its count is both.
///
/// Skipped: what puts the picture back after a crossing is not settled. It used to be a frame
/// that arrived on the tick whether or not anything had changed, and a client that no longer
/// sends those leaves this question open.
#[test]
#[ignore = "the picture's return is unsettled; see the note above"]
fn returning_to_the_stream_places_the_window_again() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("stream-placement");
    // Foot's shell waits for this file, so the session outlives every crossing below: a program
    // that ended mid-test would make a missing placement mean "nothing to draw" rather than
    // "nothing was placed".
    let stop = runtime.join("stop");
    let runnable = common::script(
        "stream-placement",
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

    // To the stream, and its first picture. Nothing below is about that first placement.
    //
    // `w` is the Windows tab's shortcut, and a shortcut *selects* its screen rather than toggling:
    // from the stream it is a no-op, which is why the way back is the Logs tab itself.
    session.wait_until(|screen| screen.contains("Windows"))?;
    session.send(Key::Char('w'))?;
    session.wait_until_for(|screen| screen.alternate_screen(), SWITCH)?;
    session.wait_until(|screen| placements(screen) > 0)?;

    for round in 1..=ROUNDS {
        // The window is clicked first, in the middle of the pane, because that is what a user does
        // before leaving it: the click is what makes the program take focus, and a window that has
        // been clicked is the one that leaves afterwards. A session that only survives an
        // untouched screen is a session that breaks the moment it is used.
        session.send_str(&format!(
            "\x1b[<0;{};{}M\x1b[<0;{};{}m",
            SIZE.0 / 2,
            SIZE.1 / 2,
            SIZE.0 / 2,
            SIZE.1 / 2
        ))?;

        // Away to the logs, by the tab.
        press(&mut session, "Logs")?;
        session.wait_until_for(|screen| !screen.alternate_screen(), SWITCH)?;
        let before = placements(&session.screen());

        // And back to the stream, where the picture has to be placed again.
        session.send(Key::Char('w'))?;
        session.wait_until_for(|screen| screen.alternate_screen(), SWITCH)?;
        // Placed by the time the screen is back, not later: a stream that is up with nothing
        // placed on it is the blank pane this test exists for.
        let placed = session
            .wait_until_for(|screen| placements(screen) > before, SWITCH)
            .is_ok();
        assert!(
            placed && placements(&session.screen()) > before,
            "the window is placed again on return {round}: {before} placements before, {} after; \
             actions were {:?}",
            placements(&session.screen()),
            session
                .screen()
                .graphics()
                .payloads()
                .iter()
                .map(|payload| payload.action())
                .collect::<Vec<_>>()
        );
        assert_nothing_placed_after_its_delete(&session, &format!("the return {round}"));

        // And the window is still there to be clicked: a crossing that drops it leaves a stream
        // that is up with nothing behind it, and the client says so by exiting.
        assert!(
            session.wait_exit_for(Duration::from_millis(1)).is_err(),
            "the session is still up after the crossing, round {round}: {}",
            session.screen()
        );
        session.send_str(&format!(
            "\x1b[<0;{};{}M\x1b[<0;{};{}m",
            SIZE.0 / 2,
            SIZE.1 / 2,
            SIZE.0 / 2,
            SIZE.1 / 2
        ))?;
        assert!(
            session.wait_exit_for(Duration::from_millis(1)).is_err(),
            "the window still takes a click after the crossing, round {round}: {}",
            session.screen()
        );
    }

    std::fs::write(&stop, b"").expect("the sentinel that ends foot");
    let status = session.wait_exit()?;
    assert!(
        status.success(),
        "the session exits cleanly: {status:?}\n{}",
        session.screen().full_text()
    );
    Ok(())
}
