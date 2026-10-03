//! Per-IP sliding-window login limiter (ported from vexboard).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct LoginRateLimiter {
    state: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
    max_attempts: u32,
    window: Duration,
}

impl LoginRateLimiter {
    pub fn new(max_attempts: u32, window_secs: u64) -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
            max_attempts,
            window: Duration::from_secs(window_secs),
        }
    }

    /// `true` if the attempt is allowed. Every allowed call is recorded.
    pub fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let cutoff = now.checked_sub(self.window);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let attempts = state.entry(ip).or_default();
        if let Some(cutoff) = cutoff {
            while attempts.front().is_some_and(|t| *t < cutoff) {
                attempts.pop_front();
            }
        }
        let allowed = (attempts.len() as u32) < self.max_attempts;
        if allowed {
            attempts.push_back(now);
        }
        if attempts.is_empty() {
            state.remove(&ip);
        }
        allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max_attempts_and_isolates_ips() {
        let limiter = LoginRateLimiter::new(2, 60);
        let a: IpAddr = "127.0.0.1".parse().unwrap();
        let b: IpAddr = "127.0.0.2".parse().unwrap();
        assert!(limiter.check(a));
        assert!(limiter.check(a));
        assert!(!limiter.check(a));
        assert!(limiter.check(b));
    }

    #[test]
    fn zero_attempts_blocks_everything_without_leaking_entries() {
        let limiter = LoginRateLimiter::new(0, 60);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(!limiter.check(ip));
        assert!(!limiter.state.lock().unwrap().contains_key(&ip));
    }
}
