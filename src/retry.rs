// Transient-error and context-overflow classification (ADR 0027 Wave 0 #2/#3).
// Ported from pi's retry.ts / overflow.ts corpora, adapted to dependency-free
// lowercased substring matching over a provider error's Display string.
//
// These are pure classifiers, NOT policy: the caller owns the retry budget,
// backoff, and reporting (mirrors pi's split of classifier from policy).

/// Account/quota/billing limits that look like 429s but will never succeed on
/// retry. Checked before the retryable set.
const NON_RETRYABLE: &[&str] = &[
    "gousagelimiterror",
    "freeusagelimiterror",
    "monthly usage limit",
    "available balance",
    "insufficient_quota",
    "out of budget",
    "quota exceeded",
    "billing",
];

/// Transient provider/transport failures worth retrying.
const RETRYABLE: &[&str] = &[
    "overloaded",
    "rate limit",
    "rate-limit",
    "too many requests",
    "429",
    "500",
    "502",
    "503",
    "504",
    "524",
    "service unavailable",
    "server error",
    "internal error",
    "provider returned error",
    "network error",
    "connection error",
    "connection refused",
    "connection lost",
    "other side closed",
    "fetch failed",
    "upstream connect",
    "reset before headers",
    "socket hang up",
    "socket connection was closed",
    "timed out",
    "timeout",
    "terminated",
    "websocket closed",
    "websocket error",
    "ended without",
    "stream ended before",
    "did not get a response",
    "you can retry",
    "try your request again",
    "please retry",
    "resourceexhausted",
    "request failed",
];

/// Non-overflow errors that would otherwise match an overflow phrase (e.g.
/// Bedrock throttling "Too many tokens, please wait"). Checked first.
const NON_OVERFLOW: &[&str] = &["throttling", "rate limit", "too many requests"];

/// Context-window overflow phrasings across providers.
const OVERFLOW: &[&str] = &[
    "prompt is too long",
    "request_too_large",
    "input is too long for requested model",
    "exceeds the context window",
    "maximum context length",
    "input token count",
    "maximum prompt length",
    "reduce the length of the messages",
    "exceeds the maximum allowed input length",
    "is longer than the model",
    "exceeds the limit of",
    "exceeds the available context size",
    "greater than the context length",
    "context window exceeds limit",
    "exceeded model token limit",
    "too large for model with",
    "configured context size",
    "model_context_window_exceeded",
    "prompt too long",
    "context_length_exceeded",
    "context length exceeded",
    "too many tokens",
    "token limit exceeded",
];

/// A context overflow is not a transient failure — retrying the same request is
/// futile — so it is classified separately and excluded from `is_retryable`.
pub fn is_context_overflow(err: &str) -> bool {
    let e = err.to_lowercase();
    if NON_OVERFLOW.iter().any(|p| e.contains(p)) {
        return false;
    }
    OVERFLOW.iter().any(|p| e.contains(p))
}

/// Whether a provider error looks like a transient throttle/transport failure the
/// caller may retry. Account limits and context overflows are excluded.
pub fn is_retryable(err: &str) -> bool {
    let e = err.to_lowercase();
    if NON_RETRYABLE.iter().any(|p| e.contains(p)) {
        return false;
    }
    if is_context_overflow(err) {
        return false;
    }
    RETRYABLE.iter().any(|p| e.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_errors_are_retryable() {
        assert!(is_retryable("OpenAI API returned 503: service unavailable"));
        assert!(is_retryable("OpenAI API returned 429: rate limit reached"));
        assert!(is_retryable("OpenAI request failed: connection reset before headers"));
        assert!(is_retryable("upstream connect error"));
        assert!(is_retryable("the operation timed out"));
    }

    #[test]
    fn permanent_and_limit_errors_are_not_retryable() {
        assert!(!is_retryable("OpenAI API returned 401: invalid api key"));
        assert!(!is_retryable("OpenAI API returned 400: bad request"));
        // Quota/billing 429s look transient but never recover.
        assert!(!is_retryable("429: insufficient_quota — you exceeded your quota"));
        assert!(!is_retryable("Monthly usage limit reached; enable available balance"));
    }

    #[test]
    fn overflow_is_not_retryable() {
        assert!(!is_retryable("prompt is too long: 213462 tokens > 200000 maximum"));
        assert!(!is_retryable(
            "Requested token count exceeds the model's maximum context length of 131072 tokens"
        ));
    }

    #[test]
    fn overflow_detection_across_providers() {
        assert!(is_context_overflow("prompt is too long: 213462 tokens > 200000 maximum"));
        assert!(is_context_overflow("Your input exceeds the context window of this model"));
        assert!(is_context_overflow("This model's maximum prompt length is 131072 but the request contains 537812 tokens"));
        assert!(is_context_overflow("Please reduce the length of the messages or completion"));
        assert!(is_context_overflow("context_length_exceeded"));
    }

    #[test]
    fn throttling_is_not_overflow() {
        // Bedrock formats throttling as "Too many tokens, please wait" — must not
        // be mistaken for overflow.
        assert!(!is_context_overflow("ThrottlingException: Too many tokens, please wait before trying again"));
        assert!(!is_context_overflow("429: rate limit exceeded"));
    }
}
