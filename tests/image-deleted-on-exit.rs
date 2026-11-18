//! A session that ends must tell the terminal to forget its picture.
//!
//! The terminal owns an image once it has been transmitted: nothing about the process going
//! away frees it, and the picture stays where it was over whatever is drawn next. So the end
//! of a session has to say so, and the only side that can is the server — it is the one
//! holding the image ids.
//!
//! Two halves started by hand rather than by `tuigui`, because the failure this exists for is
//! one `tuigui` does not have: `tuigui` starts the client inside its own pty and waits for it,
//! while `tui` running `gui` is a server that survives a client going away. What is asserted is
//! that the delete reaches the terminal, which is the thing a user sees as a stale picture.
//!
//! Foot's command is given a second of life, so the picture is up and placed before anything
//! ends: a session that is over before its first frame has nothing to delete.

#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use termlens::{Graphics, GraphicsAction, Terminal};

/// The pane the whole session is measured in.
const SIZE: (u16, u16) = (100, 40);

/// How long a delete is given to arrive once the client has gone.
///
/// The escape is written on the way out, so it is a write and not a wait. A session that does
/// not send one never will, and this is what turns that into a failure rather than a timeout.
const DELETE: Duration = Duration::from_secs(10);

/// A binary of this workspace, beside the `tuigui` this test was built with.
///
/// `CARGO_BIN_EXE_*` is only defined for the package under test, and these two belong to other
/// packages, so the path is derived rather than handed over. A missing one is a build that has
/// not produced it, which is said here instead of as a session that cannot start.
fn half(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_tuigui")).with_file_name(name);
    assert!(
        path.exists(),
        "{} is not beside tuigui; build the binaries first",
        path.display()
    );
    path
}

#[test]
fn a_client_going_away_deletes_its_picture() -> termlens::Result<()> {
    common::require_foot();
    let runtime = common::runtime_dir("image-deleted-on-exit");
    let runnable = common::script(
        "image-deleted-on-exit",
        "exec foot -- sh -c 'printf \"streaming\\n\"; sleep 1'",
    );

    // `tui` runs its command in a pty and keeps serving; `gui` is that command, and it is what
    // draws into the pane. When foot's second is up, foot ends, `gui` ends with it, and `tui`
    // is left holding a session whose client is gone — which is the moment the picture has to
    // be removed.
    let mut session = Terminal::builder()
        .size(SIZE.0, SIZE.1)
        .graphics(Graphics::Kitty)
        .cell_size(8, 16)
        .env("XDG_RUNTIME_DIR", &runtime)
        .timeout(Duration::from_secs(60))
        .arg(half("gui"))
        .arg("--channel")
        .arg("unix")
        .arg(&runnable)
        .spawn(half("tui"))?;

    // The picture is up first, or a missing delete could mean "there was nothing to delete".
    session.wait_until(|screen| screen.graphics().total() > 0)?;

    // Waited for rather than read after the exit: the escape has to be written by the session's
    // own teardown, so it arrives while the session is ending and not as a last word from the
    // shell. A terminal that only looked afterwards could not tell the two apart.
    session.wait_until_for(
        |screen| {
            screen
                .graphics()
                .payloads()
                .iter()
                .any(|payload| payload.action() == GraphicsAction::Delete)
        },
        DELETE,
    )?;

    let screen = session.screen();
    let graphics = screen.graphics();
    let actions: Vec<_> = graphics
        .payloads()
        .iter()
        .map(|payload| payload.action())
        .collect();
    assert!(
        actions.contains(&GraphicsAction::Delete),
        "the picture is deleted when the client goes: {actions:?}"
    );
    // The image is named, not left to the terminal's default: a delete with no `d` key removes
    // whatever is on screen, which is not the same statement as "remove this one".
    let named = graphics.payloads().iter().any(|payload| {
        payload.action() == GraphicsAction::Delete && payload.id().is_some()
    });
    assert!(named, "the delete names the image it removes");

    Ok(())
}
