use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Instant;

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

pub struct RateLimitInfo {
    pub retry_after_secs: f64,
}

enum Inner {
    InMemory {
        buckets: DashMap<String, TokenBucket>,
        max_tokens: f64,
        refill_rate: f64,
    },
    Postgres {
        pool: PgPool,
        max_per_minute: u32,
    },
}

/// Rate limiter with two backends:
/// - InMemory: token bucket per key (tests, single-instance)
/// - Postgres: fixed-window counter per key (production, multi-instance)
pub struct ApiRateLimiter {
    inner: Inner,
}

impl ApiRateLimiter {
    /// Create an in-memory token bucket rate limiter (for tests / single-instance).
    pub fn new_in_memory(max_requests_per_minute: u32) -> Self {
        let max = max_requests_per_minute.max(1) as f64;
        Self {
            inner: Inner::InMemory {
                buckets: DashMap::new(),
                max_tokens: max,
                refill_rate: max / 60.0,
            },
        }
    }

    /// Create a Postgres-backed fixed-window rate limiter (for production / multi-instance).
    pub fn new_postgres(pool: PgPool, max_requests_per_minute: u32) -> Self {
        Self {
            inner: Inner::Postgres {
                pool,
                max_per_minute: max_requests_per_minute,
            },
        }
    }

    /// Check if a request is allowed for the given key.
    /// Returns Ok(()) if allowed, Err(RateLimitInfo) if rate limited.
    pub async fn check(&self, key: &str) -> Result<(), RateLimitInfo> {
        match &self.inner {
            Inner::InMemory {
                buckets,
                max_tokens,
                refill_rate,
            } => {
                let now = Instant::now();
                let mut entry = buckets.entry(key.to_string()).or_insert_with(|| {
                    TokenBucket {
                        tokens: *max_tokens,
                        last_refill: now,
                    }
                });

                let bucket = entry.value_mut();

                let elapsed = now.duration_since(bucket.last_refill);
                let new_tokens = elapsed.as_secs_f64() * refill_rate;
                bucket.tokens = (bucket.tokens + new_tokens).min(*max_tokens);
                bucket.last_refill = now;

                if bucket.tokens >= 1.0 {
                    bucket.tokens -= 1.0;
                    Ok(())
                } else {
                    let deficit = 1.0 - bucket.tokens;
                    let retry_after = deficit / refill_rate;
                    Err(RateLimitInfo {
                        retry_after_secs: retry_after,
                    })
                }
            }
            Inner::Postgres {
                pool,
                max_per_minute,
            } => {
                let row: (i32,) = sqlx::query_as(
                    "INSERT INTO api_rate_limits (key, \"window\", count) \
                     VALUES ($1, date_trunc('minute', now()), 1) \
                     ON CONFLICT (key, \"window\") DO UPDATE SET count = api_rate_limits.count + 1 \
                     RETURNING count",
                )
                .bind(key)
                .fetch_one(pool)
                .await
                .map_err(|_| RateLimitInfo {
                    retry_after_secs: 1.0,
                })?;

                if row.0 as u32 > *max_per_minute {
                    // Compute seconds remaining in the current minute window
                    let now = chrono::Utc::now();
                    let secs_into_minute = now.timestamp() % 60;
                    let retry_after = (60 - secs_into_minute) as f64;
                    Err(RateLimitInfo {
                        retry_after_secs: retry_after,
                    })
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Delete expired rate-limit windows (older than 5 minutes).
pub async fn cleanup_stale_windows(pool: &PgPool) {
    let _ = sqlx::query("DELETE FROM api_rate_limits WHERE \"window\" < now() - interval '5 minutes'")
        .execute(pool)
        .await;
}

/// Axum middleware that enforces API rate limiting per bearer token.
pub async fn rate_limit_middleware(
    State(state): State<Arc<crate::AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let rate_limiter = match &state.rate_limiter {
        Some(rl) => rl,
        None => return next.run(request).await,
    };

    // Extract bearer token as rate limit key
    let key = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.to_string());

    let key = match key {
        Some(k) => k,
        None => {
            // No bearer token -> skip rate limiting (auth layer will reject)
            return next.run(request).await;
        }
    };

    // Hash the token before using as rate limit key to avoid storing raw API keys in DB
    let key_hash = {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        format!("rl:{:x}", hasher.finalize())
    };

    match rate_limiter.check(&key_hash).await {
        Ok(()) => next.run(request).await,
        Err(info) => {
            let retry_after = info.retry_after_secs.ceil() as u64;
            let body = serde_json::json!({
                "error": "rate limit exceeded",
                "retry_after_secs": info.retry_after_secs,
            });
            let mut response = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
            response.headers_mut().insert(
                "Retry-After",
                retry_after.to_string().parse().unwrap(),
            );
            response
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_allows_under_limit() {
        let limiter = ApiRateLimiter::new_in_memory(10);
        for _ in 0..10 {
            assert!(limiter.check("test-key").await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_blocks_over_limit() {
        let limiter = ApiRateLimiter::new_in_memory(5);
        for _ in 0..5 {
            assert!(limiter.check("test-key").await.is_ok());
        }
        // 6th request should be blocked
        let result = limiter.check("test-key").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rate_limiter_separate_keys() {
        let limiter = ApiRateLimiter::new_in_memory(2);
        assert!(limiter.check("key-a").await.is_ok());
        assert!(limiter.check("key-a").await.is_ok());
        assert!(limiter.check("key-a").await.is_err());
        // Different key should still have tokens
        assert!(limiter.check("key-b").await.is_ok());
    }
}
