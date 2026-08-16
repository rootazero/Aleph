//! Capability tokens for the `/canvas-asset/...` byte route.
//!
//! The whiteboard asset byte route is a **second ingress** exactly like the
//! artifact one (`artifact_caps` — read that module doc first): a plain HTTP
//! `GET` that inherits none of the `/ws` guards. A capability is what
//! re-establishes the missing authority — an unguessable bearer minted only
//! by an already-authenticated, already-visibility-gated `canvas.get`, and
//! scoped to exactly one canvas.
//!
//! A **separate table** from [`super::artifact_caps`] on purpose, not a
//! shared one: the two capabilities authorize different stores over
//! different key spaces (session vs canvas), and one table would make every
//! artifact capability a candidate secret for the canvas route and vice
//! versa — a lookup that can answer for both is a confusion the type system
//! should forbid, not a deduplication win.
//!
//! Deliberate differences from the twin:
//!
//! * **Canvas-scoped.** The key is a canvas id, minted only after
//!   `gate_canvas` admitted the caller — so holding the capability proves a
//!   recent, positive visibility verdict for that one canvas.
//! * **10-minute TTL** (vs 8 hours). Canvas visibility is live state — a
//!   roster edit can revoke it at any moment — so the capability's lifetime
//!   is the staleness we tolerate on that verdict. The Panel re-runs
//!   `canvas.get` far more often than every 10 minutes (every open, every
//!   conflict re-pull), so a fresh capability is always at hand; asset BYTES
//!   stay browser-cached regardless (content-addressed, `max-age=3600`),
//!   which is what makes the short TTL cheap.
//!
//! Everything else mirrors the twin byte for byte: 256-bit CSPRNG secrets,
//! lazy pruning on every call, constant-time comparison, a hard table
//! ceiling, and mint idempotency while an entry is fresh (stable URLs keep
//! `<image href>` browser caching working).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;

/// How long a minted canvas capability stays valid — the staleness bound on
/// the visibility verdict it carries (see the module doc).
pub const CANVAS_CAP_TTL: Duration = Duration::from_secs(10 * 60);

/// Capability entropy in bytes (256 bits).
const CAP_BYTES: usize = 32;

/// Hard ceiling on live capabilities (one per canvas).
///
/// Minting only happens after `gate_canvas` resolved a real, visible
/// document, so unlike the artifact table this cannot be grown by probing
/// made-up ids — but a large library plus a 10-minute TTL still deserves a
/// bound rather than a promise (CWE-400). At the ceiling the entry closest
/// to expiry is dropped.
const MAX_LIVE_CAPS: usize = 4096;

/// One live capability. Keyed by canvas id in the table.
struct CapEntry {
    cap: String,
    expires_at: Instant,
}

/// Process-wide capability table, `canvas_id -> CapEntry`.
///
/// Keyed by canvas (rather than by the secret) so lookup is O(1) and the
/// secret stays out of the hash — the only comparison against it is
/// constant time.
fn table() -> &'static Mutex<HashMap<String, CapEntry>> {
    static TABLE: OnceLock<Mutex<HashMap<String, CapEntry>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// OS-backed CSPRNG, mirroring `gateway::security::crypto::os_rng`.
const fn os_rng() -> UnwrapErr<SysRng> {
    UnwrapErr(SysRng)
}

/// Capability minting and checking for the canvas-asset byte route.
///
/// A unit struct over process-global state for the same reason as
/// [`super::artifact_caps::ArtifactCapabilities`]: the byte route lives on
/// the raw `axum` router with no handler context, and both slices must
/// agree on one table.
pub struct CanvasCapabilities;

impl CanvasCapabilities {
    /// Mint (or reuse) the capability for `canvas_id`.
    ///
    /// Called from `canvas.get` AFTER its visibility gate admitted the
    /// caller — that verdict is what the returned string carries forward.
    /// Returns the existing capability while one is still fresh, so
    /// `asset_base` stays stable across repeated gets and the browser keeps
    /// its image cache.
    #[must_use]
    pub fn mint(canvas_id: &str) -> String {
        let now = Instant::now();
        let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
        table.retain(|_, entry| entry.expires_at > now);

        if let Some(entry) = table.get(canvas_id) {
            return entry.cap.clone();
        }

        if table.len() >= MAX_LIVE_CAPS {
            let victim = table
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(canvas, _)| canvas.clone());
            if let Some(canvas) = victim {
                table.remove(&canvas);
            }
        }

