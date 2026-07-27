# Plan B: Environment Resilience Enhancement

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add retry with exponential backoff for LLM provider API calls, and optional provider fallback, so transient network errors / rate limits / 5xx don't crash the entire run. This addresses the P1 gaps where a single 429 or connection timeout terminates the whole process.

**Architecture:** Add a lightweight retry wrapper around `Provider::complete` with configurable backoff (initial delay, max delay, jitter). The existing `OpenAiClient` gets a `retry_config` field; the retry logic lives in a new `src/provider/retry.rs` module. Provider fallback adds a second `CODECODER_FALLBACK_API_BASE` / `CODECODER_FALLBACK_MODEL` pair, activated when the primary provider exhausts all retries.

**Tech Stack:** Rust (existing), ureq (existing — already handles HTTP), `std::time::Duration`, `rand` for jitter (or use a simple deterministic jitter to avoid adding a dependency)

## Global Constraints

- No new async runtime — retries use `std::thread::sleep`
- No new dependencies if possible (use existing `rand` crate or simple pseudo-random)
- Follow existing `CODECODER_*` env var naming convention
- All retry logic must be hermetic-testable (mock provider with controlled failures)
- Defaults must be conservative: 3 retries, 1s initial backoff, 30s max backoff
- `CODECODER_PROVIDER_RETRY_MAX` env var controls max retries (default 3, 0=disable)
- `CODECODER_PROVIDER_RETRY_INITIAL_MS` env var (default 1000)
- `CODECODER_FALLBACK_API_BASE` and `CODECODER_FALLBACK_MODEL` for fallback (optional)

---

### Task 1: Add retry config fields and env vars

**Files:**
- Modify: `src/config.rs` (add `provider_retry_max`, `provider_retry_initial_ms`, `fallback_api_base`, `fallback_model`)
- Test: inline tests in `src/config.rs`

**Interfaces:**
- Consumes: existing `Config` struct
- Produces: `Config.provider_retry_max: u32` (default 3), `Config.provider_retry_initial_ms: u64` (default 1000), `Config.fallback_api_base: Option<String>`, `Config.fallback_model: Option<String>`

- [ ] **Step 1: Add fields to Config struct**

```rust
// After pub ondemand_reaper_secs:
    /// LLM provider 调用最大重试次数(含首次)。0 = 不重试。env CODECODER_PROVIDER_RETRY_MAX, 默认 3。
    pub provider_retry_max: u32,
    /// 重试初始退避毫秒数。每次重试加倍,封顶 30s。env CODECODER_PROVIDER_RETRY_INITIAL_MS, 默认 1000。
    pub provider_retry_initial_ms: u64,
    /// 可选的主 provider 失败后的 fallback API base。env CODECODER_FALLBACK_API_BASE。
    pub fallback_api_base: Option<String>,
    /// fallback 模型的名称。env CODECODER_FALLBACK_MODEL。
    pub fallback_model: Option<String>,
```

- [ ] **Step 2: Add env parsing in `from_env()`**

```rust
            provider_retry_max: env("CODECODER_PROVIDER_RETRY_MAX")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            provider_retry_initial_ms: env("CODECODER_PROVIDER_RETRY_INITIAL_MS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
            fallback_api_base: env("CODECODER_FALLBACK_API_BASE"),
            fallback_model: env("CODECODER_FALLBACK_MODEL"),
```

- [ ] **Step 3: Add to DOTENV_ALLOWED_KEYS**

```rust
    "CODECODER_PROVIDER_RETRY_MAX",
    "CODECODER_PROVIDER_RETRY_INITIAL_MS",
    // Note: FALLBACK_API_BASE and FALLBACK_MODEL are NOT in DOTENV_ALLOWED_KEYS
    // because they could redirect traffic to a malicious endpoint.
    // They must come from the real shell env.
```

- [ ] **Step 4: Write tests**

```rust
#[test]
fn provider_retry_defaults() {
    unsafe {
        std::env::remove_var("CODECODER_PROVIDER_RETRY_MAX");
        std::env::remove_var("CODECODER_PROVIDER_RETRY_INITIAL_MS");
    }
    let cfg = Config::from_env();
    assert_eq!(cfg.provider_retry_max, 3);
    assert_eq!(cfg.provider_retry_initial_ms, 1000);
}

#[test]
fn provider_retry_overrides() {
    unsafe {
        std::env::set_var("CODECODER_PROVIDER_RETRY_MAX", "5");
        std::env::set_var("CODECODER_PROVIDER_RETRY_INITIAL_MS", "2000");
    }
    let cfg = Config::from_env();
    assert_eq!(cfg.provider_retry_max, 5);
    assert_eq!(cfg.provider_retry_initial_ms, 2000);
    unsafe {
        std::env::remove_var("CODECODER_PROVIDER_RETRY_MAX");
        std::env::remove_var("CODECODER_PROVIDER_RETRY_INITIAL_MS");
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add provider retry and fallback config fields"
```

