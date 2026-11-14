//! A window that dies mid-stream: the session ends through the failure path.
//!
//! foot is killed while the link is still up, which is the failure the session
//! has to survive: it must end, and it must give the terminal back. Whether it
//! ends cleanly is not this test's question — that it ends, and that the pty is
//! in the modes it started with, is.

use crate::modes;

#[test]
fn the_pty_keeps_the_modes_it_started_with() -> termlens::Result<()> {
    crate::common::require_foot();
    let runnable = crate::common::script(
        "error-during-stream",
        "foot -- sh -c 'sleep 3' &\n\
         pid=$!\n\
         sleep 1\n\
         kill -9 \"$pid\"\n\
         wait \"$pid\"\n\
         exit 1",
    );

    let report = modes::run("termios-error-during-stream", &runnable)?;
    assert_eq!(
        report.before, report.after,
        "the pty is back in the modes it had before the session (tuigui exited {})",
        report.code
    );
    Ok(())
}
