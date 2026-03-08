//! Retry utilities with exponential backoff for network operations.
//! 
//! This module provides:
//! - Exponential backoff retry strategy
//! - Connection retry with jitter (optional)
//! - Timeout-aware retry logic



use std::time::Duration;

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_attempts: usize,
    /// Initial delay between retries (in milliseconds).
    pub initial_delay_ms: u64,
    /// Maximum delay between retries (in milliseconds).
    pub max_delay_ms: u64,
    /// Random jitter percentage (0-100) to add to delays.
    pub jitter_percentage: u8,
    /// Whether to retry on all errors or only network errors.
    pub retry_all_errors: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay_ms: 500,
            max_delay_ms: 5000,
            jitter_percentage: 25,
            retry_all_errors: false,
        }
    }
}

#[allow(dead_code)]
impl RetryConfig {
    /// Create a new retry config with exponential backoff.
    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Create a new retry config with initial delay.
    pub fn with_initial_delay_ms(mut self, initial_delay_ms: u64) -> Self {
        self.initial_delay_ms = initial_delay_ms;
        self
    }

    /// Create a new retry config with maximum delay.
    pub fn with_max_delay_ms(mut self, max_delay_ms: u64) -> Self {
        self.max_delay_ms = max_delay_ms;
        self
    }

    /// Create a new retry config with jitter.
    pub fn with_jitter_percentage(mut self, jitter_percentage: u8) -> Self {
        self.jitter_percentage = jitter_percentage;
        self
    }

    /// Create a new retry config that retries all errors.
    pub fn with_retry_all_errors(mut self) -> Self {
        self.retry_all_errors = true;
        self
    }
}

/// Retry strategy for exponential backoff with optional jitter.
/// 
/// This strategy calculates delays based on:
/// - `initial_delay_ms * (2^(attempt-1))` with exponential backoff
/// - Capped at `max_delay_ms`
/// - Optional random jitter added to avoid thundering herd
#[allow(dead_code)]
pub struct RetryStrategy {
    config: RetryConfig,
}

#[allow(dead_code)]
impl RetryStrategy {
    /// Create a new retry strategy with default config.
    pub fn new() -> Self {
        Self {
            config: RetryConfig::default(),
        }
    }

    /// Configure the retry strategy.
    pub fn with_config(config: RetryConfig) -> Self {
        Self { config }
    }

    /// Calculate delay for a given attempt number (1-indexed).
    pub fn delay_for_attempt(&self, attempt: usize) -> Duration {
        // Calculate base delay with exponential backoff
        let base_delay = (self.config.initial_delay_ms
            * (1 << (attempt - 1))) as f64;

        // Cap at max delay
        let capped_delay = base_delay.min(self.config.max_delay_ms as f64);

        // Add random jitter if configured
        let jitter: u64 = if self.config.jitter_percentage > 0 {
            // Use a simple LCG-based pseudo-random number generator
            // seed based on attempt number for reproducibility
            let seed = attempt.wrapping_mul(1103515245).wrapping_add(12345);
            let jitter_f64 = ((seed / 65536) as f64 / 65536.0) * (self.config.jitter_percentage as f64 / 100.0);
            jitter_f64 as u64
        } else {
            0
        };

        // Convert base delay to u64 for addition
        let base_ms = capped_delay as u64;
        Duration::from_millis(base_ms + jitter)
    }

    /// Check if we should retry based on attempt number.
    pub fn should_retry(&self, attempt: usize) -> bool {
        attempt < self.config.max_attempts
    }

    /// Check if an error should be retried based on error message.
    pub fn should_retry_error(&self, error: &str) -> bool {
        if self.config.retry_all_errors {
            return true;
        }

        // Only retry transient errors
        let error_message = error.to_lowercase();
        error_message.contains("connection")
            || error_message.contains("timeout")
            || error_message.contains("dns")
            || error_message.contains("tcp")
            || error_message.contains("network")
            || error_message.contains("not found")
            || error_message.contains("reset")
    }

    /// Get maximum attempts from config.
    pub fn max_attempts(&self) -> usize {
        self.config.max_attempts
    }
}
