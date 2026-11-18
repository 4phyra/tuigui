use super::*;

/// A failure keeps the error it came from under it.
///
/// `#[error(transparent)]` forwards `Display` *and* `source` to the inner error, so there is
/// nothing above the leaf: a report drawn from it is one line with no arrow and no context. The
/// variant names itself and keeps the [`std::io::Error`] as its source instead, which is what
/// gives a reader "the session failed" and then what it failed on.
#[test]
fn the_failure_keeps_its_cause() {
    let error = Error::from(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "the program is not there",
    ));

    assert_eq!(error.to_string(), "the session failed");
    assert_eq!(
        std::error::Error::source(&error)
            .map(ToString::to_string)
            .as_deref(),
        Some("the program is not there"),
        "the error it came from is its source, so a report has a chain to draw"
    );
}
