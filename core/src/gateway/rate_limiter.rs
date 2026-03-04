//! Sliding window rate limiter for Gateway RPC methods.
//!
//! Provides per-identity, per-scope rate limiting using a sliding window
//! algorithm backed by `DashMap` for lock-free concurrent access.
//!
//! # Scopes
//!
//! Each RPC method is classified into a [`RateLimitScope`] that determines
//! its rate limit parameters. Authentication attempts have stricter limits
//! with optional lockout; heavy operations (agent.run, chat.send) have
//! lower throughput caps.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use dashmap::DashMap;

// ---------------------------------------------------------------------------
// RateLimitScope
// ---------------------------------------------------------------------------

/// Classification of an RPC call for rate-limiting purposes.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum RateLimitScope {
    /// Authentication attempts (login, token exchange).
    Auth,
    /// Normal read-only or low-cost RPC calls.
    RpcDefault,
    /// State-changing RPC calls (config.patch, memory.store, ...).
    RpcWrite,
    /// Resource-intensive RPC calls (agent.run, chat.send, ...).
    RpcHeavy,
    /// Webhook authentication attempts.
    WebhookAuth,
}

impl fmt::Display for RateLimitScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth => write!(f, "auth"),
            Self::RpcDefault => write!(f, "rpc_default"),
            Self::RpcWrite => write!(f, "rpc_write"),
            Self::RpcHeavy => write!(f, "rpc_heavy"),
            Self::WebhookAuth => write!(f, "webhook_auth"),
        }
    }
}

// ---------------------------------------------------------------------------
// RateLimitKey
// ---------------------------------------------------------------------------

/// Compound key: (identity, scope). One entry per unique combination.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct RateLimitKey {
    /// IP address or device_id of the caller.
    pub identity: String,
    /// Which rate-limit bucket this key belongs to.
    pub scope: RateLimitScope,
}

impl RateLimitKey {
    pub fn new(identity: &str, scope: RateLimitScope) -> Self {
        Self {
            identity: identity.to_owned(),
            scope,
        }
    }
}

// ---------------------------------------------------------------------------
// WindowConfig / RateLimitConfig
// ---------------------------------------------------------------------------

/// Per-scope sliding window parameters.
#[derive(Clone, Debug)]
pub struct WindowConfig {
    /// Maximum allowed requests within the window.
    pub max_requests: u32,
    /// Window duration in seconds.
    pub window_secs: u64,
    /// Optional lockout duration in seconds after the limit is exceeded.
    /// `None` means the caller is simply rejected until the window slides.
    pub lockout_secs: Option<u64>,
}

/// Aggregate rate-limit configuration for all scopes.
#[derive(Clone, Debug)]
pub struct RateLimitConfig {
    pub auth: WindowConfig,
    pub rpc_default: WindowConfig,
    pub rpc_write: WindowConfig,
    pub rpc_heavy: WindowConfig,
    /// If `true`, requests from loopback addresses bypass all limits.
    pub exempt_loopback: bool,
}

impl RateLimitConfig {
    /// Look up the [`WindowConfig`] for a given scope.
    fn config_for(&self, scope: &RateLimitScope) -> &WindowConfig {
        match scope {
            RateLimitScope::Auth | RateLimitScope::WebhookAuth => &self.auth,
            RateLimitScope::RpcDefault => &self.rpc_default,
            RateLimitScope::RpcWrite => &self.rpc_write,
            RateLimitScope::RpcHeavy => &self.rpc_heavy,
        }
    }
}

// ---------------------------------------------------------------------------
// RateLimitError
// ---------------------------------------------------------------------------

/// Error returned when a request is rate-limited.
#[derive(Debug)]
pub enum RateLimitError {
    /// The sliding window is full; try again after `retry_after_ms`.
    Exceeded {
        scope: RateLimitScope,
        retry_after_ms: u64,
    },
    /// The caller is locked out due to repeated violations.
    LockedOut {
        scope: RateLimitScope,
        lockout_remaining_ms: u64,
    },
}

impl fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exceeded { scope, retry_after_ms } => {
                write!(f, "rate limit exceeded for {scope}, retry after {retry_after_ms}ms")
            }
            Self::LockedOut { scope, lockout_remaining_ms } => {
                write!(f, "locked out for {scope}, remaining {lockout_remaining_ms}ms")
            }
        }
    }
}

impl std::error::Error for RateLimitError {}

// ---------------------------------------------------------------------------
// SlidingWindow (private)
// ---------------------------------------------------------------------------

/// Internal per-key state: a deque of recent timestamps + optional lockout.
struct SlidingWindow {
    timestamps: VecDeque<Instant>,
    lockout_until: Option<Instant>,
}

