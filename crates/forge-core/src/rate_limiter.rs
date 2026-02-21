use std::fs;
use std::path::Path;

pub struct RateLimiter {
    max_calls_per_hour: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub current_count: u32,
    pub remaining: u32,
}

#[derive(Debug, Clone)]
pub struct RateLimitState {
    pub count: u32,
    pub last_reset_epoch: u64,
}

impl RateLimiter {
    pub fn new(max_calls_per_hour: u32) -> Self {
        Self { max_calls_per_hour }
    }

    pub fn check_and_increment(
        &self,
        runtime_dir: &Path,
        now_epoch: u64,
    ) -> anyhow::Result<RateLimitResult> {
        let state = self.load_state(runtime_dir)?;
        let (count, last_reset) = self.maybe_reset(state, now_epoch);

        if count >= self.max_calls_per_hour {
            self.persist_state(runtime_dir, count, last_reset)?;
            return Ok(RateLimitResult {
                allowed: false,
                current_count: count,
                remaining: 0,
            });
        }

        let new_count = count + 1;
        self.persist_state(runtime_dir, new_count, last_reset)?;

        Ok(RateLimitResult {
            allowed: true,
            current_count: new_count,
            remaining: self.max_calls_per_hour.saturating_sub(new_count),
        })
    }

    pub fn get_state(&self, runtime_dir: &Path) -> anyhow::Result<RateLimitState> {
        self.load_state(runtime_dir)
    }

    pub fn reset(&self, runtime_dir: &Path, now_epoch: u64) -> anyhow::Result<()> {
        self.persist_state(runtime_dir, 0, now_epoch)
    }

    fn load_state(&self, runtime_dir: &Path) -> anyhow::Result<RateLimitState> {
        let count_path = runtime_dir.join(".call_count");
        let reset_path = runtime_dir.join(".last_reset");

        let count = fs::read_to_string(&count_path)
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0);

        let last_reset = fs::read_to_string(&reset_path)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0);

        Ok(RateLimitState {
            count,
            last_reset_epoch: last_reset,
        })
    }

    fn maybe_reset(&self, state: RateLimitState, now_epoch: u64) -> (u32, u64) {
        if now_epoch.saturating_sub(state.last_reset_epoch) >= 3600 {
            (0, now_epoch)
        } else {
            (state.count, state.last_reset_epoch)
        }
    }

    fn persist_state(&self, runtime_dir: &Path, count: u32, last_reset: u64) -> anyhow::Result<()> {
        let count_path = runtime_dir.join(".call_count");
        let reset_path = runtime_dir.join(".last_reset");

        fs::write(&count_path, count.to_string())?;
        fs::write(&reset_path, last_reset.to_string())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn allows_first_call() {
        let dir = tempdir().expect("tempdir");
        let limiter = RateLimiter::new(100);

        let result = limiter
            .check_and_increment(dir.path(), 1000)
            .expect("check");

        assert!(result.allowed);
        assert_eq!(result.current_count, 1);
        assert_eq!(result.remaining, 99);
    }

    #[test]
    fn blocks_when_limit_reached() {
        let dir = tempdir().expect("tempdir");
        let limiter = RateLimiter::new(3);

        limiter.check_and_increment(dir.path(), 1000).expect("1");
        limiter.check_and_increment(dir.path(), 1001).expect("2");
        limiter.check_and_increment(dir.path(), 1002).expect("3");
        let result = limiter.check_and_increment(dir.path(), 1003).expect("4");

        assert!(!result.allowed);
        assert_eq!(result.current_count, 3);
        assert_eq!(result.remaining, 0);
    }

    #[test]
    fn resets_after_one_hour() {
        let dir = tempdir().expect("tempdir");
        let limiter = RateLimiter::new(2);

        limiter.check_and_increment(dir.path(), 1000).expect("1");
        limiter.check_and_increment(dir.path(), 1001).expect("2");
        let blocked = limiter
            .check_and_increment(dir.path(), 1002)
            .expect("blocked");
        assert!(!blocked.allowed);

        let after_reset = limiter
            .check_and_increment(dir.path(), 5000)
            .expect("after reset");

        assert!(after_reset.allowed);
        assert_eq!(after_reset.current_count, 1);
    }

    #[test]
    fn reset_clears_count() {
        let dir = tempdir().expect("tempdir");
        let limiter = RateLimiter::new(100);

        limiter.check_and_increment(dir.path(), 1000).expect("1");
        limiter.check_and_increment(dir.path(), 1001).expect("2");

        limiter.reset(dir.path(), 2000).expect("reset");

        let state = limiter.get_state(dir.path()).expect("state");
        assert_eq!(state.count, 0);
        assert_eq!(state.last_reset_epoch, 2000);
    }

    #[test]
    fn reset_at_exactly_one_hour() {
        let dir = tempdir().expect("tempdir");
        let limiter = RateLimiter::new(2);

        limiter.check_and_increment(dir.path(), 1000).expect("1");
        limiter.check_and_increment(dir.path(), 1001).expect("2");

        let at_one_hour = limiter
            .check_and_increment(dir.path(), 4600)
            .expect("at one hour");
        assert!(at_one_hour.allowed);
        assert_eq!(at_one_hour.current_count, 1);

        let after_one_hour = limiter
            .check_and_increment(dir.path(), 5000)
            .expect("after hour");
        assert!(after_one_hour.allowed);
        assert_eq!(after_one_hour.current_count, 2);

        let over_limit = limiter
            .check_and_increment(dir.path(), 5001)
            .expect("over limit");
        assert!(!over_limit.allowed);
    }

    #[test]
    fn persists_state_between_instances() {
        let dir = tempdir().expect("tempdir");

        let limiter1 = RateLimiter::new(5);
        limiter1.check_and_increment(dir.path(), 1000).expect("1");
        limiter1.check_and_increment(dir.path(), 1001).expect("2");

        let limiter2 = RateLimiter::new(5);
        let state = limiter2.get_state(dir.path()).expect("state");

        assert_eq!(state.count, 2);
    }

    #[test]
    fn zero_limit_always_blocks() {
        let dir = tempdir().expect("tempdir");
        let limiter = RateLimiter::new(0);

        let result = limiter
            .check_and_increment(dir.path(), 1000)
            .expect("check");

        assert!(!result.allowed);
        assert_eq!(result.current_count, 0);
        assert_eq!(result.remaining, 0);
    }

    #[test]
    fn saturating_sub_prevents_underflow() {
        let dir = tempdir().expect("tempdir");
        let limiter = RateLimiter::new(1);

        limiter.check_and_increment(dir.path(), 1000).expect("1");

        let result = limiter.check_and_increment(dir.path(), 1001).expect("2");
        assert_eq!(result.remaining, 0);
    }

    #[test]
    fn get_state_returns_default_for_missing_files() {
        let dir = tempdir().expect("tempdir");
        let limiter = RateLimiter::new(100);

        let state = limiter.get_state(dir.path()).expect("state");

        assert_eq!(state.count, 0);
        assert_eq!(state.last_reset_epoch, 0);
    }
}
