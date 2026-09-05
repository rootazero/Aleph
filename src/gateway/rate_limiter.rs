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
use serde::{Deserialize, Serialize};

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
    /// High-frequency realtime data frames (voice.stream.audio). Streaming STT
    /// pushes one ~200 ms PCM frame per RPC — a steady 5 req/s per active mic —
    /// which would exhaust `RpcDefault` in seconds and kill live transcription.
    RpcRealtime,
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
            Self::RpcRealtime => write!(f, "rpc_realtime"),
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
    /// IP address or `device_id` of the caller.
    pub identity: String,
    /// Which rate-limit bucket this key belongs to.
    pub scope: RateLimitScope,
}

impl RateLimitKey {
    #[must_use]
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
    pub rpc_realtime: WindowConfig,
    /// If `true`, requests from loopback addresses bypass all limits.
    pub exempt_loopback: bool,
    /// Hard cap on the number of distinct `(identity, scope)` entries held in
    /// memory. Bounds heap growth against a flood of unique identities
    /// (CWE-400 / uncontrolled resource consumption) in between the periodic
    /// [`prune_stale`](RateLimiter::prune_stale) sweeps. `0` disables the cap.
    /// See [`RateLimiter::check_and_record`] for the eviction behaviour.
    pub max_entries: usize,
}

/// Default ceiling on distinct rate-limit entries (mirrors openclaw's
/// `CONTROL_PLANE_BUCKET_MAX_ENTRIES`). 10k keys is far above any legitimate
/// fan-out yet small enough to keep memory bounded under an identity flood.
pub const DEFAULT_MAX_ENTRIES: usize = 10_000;

impl RateLimitConfig {}

// ---------------------------------------------------------------------------
// TOML schema for `[gateway.rate_limit]`
// ---------------------------------------------------------------------------

/// TOML overrides for one scope's sliding window.
///
/// Every key is optional and an absent key keeps the compiled default for
/// *that scope* — the numbers live in [`RateLimitConfig::default`] and only
/// there, so a section that names one key does not silently replay (and
/// freeze) the rest of the table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowTomlConfig {
    /// Maximum allowed requests within the window.
    pub max_requests: Option<u32>,
    /// Window duration in seconds.
    pub window_secs: Option<u64>,
    /// Lockout duration in seconds after the limit is exceeded. `0` spells
    /// "no lockout" (the runtime carries `Option<u64>` and TOML has no null),
    /// so an operator can remove `auth`'s 5-minute lockout, not only lengthen
    /// it.
    pub lockout_secs: Option<u64>,
}

impl WindowTomlConfig {
    /// Overlay the configured keys onto a base window.
    fn apply(&self, base: &WindowConfig) -> WindowConfig {
        WindowConfig {
            max_requests: self.max_requests.unwrap_or(base.max_requests),
            window_secs: self.window_secs.unwrap_or(base.window_secs),
            lockout_secs: self
                .lockout_secs
                .map_or(base.lockout_secs, |secs| (secs > 0).then_some(secs)),
        }
    }
}

/// TOML schema for the `[gateway.rate_limit]` section — the operator handle
/// on the per-principal RPC ceilings.
///
/// RESTART-ONLY. The section is read once, at boot, by
/// `GatewayServer::with_config`; `[gateway.*]` is not reachable by
/// `self_config` / `ConfigPatcher`, so changing a ceiling means editing the
/// file and restarting the server. Nothing re-reads it while the process
/// lives.
///
/// Out of scope on purpose: the two byte-route limiters
/// (`server/artifact_route.rs` and `server/canvas_asset_route.rs`) each
/// document themselves as private to their route and are NOT configured from
/// here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitTomlConfig {
    /// `[gateway.rate_limit.auth]` — also governs webhook auth attempts.
    pub auth: WindowTomlConfig,
    /// `[gateway.rate_limit.rpc_default]`
    pub rpc_default: WindowTomlConfig,
    /// `[gateway.rate_limit.rpc_write]`
    pub rpc_write: WindowTomlConfig,
    /// `[gateway.rate_limit.rpc_heavy]`
    pub rpc_heavy: WindowTomlConfig,
    /// `[gateway.rate_limit.rpc_realtime]`
    pub rpc_realtime: WindowTomlConfig,
    /// Let loopback callers bypass every limit. Absent keeps the default.
    pub exempt_loopback: Option<bool>,
    /// Ceiling on distinct tracked `(identity, scope)` entries. Absent keeps
    /// [`DEFAULT_MAX_ENTRIES`].
    pub max_entries: Option<usize>,
}

