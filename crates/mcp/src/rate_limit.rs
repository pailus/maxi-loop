use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, RwLock};

use governor::clock::{Clock, DefaultClock};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};

const MAX_TRACKED_TOKENS: usize = 10_000;

struct TokenLimiterEntry {
    rpm: u32,
    limiter: Arc<DefaultDirectRateLimiter>,
}

/// Per-token GCRA rate limiter for MCP Streamable HTTP calls (`F-MCP-9`).
#[derive(Clone, Default)]
pub struct McpRateLimiter {
    entries: Arc<RwLock<HashMap<i64, TokenLimiterEntry>>>,
}

impl McpRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check(&self, token_id: i64, rate_limit_per_minute: u32) -> Result<(), u64> {
        let rpm = rate_limit_per_minute.max(1);
        let limiter = self.get_or_create_limiter(token_id, rpm);

        match limiter.check() {
            Ok(()) => Ok(()),
            Err(not_until) => {
                let wait = not_until.wait_time_from(DefaultClock::default().now());
                Err(wait.as_secs().max(1))
            }
        }
    }

    fn get_or_create_limiter(&self, token_id: i64, rpm: u32) -> Arc<DefaultDirectRateLimiter> {
        if let Ok(guard) = self.entries.read()
            && let Some(entry) = guard.get(&token_id)
            && entry.rpm == rpm
        {
            return Arc::clone(&entry.limiter);
        }

        let quota = Quota::per_minute(NonZeroU32::new(rpm).unwrap_or(NonZeroU32::MIN))
            .allow_burst(NonZeroU32::new(rpm.clamp(1, 20)).unwrap_or(NonZeroU32::MIN));
        let limiter = Arc::new(RateLimiter::direct(quota));

        if let Ok(mut guard) = self.entries.write() {
            if guard.len() >= MAX_TRACKED_TOKENS && !guard.contains_key(&token_id) {
                guard.clear();
            }
            guard.insert(
                token_id,
                TokenLimiterEntry {
                    rpm,
                    limiter: Arc::clone(&limiter),
                },
            );
        }

        limiter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_per_token_rate_limit_independently() {
        let rl = McpRateLimiter::new();
        assert!(rl.check(10, 2).is_ok());
        assert!(rl.check(10, 2).is_ok());
        let retry = rl.check(10, 2).expect_err("3rd call should rate limit");
        assert!(retry >= 1);

        // Separate token ID has its own independent bucket.
        assert!(rl.check(11, 2).is_ok());
    }
}