---

### Task 2: Implement retry logic in a new provider module

**Files:**
- Create: `src/provider/retry.rs`
- Test: inline tests in `src/provider/retry.rs`

**Interfaces:**
- `RetryConfig { max_retries: u32, initial_delay_ms: u64 }`
- `pub fn with_retry<F, T>(config: &RetryConfig, f: F) -> Result<T, RetryError>` — calls `f()` up to `max_retries` times with exponential backoff + jitter
- `RetryError { attempts: u32, last_error: String, is_rate_limited: bool }`

- [ ] **Step 1: Create `src/provider/retry.rs`**

```rust
// src/provider/retry.rs
// Retry with exponential backoff + jitter for LLM provider calls.

use std::time::Duration;

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of attempts (including the first). 0 = no retry.
    pub max_retries: u32,
    /// Initial delay in milliseconds. Doubled after each retry, capped at 30s.
    pub initial_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self { max_retries: 3, initial_delay_ms: 1000 }
    }
}

/// The error returned after all retries are exhausted.
#[derive(Debug)]
pub struct RetryError {
    pub attempts: u32,
    pub last_error: String,
    pub is_rate_limited: bool,
}

impl std::fmt::Display for RetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "after {} attempt(s): {} (rate_limited={})",
            self.attempts, self.last_error, self.is_rate_limited)
    }
}

impl std::error::Error for RetryError {}

/// Simple deterministic jitter: `((attempt as u64).wrapping_mul(7919)) % (delay.as_millis() as u64 / 4 + 1)` ms.
/// Avoids thundering herd without requiring the `rand` crate.
fn jitter_delay(attempt: u32, delay: Duration) -> Duration {
    let base_ms = delay.as_millis() as u64;
    if base_ms == 0 {
        return delay;
    }
    let jitter_ms = (attempt as u64).wrapping_mul(7919) % (base_ms / 4 + 1);
    Duration::from_millis(base_ms + jitter_ms)
}

/// Determine if an HTTP status / error string indicates a retryable failure.
pub fn is_retryable(error: &str, status_code: Option<u16>) -> bool {
    if let Some(code) = status_code {
        // 429 (rate limit), 500, 502, 503, 504 (server errors)
        if code == 429 || (500..=599).contains(&code) {
            return true;
        }
    }
    let lower = error.to_lowercase();
    // Network-level errors
    lower.contains("timeout")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("eof")
        || lower.contains("temporary")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("service unavailable")
        || lower.contains("internal server error")
}

/// Call `f` with retry logic. Returns the first successful result, or a `RetryError`
/// after all retries are exhausted.
pub fn with_retry<F, T>(config: &RetryConfig, f: F) -> Result<T, RetryError>
where
    F: Fn() -> Result<T, (String, Option<u16>)>,
{
    let max = config.max_retries.max(1); // at least 1 attempt
    let mut last_error = String::new();
    let mut is_rate_limited = false;
    let mut delay = Duration::from_millis(config.initial_delay_ms);
    let max_delay = Duration::from_secs(30);

    for attempt in 0..max {
        if attempt > 0 {
            // Apply jittered delay before retry
            let sleep_dur = jitter_delay(attempt, delay).min(max_delay);
            std::thread::sleep(sleep_dur);
            // Double delay for next iteration (capped at max_delay)
            delay = (delay * 2).min(max_delay);
        }
        match f() {
            Ok(result) => return Ok(result),
            Err((err, code)) => {
                last_error = err;
                is_rate_limited = code == Some(429);
                if !is_retryable(&last_error, code) {
                    // Non-retryable error — bail immediately
                    return Err(RetryError {
                        attempts: attempt + 1,
                        last_error,
                        is_rate_limited,
                    });
                }
            }
        }
    }
    Err(RetryError {
        attempts: max,
        last_error,
        is_rate_limited,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn succeeds_on_first_try() {
        let cfg = RetryConfig { max_retries: 3, initial_delay_ms: 1 };
        let result = with_retry(&cfg, || Ok::<_, (String, Option<u16>)>(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn retries_on_429_then_succeeds() {
        let cfg = RetryConfig { max_retries: 3, initial_delay_ms: 1 };
        let mut calls = 0u32;
        let result = with_retry(&cfg, || {
            calls += 1;
            if calls < 3 {
                Err(("rate limited".into(), Some(429)))
            } else {
                Ok(42)
            }
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls, 3);
    }

    #[test]
    fn exhausts_retries_on_persistent_429() {
        let cfg = RetryConfig { max_retries: 3, initial_delay_ms: 1 };
        let result = with_retry(&cfg, || {
            Err(("rate limited".into(), Some(429)))
        });
        let err = result.unwrap_err();
        assert_eq!(err.attempts, 3);
        assert!(err.is_rate_limited);
    }

    #[test]
    fn non_retryable_error_bails_immediately() {
        let cfg = RetryConfig { max_retries: 3, initial_delay_ms: 1 };
        let mut calls = 0u32;
        let result = with_retry(&cfg, || {
            calls += 1;
            Err(("invalid api key".into(), Some(401)))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1); // no retry on 401
    }

    #[test]
    fn is_retryable_codes() {
        assert!(is_retryable("", Some(429)));
        assert!(is_retryable("", Some(500)));
        assert!(is_retryable("", Some(502)));
        assert!(is_retryable("", Some(503)));
        assert!(is_retryable("", Some(504)));
        assert!(!is_retryable("", Some(400)));
        assert!(!is_retryable("", Some(401)));
        assert!(!is_retryable("", Some(403)));
        assert!(!is_retryable("", Some(404)));
    }

    #[test]
    fn zero_retries_means_one_attempt() {
        let cfg = RetryConfig { max_retries: 0, initial_delay_ms: 1 };
        let mut calls = 0u32;
        let result = with_retry(&cfg, || {
            calls += 1;
            Err(("timeout".into(), None))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test provider::retry::tests
```
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/provider/retry.rs
git commit -m "feat(provider): add retry with exponential backoff for provider calls"
```

---

### Task 3: Wire retry into OpenAiClient::complete

**Files:**
- Modify: `src/provider/openai.rs` (add retry wrapper around the HTTP call)
- Modify: `src/provider/mod.rs` (expose `retry` module, pass `RetryConfig` to provider constructors)
- Test: inline tests in `src/provider/openai.rs`

**Interfaces:**
- `OpenAiClient` gains a `retry_config: RetryConfig` field
- `OpenAiClient::complete` wraps its HTTP call in `with_retry`
- `select_provider` passes retry config from `Config`

- [ ] **Step 1: Add retry_config to OpenAiClient**

```rust
// In src/provider/openai.rs, add to OpenAiClient struct:
    retry_config: crate::provider::retry::RetryConfig,
