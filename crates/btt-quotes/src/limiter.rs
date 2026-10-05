//! Limits how fast requests go to a booking site.

use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Spaces requests at least a minimum time apart, and stops them for a while after a failure.
pub struct RequestLimiter {
    /// The least time between two requests.
    minimum_interval: Duration,
    /// How long requests stay stopped after a failure.
    failure_pause: Duration,
    /// When the last request went out and until when requests are stopped.
    state: Mutex<LimiterState>,
}

/// What the limiter remembers.
#[derive(Default)]
struct LimiterState {
    /// When the last request was allowed.
    last_request: Option<Instant>,
    /// Requests are stopped until this moment.
    paused_until: Option<Instant>,
}

impl RequestLimiter {
    /// Creates a limiter.
    pub fn new(minimum_interval: Duration, failure_pause: Duration) -> Self {
        Self {
            minimum_interval,
            failure_pause,
            state: Mutex::new(LimiterState::default()),
        }
    }

    /// Waits until a request may go out. Returns `false` without waiting when requests are
    /// stopped after a failure.
    ///
    /// The lock is held while waiting, so callers queue up and requests leave one at a time.
    pub async fn wait_for_turn(&self) -> bool {
        let mut state = self.state.lock().await;
        if state
            .paused_until
            .is_some_and(|until| Instant::now() < until)
        {
            return false;
        }
        if let Some(last) = state.last_request {
            let next = last + self.minimum_interval;
            if let Some(wait) = next.checked_duration_since(Instant::now()) {
                tokio::time::sleep(wait).await;
            }
        }
        state.last_request = Some(Instant::now());
        true
    }

    /// Stops requests for the failure pause.
    pub async fn report_failure(&self) {
        self.state.lock().await.paused_until = Some(Instant::now() + self.failure_pause);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn requests_leave_at_least_the_interval_apart() {
        let limiter = RequestLimiter::new(Duration::from_secs(2), Duration::from_mins(10));
        let start = Instant::now();
        assert!(limiter.wait_for_turn().await);
        assert!(limiter.wait_for_turn().await);
        assert!(limiter.wait_for_turn().await);
        assert!(start.elapsed() >= Duration::from_secs(4));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_stops_requests_for_the_pause() {
        let limiter = RequestLimiter::new(Duration::from_secs(1), Duration::from_mins(10));
        assert!(limiter.wait_for_turn().await);
        limiter.report_failure().await;
        assert!(!limiter.wait_for_turn().await);
    }
}
