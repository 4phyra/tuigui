//! The shape every failure this program prints has.
//!
//! One handler for both halves: the client and the server are the same binary, they print to the
//! same terminal, and a report should not look different depending on which half raised it.

/// Make every `miette::Report` printed by this process a drawn one.
///
/// `fancy-no-syscall` is what this build carries, and it answers "no" to every terminal probe —
/// colour, unicode, width — because a process inside a pty has no way to ask. Here the answer is
/// known instead: this is a terminal, a person reads what is printed on it, and the width is the
/// window's own.
///
/// Once per process, and the second call is not an error: a report is printed once, at the end,
/// so the first install is the one that matters.
pub fn install_report_handler() {
    let width = rustix::termios::tcgetwinsize(std::io::stdout())
        .map(|size| usize::from(size.ws_col))
        .ok()
        .filter(|width| *width > 0)
        .unwrap_or(80);

    let _ = miette::set_hook(Box::new(move |_| {
        Box::new(
            miette::MietteHandlerOpts::new()
                .color(true)
                .unicode(true)
                .width(width)
                .build(),
        )
    }));
}

#[cfg(test)]
#[path = "../tests/unit/report.rs"]
mod tests;
