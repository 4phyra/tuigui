//! A session whose client failed must end, rather than leave a process waiting.
//!
//! The failure path is the one that hung. `gui::run` returns an error, `main` returned it, and
//! returning from `main` drops the tokio runtime — whose drop waits for the blocking stdin read
//! parked on the pty this process was started in. The master of that pty is held by the server on
//! the other side, so the read never ends, the client stays alive forever, and the server waits
//! for a child that is waiting for a reader. Eleven of those were left behind by one afternoon of
//! test runs, each one `S` in `futex_do_wait`.
//!
//! What is asserted is only that the session ends. It says nothing about the report's wording or
//! where it lands. The program does not exist, so the client's very first step — starting it —
//! fails, and fails at once: no graphics protocol, no window and no timing involved.
//!
//! The fix is killed by putting `gui.run(...).await?` back in place of the match in `src/main.rs`:
//! this then hangs until the harness deadline instead of failing.

#![cfg(target_os = "linux")]

mod common;

use std::time::Duration;

use termlens::Terminal;

/// How long the session is given to end.
///
/// A session that is going to end does it in milliseconds. The failure this test exists for is one
/// that never ends at all, so the deadline only has to be longer than a clean shutdown.
const EXIT: Duration = Duration::from_secs(20);

#[test]
fn a_failed_session_ends() -> termlens::Result<()> {
    let runtime = common::runtime_dir("failed-session-ends");
    // A program that is not there. The client starts it before anything is drawn, so this is the
    // earliest failure a session can have and the one that needs no cooperation from the program.
    let missing = runtime.join("no-such-program");

    let mut session = Terminal::builder()
        .size(100, 40)
        .env("XDG_RUNTIME_DIR", &runtime)
        .timeout(Duration::from_secs(60))
        .arg(&missing)
        .spawn(env!("CARGO_BIN_EXE_tuigui"))?;

    let status = session.wait_exit_for(EXIT)?;
    assert!(
        !status.success(),
        "a session whose program could not start exits with a failure: {status:?}\n{}",
        session.screen().full_text()
    );
    Ok(())
}
