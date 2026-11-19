//! Why a session failed.

/// The failure a client exits on.
///
/// One variant, because every step of the run path already reports an [`std::io::Error`]: the
/// link, the program and the terminal are one kind of failure to this side. Named rather than
/// left bare so the diagnostic can be derived on it, which is what makes `?` out of the process
/// draw the report the user reads.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum Error {
    /// The terminal, the link, or the program's own session failed.
    ///
    /// A message of its own with the [`std::io::Error`] as its source, and not `transparent`:
    /// a transparent variant forwards `Display` *and* `source` to the inner error, so there is
    /// nothing above the leaf and a report has no chain to draw — one line, no arrow, no
    /// context. The arrow is what says "the session failed" and then what it failed on.
    #[error("the session failed")]
    Io(#[from] std::io::Error),

    /// The server failed, and this is its own report.
    ///
    /// Its own variant because the failure is not this side's: it happened in the process that
    /// decodes, stages and places, and the report already says everything — the message, the
    /// line, the code, the medium's own reason. Wrapping it in "the session failed" would put
    /// this side's words over the other side's, which is what made a server failure read like
    /// a client one. The text is printed as it arrived, and it is the whole message.
    #[error("{0}")]
    Reported(String),
}

#[cfg(test)]
#[path = "../tests/unit/error.rs"]
mod tests;
