//! What the pane has put on the wire, counted where it leaves.
//!
//! One process runs one session, and the two ends of this number — the window's servant,
//! which sends the frames, and the dock, which draws the rate — never meet. A process-wide
//! static rather than a counter threaded through every frame of the render path: there is
//! nothing to thread, because there is nothing per-window about it.
//!
//! The counter is bytes; the rate is computed here, windowed over the last second.

use std::sync::RwLock;
use std::time::{Duration, Instant};

/// The share of a second a window has to cover before its rate is worth showing. Shorter and a
/// single frame landing just before a sample makes a wild number; longer and the display lags.
const WINDOW: Duration = Duration::from_secs(1);

/// Upload progress since the session started.
///
/// Frames are not counted: what the pane wants to show is how much is going out, and a frame
/// either carries bytes or it does not.
#[derive(Default)]
pub(crate) struct Stats {
    /// Count upload once the send succeeded: a frame that failed to leave is not upload.
    bytes: u64,
    /// When the current window started; `None` until the first sample.
    start: Option<Instant>,
    /// The counter reading at the start of the window.
    at_bytes: u64,
    /// The rate the last recompute produced, reshown between recomputes.
    per_sec: f64,
}

/// The one set of counters this process has: the send path and the dock both name these
/// functions, and neither has to hold anything.
pub(crate) static STATS: RwLock<Stats> = RwLock::new(Stats {
    bytes: 0,
    start: None,
    at_bytes: 0,
    per_sec: 0.0,
});

/// Count `bytes` as uploaded. Called after the send succeeded.
#[tracing::instrument(skip_all)]
pub(crate) fn record(bytes: u64) {
    STATS.write().unwrap_or_else(|e| e.into_inner()).bytes += bytes;
}

/// Bytes per second over the last window, recomputed at most once per `WINDOW`.
#[tracing::instrument(skip_all)]
pub(crate) fn bytes_per_sec() -> u64 {
    STATS.write().unwrap_or_else(|e| e.into_inner()).per_sec_at(Instant::now()) as u64
}

impl Stats {
    #[tracing::instrument(skip_all)]
    fn per_sec_at(&mut self, now: Instant) -> f64 {
        let Some(start) = self.start else {
            self.start = Some(now);
            self.at_bytes = self.bytes;
            return self.per_sec;
        };
        let elapsed = now.duration_since(start);
        if elapsed < WINDOW {
            return self.per_sec;
        }
        self.per_sec = (self.bytes - self.at_bytes) as f64 / elapsed.as_secs_f64();
        self.start = Some(now);
        self.at_bytes = self.bytes;
        self.per_sec
    }
}

#[cfg(test)]
#[path = "../tests/unit/stats.rs"]
mod tests;
