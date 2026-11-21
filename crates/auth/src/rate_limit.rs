use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
struct Bucket {
    attempts: u32,
    window_start: Instant,
}

#[derive(Clone, Debug)]
pub struct LoginRateLimiter {
    max_attempts: u32,
    window: Duration,
    max_tracked_keys: usize,
    buckets: Arc<Mutex<HashMap<String, Bucket>>>,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new(8, Duration::from_secs(60), 4096)
    }
}

impl LoginRateLimiter {
    pub fn new(max_attempts: u32, window: Duration, max_tracked_keys: usize) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            window,
            max_tracked_keys: max_tracked_keys.max(16),
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check whether `key` is within the allowed login attempt rate and record an attempt.
    /// Returns `Err(retry_after_secs)` if the key has exceeded `max_attempts` in the active window.
    pub fn check_and_record(&self, key: &str, now: Instant) -> Result<(), u64> {
        let normalized = key.trim().to_ascii_lowercase();
        let mut guard = match self.buckets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if guard.len() >= self.max_tracked_keys && !guard.contains_key(&normalized) {
            let window = self.window;
            guard.retain(|_, b| now.saturating_duration_since(b.window_start) < window);
            if guard.len() >= self.max_tracked_keys
                && let Some(oldest_key) = guard
                    .iter()
                    .min_by_key(|(_, b)| b.window_start)
                    .map(|(k, _)| k.clone())
            {
                guard.remove(&oldest_key);
            }
        }

        let entry = guard.entry(normalized).or_insert(Bucket {
            attempts: 0,
            window_start: now,
        });

        let elapsed = now.saturating_duration_since(entry.window_start);
        if elapsed >= self.window {
            entry.attempts = 0;
            entry.window_start = now;
        }

        if entry.attempts >= self.max_attempts {
            let remaining = self.window.saturating_sub(elapsed).as_secs().max(1);
            return Err(remaining);
        }

        entry.attempts += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max_attempts_and_recovers_after_window() {
        let limiter = LoginRateLimiter::new(3, Duration::from_secs(60), 32);
        let t0 = Instant::now();

        assert_eq!(limiter.check_and_record("user@example.com", t0), Ok(()));
        assert_eq!(limiter.check_and_record("USER@example.com", t0), Ok(()));
        assert_eq!(limiter.check_and_record("user@example.com", t0), Ok(()));

        let err = limiter
            .check_and_record("user@example.com", t0 + Duration::from_secs(10))
            .unwrap_err();
        assert_eq!(err, 50);

        assert_eq!(
            limiter.check_and_record("user@example.com", t0 + Duration::from_secs(61)),
            Ok(())
        );
    }
}
