//! Pacing and exponential backoff logic for periodic Shazam acoustic queries.

use std::time::{Duration, Instant};

/// Steady-state polling interval between Shazam queries during playback.
pub(super) const SHAZAM_RECHECK: Duration = Duration::from_secs(8);
/// Accelerated polling interval while waiting for a confirmation match.
pub(super) const SHAZAM_CONFIRM: Duration = Duration::from_secs(3);
/// Maximum consecutive quick confirmation checks before reverting to steady-state interval.
const MAX_QUICK_CHECKS: u8 = 4;
/// Minimum and maximum wait times following a "no match" response.
pub(super) const SHAZAM_RETRY: Duration = Duration::from_secs(3);
const SHAZAM_RETRY_MAX: Duration = Duration::from_secs(20);
/// Maximum exponential backoff interval after repeated network or server errors.
const SHAZAM_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Delay before retrying when the audio ring buffer does not have enough samples yet.
pub(super) const SHAZAM_NO_AUDIO_RETRY: Duration = Duration::from_secs(1);

/// Pacing state determining when the next Shazam acoustic query is due.
pub(super) struct Schedule {
    pub(super) next_at: Instant,
    /// Consecutive failed request count.
    pub(super) failures: u32,
    /// Consecutive quick confirmation check count.
    quick_checks: u8,
}

impl Default for Schedule {
    /// Schedules an immediate query.
    fn default() -> Self {
        Self {
            next_at: Instant::now(),
            failures: 0,
            quick_checks: 0,
        }
    }
}

impl Schedule {
    pub(super) fn is_due(&self, now: Instant) -> bool {
        now >= self.next_at
    }

    /// Checks if requests are currently backing off due to errors.
    pub(super) fn is_backing_off(&self) -> bool {
        self.failures > 0
    }

    /// Checks if the query can be scheduled earlier without violating error backoff.
    pub(super) fn may_hurry(&self, now: Instant) -> bool {
        self.failures == 0 && self.next_at > now + SHAZAM_RETRY
    }

    /// Accelerates the next check to `now` unless currently backing off.
    pub(super) fn hurry(&mut self, now: Instant) {
        if self.failures == 0 {
            self.next_at = self.next_at.min(now);
        }
    }

    fn wait(&mut self, wait: Duration) {
        self.next_at = Instant::now() + wait;
    }

    /// Updates schedule after a match. Uses a shorter interval when `awaits_confirmation` is true.
    pub(super) fn after_match(&mut self, awaits_confirmation: bool) {
        self.failures = 0;
        let quick = awaits_confirmation && self.quick_checks < MAX_QUICK_CHECKS;
        self.quick_checks = if quick { self.quick_checks + 1 } else { 0 };
        self.wait(if quick {
            SHAZAM_CONFIRM
        } else {
            SHAZAM_RECHECK
        });
    }

    /// Updates schedule after a no-match response, honoring the server's requested retry delay if given.
    pub(super) fn after_no_match(&mut self, retry_after: Option<Duration>) {
        self.failures = 0;
        self.quick_checks = 0;
        self.wait(
            retry_after
                .unwrap_or(SHAZAM_RETRY)
                .clamp(SHAZAM_RETRY, SHAZAM_RETRY_MAX),
        );
    }

    /// Pauses briefly when insufficient audio has accumulated in the tap buffer.
    pub(super) fn after_no_audio(&mut self) {
        self.wait(SHAZAM_NO_AUDIO_RETRY);
    }

    /// Applies exponential backoff after a failed request. Returns the backoff duration for logging.
    pub(super) fn after_failure(&mut self, retry_after: Option<Duration>) -> Duration {
        self.failures += 1;
        let wait = backoff(self.failures).max(retry_after.unwrap_or_default());
        self.wait(wait);
        wait
    }
}

/// Computes exponential backoff delay starting at 3s and doubling up to 60s.
fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(8);
    (SHAZAM_RETRY * 2u32.pow(doublings)).min(SHAZAM_BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn due_in(schedule: &Schedule) -> u64 {
        let wait = schedule.next_at.saturating_duration_since(Instant::now());
        wait.as_secs_f64().round() as u64
    }

    #[test]
    fn test_failures_back_off_by_doubling_up_to_a_minute() {
        let mut schedule = Schedule::default();
        for wait in [3, 6, 12, 24, 48, 60, 60] {
            schedule.after_failure(None);
            assert_eq!(due_in(&schedule), wait);
        }
        assert_eq!(backoff(30), SHAZAM_BACKOFF_MAX);
        // A refusal may ask for more
        assert_eq!(
            schedule
                .after_failure(Some(Duration::from_secs(90)))
                .as_secs(),
            90
        );
    }

    #[test]
    fn test_no_audio_is_neither_a_failure_nor_an_answer() {
        let mut schedule = Schedule::default();
        schedule.after_failure(None);
        schedule.after_match(true);
        schedule.after_failure(None);
        let (failures, quick) = (schedule.failures, schedule.quick_checks);

        schedule.after_no_audio();
        assert_eq!(
            (schedule.failures, schedule.quick_checks),
            (failures, quick)
        );
        assert_eq!(due_in(&schedule), SHAZAM_NO_AUDIO_RETRY.as_secs());
    }

    #[test]
    fn test_an_answer_that_needs_a_second_one_is_checked_quickly_a_few_times_only() {
        let mut schedule = Schedule::default();
        for _ in 0..MAX_QUICK_CHECKS {
            schedule.after_match(true);
            assert_eq!(due_in(&schedule), SHAZAM_CONFIRM.as_secs());
        }
        schedule.after_match(true);
        assert_eq!(due_in(&schedule), SHAZAM_RECHECK.as_secs());
        // Settled answers go back to the usual pace, and forget the quick checks
        schedule.after_match(false);
        assert_eq!(due_in(&schedule), SHAZAM_RECHECK.as_secs());
        schedule.after_match(true);
        assert_eq!(due_in(&schedule), SHAZAM_CONFIRM.as_secs());
    }

    #[test]
    fn test_a_running_backoff_is_never_hurried() {
        let mut schedule = Schedule::default();
        schedule.after_match(false);
        assert!(schedule.may_hurry(Instant::now()));
        schedule.after_failure(None);
        schedule.after_failure(None);
        let now = Instant::now();
        assert!(!schedule.may_hurry(now));
        schedule.hurry(now);
        assert!(!schedule.is_due(now));
        assert_eq!(due_in(&schedule), 6);
    }

    #[test]
    fn test_no_match_waits_as_shazam_asks_within_limits() {
        let mut schedule = Schedule::default();
        schedule.after_no_match(None);
        assert_eq!(due_in(&schedule), SHAZAM_RETRY.as_secs());
        schedule.after_no_match(Some(Duration::from_secs(9)));
        assert_eq!(due_in(&schedule), 9);
        schedule.after_no_match(Some(Duration::from_secs(600)));
        assert_eq!(due_in(&schedule), SHAZAM_RETRY_MAX.as_secs());
    }
}
