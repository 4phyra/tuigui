//! A window inside a window is two pictures on the wire, not one.
//!
//! The pane transmits each window's frame as its own kitty image, under an id
//! derived from that window's surface. Two windows are therefore two ids, and two
//! ids that appear and stay is what says both are being drawn rather than one
//! having replaced the other.
//!
//! This is the same nested `foot` the phantom-cells test runs, read sooner: it
//! waits for the second id and says what it found. It repeats, because the two
//! windows arrive on separate streams and a session has a narrow window in which
//! the inner one has announced itself but not yet committed a frame; the repeat is
//! what turns "it happened to be there" into "it is there".

#![cfg(target_os = "linux")]

mod common;

use std::collections::HashSet;
use std::time::Duration;
use termlens::{Graphics, GraphicsAction, Screen, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (80, 24);

/// How many times the session is run. Each run is a fresh process tree, so a
/// failure here is a fact about the nesting and not about one build's timing.
///
/// Five, because that is the repeat that fits a suite people run per change. A
/// longer run — twenty — is how the teardown race was found rather than a rate to
/// pay every time: it fails about once in twenty sessions with "the close request
/// was dropped unread", which is the client's close arm meeting a channel whose
/// sender has already gone. That is a bug of its own, not this test's to prove.
const ROUNDS: usize = 5;

#[test]
fn nested_foot_streams_two_kitty_images() -> termlens::Result<()> {
    common::require_foot();
    for round in 0..ROUNDS {
        run_round(round)?;
    }
    Ok(())
}

fn run_round(round: usize) -> termlens::Result<()> {
    let runtime = common::runtime_dir(&format!("nested-two-streams-{round}"));
    // The inner program waits for this file, so the session ends by the programs
    // finishing rather than by a signal in the middle of a frame.
    let stop = runtime.join("stop");
    let inner = common::script(
        &format!("nested-two-streams-inner-{round}"),
        &format!(
            "exec foot -- sh -c 'printf \"nested\\n\"; \
             while [ ! -e {} ]; do sleep 0.05; done'",
            stop.display()
        ),
    );
    let runnable = common::script(
        &format!("nested-two-streams-{round}"),
        &format!("exec foot -- {}", inner.display()),
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
        .arg(&runnable)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;

    // Two windows, so two ids. An id is derived from the surface a frame belongs
    // to, which is unlike anything else either window draws.
    session.wait_until(|screen| ids(screen).len() >= 2)?;

    let screen = session.screen();
    let found = ids(&screen);
    assert_eq!(
        found.len(),
        2,
        "the session draws exactly two pictures, round {round}: {found:?}\n{screen}"
    );
    // Each is placed, not merely transmitted: a picture that never reaches a
    // placement is one the pane holds and no window shows. The newest payload for
    // each id says what happened to that image last — a frame carrying pixels
    // transmits and places in one command, and a frame that only moved the window
    // places the picture it already has.
    let pictures = screen.graphics();
    for id in &found {
        let payload = pictures
            .payloads()
            .iter()
            .filter(|payload| payload.id() == Some(*id))
            .next_back()
            .expect("an id came from a payload");
        assert!(
            matches!(
                payload.action(),
                GraphicsAction::Place | GraphicsAction::TransmitAndPlace
            ),
            "the newest command for {id} places its picture, round {round}: {payload:?}"
        );
        assert!(
            payload.cells().is_some(),
            "the picture is placed in a box of cells, round {round}: {payload:?}"
        );
    }

    std::fs::write(&stop, b"").expect("the sentinel that ends the inner foot");
    let status = session.wait_exit()?;
    assert!(
        status.success(),
        "the session exits cleanly, round {round}: {status:?}\n{}",
        session.screen().full_text()
    );
    Ok(())
}

/// The image ids the session has transmitted.
fn ids(screen: &Screen) -> HashSet<u32> {
    screen
        .graphics()
        .payloads()
        .iter()
        .filter_map(|payload| payload.id())
        .collect()
}