        let mut bytes = [0u8; CAP_BYTES];
        os_rng().fill_bytes(&mut bytes);
        let cap = URL_SAFE_NO_PAD.encode(bytes);
        table.insert(
            canvas_id.to_string(),
            CapEntry {
                cap: cap.clone(),
                expires_at: now + CANVAS_CAP_TTL,
            },
        );
        cap
    }

    /// Check that `cap` is the live capability for `canvas_id`.
    ///
    /// Returns `false` for an unknown canvas, an expired capability, or a
    /// capability belonging to a different canvas.
    #[must_use]
    pub fn validate(cap: &str, canvas_id: &str) -> bool {
        let now = Instant::now();
        let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
        table.retain(|_, entry| entry.expires_at > now);

        table.get(canvas_id).is_some_and(|entry| {
            crate::security::secret_equal_bytes(cap.as_bytes(), entry.cap.as_bytes())
        })
    }

    /// Resolve which canvas a capability authorises, if any.
    ///
    /// The byte route resolves the canvas *from the capability* and then
    /// requires the URL to name that same canvas — it never trusts the
    /// client's path segment as the scope. Scans with a constant-time
    /// compare; at most one entry per live canvas.
    #[must_use]
    pub fn canvas_for(cap: &str) -> Option<String> {
        let now = Instant::now();
        let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
        table.retain(|_, entry| entry.expires_at > now);

        table
            .iter()
            .find(|(_, entry)| {
                crate::security::secret_equal_bytes(cap.as_bytes(), entry.cap.as_bytes())
            })
            .map(|(canvas_id, _)| canvas_id.clone())
    }

    /// Age a canvas's capability past its TTL.
    ///
    /// `Instant` cannot be advanced, so expiry is exercised by moving the
    /// entry into the past. Crate-visible because the byte route's tests
    /// live in another module and must be able to build an expired
    /// capability.
    #[cfg(test)]
    pub(crate) fn expire_for_test(canvas_id: &str) {
        let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = table.get_mut(canvas_id) {
            entry.expires_at = Instant::now() - Duration::from_secs(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests share one process-global table, so each uses a unique canvas id.
    fn unique_canvas(tag: &str) -> String {
        format!("cv-{tag}{}", uuid::Uuid::new_v4().simple())
    }

    #[test]
    fn minted_capability_validates_for_its_canvas_and_is_reused_while_fresh() {
        let id = unique_canvas("mint");
        let cap = CanvasCapabilities::mint(&id);
        assert!(!cap.is_empty());
        assert!(CanvasCapabilities::validate(&cap, &id));
        assert_eq!(
            cap,
            CanvasCapabilities::mint(&id),
            "a fresh capability must be reused so asset URLs stay stable"
        );
    }

    #[test]
    fn capability_for_one_canvas_does_not_open_another() {
        let a = unique_canvas("a");
        let b = unique_canvas("b");
        let cap_a = CanvasCapabilities::mint(&a);
        let cap_b = CanvasCapabilities::mint(&b);
        assert_ne!(cap_a, cap_b);
        assert!(!CanvasCapabilities::validate(&cap_a, &b));
        assert!(!CanvasCapabilities::validate(&cap_b, &a));
        assert_eq!(CanvasCapabilities::canvas_for(&cap_a), Some(a));
    }

    #[test]
    fn unknown_and_expired_capabilities_are_rejected() {
        let id = unique_canvas("dead");
        let cap = CanvasCapabilities::mint(&id);
        assert!(!CanvasCapabilities::validate("not-a-capability", &id));
        assert!(CanvasCapabilities::canvas_for("not-a-capability").is_none());

        CanvasCapabilities::expire_for_test(&id);
        assert!(!CanvasCapabilities::validate(&cap, &id));
        assert!(CanvasCapabilities::canvas_for(&cap).is_none());
    }

    #[test]
    fn capability_carries_full_entropy_and_a_short_ttl() {
        let id = unique_canvas("entropy");
        let cap = CanvasCapabilities::mint(&id);
        let decoded = URL_SAFE_NO_PAD.decode(&cap).expect("base64url");
        assert_eq!(decoded.len(), CAP_BYTES, "256 bits of randomness");
        // The TTL is the staleness bound on a LIVE visibility verdict —
        // an hour here would be a policy change, not a tweak.
        assert!(CANVAS_CAP_TTL <= Duration::from_secs(10 * 60));
    }
}