impl SlidingWindow {
    fn new() -> Self {
        Self {
            timestamps: VecDeque::new(),
            lockout_until: None,
        }
    }
}

// ---------------------------------------------------------------------------
// RateLimiter
// ---------------------------------------------------------------------------

/// Concurrent sliding-window rate limiter.
///
/// Thread-safe via [`DashMap`]; each unique `(identity, scope)` pair gets
/// its own [`SlidingWindow`].
pub struct RateLimiter {
    config: RateLimitConfig,
    windows: DashMap<RateLimitKey, SlidingWindow>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            windows: DashMap::new(),
        }
    }

    /// Check whether the request is allowed and, if so, record it.
    ///
    /// Returns `Ok(())` when the request is permitted, or a
    /// [`RateLimitError`] describing why it was rejected.
    pub fn check_and_record(&self, key: &RateLimitKey) -> Result<(), RateLimitError> {
        // Loopback exemption
        if self.config.exempt_loopback && is_loopback(&key.identity) {
            return Ok(());
        }

        let wc = self.config.config_for(&key.scope);
        let window_dur = Duration::from_secs(wc.window_secs);
        let max = wc.max_requests;
        let now = Instant::now();

        let mut entry = self.windows.entry(key.clone()).or_insert_with(SlidingWindow::new);
        let sw = entry.value_mut();

        // 1. Check lockout
        if let Some(until) = sw.lockout_until {
            if now < until {
                let remaining = until.duration_since(now);
                return Err(RateLimitError::LockedOut {
                    scope: key.scope.clone(),
                    lockout_remaining_ms: remaining.as_millis() as u64,
                });
            }
            // Lockout expired — clear it and reset window
            sw.lockout_until = None;
            sw.timestamps.clear();
        }

        // 2. Evict expired timestamps
        let cutoff = now - window_dur;
        while let Some(&front) = sw.timestamps.front() {
            if front < cutoff {
                sw.timestamps.pop_front();
            } else {
                break;
            }
        }

        // 3. Check limit
        if sw.timestamps.len() as u32 >= max {
            // Trigger lockout if configured
            if let Some(lockout_secs) = wc.lockout_secs {
                sw.lockout_until = Some(now + Duration::from_secs(lockout_secs));
                return Err(RateLimitError::LockedOut {
                    scope: key.scope.clone(),
                    lockout_remaining_ms: lockout_secs * 1000,
                });
            }

            // No lockout — compute retry_after from oldest timestamp
            let oldest = sw.timestamps.front().expect("timestamps non-empty when over limit");
            let expires_at = *oldest + window_dur;
            let retry_after = if expires_at > now {
                expires_at.duration_since(now)
            } else {
                Duration::ZERO
            };
            return Err(RateLimitError::Exceeded {
                scope: key.scope.clone(),
                retry_after_ms: retry_after.as_millis() as u64,
            });
        }

        // 4. Record
        sw.timestamps.push_back(now);
        Ok(())
    }

    /// Remove stale entries whose last activity is older than `max_age`
    /// and that have no active lockout. Call periodically to reclaim memory.
    pub fn prune_stale(&self, max_age: Duration) {
        let now = Instant::now();
        self.windows.retain(|_key, sw| {
            // Keep if lockout is still active
            if let Some(until) = sw.lockout_until {
                if now < until {
                    return true;
                }
            }
            // Keep if any timestamp is recent enough
            match sw.timestamps.back() {
                Some(&last) => now.duration_since(last) < max_age,
                None => false,
            }
        });
    }
}

// ---------------------------------------------------------------------------
// scope_for_method
// ---------------------------------------------------------------------------

