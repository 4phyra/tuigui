use super::*;

/// A failure keeps the error it came from under it, two links deep.
///
/// The chain is now `Error` → `InternalError` → the stream's own error, because the wrapper is
/// what carries the location and the kind is what carries the cause. A reader gets "the session
/// failed" at the line, then which kind, then what the stream said — and a test that only looked
/// one hop down would miss the leaf that actually explains it.
#[test]
fn the_failure_keeps_its_cause() {
    let error = Error::from(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "the program is not there",
    ));

    let kind = std::error::Error::source(&error).expect("the kind is the source");
    assert_eq!(kind.to_string(), "the session failed");

    let leaf = kind.source().expect("the stream's error is under the kind");
    assert_eq!(leaf.to_string(), "the program is not there");
}
