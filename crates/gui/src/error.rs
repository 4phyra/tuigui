//! Why a session failed.

/// The failure a client exits on.
///
/// One variant, because every step of the run path already reports an [`std::io::Error`]: the
/// link, the program and the terminal are one kind of failure to this side. Named rather than
/// left bare so the diagnostic can be derived on it, which is what makes `?` out of the process
/// draw the report the user reads.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum Error {
    /// The program ended.
    ///
    /// Not a failure: a session whose program exits cleanly is over. Split out so a caller can
    /// tell "it finished" from "it broke" without looking at what is inside.
    #[error("the program ended")]
    Execution,

    /// A stream failed, and this is the line that was running when it did.
    ///
    /// The fact this side can add and a reader cannot recover. `std::io::Error` says what went
    /// wrong — "The cursor position could not be read within a normal duration" — and names no
    /// caller, so a report of it points at nothing in this program.
    ///
    /// The position is carried the way miette draws one: a span into a named source. The span is
    /// the label and the source supplies the quoted line, so there is no separate location field
    /// — one miette cannot see would only be interpolated into the message as text.
    ///
    /// The site and the source live here and nowhere else: every internal failure gets the same
    /// two facts from the same place, so a variant of [`InternalError`] does not repeat them.
    #[error("gui: the session failed")]
    Internal {
        #[source]
        source: InternalError,
        #[label("this call failed")]
        site: miette::SourceSpan,
        #[source_code]
        source_code: miette::NamedSource<String>,
    },

    /// The server failed, and these are the layers it reported, outermost first.
    ///
    /// Its own variant because the failure is not this side's: it happened in the process that
    /// decodes, stages and places. There is no location from this side, and deliberately: a span
    /// here would name a line that had nothing to do with it. Each line already carries the
    /// server's own `file:line:col`.
    ///
    /// The string is miette's own drawing from the other side, colour included, so it is printed
    /// as it came and not rebuilt from parts.
    #[error("gui: the session failed\n{0}")]
    #[diagnostic(help("this is a bug in tuigui; please report it"))]
    Reported(String),
}

/// What can go wrong, apart from where it happened.
///
/// One variant per kind, each with a `#[from]` so a `?` converts into it without a `map_err` at
/// the call site. `Error` adds the location for all of them at once; this type is what a caller
/// that does not care about a line number can match on.
#[derive(Debug, thiserror::Error)]
pub enum InternalError {
    /// The link, the terminal, or the program's own session failed.
    ///
    /// Not `transparent`: a transparent variant forwards `Display` *and* `source` to the inner
    /// error, so there is nothing above the leaf and a report has no chain to draw — one line,
    /// no arrow, no context. The arrow is what says "the session failed" and then what it
    /// failed on.
    #[error("the session failed")]
    Io(#[from] std::io::Error),

    /// The codec failed, encoding or decoding.
    ///
    /// Carried through because which half failed is the codec's own error to say; this is the
    /// boundary where it becomes the session's.
    #[error(transparent)]
    Codec(#[from] codecs::Error),

    /// The program's own stage failed.
    ///
    /// The message was made inside a `StatefulWidget`, whose `render` returns nothing, so the
    /// failure was kept and raised here. Its own variant because it is not an [`std::io::Error`]
    /// when it is made, and wrapping it in one would have the report name a stream that had
    /// nothing to do with it.
    #[error("the program's stage failed: {0}")]
    Stage(String),
}

#[cfg(test)]
#[path = "../tests/unit/error.rs"]
mod tests;

impl Error {
    /// A failed call, with the line it was called on.
    ///
    /// `#[track_caller]`, so the location is the caller's and not this function's.
    ///
    /// The location is used here and then dropped: what miette draws is the span and the source,
    /// and it cannot see a `Location`. Keeping it as a field would only be something to
    /// interpolate into the message, which is the header this avoids.
    ///
    /// When the source is not on disk — a binary shipped without its sources, a build from a
    /// different tree — the span is empty and the label points at the top of the file. A report
    /// without a quote is worse than one with it, and much better than a failure to report at
    /// all.
    #[track_caller]
    pub fn at(source: impl Into<InternalError>) -> Self {
        let location = std::panic::Location::caller();
        let Ok(text) = std::fs::read_to_string(location.file()) else {
            return Self::Internal {
                source: source.into(),
                site: (0, 0).into(),
                source_code: miette::NamedSource::new(location.file(), String::new()),
            };
        };
        let start = miette::SourceOffset::from_location(
            &text,
            location.line() as usize,
            location.column() as usize,
        );
        Self::Internal {
            source: source.into(),
            // One whole line from the caller's column on, so the quote is the call rather than
            // a caret under a single character.
            site: (
                start.offset(),
                text[start.offset()..].lines().next().map_or(0, str::len),
            )
                .into(),
            source_code: miette::NamedSource::new(location.file(), text),
        }
    }
}

/// `?` on the errors this crate actually meets, each keeping the line that raised it.
///
/// One impl per source type rather than one from [`InternalError`], because `?` performs exactly
/// one `From` conversion: a `?` on an `io::Error` looks for `From<io::Error> for Error` and will
/// not take the two-hop route through `InternalError`. Each is `#[track_caller]`, so the location
/// recorded is the `?` that converted it — which is the call that failed.
macro_rules! from_internal {
    ($($type:ty => $variant:ident),* $(,)?) => {
        $(
            impl From<$type> for Error {
                #[track_caller]
                fn from(source: $type) -> Self {
                    Self::at(InternalError::$variant(source))
                }
            }
        )*
    };
}

from_internal! {
    std::io::Error => Io,
    codecs::Error => Codec,
}

impl From<InternalError> for Error {
    /// A failure that is already the right kind, given a place here.
    ///
    /// `#[track_caller]` like the rest, so `?` on an `InternalError` records the line that did
    /// the converting rather than this impl's.
    #[track_caller]
    fn from(inner: InternalError) -> Self {
        Self::at(inner)
    }
}