impl From<RateLimitTomlConfig> for RateLimitConfig {
    fn from(schema: RateLimitTomlConfig) -> Self {
        let base = Self::default();
        Self {
            auth: schema.auth.apply(&base.auth),
            rpc_default: schema.rpc_default.apply(&base.rpc_default),
            rpc_write: schema.rpc_write.apply(&base.rpc_write),
            rpc_heavy: schema.rpc_heavy.apply(&base.rpc_heavy),
            rpc_realtime: schema.rpc_realtime.apply(&base.rpc_realtime),
            exempt_loopback: schema.exempt_loopback.unwrap_or(base.exempt_loopback),
            max_entries: schema.max_entries.unwrap_or(base.max_entries),
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            auth: WindowConfig {
                max_requests: 10,
                window_secs: 60,
                lockout_secs: Some(300),
            },
            rpc_default: WindowConfig {
                max_requests: 100,
                window_secs: 60,
                lockout_secs: None,
            },
            rpc_write: WindowConfig {
                max_requests: 20,
                window_secs: 60,
                lockout_secs: None,
            },
            rpc_heavy: WindowConfig {
                // A live voice conversation issues one chat.send per spoken
                // utterance — easily 10-20/min in quick exchanges. 5/min made
                // remote (LAN Panel) voice mode collapse after five turns; 30
                // still caps a runaway loop while leaving conversation room.
                max_requests: 30,
                window_secs: 60,
                lockout_secs: None,
            },
            rpc_realtime: WindowConfig {
                // Streaming STT frames: 5/s per mic (200 ms cadence) = 300/min;
                // 1500 leaves headroom for several concurrent voice sessions.
                max_requests: 1500,
                window_secs: 60,
                lockout_secs: None,
            },
            exempt_loopback: true,
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }
}

impl RateLimitConfig {
    /// Look up the [`WindowConfig`] for a given scope.
    const fn config_for(&self, scope: &RateLimitScope) -> &WindowConfig {
        match scope {
            RateLimitScope::Auth | RateLimitScope::WebhookAuth => &self.auth,
            RateLimitScope::RpcDefault => &self.rpc_default,
            RateLimitScope::RpcWrite => &self.rpc_write,
            RateLimitScope::RpcHeavy => &self.rpc_heavy,
            RateLimitScope::RpcRealtime => &self.rpc_realtime,
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
            Self::Exceeded {
                scope,
                retry_after_ms,
            } => {
                write!(
                    f,
                    "rate limit exceeded for {scope}, retry after {retry_after_ms}ms"
                )
            }
            Self::LockedOut {
                scope,
                lockout_remaining_ms,
            } => {
                write!(
                    f,
                    "locked out for {scope}, remaining {lockout_remaining_ms}ms"
                )
            }
        }
    }
}

impl std::error::Error for RateLimitError {}

impl RateLimitError {
    /// Convert to an HTTP 429 JSON body suitable for JSON-RPC error responses.
    #[must_use]
    pub fn to_jsonrpc_error(&self) -> String {
        let (retry_after_ms, message) = match self {
            Self::Exceeded {
                scope,
                retry_after_ms,
            } => (*retry_after_ms, format!("Rate limit exceeded for {scope}")),
            Self::LockedOut {
                scope,
                lockout_remaining_ms,
            } => (*lockout_remaining_ms, format!("Locked out for {scope}")),
        };
        format!(
            r#"{{"jsonrpc":"2.0","error":{{"code":-32029,"message":"{message}","data":{{"retry_after_ms":{retry_after_ms}}}}},"id":null}}"#
        )
    }

    /// Return the retry-after value in seconds (for HTTP Retry-After header).
    #[must_use]
    pub const fn retry_after_secs(&self) -> u64 {
        let ms = match self {
            Self::Exceeded { retry_after_ms, .. } => *retry_after_ms,
            Self::LockedOut {
                lockout_remaining_ms,
                ..
            } => *lockout_remaining_ms,
        };
        ms.div_ceil(1000)
    }
}

// ---------------------------------------------------------------------------
// SlidingWindow (private)
// ---------------------------------------------------------------------------

/// Internal per-key state: a deque of recent timestamps + optional lockout.
struct SlidingWindow {
    timestamps: VecDeque<Instant>,
    lockout_until: Option<Instant>,
}

impl SlidingWindow {
    const fn new() -> Self {
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
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            windows: DashMap::new(),
        }
    }

