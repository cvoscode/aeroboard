//! Exponential backoff with jitter for GreptimeDB requests.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Total attempts, including the first one.
    pub max_attempts: usize,
    /// Delay before the first retry; doubled after every attempt.
    pub initial_delay_ms: u64,
    /// Upper bound for the un-jittered delay.
    pub max_delay_ms: u64,
    /// Up to this percentage of the delay is added at random so many writers
    /// recovering from the same outage don't retry in lockstep.
    pub jitter_percentage: u8,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay_ms: 500,
            max_delay_ms: 5000,
            jitter_percentage: 25,
        }
    }
}

impl RetryConfig {
    /// Delay to wait after failed attempt number `attempt` (1-indexed).
    pub fn delay_for_attempt(&self, attempt: usize) -> Duration {
        let exponent = attempt.saturating_sub(1).min(20) as u32;
        let base_ms = self
            .initial_delay_ms
            .saturating_mul(1u64 << exponent)
            .min(self.max_delay_ms);
        let max_jitter_ms = base_ms * u64::from(self.jitter_percentage.min(100)) / 100;
        let jitter_ms = if max_jitter_ms == 0 {
            0
        } else {
            fastrand::u64(0..=max_jitter_ms)
        };
        Duration::from_millis(base_ms + jitter_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_grows_exponentially_is_capped_and_jittered() {
        let config = RetryConfig {
            max_attempts: 5,
            initial_delay_ms: 100,
            max_delay_ms: 1000,
            jitter_percentage: 25,
        };
        for (attempt, base) in [
            (1, 100),
            (2, 200),
            (3, 400),
            (4, 800),
            (5, 1000),
            (60, 1000),
        ] {
            let delay = config.delay_for_attempt(attempt).as_millis() as u64;
            assert!(
                (base..=base + base / 4).contains(&delay),
                "attempt {attempt}: {delay}ms"
            );
        }

        let no_jitter = RetryConfig {
            jitter_percentage: 0,
            ..config
        };
        assert_eq!(no_jitter.delay_for_attempt(2), Duration::from_millis(200));
    }
}
