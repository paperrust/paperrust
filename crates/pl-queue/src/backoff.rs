//! Exponential backoff for the background loops (worker claim, beat, LISTEN).
//!
//! E2E finding 2026-11-08: when `paperrust_task_queue` was missing (DB dropped and recreated while the
//! server was running), the worker loop ran without a pause and wrote ~1000 error lines per second.
//! Now: wait `initial · 2ⁿ` up to `max` (default 0.5 s → 30 s), and one log line **per
//! backoff step** (so at most one per `max` once the top step is reached).
//! No upstream counterpart (Celery/kombu reconnect with their own retry logic).

use std::time::Duration;

/// State of a backoff.
#[derive(Debug, Clone)]
pub struct Backoff {
    initial: Duration,
    max: Duration,
    /// Consecutive failures since the last success.
    failures: u32,
}

/// Result of [`Backoff::fail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// Wait this long before the next attempt.
    pub delay: Duration,
    /// Running number of the failure (1 = first failure after a success).
    pub attempt: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Duration::from_millis(500), Duration::from_secs(30))
    }
}

impl Backoff {
    pub fn new(initial: Duration, max: Duration) -> Self {
        Self { initial: initial.max(Duration::from_millis(1)), max: max.max(initial), failures: 0 }
    }

    /// One failure: wait time of the new step. Whoever logs one line per failure and then waits
    /// `delay` thus logs exactly one line per step (one per `max` at the top step).
    pub fn fail(&mut self) -> Step {
        self.failures = self.failures.saturating_add(1);
        let exp = (self.failures - 1).min(20);
        let delay = self.initial.saturating_mul(1u32 << exp).min(self.max);
        Step { delay, attempt: self.failures }
    }

    /// Success: reset the backoff. Returns: number of failures before it (for a
    /// "back to normal" message), 0 if there were none.
    pub fn succeed(&mut self) -> u32 {
        std::mem::take(&mut self.failures)
    }

    /// Currently in the failure state?
    pub fn failing(&self) -> bool {
        self.failures > 0
    }
}

/// SQLSTATE `42P01` (undefined_table): the queue tables are missing, e.g. because the DB
/// was recreated under the running server → apply the migration again.
pub fn is_undefined_table(e: &crate::QueueError) -> bool {
    match e {
        crate::QueueError::Db(sqlx::Error::Database(d)) => d.code().as_deref() == Some("42P01"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_up_to_max_and_resets() {
        let mut b = Backoff::default();
        let delays: Vec<u64> = (0..9).map(|_| b.fail().delay.as_millis() as u64).collect();
        assert_eq!(delays, vec![500, 1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
        assert!(b.failing());
        assert_eq!(b.succeed(), 9);
        assert!(!b.failing());
        assert_eq!(b.succeed(), 0);
        assert_eq!(b.fail().delay, Duration::from_millis(500));
    }

    #[test]
    fn many_failures_do_not_overflow() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(5));
        for _ in 0..100 {
            assert!(b.fail().delay <= Duration::from_secs(5));
        }
    }

    /// Log rate: one line per step → at most ~26 lines in 10 minutes of continuous failure instead of ~600,000.
    #[test]
    fn log_lines_per_ten_minutes() {
        let mut b = Backoff::default();
        let (mut t, mut lines) = (Duration::ZERO, 0);
        while t < Duration::from_secs(600) {
            lines += 1;
            t += b.fail().delay;
        }
        assert!(lines <= 26, "{lines}");
    }
}
