use std::time::Duration;

/// Exponential reconnect delay: `base`, `2 * base`, `4 * base`, ... capped at `max`.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    /// A backoff starting at `base` and never above `max`.
    pub fn new(base: Duration, max: Duration) -> Backoff {
        Backoff {
            base,
            max,
            attempt: 0,
        }
    }

    /// The delay before the next attempt.
    pub fn next_delay(&mut self) -> Duration {
        let factor = 1u32.checked_shl(self.attempt.min(31)).unwrap_or(u32::MAX);
        self.attempt = self.attempt.saturating_add(1);
        self.base.saturating_mul(factor).min(self.max)
    }

    /// Start over after a successful connection.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

impl Default for Backoff {
    /// 1 s doubling up to 60 s.
    fn default() -> Backoff {
        Backoff::new(Duration::from_secs(1), Duration::from_secs(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_and_caps() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_millis(1000));
        let got: Vec<u128> = (0..6).map(|_| b.next_delay().as_millis()).collect();
        assert_eq!(got, [100, 200, 400, 800, 1000, 1000]);
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_millis(100));
    }

    #[test]
    fn never_overflows() {
        let mut b = Backoff::default();
        for _ in 0..100 {
            assert!(b.next_delay() <= Duration::from_secs(60));
        }
    }
}
