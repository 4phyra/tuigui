//! The shape every failure this program prints has.
//!
//! One handler for both halves: the client and the server are the same binary, they print to the
//! same terminal, and a report should not look different depending on which half raised it.
//!
//! A failure crosses the link as [`wire::message::Message::Error`], which carries a string. That
//! string is the report *already drawn* — the labels, the quoted source line and the chain —
//! rather than a flattened message, because the side that knows the file and the line is the side
//! that failed. `miette::Report` itself cannot cross: it is a boxed `dyn Diagnostic`, so it has no
//! layout rkyv could archive and its sources are borrowed trait objects. The drawn text is the
//! portable part, and it is the part a person reads.

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

/// Draw a failure the way the terminal would print it, as the text the wire carries.
///
/// `Debug` on a `Report` is miette's graphical form: the message, the labels, and the chain
/// beneath. `Display` is one line of prose with all of that thrown away, which is why this
/// exists rather than a `to_string()` at the call site.
pub fn drawn(report: miette::Report) -> String {
    format!("{report:?}")
}

/// The source a label points into, and the span of one line of it.
///
/// A report that says `session.rs:504` still makes the reader open the file; a report that
/// quotes the line does not. The text is compiled in, because it is the text that was
/// compiled: a label into a file read at run time would point at whatever is on disk now,
/// which is not what failed.
///
/// The span is the whole line, found by walking to the `line`th newline. Offsets are byte
/// offsets into `source`, which is what `SourceSpan` takes.
pub fn line_span(source: &'static str, line: usize) -> miette::SourceSpan {
    let mut start = 0;
    for _ in 1..line {
        match source[start..].find('\n') {
            Some(at) => start += at + 1,
            // Past the end, which a `line!()` cannot be for its own file: the whole of what
            // is left is the best answer, and it is still a line of the right file.
            None => return (start, source.len() - start).into(),
        }
    }
    let width = source[start..].find('\n').unwrap_or(source.len() - start);
    (start, width).into()
}

#[cfg(test)]
#[path = "../tests/unit/report.rs"]
mod tests;
