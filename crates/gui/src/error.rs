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
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
