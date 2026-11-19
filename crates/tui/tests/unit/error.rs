use super::*;

/// The line `F::request` is on, counted from one.
///
/// Found rather than written down: a number in a test is wrong the moment a line is added
/// above it, and this test exists to prove the report points at the *call*, so a stale
/// number would either pass while pointing at a comment or fail for the wrong reason.
fn staging_call_line() -> usize {
    SESSION_SOURCE
        .lines()
        .position(|line| line.contains("F::request(&self.controller"))
        .expect("the staging call is in this file")
        + 1
}

/// A staging failure is drawn with the window, the line, and the code itself.
///
/// The three facts a reader needs and a message alone cannot give: which picture, which
/// call, and what that call looks like. The label points at the line and the source is
/// compiled in, so the report quotes it — no opening the file, no guessing which of the
/// staging calls it was.
#[test]
fn a_staging_failure_names_the_window_and_quotes_the_line() {
    let error = Error::Stage {
        source: easytgp::TGPError::Shm("creating /dev/shm/easytgp1-0: Too many open files".into()),
        surface: wire::message::SurfaceId {
            symbol: '❋',
            fg: (1, 2, 3),
            bg: (4, 5, 6),
        },
        site: shared_utils::report::line_span(SESSION_SOURCE, staging_call_line()),
        source_code: miette::NamedSource::new("crates/tui/src/session.rs", SESSION_SOURCE),
    };

    shared_utils::report::install_report_handler();
    let drawn = format!("{:?}", miette::Report::new(error));

    // The chain: what failed, then the medium's own reason under it.
    assert!(drawn.contains("staging a frame for the terminal"), "{drawn}");
    assert!(drawn.contains("Too many open files"), "{drawn}");
    // Which window.
    assert!(drawn.contains("SurfaceId"), "{drawn}");
    // Where, and the code at that place: the file, and the call quoted under it. Not the
    // comment above the call, which is what an off-by-one points at.
    assert!(drawn.contains("session.rs"), "{drawn}");
    assert!(
        drawn.contains("F::request(&self.controller, pixels)"),
        "the label quotes the call, not the line above it: {drawn}"
    );
}
