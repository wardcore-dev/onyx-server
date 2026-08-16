use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Per-user sliding-window rate limiter.
/// Stores the timestamps of recent actions for each user and rejects
/// requests that would exceed `max_per_minute` within any 60-second window.
pub struct RateLimiter {
    windows: Mutex<HashMap<String, Vec<Instant>>>,
    max_per_minute: u32,
}

impl RateLimiter {
    pub fn new(max_per_minute: u32) -> Arc<Self> {
        Arc::new(Self {
            windows: Mutex::new(HashMap::new()),
            max_per_minute,
        })
    }

    /// Returns `true` if the action is allowed and records it.
    /// Returns `false` if the user has exceeded the rate limit.
    pub fn check_and_record(&self, username: &str) -> bool {
        let mut windows = self.windows.lock().unwrap();
        let now = Instant::now();
        let timestamps = windows.entry(username.to_string()).or_default();

        // Drop timestamps outside the 60-second window
        timestamps.retain(|t| now.duration_since(*t).as_secs() < 60);

        if timestamps.len() >= self.max_per_minute as usize {
            return false;
        }

        timestamps.push(now);
        true
    }
}

pub type SharedRateLimiter = Arc<RateLimiter>;

/// Tracks the last message timestamp per user to enforce a configurable
/// per-group "slow mode" delay (distinct from the global flood-protection
/// RateLimiter above, which caps messages per minute regardless of pacing).
pub struct SlowModeTracker {
    last_post: Mutex<HashMap<String, Instant>>,
}

impl SlowModeTracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { last_post: Mutex::new(HashMap::new()) })
    }

    /// Returns `Ok(())` and records the post time if the user may post now.
    /// Returns `Err(seconds_remaining)` if the user must wait longer.
    pub fn check_and_record(&self, username: &str, seconds: i64) -> Result<(), i64> {
        if seconds <= 0 {
            return Ok(());
        }
        let mut map = self.last_post.lock().unwrap();
        let now = Instant::now();
        if let Some(last) = map.get(username) {
            let elapsed = now.duration_since(*last).as_secs() as i64;
            if elapsed < seconds {
                return Err(seconds - elapsed);
            }
        }
        map.insert(username.to_string(), now);
        Ok(())
    }
}

pub type SharedSlowModeTracker = Arc<SlowModeTracker>;
