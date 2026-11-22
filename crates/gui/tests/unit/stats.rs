use super::*;

#[test]
fn the_first_sample_reports_nothing_yet() {
    let mut stats = Stats::default();
    assert_eq!(stats.per_sec_at(Instant::now()), 0.0);
}

#[test]
fn one_full_window_is_bytes_over_time() {
    let mut stats = Stats::default();
    let start = Instant::now();
    stats.bytes += 1024;
    assert_eq!(
        stats.per_sec_at(start),
        0.0,
        "the first sample opens the window"
    );
    stats.bytes += 40 * 1024;
    assert_eq!(stats.per_sec_at(start + Duration::from_secs(1)), 40960.0);
}

#[test]
fn a_partial_window_keeps_the_last_reading() {
    let mut stats = Stats::default();
    let start = Instant::now();
    stats.per_sec_at(start);
    stats.bytes += 60 * 1024;
    stats.per_sec_at(start + Duration::from_secs(1));
    // Half a window later there is not enough new evidence to say anything.
    assert_eq!(stats.per_sec_at(start + Duration::from_millis(1500)), 61440.0);
}
