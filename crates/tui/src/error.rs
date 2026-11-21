//! What can go wrong in `tui`, as types rather than strings.
//!
//! Every failure here was previously an `io::Error::other(format!(...))`, which has
//! two costs: the caller cannot tell one failure from another without reading English
//! out of a message, and the `io::ErrorKind` is always `Other`, which says nothing.
//! A type per case is what lets a caller decide — a dropped frame is worth continuing
//! past, a failed transmit is not — and it is where the message belongs, next to the
//! code that knows what actually happened.
//!
//! # Two halves
//!
//! [`Error`] is one failure **at a place**. It is a struct, not an enum, because the
//! position belongs to the raising and not to the kind: the same `io::Error` from two
//! lines is a different failure to a reader, and a site on every variant would carry
//! those fields once per arm.
//!
//! [`ErrorInner`] is the catalogue — one variant per kind, holding the `#[from]`s — so
//! a conversion never has to invent a location. `#[diagnostic(forward(inner))]` is what
//! makes the wrapper hand the inner's own help, code and severity through: this type
//! adds a position, and a position is not a reason to drop the advice it came with.

/// A failure while serving a session, at the line that raised it.
///
/// No quoted source: reading a file at failure time can itself fail, and does when the failure
/// is "too many open files". The location is text, taken from `#[track_caller]`.
#[derive(Debug, miette::Diagnostic)]
#[diagnostic(forward(inner))]
pub struct Error {
    /// What happened, without a place.
    ///
    /// Not a `#[source]`: this type's own message already says it, so a source link would draw
    /// it twice. `source` below hands on the inner's own cause instead.
    inner: ErrorInner,
    /// The line that was running, as `#[track_caller]` recorded it.
    location: &'static std::panic::Location<'static>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.inner, self.location)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.inner)
    }
}

impl Error {
    /// A failure, with the line that raised it.
    ///
    /// `#[track_caller]`, so the location is the caller's: a `?` records its own file and
    /// line, which is the fact an `io::Error` cannot carry and the one a reader needs.
    #[track_caller]
    pub fn at(inner: impl Into<ErrorInner>) -> Self {
        Self {
            inner: inner.into(),
            location: std::panic::Location::caller(),
        }
    }
}

/// `?` on each error this crate meets, keeping the line that raised it.
///
/// One impl per source type, and not one from [`ErrorInner`], because `?` performs exactly one
/// `From` conversion: a `?` on an `io::Error` looks for `From<io::Error> for Error` and will not
/// take the two-hop route. Each is `#[track_caller]`, so the location is the `?` that converted.
macro_rules! from_inner {
    ($($type:ty => $variant:ident),* $(,)?) => {
        $(
            impl From<$type> for Error {
                #[track_caller]
                fn from(source: $type) -> Self {
                    Self::at(ErrorInner::$variant(source))
                }
            }
        )*
    };
}

from_inner! {
    std::io::Error => Io,
    codecs::Error => Codec,
    easytgp::TGPError => Graphics,
}

impl From<ErrorInner> for Error {
    /// A failure that is already the right kind, given a place here.
    ///
    /// `#[track_caller]` like the rest, so `?` on an `ErrorInner` and `.into()` both record the
    /// line that did the converting rather than this impl's. Without it a value built by hand —
    /// `ErrorInner::Stage { .. }.into()` — would point at this function in every report.
    #[track_caller]
    fn from(inner: ErrorInner) -> Self {
        Self::at(inner)
    }
}

