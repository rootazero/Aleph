//! Capability tokens for the `/artifact/...` byte route.
//!
//! The artifact byte route is a **second ingress**: a plain HTTP `GET` that
//! inherits none of the `/ws` guards (no `connect` handshake, no device tier,
//! no session). A capability is what re-establishes the missing authority — an
//! unguessable bearer minted only by an already-authenticated WS RPC and scoped
//! to exactly one session key.
//!
//! Properties that matter:
//!
//! * **256 bits of CSPRNG randomness**, base64url — not guessable, not derived
//!   from anything an attacker controls.
//! * **Session-scoped.** A capability names one session. Holding session A's
//!   capability never reads session B's bytes, because the byte route resolves
//!   the session *from the capability* and asks the store only within it.
//! * **Expiring.** 8 hours, so a capability that leaks into a browser history
//!   or a copied URL stops working. Expired entries are pruned lazily on every
//!   call — the table only ever holds live sessions, so no reaper task.
//! * **Constant-time comparison.** Lookup is keyed by session (not by the
//!   secret), and the secret itself is compared with
//!   [`crate::security::secret_equal_bytes`].
//!
//! Minting is idempotent per session while a capability is still fresh: the
//! Panel re-lists artifacts constantly, and a new capability per list would
//! both grow this table without bound and break browser caching of `<img src>`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;

/// How long a minted capability stays valid.
pub const CAP_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// Capability entropy in bytes (256 bits).
const CAP_BYTES: usize = 32;

/// Hard ceiling on live capabilities (one per session).
///
/// Minting is reachable from `artifacts.list`, whose `session_key` is a caller
/// string — a client can therefore ask for a capability for a session that does
/// not exist, and expiry alone would let 8 hours of that grow the table without
/// bound (CWE-400). At the ceiling the entry closest to expiry is dropped; the
/// same bounding the rate limiter applies to its own identity map.
const MAX_LIVE_CAPS: usize = 4096;

/// One live capability. Keyed by session key in the table.
struct CapEntry {
    cap: String,
    expires_at: Instant,
}

/// Process-wide capability table, `session_key -> CapEntry`.
///
/// Keying by session (rather than by the secret) keeps lookup O(1) *and* keeps
/// the secret out of the hash — the only comparison against it is constant
/// time.
fn table() -> &'static Mutex<HashMap<String, CapEntry>> {
    static TABLE: OnceLock<Mutex<HashMap<String, CapEntry>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// OS-backed CSPRNG, mirroring `gateway::security::crypto::os_rng`.
const fn os_rng() -> UnwrapErr<SysRng> {
    UnwrapErr(SysRng)
}

/// Capability minting and checking for the artifact byte route.
///
/// A unit struct over process-global state rather than an injected service:
/// the byte route is registered on the raw `axum` router, which has no access
/// to the gateway's handler context, and both slices must agree on one table.
pub struct ArtifactCapabilities;

impl ArtifactCapabilities {
    /// Mint (or reuse) the capability for `session_key`.
    ///
    /// Called from an already-authenticated WS RPC — that authentication is
    /// what the returned string carries forward. Returns the existing
    /// capability when one is still fresh, so URLs stay stable across repeated
    /// listings.
    #[must_use]
    pub fn mint(session_key: &str) -> String {
        let now = Instant::now();
        let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
        table.retain(|_, entry| entry.expires_at > now);

        if let Some(entry) = table.get(session_key) {
            return entry.cap.clone();
        }

        if table.len() >= MAX_LIVE_CAPS {
            let victim = table
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(session, _)| session.clone());
            if let Some(session) = victim {
                table.remove(&session);
            }
        }

