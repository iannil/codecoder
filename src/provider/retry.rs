// Retry with exponential backoff + jitter for LLM provider calls.

use std::time::Duration;

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of attempts (including the first). 0 = no retry.
    pub max_retries: u32,
    /// Initial delay in milliseconds. Doubled after each retry, capped at 30s.
    pub initial_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self { max_retries: 3, initial_delay_ms: 1000 }
    }
}

/// The error returned after all retries are exhausted.
#[derive(Debug)]
pub struct RetryError {
    pub attempts: u32,
    pub last_error: String,
    pub is_rate_limited: bool,
}

impl std::fmt::Display for RetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "after {} attempt(s): {} (rate_limited={})",
            self.attempts, self.last_error, self.is_rate_limited)
    }
}

impl std::error::Error for RetryError {}

/// Simple deterministic jitter: `((attempt as u64).wrapping_mul(7919)) % (delay.as_millis() as u64 / 4 + 1)` ms.
/// Avoids thundering herd without requiring the `rand` crate.
fn jitter_delay(attempt: u32, delay: Duration) -> Duration {
    let base_ms = delay.as_millis() as u64;
    if base_ms == 0 {
        return delay;
    }
    let jitter_ms = (attempt as u64).wrapping_mul(7919) % (base_ms / 4 + 1);
    Duration::from_millis(base_ms + jitter_ms)
}

/// Determine if an HTTP status / error string indicates a retryable failure.
pub fn is_retryable(error: &str, status_code: Option<u16>) -> bool {
    if let Some(code) = status_code {
        // 429 (rate limit), 500, 502, 503, 504 (server errors)
        if code == 429 || (500..=599).contains(&code) {
            return true;
        }
    }
    let lower = error.to_lowercase();
    // Network-level errors
    lower.contains("timeout")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("eof")
        || lower.contains("temporary")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("service unavailable")
        || lower.contains("internal server error")
}

/// Call `f` with retry logic. Returns the first successful result, or a `RetryError`
/// after all retries are exhausted.
pub fn with_retry<F, T>(config: &RetryConfig, mut f: F) -> Result<T, RetryError>
where
    F: FnMut() -> Result<T, (String, Option<u16>)>,
{
    let max = config.max_retries.max(1); // at least 1 attempt
    let mut last_error = String::new();
    let mut is_rate_limited = false;
    let mut delay = Duration::from_millis(config.initial_delay_ms);
    let max_delay = Duration::from_secs(30);

    for attempt in 0..max {
        if attempt > 0 {
            // Apply jittered delay before retry
            let sleep_dur = jitter_delay(attempt, delay).min(max_delay);
            std::thread::sleep(sleep_dur);
            // Double delay for next iteration (capped at max_delay)
            delay = (delay * 2).min(max_delay);
        }
        match f() {
            Ok(result) => return Ok(result),
            Err((err, code)) => {
                last_error = err;
                is_rate_limited = code == Some(429);
                if !is_retryable(&last_error, code) {
                    // Non-retryable error — bail immediately
                    return Err(RetryError {
                        attempts: attempt + 1,
                        last_error,
                        is_rate_limited,
                    });
                }
            }
        }
    }
    Err(RetryError {
        attempts: max,
        last_error,
        is_rate_limited,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn succeeds_on_first_try() {
        let cfg = RetryConfig { max_retries: 3, initial_delay_ms: 1 };
        let result = with_retry(&cfg, || Ok::<_, (String, Option<u16>)>(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn retries_on_429_then_succeeds() {
        let cfg = RetryConfig { max_retries: 3, initial_delay_ms: 1 };
        let mut calls = 0u32;
        let result: Result<i32, _> = with_retry(&cfg, || {
            calls += 1;
            if calls < 3 {
                Err(("rate limited".into(), Some(429)))
            } else {
                Ok(42)
            }
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls, 3);
    }

    #[test]
    fn exhausts_retries_on_persistent_429() {
        let cfg = RetryConfig { max_retries: 3, initial_delay_ms: 1 };
        let result: Result<i32, _> = with_retry(&cfg, || {
            Err(("rate limited".into(), Some(429)))
        });
        let err = result.unwrap_err();
        assert_eq!(err.attempts, 3);
        assert!(err.is_rate_limited);
    }

    #[test]
    fn non_retryable_error_bails_immediately() {
        let cfg = RetryConfig { max_retries: 3, initial_delay_ms: 1 };
        let mut calls = 0u32;
        let result: Result<i32, _> = with_retry(&cfg, || {
            calls += 1;
            Err(("invalid api key".into(), Some(401)))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1); // no retry on 401
    }

    #[test]
    fn is_retryable_codes() {
        assert!(is_retryable("", Some(429)));
        assert!(is_retryable("", Some(500)));
        assert!(is_retryable("", Some(502)));
        assert!(is_retryable("", Some(503)));
        assert!(is_retryable("", Some(504)));
        assert!(!is_retryable("", Some(400)));
        assert!(!is_retryable("", Some(401)));
        assert!(!is_retryable("", Some(403)));
        assert!(!is_retryable("", Some(404)));
    }

    #[test]
    fn zero_retries_means_one_attempt() {
        let cfg = RetryConfig { max_retries: 0, initial_delay_ms: 1 };
        let mut calls = 0u32;
        let result: Result<i32, _> = with_retry(&cfg, || {
            calls += 1;
            Err(("timeout".into(), None))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
}