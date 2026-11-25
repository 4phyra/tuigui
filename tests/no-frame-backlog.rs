//! Frames do not pile up when the TUI is slower than the GUI.
//!
//! The producer and the consumer are two processes joined by a socket, so nothing makes
//! them run at the same speed. When the consumer is the slower one, every frame it has
//! not read yet is waiting somewhere — in the kernel's socket buffer and in the reader in
//! front of the codec — and those frames are drawn after the program that produced them
//! has stopped. What a user sees is a pane that keeps scrolling and redrawing a program
//! that is already gone.
//!
//! The gap is made unmissable rather than tuned: a `foot` that scrolls a line at ~20 fps
//! and a TUI told to spend a second on every frame it handles. What is asserted is the
//! end of the session — the program is stopped, and the pictures presented after that
//! are the backlog.
//!
//! The deadline is an assertion, not a sleep. A session that ends is expected to end
//! within the time a correct implementation spends on at most one frame in flight; a
//! session that takes seconds to notice is spending them on queued ones.

#![cfg(target_os = "linux")]

mod common;

use std::time::{Duration, Instant};

use termlens::{Graphics, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (100, 40);

/// How long the TUI spends on every frame it handles.
///
/// The producer scrolls at ~20 fps, so this makes the consumer twenty times slower than
/// the stream it is reading.
const LAG_MS: u64 = 1000;

/// The frames that prove the consumer is behind rather than stuck.
///
/// At one second per frame these take seconds to show, which is the point: the queue is
/// already forming when the stop is written.
const PROOF: u32 = 3;

/// How long the session is given to end once the program stops drawing.
///
/// A correct implementation draws nothing after the stop, because nothing is produced
/// and at most the frame already in hand is left to place. A backlog is drained at one
/// frame per lag period, and the producer stays alive that long, so the deadline only
/// has to be longer than a drained queue.
const EXIT: Duration = Duration::from_secs(20);

#[test]
fn stale_frames_are_not_drawn_after_the_program_stops() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("no-frame-backlog");
    // The producer runs until this file exists, and then stops drawing while staying
    // alive: a program that exits would take the session with it, and a session that is
    // over has no frames left to present. The window outliving its own drawn output is
    // the case where a backlog shows: the screen keeps changing with nothing producing.
    let stop = runtime.join("stop");

    // A line per turn: the screen scrolls, so every turn is a new frame rather than a
    // repeated one the client could recognise.
    let runnable = common::script(
        "no-frame-backlog",
        &format!(
            "exec foot -- sh -c 'i=0; \
             while [ ! -e {} ]; do printf \"line %s\\n\" \"$i\"; i=$((i+1)); \
             sleep 0.05; done; sleep 4'",
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
        // machine, hardware AV1 or not.
        .arg("--jpeg")
        .arg("--dbg-frame-lag-ms")
        .arg(LAG_MS.to_string())
        .arg(&runnable)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;

    // A picture first: a session that never draws has no backlog to read.
    session.wait_until(|screen| screen.graphics().total() > 0)?;
    // And enough of them that the consumer is provably slower than the producer: with
    // one frame in flight this wait could not pass, because each costs a lag period.
    session.wait_until_for(
        |screen| screen.graphics().total() >= PROOF,
        Duration::from_secs(15),
    )?;

    // The program stops drawing, and everything after this is what the TUI had already
    // queued. The program is still alive, so the session is there to draw them.
    std::fs::write(&stop, b"").expect("the stop file");
    let before = session.screen().graphics().total();
    let started = Instant::now();

    let status = match session.wait_exit_for(EXIT) {
        Ok(status) => status,
        Err(error) => panic!(
            "the session was still running {:.1}s after the program was stopped: {error}\n\
             {} pictures before, {} after\n{}",
            started.elapsed().as_secs_f64(),
            before,
            session.screen().graphics().total(),
            session.screen().full_text()
        ),
    };

    let after = session.screen().graphics().total();
    // A few frames can arrive while the stop is being noticed — one in flight, and one
    // the client had already written. A backlog is many.
    assert!(
        after.saturating_sub(before) <= 3,
        "stale frames were presented after the program stopped: {before} before, \
         {after} after, {:.1}s elapsed\n{}",
        started.elapsed().as_secs_f64(),
        session.screen().full_text()
    );
    assert!(
        status.success(),
        "the session ends with a success: {status:?}\n{}",
        session.screen().full_text()
    );
    Ok(())
}
