//! Transient-failure retry policy for the openai-compat transport.
//!
//! Local LLM servers (vLLM most commonly) routinely hit transient
//! unavailability when multiple parallel runs share one box: the
//! server briefly refuses connections during model reload, returns
//! 503 under request pressure, or times out on the first byte
//! while another request finishes warming the KV cache. Without a
//! retry policy every such blip aborts the orchestrator and forces
//! the operator to restart the run. With one, the dispatch loop
//! waits-and-retries until either the server recovers or a
//! configurable wall-clock budget elapses.
//!
//! Default budget: 600 seconds (10 minutes). Override via:
//!   - `SIM_FLOW_RETRY_BUDGET_SECS=<seconds>` env var (transport)
//!   - `--llm-retry-budget-secs <seconds>` CLI flag (auto loop)
//!   - VS Code setting `sim-flow.llm.retryBudgetSeconds`
//!     (forwarded into the CLI flag by the extension)
//!
//! `0` disables retries entirely (first failure is the final
//! error), which the e2e_mocked and integration tests use to keep
//! their negative-path assertions deterministic.

use std::time::Duration;

/// Retriable HTTP status codes from the vLLM / openai-compat
/// server. Non-retriable codes (e.g. 400 bad request, 401 auth,
/// 404 model-not-found) fail immediately so we don't loop on an
/// operator-fixable misconfiguration.
pub fn is_retriable_status(code: u16) -> bool {
    matches!(code, 408 | 429 | 502 | 503 | 504)
}

/// Every `ureq::Error::Transport` represents a network-layer
/// failure (connection refused, DNS resolution, read timeout,
/// connection reset, etc.) — all retriable under the policy. The
/// function exists as a named predicate so call sites stay
/// self-documenting.
pub fn is_retriable_transport_error(_e: &ureq::Transport) -> bool {
    true
}

/// Exponential backoff with a cap. Attempt 1 sleeps 1s, attempt 2
/// sleeps 2s, attempt 3 sleeps 4s, ... capped at 30s per attempt.
/// The cap keeps a long retry window (e.g. the 10-minute default)
/// retrying fast enough to actually hit the recovering server
/// instead of sleeping through the whole window.
pub fn backoff_for_attempt(attempt: u32) -> Duration {
    // Clamp the shift count to 8 so a runaway attempt counter
    // can't overflow the `u64` shift on a long-stuck server.
    // 1 << 8 = 256 is well above the 30s cap that follows.
    let shift = attempt.saturating_sub(1).min(8);
    let secs = 1u64 << shift;
    Duration::from_secs(secs.min(30))
}

/// Outcome of one retriable-error decision: either retry after
/// sleeping `backoff` (which is already clamped to whatever
/// remains in the wall-clock budget), or give up because the
/// budget is exhausted.
pub enum RetryDecision {
    Retry(Duration),
    Giveup,
}

/// Given the wall-clock budget total, the moment the dispatch
/// loop started, and the attempt number, decide whether to retry
/// and how long to sleep first.
pub fn decide(budget: Duration, started: std::time::Instant, attempt: u32) -> RetryDecision {
    let elapsed = started.elapsed();
    if elapsed >= budget {
        return RetryDecision::Giveup;
    }
    let remaining = budget - elapsed;
    let backoff = backoff_for_attempt(attempt).min(remaining);
    if backoff.is_zero() {
        RetryDecision::Giveup
    } else {
        RetryDecision::Retry(backoff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn retriable_statuses_match_the_documented_set() {
        for code in [408, 429, 502, 503, 504] {
            assert!(is_retriable_status(code), "code {code} should be retriable");
        }
        for code in [400, 401, 403, 404, 422, 500, 501] {
            assert!(!is_retriable_status(code), "code {code} should NOT retry");
        }
    }

    #[test]
    fn backoff_doubles_until_cap() {
        assert_eq!(backoff_for_attempt(1), Duration::from_secs(1));
        assert_eq!(backoff_for_attempt(2), Duration::from_secs(2));
        assert_eq!(backoff_for_attempt(3), Duration::from_secs(4));
        assert_eq!(backoff_for_attempt(4), Duration::from_secs(8));
        assert_eq!(backoff_for_attempt(5), Duration::from_secs(16));
        // Cap at 30s.
        assert_eq!(backoff_for_attempt(6), Duration::from_secs(30));
        assert_eq!(backoff_for_attempt(20), Duration::from_secs(30));
    }

    #[test]
    fn decide_retries_within_budget() {
        let budget = Duration::from_secs(60);
        let started = Instant::now();
        match decide(budget, started, 1) {
            RetryDecision::Retry(d) => assert_eq!(d, Duration::from_secs(1)),
            RetryDecision::Giveup => panic!("expected retry within budget"),
        }
    }

    #[test]
    fn decide_gives_up_when_budget_exhausted() {
        // Budget = 0 -> immediate give-up.
        let started = Instant::now();
        assert!(matches!(
            decide(Duration::from_secs(0), started, 1),
            RetryDecision::Giveup
        ));
    }

    #[test]
    fn decide_clamps_backoff_to_remaining_budget() {
        // Simulate ~58 seconds already elapsed of a 60-second budget
        // by passing a `started` that's 58s in the past. The
        // attempt-3 backoff is 4s, but only ~2s of budget remains,
        // so the returned sleep should be the smaller value.
        let started = Instant::now() - Duration::from_secs(58);
        match decide(Duration::from_secs(60), started, 3) {
            RetryDecision::Retry(d) => {
                // Tolerate scheduling jitter on slow CI: the
                // returned backoff must be strictly less than the
                // attempt-3 default (4s) since we're capped by
                // remaining budget.
                assert!(
                    d < Duration::from_secs(4),
                    "expected clamped backoff < 4s, got {d:?}"
                );
            }
            RetryDecision::Giveup => {
                // Acceptable: budget could be exhausted by the time
                // the test thread polls.
            }
        }
    }
}
