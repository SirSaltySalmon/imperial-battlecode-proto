//! Exponential backoff with jitter (docs/JOBS.md: 1 s doubling to 60 s), and sleeps that can be interrupted.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Backoff {
    min: Duration,
    max: Duration,
    cur: Duration,
    rng: u64,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let rng = (nanos ^ (u64::from(std::process::id()) << 32)) | 1;
        Backoff {
            min,
            max: max.max(min),
            cur: min,
            rng,
        }
    }

    pub fn reset(&mut self) {
        self.cur = self.min;
    }

    /// The next delay: uniform in `[step / 2, step]`, where `step` doubles from `min` up to `max`.
    /// Jitter keeps a fleet that lost the coordinator together from coming back in lock-step.
    pub fn next_delay(&mut self) -> Duration {
        let step = self.cur;
        self.cur = (self.cur * 2).min(self.max);
        // xorshift64: plenty for jitter, and no dependency.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let half = step.as_millis() as u64 / 2;
        Duration::from_millis(half + self.rng % (half + 1))
    }
}

/// Sleep for `d`, waking early (and returning `true`) as soon as `stop()` holds.
pub fn sleep_unless(d: Duration, stop: impl Fn() -> bool) -> bool {
    let end = Instant::now() + d;
    loop {
        if stop() {
            return true;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_to_the_cap_with_jitter_in_range() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
        let mut step = 1000u128;
        for _ in 0..12 {
            let d = b.next_delay().as_millis();
            assert!(
                d >= step / 2 && d <= step,
                "{d} outside [{}, {step}]",
                step / 2
            );
            step = (step * 2).min(60_000);
        }
        b.reset();
        assert!(b.next_delay() <= Duration::from_secs(1));
    }

    #[test]
    fn sleep_is_interruptible() {
        let t = Instant::now();
        assert!(sleep_unless(Duration::from_secs(30), || true));
        assert!(t.elapsed() < Duration::from_secs(1));
        assert!(!sleep_unless(Duration::from_millis(10), || false));
    }
}
