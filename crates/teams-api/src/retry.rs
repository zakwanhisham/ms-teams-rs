//! Retry policy: jittered exponential backoff with `Retry-After` support.
//!
//! Only *status-based* failures are retried (throttling and transient front-door
//! errors); transport errors are left to the caller, because the write path (Phase 5)
//! will post messages over the same plumbing and blind retries there could duplicate
//! sends.

use std::time::Duration;

use chrono::{DateTime, Utc};

/// Statuses worth another attempt: throttling plus transient upstream/front-door errors.
const RETRYABLE: [u16; 4] = [429, 502, 503, 504];

/// Upper bound for the backoff exponent — beyond that the schedule is capped anyway and
/// `f64::powi` would overflow.
const MAX_EXPONENT: u32 = 16;

/// How a client backs off when a service asks it to slow down or fails transiently.
#[derive(Debug, Clone, PartialEq)]
pub struct RetryPolicy {
    /// Total attempts including the first one (minimum `1` = no retrying).
    pub max_attempts: u32,
    /// Delay before the second attempt, before growth and jitter are applied.
    pub base_delay: Duration,
    /// Ceiling applied to the computed delay (and to the jittered result).
    pub max_delay: Duration,
    /// Growth factor per attempt (`2.0` = doubling).
    pub factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(400),
            max_delay: Duration::from_secs(8),
            factor: 2.0,
        }
    }
}

impl RetryPolicy {
    /// Whether `status` is worth another attempt.
    #[must_use]
    pub fn is_retryable(status: u16) -> bool {
        RETRYABLE.contains(&status)
    }

    /// Delay to sleep before attempt number `attempt + 1`, where `attempt` counts the
    /// failed tries so far (`1` after the first failure).
    ///
    /// - With a service-provided `Retry-After`, that value is honoured **as-is** — the
    ///   service knows when it wants us back (no jitter: waking up earlier than asked
    ///   would defeat it).
    /// - Otherwise: `base_delay × factor^(attempt-1)`, capped at `max_delay`, then
    ///   jittered by ±25 % so that clients caught in a 429 storm desynchronise.
    #[must_use]
    pub fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(after) = retry_after {
            return after;
        }

        let exponent = attempt.saturating_sub(1).min(MAX_EXPONENT);
        let grown = self.base_delay.mul_f64(self.factor.powi(i32::try_from(exponent).unwrap_or(1)));
        let capped = grown.min(self.max_delay);
        jitter(capped).min(self.max_delay)
    }
}

/// Spread a delay by up to ±25 % using the wall-clock sub-second reading.
///
/// Dependency-free by design: a millisecond-granular clock reading is enough entropy to
/// stop a fleet of clients retrying in lockstep after a throttle.
fn jitter(delay: Duration) -> Duration {
    let fraction = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| f64::from(elapsed.subsec_nanos()) % 10_000_000.0 / 10_000_000.0);
    delay.mul_f64(0.75 + 0.5 * fraction)
}

/// Parse a `Retry-After` header: delta-seconds (what Teams front doors send) or an
/// HTTP-date. Returns `None` when the value is unparseable so the caller falls back to
/// its own schedule instead of waiting forever.
#[must_use]
pub fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<Duration> {
    let value = value.trim();

    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    // A date in the past means "retry immediately" — clamp instead of erroring.
    Some((date.with_timezone(&now.timezone()) - now).to_std().unwrap_or(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RetryPolicy {
        RetryPolicy::default()
    }

    #[test]
    fn retryable_statuses_are_throttling_and_transient_gateway_errors() {
        for status in [429, 502, 503, 504] {
            assert!(RetryPolicy::is_retryable(status), "{status} should be retried");
        }
        for status in [200, 201, 400, 401, 403, 404, 500, 501] {
            assert!(!RetryPolicy::is_retryable(status), "{status} must not be retried");
        }
    }

    /// 401 in particular: it is handled by the re-mint path, never by backoff.
    #[test]
    fn unauthorized_is_not_a_retryable_status() {
        assert!(!RetryPolicy::is_retryable(401));
    }

    #[test]
    fn backoff_grows_exponentially_and_stays_within_the_jitter_window() {
        let policy = policy();

        // Each delay must sit in the [0.75, 1.25] window around base × factor^(attempt-1).
        for attempt in 1..=5u32 {
            let nominal =
                policy.base_delay.mul_f64(policy.factor.powi(i32::try_from(attempt - 1).unwrap()));
            let delay = policy.delay(attempt, None);
            assert!(
                delay >= nominal.mul_f64(0.75) && delay <= nominal.mul_f64(1.25),
                "attempt {attempt}: {delay:?} outside ±25 % of {nominal:?}"
            );
        }
    }

    #[test]
    fn backoff_never_exceeds_max_delay() {
        let policy = policy();
        for attempt in [1u32, 5, 20, 500] {
            let delay = policy.delay(attempt, None);
            assert!(delay <= policy.max_delay, "attempt {attempt}: {delay:?} exceeds the cap");
            assert!(delay >= policy.base_delay.mul_f64(0.75), "attempt {attempt} lost its floor");
        }
    }

    /// The service's own instruction wins over our schedule (no jitter, no cap).
    #[test]
    fn retry_after_is_honoured_as_is() {
        let policy = policy();
        assert_eq!(policy.delay(1, Some(Duration::from_secs(1))), Duration::from_secs(1));
        assert_eq!(
            policy.delay(3, Some(Duration::from_secs(60))),
            Duration::from_secs(60),
            "a long service-requested pause is not truncated"
        );
    }

    #[test]
    fn zero_attempts_policy_still_reports_a_delay() {
        // A caller that does not want retries still gets a sane number; the *loop* is
        // what stops, via max_attempts.
        let policy = RetryPolicy { max_attempts: 1, ..RetryPolicy::default() };
        assert_eq!(policy.max_attempts, 1);
        assert!(policy.delay(1, None) >= Duration::ZERO);
    }

    #[test]
    fn retry_after_parses_delta_seconds() {
        let now = Utc::now();
        assert_eq!(parse_retry_after("1", now), Some(Duration::from_secs(1)));
        assert_eq!(parse_retry_after(" 120 ", now), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after("0", now), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("-5", now), None, "negative seconds are not a valid form");
        assert_eq!(parse_retry_after("", now), None);
        assert_eq!(parse_retry_after("soon", now), None);
    }

    #[test]
    fn retry_after_parses_http_dates() {
        let now = DateTime::parse_from_rfc2822("Tue, 27 Oct 2026 12:00:00 GMT")
            .expect("date")
            .with_timezone(&Utc);

        let future =
            parse_retry_after("Tue, 27 Oct 2026 12:00:30 GMT", now).expect("future date parses");
        assert_eq!(future, Duration::from_secs(30));

        let past = parse_retry_after("Tue, 27 Oct 2026 11:59:00 GMT", now).expect("date parses");
        assert_eq!(past, Duration::ZERO, "a past date means retry now");
    }
}
