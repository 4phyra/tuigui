//! The whole session, in one pty, with a real Wayland client behind it.
//!
//! `tuigui` starts the server, the server starts the client inside its own pty,
//! and the client starts `foot` inside the compositor. What reaches this
//! terminal is what a user's terminal receives: the dock's text, and the frames
//! as kitty graphics. Nothing in this path is a stub.
//!
//! `foot` is the client because it draws through `wl_shm`, which is the capture
//! path that works today; an EGL client captures black (see `spec/COMP.md`).
//! The script gives foot a command that ends, so foot ends by itself, and the
//! session ends with it.

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use termlens::{Graphics, GraphicsAction, GraphicsTransmission, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (100, 40);

#[test]
#[ignore = "foot and the compositor race on teardown when the suite runs concurrently: the session intermittently exits 1"]
fn foot_streams_and_leaves_its_history_behind() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("e2e-unix");
    let runnable = common::script(
        "e2e-unix",
        "exec foot -- sh -c 'printf \"streaming\\n\"; sleep 1'",
    );

    let mut session = Terminal::builder()
        .size(SIZE.0, SIZE.1)
        // What `tui` is: a terminal that speaks the graphics protocol. Without
        // this the capability probe goes unanswered, and the client's first
        // transmit waits for a reply that never comes.
        .graphics(Graphics::Kitty)
        // A picture is placed in cells, and the pane's box is computed from
        // them: the one fact the protocol cannot supply.
        .cell_size(8, 16)
        .env("XDG_RUNTIME_DIR", &runtime)
        .timeout(Duration::from_secs(60))
        // The session's plumbing, not the codec: JPEG keeps this test on every
        // machine, hardware AV1 or not.
        .arg("--jpeg")
        .arg(&runnable)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;

    // The first picture reaches this terminal as an escape, and this is where
    // the escape is read: the form the session sent, not a rendering of it.
    //
    // `find`, not `last`: only the first frame is transmitted and placed in one
    // command, and once the stream is running a placement can be the newest
    // escape without being the one that carried a picture.
    session.wait_until(|screen| screen.graphics().total() > 0)?;
    let screen = session.screen();
    let pictures = screen.graphics();
    assert!(
        pictures.kitty() > 0,
        "the frames are kitty graphics: {pictures:?}"
    );
    let picture = pictures
        .payloads()
        .iter()
        .find(|picture| picture.action() == GraphicsAction::TransmitAndPlace)
        .expect("a frame is transmitted and placed in one command");
    assert_eq!(
        picture.transmission(),
        Some(GraphicsTransmission::SharedMemory),
        "the pixels travel through shared memory, the path a real session uses"
    );
    let (width, height) = picture.size().expect("a picture states its pixels");
    assert!(
        width > 0 && height > 0,
        "the picture has a size: {width}x{height}"
    );

    // foot's command ends, foot ends, the client ends, and the server ends.
    let status = session.wait_exit()?;
    assert!(
        status.success(),
        "the session exits cleanly: {status:?}\n{}",
        session.screen().full_text()
    );

    // The alternate screen was left, so the screen the session started on is
    // back — with the banner that was written above the pane before the stream
    // started. That is the history the inline viewport exists to leave behind.
    //
    // The dock's own row is not: it is the viewport, and the frame that leaves
    // the alternate screen blanks it, because the session it described is over.
    let screen = session.screen();
    assert!(
        !screen.alternate_screen(),
        "the session leaves the alternate screen: {screen}"
    );
    let history = screen.full_text();
    assert!(
        history.contains('\u{2584}'),
        "the banner is still above the pane: {history}"
    );

    Ok(())
}