        let mut bytes = [0u8; CAP_BYTES];
        os_rng().fill_bytes(&mut bytes);
        let cap = URL_SAFE_NO_PAD.encode(bytes);
        table.insert(
            session_key.to_string(),
            CapEntry {
                cap: cap.clone(),
                expires_at: now + CAP_TTL,
            },
        );
        cap
    }

    /// Check that `cap` is the live capability for `session_key`.
    ///
    /// Returns `false` for an unknown session, an expired capability, or a
    /// capability belonging to a different session.
    #[must_use]
    pub fn validate(cap: &str, session_key: &str) -> bool {
        let now = Instant::now();
        let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
        table.retain(|_, entry| entry.expires_at > now);

        table.get(session_key).is_some_and(|entry| {
            crate::security::secret_equal_bytes(cap.as_bytes(), entry.cap.as_bytes())
        })
    }

    /// Resolve which session a capability authorises, if any.
    ///
    /// The byte route's URL carries only the capability, so this is how it
    /// learns which session's store to read — it must never take a session
    /// from the client. Scans with a constant-time compare; the table holds at
    /// most one entry per live session.
    #[must_use]
    pub fn session_for(cap: &str) -> Option<String> {
        let now = Instant::now();
        let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
        table.retain(|_, entry| entry.expires_at > now);

        table
            .iter()
            .find(|(_, entry)| {
                crate::security::secret_equal_bytes(cap.as_bytes(), entry.cap.as_bytes())
            })
            .map(|(session_key, _)| session_key.clone())
    }

    /// Age a session's capability past its TTL.
    ///
    /// `Instant` cannot be advanced, so expiry is exercised by moving the entry
    /// into the past. Crate-visible because the byte route's tests live in
    /// another module and must be able to build an expired capability.
    #[cfg(test)]
    pub(crate) fn expire_for_test(session_key: &str) {
        let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(session_key) {
            entry.expires_at = Instant::now() - Duration::from_secs(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests share one process-global table, so each uses a unique session key.
    fn unique_key(tag: &str) -> String {
        format!("agent:test:{tag}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn minted_capability_validates_for_its_session() {
        let key = unique_key("mint");
        let cap = ArtifactCapabilities::mint(&key);
        assert!(!cap.is_empty());
        assert!(ArtifactCapabilities::validate(&cap, &key));
    }

    #[test]
    fn capability_is_reused_while_fresh() {
        let key = unique_key("reuse");
        let first = ArtifactCapabilities::mint(&key);
        let second = ArtifactCapabilities::mint(&key);
        assert_eq!(first, second, "a fresh capability must be reused");
    }

    #[test]
    fn capability_for_one_session_does_not_open_another() {
        let a = unique_key("a");
        let b = unique_key("b");
        let cap_a = ArtifactCapabilities::mint(&a);
        let cap_b = ArtifactCapabilities::mint(&b);
        assert_ne!(cap_a, cap_b);
        assert!(!ArtifactCapabilities::validate(&cap_a, &b));
        assert!(!ArtifactCapabilities::validate(&cap_b, &a));
    }

    #[test]
    fn unknown_capability_is_rejected() {
        let key = unique_key("unknown");
        let _live = ArtifactCapabilities::mint(&key);
        assert!(!ArtifactCapabilities::validate("not-a-capability", &key));
        assert!(!ArtifactCapabilities::validate("", &key));
        assert!(ArtifactCapabilities::session_for("not-a-capability").is_none());
    }

    #[test]
    fn capability_for_an_unminted_session_is_rejected() {
        let minted = unique_key("minted");
        let never = unique_key("never");
        let cap = ArtifactCapabilities::mint(&minted);
        assert!(!ArtifactCapabilities::validate(&cap, &never));
    }

    #[test]
    fn expired_capability_is_pruned_and_rejected() {
        let key = unique_key("expired");
        let cap = ArtifactCapabilities::mint(&key);
        {
            let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
            let entry = table.get_mut(&key).expect("just minted");
            entry.expires_at = Instant::now() - Duration::from_secs(1);
        }
        assert!(!ArtifactCapabilities::validate(&cap, &key));
        assert!(ArtifactCapabilities::session_for(&cap).is_none());
    }

    #[test]
    fn session_for_resolves_the_owning_session() {
        let key = unique_key("resolve");
        let cap = ArtifactCapabilities::mint(&key);
        assert_eq!(ArtifactCapabilities::session_for(&cap), Some(key));
    }

    #[test]
    fn capability_carries_full_entropy() {
        let key = unique_key("entropy");
        let cap = ArtifactCapabilities::mint(&key);
        let decoded = URL_SAFE_NO_PAD.decode(&cap).expect("base64url");
        assert_eq!(decoded.len(), CAP_BYTES, "256 bits of randomness");
    }
}