    /// Number of distinct `(identity, scope)` windows currently tracked.
    ///
    /// Read-only view used by the `/metrics` endpoint to surface limiter
    /// memory pressure relative to [`RateLimitConfig::max_entries`].
    #[must_use]
    pub fn tracked_entries(&self) -> usize {
        self.windows.len()
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

        // Bound distinct-key growth (CWE-400). Existing keys are always served;
        // only a *new* key is gated once the map is full. Before rejecting,
        // opportunistically reclaim entries idle beyond one window — a flood of
        // throwaway identities self-heals while legitimate callers persist.
        if self.config.max_entries > 0
            && self.windows.len() >= self.config.max_entries
            && !self.windows.contains_key(key)
        {
            self.prune_stale(window_dur);
            if self.windows.len() >= self.config.max_entries {
                return Err(RateLimitError::Exceeded {
                    scope: key.scope.clone(),
                    retry_after_ms: wc.window_secs * 1000,
                });
            }
        }

        let mut entry = self
            .windows
            .entry(key.clone())
            .or_insert_with(SlidingWindow::new);
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

        // 2. Evict expired timestamps. `Instant - Duration` panics on
        // underflow (monotonic clock younger than the window, e.g. right
        // after boot); with no valid cutoff nothing can be expired yet.
        if let Some(cutoff) = now.checked_sub(window_dur) {
            while let Some(&front) = sw.timestamps.front() {
                if front < cutoff {
                    sw.timestamps.pop_front();
                } else {
                    break;
                }
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

            // No lockout — compute retry_after from oldest timestamp. Fall back
            // to ZERO if the deque is empty (only possible if a window is
            // configured with max_requests == 0, where `len >= max` is true
            // even with no timestamps) rather than panicking.
            let retry_after = sw.timestamps.front().map_or(Duration::ZERO, |oldest| {
                let expires_at = *oldest + window_dur;
                if expires_at > now {
                    expires_at.duration_since(now)
                } else {
                    Duration::ZERO
                }
            });
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
                Some(&last) => now.saturating_duration_since(last) < max_age,
                None => false,
            }
        });
    }
}

// ---------------------------------------------------------------------------
// scope_for_method
// ---------------------------------------------------------------------------

