//! A lock-free "at most one event per interval" gate, for log lines that a
//! misbehaving peer could otherwise produce without bound — a target app that
//! refuses every window write, a local sender spraying `tomari://` URLs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// At most one event per `interval`. Lock-free; the first call always passes.
pub struct RateLimit {
    interval: Duration,
    /// Milliseconds since [`EPOCH`] of the last event let through, or `0` for
    /// none yet.
    last_ms: AtomicU64,
}

impl RateLimit {
    pub const fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_ms: AtomicU64::new(0),
        }
    }

    /// Whether an event at `now` may go through.
    pub fn allow(&self, now: Instant) -> bool {
        // Saturating: the first caller may have read `now` before `EPOCH` was
        // initialised, i.e. slightly before it.
        let now_ms = now.saturating_duration_since(*EPOCH).as_millis() as u64 + 1;
        let last = self.last_ms.load(Ordering::Relaxed);
        if last != 0 && now_ms.saturating_sub(last) < self.interval.as_millis() as u64 {
            return false;
        }
        self.last_ms
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

/// Reference point for [`RateLimit`]'s timestamps (an `Instant` cannot be a
/// `const`).
pub static EPOCH: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_one_event_per_interval() {
        let limit = RateLimit::new(Duration::from_secs(10));
        let t0 = *EPOCH + Duration::from_secs(100);
        assert!(limit.allow(t0));
        assert!(!limit.allow(t0 + Duration::from_secs(5)));
        assert!(!limit.allow(t0 + Duration::from_millis(9_999)));
        assert!(limit.allow(t0 + Duration::from_secs(10)));
        assert!(!limit.allow(t0 + Duration::from_secs(11)));
    }
}
