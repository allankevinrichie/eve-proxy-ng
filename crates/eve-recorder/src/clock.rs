//! Session-wide monotonic clock.
//!
//! All streams (input jsonl, observation jsonl, video anchor) timestamp with
//! milliseconds relative to the session start, so any event can be compared
//! across streams directly. The wall-clock epoch of the start is kept so
//! relative times can be mapped back to absolute times when analyzing.

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct SessionClock {
    started_at_unix_ms: u128,
    start: Instant,
}

impl SessionClock {
    pub fn new() -> Self {
        Self {
            started_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            start: Instant::now(),
        }
    }

    /// Milliseconds elapsed since the session started (monotonic).
    pub fn now_rel_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Relative session timestamp of an Instant captured earlier (e.g.
    /// inside a hook callback). Clamps pre-session instants to 0.
    pub fn rel_of(&self, t: Instant) -> u64 {
        t.checked_duration_since(self.start).map(|d| d.as_millis() as u64).unwrap_or(0)
    }

    /// The wall-clock (Unix epoch ms) this session started at.
    pub fn started_at_unix_ms(&self) -> u128 {
        self.started_at_unix_ms
    }

    /// Map a relative timestamp back to Unix epoch ms.
    pub fn to_unix_ms(&self, rel_ms: u64) -> u128 {
        self.started_at_unix_ms + rel_ms as u128
    }
}

impl Default for SessionClock {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedClock = Arc<SessionClock>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_and_epoch_backmap() {
        let clock = SessionClock::new();
        let a = clock.now_rel_ms();
        let b = clock.now_rel_ms();
        assert!(b >= a);
        assert!(clock.to_unix_ms(b) >= clock.started_at_unix_ms());
    }
}
