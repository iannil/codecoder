// Provider health probe for daemon-level liveliness checking.
// Before each workgraph tick, a tiny probe request verifies the LLM endpoint
// is reachable and responsive. Consecutive failures suppress work and trigger
// alerts instead of burning retry budget on every tick.

use crate::provider::{CompletionRequest, Provider};
use std::time::{Duration, Instant};

/// Health tracking state for one provider endpoint.
#[derive(Debug, Clone)]
pub struct HealthState {
    /// How many consecutive probes have failed.
    pub consecutive_failures: u32,
    /// When the last successful probe completed (if any).
    pub last_success: Option<Instant>,
    /// When the last failure occurred and what the error was.
    pub last_failure: Option<(Instant, String)>,
}

impl HealthState {
    pub fn new() -> Self {
        Self {
            consecutive_failures: 0,
            last_success: None,
            last_failure: None,
        }
    }

    /// Record a successful probe.
    pub fn record_success(&mut self) {
        let _was_failing = self.consecutive_failures > 0;
        self.consecutive_failures = 0;
        self.last_success = Some(Instant::now());
        self.last_failure = None;
    }

    /// Record a failed probe.
    pub fn record_failure(&mut self, error: String) {
        self.consecutive_failures += 1;
        self.last_failure = Some((Instant::now(), error));
    }

    /// Whether the provider is currently considered unhealthy (exceeded threshold).
    pub fn is_unhealthy(&self, threshold: u32) -> bool {
        threshold > 0 && self.consecutive_failures >= threshold
    }

    /// Whether this state represents a *recently recovered* provider — it was
    /// failing before but just succeeded.
    pub fn just_recovered(&self, was_unhealthy: bool) -> bool {
        was_unhealthy && self.consecutive_failures == 0
    }
}

/// Probe the LLM provider with a minimal request.
/// Returns the round-trip duration on success, or an error string.
///
/// Uses `max_tokens=10, temperature=0` so the call is as cheap as possible
/// (a tiny text generation). The prompt asks the model to simply respond "OK".
pub fn probe(provider: &dyn Provider, model: &str) -> Result<Duration, String> {
    let start = Instant::now();
    let req = CompletionRequest {
        model: model.to_string(),
        messages: vec![crate::message::Message {
            id: 0,
            role: crate::message::Role::User,
            items: vec![crate::message::MessageItem::Text {
                text: "Respond with exactly one word: OK".into(),
            }],
        }],
        max_tokens: 10,
        temperature: 0.0,
        tools: vec![],
    };
    match provider.complete(&req) {
        Ok(_) => Ok(start.elapsed()),
        Err(e) => Err(format!("probe failed: {e}")),
    }
}

/// Run a probe and update the health state accordingly.
/// Returns `true` if the provider is healthy and work should proceed,
/// `false` if it should be skipped this tick.
pub fn probe_and_update(
    state: &mut HealthState,
    provider: &dyn Provider,
    model: &str,
    threshold: u32,
) -> bool {
    let was_unhealthy = state.is_unhealthy(threshold);
    match probe(provider, model) {
        Ok(dur) => {
            state.record_success();
            if state.just_recovered(was_unhealthy) {
                eprintln!(
                    "[health] provider recovered (was down for {} consecutive failures, response in {:.0}ms)",
                    state.consecutive_failures + 1, // +1 for the probe that just succeeded
                    dur.as_millis(),
                );
            }
            true
        }
        Err(e) => {
            state.record_failure(e.clone());
            eprintln!(
                "[health] provider probe {}/{} failed: {e}",
                state.consecutive_failures,
                threshold,
            );
            if state.is_unhealthy(threshold) {
                eprintln!(
                    "[health] provider UNHEALTHY after {} consecutive failures — suppressing work",
                    state.consecutive_failures,
                );
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::stub::StubClient;
    use std::sync::Arc;

    #[test]
    fn probe_succeeds_with_stub() {
        let provider = StubClient;
        let dur = probe(&provider, "gpt-4o").unwrap();
        // StubClient returns synchronously, so elapsed may be zero.
// The assertion is that the probe *returns* a value (no error).
assert!(dur.as_nanos() >= 0, "probe should return a valid duration");
    }

    #[test]
    fn health_state_starts_healthy() {
        let h = HealthState::new();
        assert_eq!(h.consecutive_failures, 0);
        assert!(h.last_success.is_none());
        assert!(h.last_failure.is_none());
        assert!(!h.is_unhealthy(5));
    }

    #[test]
    fn health_state_tracks_failures_and_threshold() {
        let mut h = HealthState::new();
        h.record_failure("timeout".into());
        assert_eq!(h.consecutive_failures, 1);
        assert!(!h.is_unhealthy(3));
        h.record_failure("500".into());
        h.record_failure("down".into());
        assert!(h.is_unhealthy(3));
        assert!(h.last_failure.is_some());
    }

    #[test]
    fn health_state_resets_on_success() {
        let mut h = HealthState::new();
        h.record_failure("err".into());
        h.record_failure("err".into());
        assert!(h.is_unhealthy(2));
        h.record_success();
        assert_eq!(h.consecutive_failures, 0);
        assert!(!h.is_unhealthy(2));
        assert!(h.last_success.is_some());
        assert!(h.last_failure.is_none());
    }

    #[test]
    fn just_recovered_detects_transition() {
        let mut h = HealthState::new();
        h.record_failure("err".into());
        h.record_failure("err".into());
        let was_unhealthy = h.is_unhealthy(2);
        assert!(was_unhealthy);
        h.record_success();
        assert!(h.just_recovered(was_unhealthy));
        // Second success after recovery → not "just recovered"
        assert!(!h.just_recovered(false));
    }

    #[test]
    fn threshold_zero_never_unhealthy() {
        let mut h = HealthState::new();
        h.record_failure("err".into());
        h.record_failure("err".into());
        assert!(!h.is_unhealthy(0), "threshold 0 means disabled");
    }

    #[test]
    fn probe_and_update_handles_stub_provider() {
        let provider = Arc::new(StubClient) as Arc<dyn Provider>;
        let mut state = HealthState::new();
        let ok = probe_and_update(&mut state, provider.as_ref(), "gpt-4o", 5);
        assert!(ok, "stub should pass probe");
        assert_eq!(state.consecutive_failures, 0);
    }
}
