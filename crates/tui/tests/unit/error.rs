use super::*;

/// The failure, as a user reads it.
///
/// The whole drawn report is the snapshot, because what is reviewed is its *shape*: the chain
/// arrow, the label hanging off the span, the quoted line, the colours. Assertions that the text
/// contains three words would pass with the label pointing at the wrong line and the handler
/// falling back to ascii — both of which have happened here, which is why this is a snapshot and
/// not a `contains`.
#[test]
fn a_staging_failure_is_drawn_with_its_window_line_and_code() {
    // The process-global hook, once: the first install decides what every report in this binary
    // looks like, and the server installs the same one.
    shared_utils::report::install_report_handler();

    insta::assert_snapshot!(format!("{:?}", miette::Report::new(staging_failure())));
}

/// A staging failure raised here, so the label points at a line this file controls.
///
/// `#[track_caller]` on the helper and on [`Error::at`] both pass the caller's location down, so
/// the span names the line below rather than inside `Error::at`. A snapshot taken of the test
/// body instead would move every time anything is added above it.
#[track_caller]
fn staging_failure() -> Error {
    Error::at(ErrorInner::Stage {
        source: easytgp::TGPError::Shm("creating /dev/shm/easytgp1-0: Too many open files".into()),
        surface: wire::message::SurfaceId {
            symbol: '❋',
            fg: (1, 2, 3),
            bg: (4, 5, 6),
        },
    })
}