```

- [ ] **Step 2: Wrap the HTTP call in `complete()` with retry**

The `complete()` method's HTTP call should be wrapped: capture the error string and HTTP status code, pass to `with_retry`. On `Err(RetryError)`, log the error and propagate to `AgentLoop` which already handles provider errors.

- [ ] **Step 3: Pass config through `select_provider` in `src/provider/mod.rs`**

- [ ] **Step 4: Run tests**

```bash
cargo test
```
Expected: All existing tests pass (retry is transparent when no errors occur).

- [ ] **Step 5: Commit**

```bash
git add src/provider/openai.rs src/provider/mod.rs
git commit -m "feat(provider): wire retry logic into OpenAiClient::complete"
```

---

### Task 4: Implement provider fallback

**Files:**
- Modify: `src/provider/mod.rs` (add fallback logic in `select_provider` or a new `FallbackProvider`)
- Test: inline tests

**Interfaces:**
- When primary provider exhausts all retries with a 5xx/connection error, automatically switch to fallback
- `FallbackProvider` wraps two `Provider` impls: try primary with retry, then fallback with retry
- If `fallback_api_base` is not set, no fallback happens (behavior unchanged)

- [ ] **Step 1: Add `FallbackProvider`**

```rust
// In src/provider/mod.rs:
pub struct FallbackProvider {
    primary: Arc<dyn Provider>,
    fallback: Arc<dyn Provider>,
}

impl Provider for FallbackProvider {
    fn complete(&self, req: &CompletionRequest) -> Result<Completion, Box<dyn std::error::Error>> {
        match self.primary.complete(req) {
            Ok(c) => Ok(c),
            Err(e) => {
                eprintln!("ccd: primary provider failed: {e}, trying fallback");
                self.fallback.complete(req)
            }
        }
    }
}
```

- [ ] **Step 2: Wire in `select_provider`**

When `fallback_api_base` is set, construct a second `OpenAiClient` and wrap both in `FallbackProvider`.

- [ ] **Step 3: Run tests**

```bash
cargo test
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/provider/mod.rs
git commit -m "feat(provider): add FallbackProvider for primary-fallback failover"
```

---

### Task 5: Update docs

**Files:**
- Modify: `README.md` (add new env vars to the table)

**Interfaces:**
- Documentation only.

- [ ] **Step 1: Update README.md env table**

Add `CODECODER_PROVIDER_RETRY_MAX`, `CODECODER_PROVIDER_RETRY_INITIAL_MS`, `CODECODER_FALLBACK_API_BASE`, `CODECODER_FALLBACK_MODEL`.

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: document provider retry and fallback env vars"
```