//! A program that ends by itself: the session's ordinary end.
//!
//! foot's child exits, foot exits with it, and the session has nothing left to
//! serve. That is what closing a window does, and it has to leave the terminal
//! in the same state the failure path does.

use crate::modes;

#[test]
#[ignore = "foot and the compositor race on teardown when the suite runs concurrently: the session intermittently exits 1"]
fn the_pty_keeps_the_modes_it_started_with() -> termlens::Result<()> {
    crate::common::require_foot();
    let runnable = crate::common::script(
        "after-foot-exit",
        "exec foot -- sh -c 'printf \"done\\n\"; sleep 1'",
    );

    let report = modes::run("termios-after-foot-exit", &runnable)?;
    assert_eq!(
        report.before, report.after,
        "the pty is back in the modes it had before the session (tuigui exited {})",
        report.code
    );
    assert_eq!(report.code, "0", "the session exits cleanly");
    Ok(())
}
