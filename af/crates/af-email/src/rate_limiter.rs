use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Token-bucket rate limiter (same pattern as af-web-gateway).
pub struct RateLimiter {
    max_tokens: u32,
    refill_interval: Duration,
    state: Mutex<RateLimiterState>,
}

struct RateLimiterState {
    tokens: u32,
    last_refill: Instant,
}

#[derive(Debug)]
pub enum RateLimitError {
    Exceeded { wait_secs: f64 },
    Timeout { timeout_secs: u64 },
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RateLimitError::Exceeded { wait_secs } => {
                write!(f, "rate limit exceeded, retry after {:.1}s", wait_secs)
            }
            RateLimitError::Timeout { timeout_secs } => {
                write!(f, "rate limit timeout after {}s", timeout_secs)
            }
        }
    }
}

impl RateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        let rpm = requests_per_minute.max(1);
        Self {
            max_tokens: rpm,
            refill_interval: Duration::from_secs_f64(60.0 / rpm as f64),
            state: Mutex::new(RateLimiterState {
                tokens: rpm,
                last_refill: Instant::now(),
            }),
        }
    }

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
            }
            if Instant::now() >= deadline {
                return Err(RateLimitError::Timeout {
                    timeout_secs: timeout.as_secs(),
                });
            }
            let sleep_time = self.refill_interval.min(deadline - Instant::now());
            tokio::time::sleep(sleep_time).await;
        }
    }

    fn refill(&self, state: &mut RateLimiterState) {
        let now = Instant::now();
        let elapsed = now.duration_since(state.last_refill);
        let intervals = (elapsed.as_secs_f64() / self.refill_interval.as_secs_f64()) as u32;
        if intervals > 0 {
            state.tokens = (state.tokens + intervals).min(self.max_tokens);
            state.last_refill += self.refill_interval * intervals;
        }
    }
}