/// Everything that can fail while serving a session, before it is given a place.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum ErrorInner {
    /// The codec failed, encoding or decoding.
    ///
    /// Carried through because which half failed is the codec's own error to say;
    /// this is the boundary where it becomes the session's.
    #[error(transparent)]
    Codec(#[from] codecs::Error),

    /// The decoder returned pixels that do not match the size it declared.
    ///
    /// Not possible with a working decoder, which is why it is worth its own variant:
    /// if this is ever reached, the decoder's contract is the thing that broke, and a
    /// generic message would hide that.
    #[error("a decoded frame is {width}x{height}, which its buffer does not match")]
    FrameSize { width: u32, height: u32 },

    /// The image could not be turned into a request for its medium.
    ///
    /// Raised by staging the pixels for the terminal — a shared memory object that
    /// cannot be created, a PNG that cannot be encoded — so it is the medium's failure
    /// rather than the frame's.
    ///
    /// The window travels with it because a session serves several: without it a report says
    /// "staging a frame failed" and leaves the reader to work out which picture that was.
    #[error("staging a frame for the terminal")]
    Stage {
        #[source]
        source: easytgp::TGPError,
        surface: wire::message::SurfaceId,
    },

    /// The graphics protocol rejected an operation.
    ///
    /// The terminal's own answer, passed through: it is the side that knows why it
    /// refused, and replacing its wording with ours would lose that.
    #[error("the graphics protocol failed: {0}")]
    Graphics(#[source] easytgp::TGPError),

    /// The image was not acknowledged by the terminal in time.
    ///
    /// A terminal that never answers will never draw the frame, so waiting past the
    /// point where a working one would have replied is waiting for nothing. Named
    /// separately from a protocol error because the terminal did not refuse: it said
    /// nothing at all, which is a different fault with a different cause.
    #[error("the terminal did not acknowledge a transmit within {0:?}")]
    TransmitTimeout(std::time::Duration),

    /// Writing to the screen failed.
    ///
    /// The terminal is gone, or the stream is not writable. Separate from [`ErrorInner::Io`]
    /// because the caller knows which stream it was writing to, and that is the fact
    /// worth reporting.
    #[error("writing to the screen: {0}")]
    Screen(#[source] std::io::Error),

    /// A stream failed.
    ///
    /// The catch-all, and deliberately last: anything with a name of its own above is
    /// something a caller might act on.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The `Result` every fallible operation in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

/// What to do about a failed attempt to tell a client that something failed.
///
/// `futures-retry` rather than a fixed backoff because these errors are not the same kind of
/// thing: a peer that is gone will still be gone in 20ms, so retrying it only delays telling
/// the user, while a write that failed against a full pipe is exactly the case that clears on
/// its own. Retrying everything is what a strategy does, and half of it would be waiting for
/// nothing.
///
/// A named handler rather than a closure because the decision needs the attempt number, and
/// `futures-retry`'s blanket impl for closures passes only the error — the attempt is available
/// on the `ErrorHandler` trait and nowhere else.
#[derive(Debug, Default)]
pub(crate) struct ReportFailure;

impl futures_retry::ErrorHandler<std::io::Error> for ReportFailure {
    type OutError = std::io::Error;

    fn handle(
        &mut self,
        attempt: usize,
        e: std::io::Error,
    ) -> futures_retry::RetryPolicy<std::io::Error> {
        // More than one, because the first failure is often the transient one: a write that
        // raced a full pipe, a peer that had not started reading yet.
        const ATTEMPTS: usize = 3;
        // Short: this is a local write to a socket or a pipe, so a failure that clears will
        // clear almost immediately. A longer pause would only delay the pane saying what broke.
        const BACKOFF: std::time::Duration = std::time::Duration::from_millis(20);

        match (e.kind(), attempt) {
            // Nothing to wait for: the link is not there at all.
            (std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::NotConnected, _) => {
                futures_retry::RetryPolicy::ForwardError(e)
            }
            (_, attempt) if attempt >= ATTEMPTS => futures_retry::RetryPolicy::ForwardError(e),
            _ => futures_retry::RetryPolicy::WaitRetry(BACKOFF),
        }
    }
}

/// What a caller outside this crate gets.
///
/// `io::Error` is what the binary reports on and what `Tui::run` has always returned, so the
/// typed error is converted back at that one boundary rather than changing the public
/// signature.
///
/// The conversion flattens the message rather than carrying the error, because a graphics error
/// can hold a `Box<dyn Restorable>`, which is not `Send`, and `io::Error::other` requires both.
/// Inside the crate the typed error and its source are intact, so a caller that wants to match
/// on the cause has it; only this last hop to `io::Error` loses the chain.
impl From<Error> for std::io::Error {
    fn from(error: Error) -> Self {
        std::io::Error::other(error.to_string())
    }
}
