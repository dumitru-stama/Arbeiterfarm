use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Token bucket rate limiter for VT API requests.
pub struct RateLimiter {
    max_tokens: u32,
    refill_interval: Duration,
    state: Mutex<RateLimiterState>,
}

struct RateLimiterState {
    tokens: u32,
    last_refill: Instant,
}

#[derive(Debug, thiserror::Error)]
pub enum RateLimitError {
    #[error("rate limit exceeded, retry after {wait_secs:.1}s")]
    Exceeded { wait_secs: f64 },
    #[error("rate limit wait timed out after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
}

impl RateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        let rpm = requests_per_minute.max(1);
        Self {
            max_tokens: rpm,
            refill_interval: Duration::from_secs(60) / rpm,
            state: Mutex::new(RateLimiterState {
                tokens: rpm,
                last_refill: Instant::now(),
            }),
        }
    }

    /// Wait until a token is available, then consume it.
    /// Returns Err if wait would exceed timeout.
    pub async fn acquire(&self, timeout: Duration) -> Result<(), RateLimitError> {
        let deadline = Instant::now() + timeout;

        loop {
            {
                let mut state = self.state.lock().await;
                self.refill(&mut state);

                if state.tokens > 0 {
                    state.tokens -= 1;
                    return Ok(());
                }

                // Calculate wait time until next refill
                let elapsed = state.last_refill.elapsed();
                let wait = if elapsed < self.refill_interval {
                    self.refill_interval - elapsed
                } else {
                    Duration::from_millis(10)
                };

                if Instant::now() + wait > deadline {
                    return Err(RateLimitError::Timeout {
                        timeout_secs: timeout.as_secs(),
                    });
                }

                drop(state);
                tokio::time::sleep(wait).await;
            }
        }
    }

    fn refill(&self, state: &mut RateLimiterState) {
        let elapsed = state.last_refill.elapsed();
        if elapsed >= self.refill_interval {
            let intervals = elapsed.as_millis() / self.refill_interval.as_millis();
            let new_tokens = intervals as u32;
            state.tokens = (state.tokens + new_tokens).min(self.max_tokens);
            // Advance by exact intervals to preserve fractional remainder
            state.last_refill += self.refill_interval * new_tokens;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_acquire_immediate() {
        let limiter = RateLimiter::new(4);
        // Should be able to acquire 4 tokens immediately
        for _ in 0..4 {
            limiter.acquire(Duration::from_secs(1)).await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_acquire_timeout() {
        let limiter = RateLimiter::new(1);
        // Consume the only token
        limiter.acquire(Duration::from_secs(1)).await.unwrap();
        // Next acquire should timeout with a very short timeout
        let result = limiter.acquire(Duration::from_millis(10)).await;
        assert!(result.is_err());
    }
}