/// Map an RPC method name to its [`RateLimitScope`].
#[must_use]
pub fn scope_for_method(method: &str) -> RateLimitScope {
    match method {
        // Authentication attempts: `connect` is where a remote client
        // presents (or retries) a Gateway token, so it gets the strict
        // brute-force window (10/min + 5-min lockout) instead of the
        // generous RpcDefault one. Loopback is exempt upstream, and a
        // legitimate remote client issues one `connect` per session — only
        // token guessing and stale-token retry storms hit this ceiling.
        "connect" => RateLimitScope::Auth,

        // State-changing writes. sessions.reset (clear all messages) and
        // session.truncate (drop messages past N) are destructive session
        // mutations in the same risk class as sessions.delete / session.compact,
        // so they get the strict write window too.
        "config.patch" | "memory.delete" | "session.compact" | "sessions.delete"
        | "sessions.reset" | "session.truncate" | "plugins.install" | "plugins.uninstall"
        | "skills.install" | "skills.remove" | "bundled.sync" | "moa.savePreset"
        | "moa.deletePreset" | "moa.setDefault" | "moa.setSaveTraces" => RateLimitScope::RpcWrite,

        // Resource-intensive operations. `session.export_html` reads the whole
        // transcript plus every inlinable artifact byte and renders a document
        // that is then stored — one call can move tens of megabytes, so it
        // belongs here rather than in the generous default window.
        "agent.run" | "chat.send" | "session.export_html" => RateLimitScope::RpcHeavy,

        // Realtime audio frames (one ~200 ms PCM chunk per call while a
        // streaming-STT mic is live)
        "voice.stream.audio" => RateLimitScope::RpcRealtime,

        // Everything else
        _ => RateLimitScope::RpcDefault,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check whether an identity string represents a loopback address.
///
/// Handles both bare IPs ("127.0.0.1") and socket-address strings
/// that include a port suffix ("127.0.0.1:54321", "[`::1`]:8080").
fn is_loopback(identity: &str) -> bool {
    identity == "127.0.0.1"
        || identity == "::1"
        || identity == "localhost"
        || identity.starts_with("127.0.0.1:")
        || identity.starts_with("[::1]:")
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
            rpc_realtime: WindowConfig {
                max_requests: 20,
                window_secs: 1,
                lockout_secs: None,
            },
            exempt_loopback: true,
            max_entries: DEFAULT_MAX_ENTRIES,
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
            RateLimitError::LockedOut {
                scope,
                lockout_remaining_ms,
            } => {
                assert_eq!(scope, RateLimitScope::Auth);
                assert!(
                    lockout_remaining_ms > 0,
                    "lockout should have positive remaining time"
                );
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
        assert!(
            limiter.check_and_record(&auth_key).is_err(),
            "auth should be exhausted"
        );

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
            "config.patch",
            "memory.delete",
            "session.compact",
            "sessions.delete",
            "sessions.reset",
            "session.truncate",
            "plugins.install",
            "plugins.uninstall",
            "skills.install",
            "skills.remove",
            "bundled.sync",
        ] {
            assert_eq!(
                scope_for_method(method),
                RateLimitScope::RpcWrite,
                "{method} should be RpcWrite"
            );
        }

        // RpcHeavy methods
        for method in &["agent.run", "chat.send", "session.export_html"] {
            assert_eq!(
                scope_for_method(method),
                RateLimitScope::RpcHeavy,
                "{method} should be RpcHeavy"
            );
        }

        // Realtime audio frames get their own high-rate bucket — RpcDefault
        // would starve a live mic (5 frames/s) within seconds.
        assert_eq!(
            scope_for_method("voice.stream.audio"),
            RateLimitScope::RpcRealtime
        );

        // Token-bearing handshake gets the strict auth window.
        assert_eq!(scope_for_method("connect"), RateLimitScope::Auth);

        // Default
        assert_eq!(scope_for_method("session.list"), RateLimitScope::RpcDefault);
        assert_eq!(scope_for_method("config.get"), RateLimitScope::RpcDefault);
        assert_eq!(
            scope_for_method("unknown.method"),
            RateLimitScope::RpcDefault
        );
        // Other voice methods stay in the default bucket (they are per-turn,
        // not per-frame).
        assert_eq!(
            scope_for_method("voice.transcribe"),
            RateLimitScope::RpcDefault
        );
    }

    #[test]
    fn default_limits_leave_room_for_a_voice_conversation() {
        // A remote (LAN) voice session: ~15 utterances/min = 15 chat.send,
        // plus a continuous 5 frames/s streaming mic = 300 voice.stream.audio.
        // Both must clear the production defaults — 5/min rpc_heavy was the
        // "voice chat dies after five turns" bug.
        let limiter = RateLimiter::new(RateLimitConfig::default());
        let heavy = RateLimitKey::new("192.168.1.20", RateLimitScope::RpcHeavy);
        for i in 0..15 {
            assert!(
                limiter.check_and_record(&heavy).is_ok(),
                "chat.send {i} within a minute must pass"
            );
        }
        let frames = RateLimitKey::new("192.168.1.20", RateLimitScope::RpcRealtime);
        for i in 0..300 {
            assert!(
                limiter.check_and_record(&frames).is_ok(),
                "streaming frame {i} within a minute must pass"
            );
        }
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

    #[test]
    fn test_max_entries_caps_distinct_keys() {
        // Cap of 3 distinct entries; non-loopback so the limiter actually tracks.
        let config = RateLimitConfig {
            max_entries: 3,
            ..test_config()
        };
        let limiter = RateLimiter::new(config);

        // First 3 unique identities each create an entry and are served.
        for i in 0..3 {
            let key = RateLimitKey::new(&format!("10.0.0.{i}"), RateLimitScope::RpcDefault);
            assert!(
                limiter.check_and_record(&key).is_ok(),
                "entry {i} should fit"
            );
        }
        assert_eq!(limiter.windows.len(), 3);

        // A 4th *new* identity is rejected (entries are fresh, prune reclaims none).
        let overflow = RateLimitKey::new("10.0.0.99", RateLimitScope::RpcDefault);
        assert!(
            matches!(
                limiter.check_and_record(&overflow),
                Err(RateLimitError::Exceeded { .. })
            ),
            "new key beyond cap must be rejected"
        );

        // An *existing* identity is still served even though the map is full.
        let existing = RateLimitKey::new("10.0.0.0", RateLimitScope::RpcDefault);
        assert!(
            limiter.check_and_record(&existing).is_ok(),
            "existing key must not be gated by the cap"
        );
    }

    #[test]
    fn test_max_entries_zero_disables_cap() {
        let config = RateLimitConfig {
            max_entries: 0,
            ..test_config()
        };
        let limiter = RateLimiter::new(config);
        for i in 0..50 {
            let key = RateLimitKey::new(&format!("10.1.0.{i}"), RateLimitScope::RpcDefault);
            assert!(limiter.check_and_record(&key).is_ok());
        }
        assert_eq!(limiter.windows.len(), 50);
    }

    #[test]
    fn test_rate_limit_error_to_jsonrpc() {
        let err = RateLimitError::Exceeded {
            scope: RateLimitScope::Auth,
            retry_after_ms: 5000,
        };
        let json = err.to_jsonrpc_error();
        assert!(json.contains("-32029"));
        assert!(json.contains("5000"));
        assert_eq!(err.retry_after_secs(), 5);
    }

    #[test]
    fn test_retry_after_secs_rounds_up() {
        let err = RateLimitError::Exceeded {
            scope: RateLimitScope::RpcDefault,
            retry_after_ms: 1500,
        };
        assert_eq!(err.retry_after_secs(), 2);
    }
}
