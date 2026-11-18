use super::*;

/// A failure, with a cause under it, so the shape asserted is the whole one: header, arrow, cause.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("the outer failure")]
struct Chained(#[source] Inner);

#[derive(Debug, thiserror::Error)]
#[error("the inner failure")]
struct Inner;

/// A report is drawn, not flattened.
///
/// `fancy-no-syscall` is what this build carries, and it answers "no" to every terminal probe:
/// without the handler installed here a report is one ascii line, which is the difference between
/// a reader seeing what failed and reading a sentence about it.
///
/// The whole rendered report is the snapshot, because the thing being reviewed is its *shape* —
/// colour, glyphs, where the cause hangs — and a test that asserted three characters of it would
/// pass while the shape was wrong.
#[test]
fn a_report_is_drawn_with_colour_and_unicode() {
    // The process-global hook, once: a second install in one test binary is a no-op by design,
    // and it is the first that decides what every report looks like.
    install_report_handler();

    let drawn = format!("{:?}", miette::Report::new(Chained(Inner)));
    insta::assert_snapshot!(drawn);
}
