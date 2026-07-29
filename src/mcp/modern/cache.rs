//! Freshness hints on cacheable results (`2026-07-28`).
//!
//! Revision `2026-07-28` requires `ttlMs` and `cacheScope` on the results of
//! `tools/list`, `prompts/list`, `resources/list`, `resources/templates/list`,
//! `resources/read`, and `server/discover`. They complement — rather than
//! replace — the `listChanged` notifications Aleph already consumes: a
//! client that cannot hold a long-lived notification stream open (which,
//! post-`2026-07-28`, means any client that has not opted into
//! `subscriptions/listen`) would otherwise serve a stale tool list until the
//! next reconnect.
//!
//! Only `ttlMs` is retained. `cacheScope` (`"public"` / `"private"`) governs
//! whether *shared intermediaries* may cache a response; Aleph's cache lives on
//! a single `McpServerConnection`, is never shared across servers, and is
//! filled using one set of credentials — it is a private cache by construction,
//! so the field cannot change any decision here and is not stored.

use std::time::{Duration, Instant};

use serde_json::Value;

/// Upper bound on a server-supplied TTL.
///
/// A server that asks to be cached for a year is almost certainly reporting
/// seconds-as-milliseconds or an outright bug, and honoring it would strand
/// Aleph on a tool list that no longer exists. One day is far past any useful
/// freshness window while still bounding the damage.
const MAX_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a cacheable result stays fresh.
///
/// `None` means the server gave no hint — the entry then behaves exactly as it
/// did before this revision: fresh until a `listChanged` notification or a
/// reconnect replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheDirective {
    ttl: Option<Duration>,
}

impl CacheDirective {
    /// Read the freshness hint off a raw JSON-RPC result.
    ///
    /// A missing, non-numeric, or zero `ttlMs` yields "no hint". Zero is
    /// treated as absent rather than as "instantly stale": re-fetching a list
    /// on every single read would turn one round-trip into thousands, and a
    /// server that genuinely wants that has `listChanged` instead.
    #[must_use]
    pub fn from_result(result: &Value) -> Self {
        let ttl = result
            .get("ttlMs")
            .and_then(Value::as_u64)
            .filter(|ms| *ms > 0)
            .map(|ms| Duration::from_millis(ms).min(MAX_TTL));
        Self { ttl }
    }

    /// The instant this entry goes stale, given when it was fetched.
    ///
    /// `None` when the server supplied no hint, which callers read as "never
    /// expires on a timer".
    #[must_use]
    pub fn expires_at(&self, fetched_at: Instant) -> Option<Instant> {
        self.ttl.map(|ttl| fetched_at + ttl)
    }

    /// The freshness window itself, for logging and tests.
    #[must_use]
    pub const fn ttl(&self) -> Option<Duration> {
        self.ttl
    }
}

/// Whether a cache entry that expires at `expiry` is stale as of `now`.
///
/// An entry with no expiry never goes stale on a timer.
#[must_use]
pub fn is_stale(expiry: Option<Instant>, now: Instant) -> bool {
    expiry.is_some_and(|at| now >= at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_ttl_from_result() {
        let directive = CacheDirective::from_result(&json!({
            "tools": [],
            "ttlMs": 3_600_000,
            "cacheScope": "public"
        }));

        assert_eq!(directive.ttl(), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn absent_ttl_means_no_timer() {
        // Every pre-2026-07-28 list result lands here; behavior must be
        // identical to before the field existed.
        let directive = CacheDirective::from_result(&json!({"tools": []}));

        assert_eq!(directive.ttl(), None);
        assert_eq!(directive.expires_at(Instant::now()), None);
        assert!(!is_stale(None, Instant::now()));
    }

    #[test]
    fn zero_and_malformed_ttl_are_treated_as_absent() {
        assert_eq!(
            CacheDirective::from_result(&json!({"ttlMs": 0})).ttl(),
            None
        );
        assert_eq!(
            CacheDirective::from_result(&json!({"ttlMs": "3600"})).ttl(),
            None
        );
        assert_eq!(
            CacheDirective::from_result(&json!({"ttlMs": -5})).ttl(),
            None
        );
    }

    #[test]
    fn absurd_ttl_is_clamped() {
        // A server reporting a year keeps Aleph on a tool list that may no
        // longer exist; the clamp bounds that without rejecting the hint.
        let directive = CacheDirective::from_result(&json!({"ttlMs": 365u64 * 24 * 3600 * 1000}));

        assert_eq!(directive.ttl(), Some(MAX_TTL));
    }

    #[test]
    fn entry_is_stale_only_after_its_expiry() {
        let fetched = Instant::now();
        let directive = CacheDirective::from_result(&json!({"ttlMs": 60_000}));
        let expiry = directive.expires_at(fetched);

        assert!(!is_stale(expiry, fetched));
        assert!(!is_stale(expiry, fetched + Duration::from_secs(59)));
        assert!(is_stale(expiry, fetched + Duration::from_secs(60)));
        assert!(is_stale(expiry, fetched + Duration::from_secs(61)));
    }
}
