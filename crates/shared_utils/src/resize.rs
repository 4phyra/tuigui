//! The terminal's size, and the next time it changes.

/// This terminal's size now, as the terminal itself reports it.
///
/// Read from stdin's device, which is the terminal the user is looking at. The struct carries both
/// what a caller lays out in (cells) and what it encodes for (pixels), so one call answers for a
/// grid and for an image rather than each asking separately.
///
/// A terminal that reports no cells is a failure rather than a guess: a pane laid out 24x80 on a
/// real screen is a picture placed at the wrong cell, which looks like a bug in the placement.
pub fn size() -> std::io::Result<rustix::termios::Winsize> {
    let ws = rustix::termios::tcgetwinsize(std::io::stdin()).map_err(std::io::Error::from)?;
    if ws.ws_row == 0 || ws.ws_col == 0 {
        return Err(std::io::Error::other("this terminal reports no cells"));
    }
    Ok(ws)
}

/// Wait until this terminal is resized, and answer with its new size.
///
/// A future, not a subscription: the caller awaits it where it already awaits everything else, and
/// calling it again after it resolves waits for the next change. The signal is process-wide and
/// tokio keeps one handler, so several callers can each await their own copy — the pty and the
/// proxy both hold a size, and neither has to know about the other.
///
/// `SIGWINCH` carries nothing, so the size is read after it arrives rather than from it.
pub async fn resized() -> std::io::Result<rustix::termios::Winsize> {
    let mut changed = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;
    // `None` is the handler gone, which tokio only reports when the process is losing it. Waiting
    // for the next change would then wait forever, so it is reported instead.
    changed
        .recv()
        .await
        .ok_or_else(|| std::io::Error::other("the resize handler is gone"))?;
    size()
}