/// Map an RPC method name to its [`RateLimitScope`].
pub fn scope_for_method(method: &str) -> RateLimitScope {
    match method {
        // State-changing writes
        "config.patch" | "config.apply" | "config.set"
        | "memory.store" | "memory.delete"
        | "session.compact" | "session.delete"
        | "plugins.install" | "plugins.uninstall"
        | "skills.install" | "skills.delete" => RateLimitScope::RpcWrite,

        // Resource-intensive operations
        "agent.run" | "chat.send" | "poe.run" | "poe.prepare" => RateLimitScope::RpcHeavy,

        // Everything else
        _ => RateLimitScope::RpcDefault,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check whether an identity string represents a loopback address.
fn is_loopback(identity: &str) -> bool {
    identity == "127.0.0.1" || identity == "::1" || identity == "localhost"
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a config with sensible test defaults.
    fn test_config() -> RateLimitConfig {
        RateLimitConfig {
            auth: WindowConfig {
                max_requests: 3,
                window_secs: 60,
                lockout_secs: Some(300),
            },
            rpc_default: WindowConfig {
                max_requests: 10,
                window_secs: 1,
                lockout_secs: None,
            },
            rpc_write: WindowConfig {
                max_requests: 5,
                window_secs: 1,
                lockout_secs: None,
            },
            rpc_heavy: WindowConfig {
                max_requests: 2,
                window_secs: 1,
                lockout_secs: None,
            },
            exempt_loopback: true,
        }
    }

    #[test]
    fn test_allows_under_limit() {
        let limiter = RateLimiter::new(test_config());
        let key = RateLimitKey::new("192.168.1.1", RateLimitScope::RpcDefault);

        for i in 0..10 {
            assert!(
                limiter.check_and_record(&key).is_ok(),
                "request {i} should succeed"
            );
        }
    }

    #[test]
    fn test_rejects_over_limit() {
        let limiter = RateLimiter::new(test_config());
        let key = RateLimitKey::new("192.168.1.1", RateLimitScope::RpcDefault);

        // Fill the window (limit = 10)
        for _ in 0..10 {
            limiter.check_and_record(&key).unwrap();
        }

        // 11th should be rejected
        let err = limiter.check_and_record(&key).unwrap_err();
        match err {
            RateLimitError::Exceeded { scope, .. } => {
                assert_eq!(scope, RateLimitScope::RpcDefault);
            }
            other => panic!("expected Exceeded, got: {other:?}"),
        }
    }

    #[test]
    fn test_auth_lockout() {
        let limiter = RateLimiter::new(test_config());
        let key = RateLimitKey::new("10.0.0.1", RateLimitScope::Auth);

        // Exhaust auth limit (3)
        for _ in 0..3 {
            limiter.check_and_record(&key).unwrap();
        }

        // 4th triggers lockout
        let err = limiter.check_and_record(&key).unwrap_err();
        match err {
            RateLimitError::LockedOut { scope, lockout_remaining_ms } => {
                assert_eq!(scope, RateLimitScope::Auth);
                assert!(lockout_remaining_ms > 0, "lockout should have positive remaining time");
            }
            other => panic!("expected LockedOut, got: {other:?}"),
        }
    }

    #[test]
    fn test_loopback_exempt() {
        let limiter = RateLimiter::new(test_config());

        // rpc_default limit is 10, but loopback is exempt
        for addr in &["127.0.0.1", "::1", "localhost"] {
            let key = RateLimitKey::new(addr, RateLimitScope::RpcDefault);
            for i in 0..100 {
                assert!(
                    limiter.check_and_record(&key).is_ok(),
                    "loopback {addr} request {i} should succeed"
                );
            }
        }
    }

    #[test]
    fn test_scopes_are_independent() {
        let limiter = RateLimiter::new(test_config());
        let identity = "10.0.0.5";

        // Exhaust Auth scope (limit 3)
        let auth_key = RateLimitKey::new(identity, RateLimitScope::Auth);
        for _ in 0..3 {
            limiter.check_and_record(&auth_key).unwrap();
        }
        assert!(limiter.check_and_record(&auth_key).is_err(), "auth should be exhausted");

        // RpcDefault should still work
        let rpc_key = RateLimitKey::new(identity, RateLimitScope::RpcDefault);
        assert!(
            limiter.check_and_record(&rpc_key).is_ok(),
            "rpc_default should be independent of auth"
        );
    }

    #[test]
    fn test_scope_for_method() {
        // RpcWrite methods
        for method in &[
            "config.patch", "config.apply", "config.set",
            "memory.store", "memory.delete",
            "session.compact", "session.delete",
            "plugins.install", "plugins.uninstall",
            "skills.install", "skills.delete",
        ] {
            assert_eq!(
                scope_for_method(method),
                RateLimitScope::RpcWrite,
                "{method} should be RpcWrite"
            );
        }

        // RpcHeavy methods
        for method in &["agent.run", "chat.send", "poe.run", "poe.prepare"] {
            assert_eq!(
                scope_for_method(method),
                RateLimitScope::RpcHeavy,
                "{method} should be RpcHeavy"
            );
        }

        // Default
        assert_eq!(scope_for_method("session.list"), RateLimitScope::RpcDefault);
        assert_eq!(scope_for_method("config.get"), RateLimitScope::RpcDefault);
        assert_eq!(scope_for_method("unknown.method"), RateLimitScope::RpcDefault);
    }

    #[test]
    fn test_prune_stale() {
        let limiter = RateLimiter::new(test_config());
        let key = RateLimitKey::new("10.0.0.99", RateLimitScope::RpcDefault);
        limiter.check_and_record(&key).unwrap();

        // With a zero-duration max_age, everything is stale
        limiter.prune_stale(Duration::ZERO);
        // The entry should have been removed — next request starts fresh
        assert!(limiter.windows.is_empty(), "stale entries should be pruned");
    }
}
