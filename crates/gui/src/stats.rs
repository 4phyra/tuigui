//! What the pane has put on the wire, counted where it leaves.
//!
//! One process runs one session, and the two ends of this number — the window's servant,
//! which sends the frames, and the dock, which draws the rate — never meet. A process-wide
//! static rather than a counter threaded through every frame of the render path: there is
//! nothing to thread, because there is nothing per-window about it.
//!
//! The counter is bytes, and the number of frames that carried them; the rates are computed
//! here, windowed over the last second.

use std::sync::RwLock;
use std::time::{Duration, Instant};

/// The share of a second a window has to cover before its rate is worth showing. Shorter and a
/// single frame landing just before a sample makes a wild number; longer and the display lags.
const WINDOW: Duration = Duration::from_secs(1);

/// Upload progress since the session started.
///
/// Two counters, because they answer different questions: how much is going out, and how often the
/// pane says it placed a picture — which is the rate a user can actually see.
#[derive(Default)]
pub(crate) struct Stats {
    /// Count upload once the send succeeded: a frame that failed to leave is not upload.
    bytes: u64,
    /// Every picture the pane reported placing.
    placed: u64,
    /// When the current window started; `None` until the first sample.
    start: Option<Instant>,
    /// The counter readings at the start of the window.
    at_bytes: u64,
    at_placed: u64,
    /// The byte rate the last recompute produced, reshown between recomputes.
    per_sec: f64,
    /// The pictures the last window held. The window is one second, so that count is the rate:
    /// dividing it by the window again would be a second field holding the same fact.
    placed_last: u64,
}

/// The one set of counters this process has: the send path and the dock both name these
/// functions, and neither has to hold anything.
pub(crate) static STATS: RwLock<Stats> = RwLock::new(Stats {
    bytes: 0,
    placed: 0,
    start: None,
    at_bytes: 0,
    at_placed: 0,
    per_sec: 0.0,
    placed_last: 0,
});

/// Count one frame of `bytes` as uploaded. Called after the send succeeded.
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
pub(crate) fn record(bytes: u64) {
    STATS.write().unwrap_or_else(|e| e.into_inner()).bytes += bytes;
}

/// Count one picture as placed. Called for every `Pace` the pane sends, which is one per picture
/// it put on the screen.
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
pub(crate) fn record_placed() {
    let mut stats = STATS.write().unwrap_or_else(|e| e.into_inner());
    stats.placed += 1;
}

/// Every byte the send path has put on the wire since the process started, for the test that
/// measures one session.
#[cfg(all(test, feature = "test-windowing"))]
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
pub(crate) fn uploaded() -> u64 {
    STATS.read().unwrap_or_else(|e| e.into_inner()).bytes
}

/// Zero the counters, for a test that measures one session rather than a process.
#[cfg(all(test, feature = "test-windowing"))]
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
pub(crate) fn reset() {
    let mut stats = STATS.write().unwrap_or_else(|e| e.into_inner());
    stats.bytes = 0;
    stats.placed = 0;
}

/// Pictures placed and bytes per second over the last window, recomputed at most once per
/// `WINDOW`.
///
/// Both from one sample: two calls would be two chances to fall either side of a window
/// boundary, and they are drawn as one fact.
#[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
pub(crate) fn per_sec() -> (u64, u64) {
    let mut stats = STATS.write().unwrap_or_else(|e| e.into_inner());
    stats.per_sec_at(Instant::now());
    (stats.placed_last, stats.per_sec as u64)
}

impl Stats {
    /// Bytes per second over the window ending at `now`, recomputed only once a full window has
    /// passed — and the frames that window held with it.
    #[cfg_attr(feature = "tracy", tracing::instrument(skip_all))]
    fn per_sec_at(&mut self, now: Instant) -> f64 {
        let Some(start) = self.start else {
            self.start = Some(now);
            self.at_bytes = self.bytes;
            self.at_placed = self.placed;
            return self.per_sec;
        };
        let elapsed = now.duration_since(start);
        if elapsed < WINDOW {
            return self.per_sec;
        }
        self.per_sec = (self.bytes - self.at_bytes) as f64 / elapsed.as_secs_f64();
        self.placed_last = self.placed - self.at_placed;
        self.start = Some(now);
        self.at_bytes = self.bytes;
        self.at_placed = self.placed;
        self.per_sec
    }
}

#[cfg(test)]
#[path = "../tests/unit/stats.rs"]
mod tests;
