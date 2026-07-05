//! Process-global per-session MoA activation state.
//!
//! Mirrors [`session_model_handle`](super::session_model_handle): a
//! process-global lock-guarded map keyed by the canonical `SessionKey`
//! string; written by the `moa` tool / the `/moa` one-shot intercept, read
//! (and for one-shots, consumed) at run construction in `harness_bridge`.
//! In-memory by design — soft UX state that resets on restart.
//!
//! One-shot restore is a single mechanism: [`take_for_run`] removes a
//! `one_shot` pref atomically under the write lock, so success, error and
//! cancel paths all leave no state behind (hermes needed three divergent
//! restore implementations; this is surpass item ④ in the spec).

use crate::sync_primitives::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

/// A session's MoA activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMoaPref {
    /// Preset name; `None` = the config `default_preset`.
    pub preset: Option<String>,
    /// `true` = applies to exactly one run, consumed by [`take_for_run`].
    pub one_shot: bool,
}

static SESSION_MOA: OnceLock<RwLock<HashMap<String, SessionMoaPref>>> = OnceLock::new();

fn map() -> &'static RwLock<HashMap<String, SessionMoaPref>> {
    SESSION_MOA.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Record (or overwrite) the session's MoA activation.
pub fn set_session_moa(session_key: &str, preset: Option<String>, one_shot: bool) {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_key.to_string(), SessionMoaPref { preset, one_shot });
}

/// Non-consuming read (for `moa status`).
#[must_use]
pub fn get_session_moa(session_key: &str) -> Option<SessionMoaPref> {
    map()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_key)
        .cloned()
}

/// Read for run construction. A `one_shot` pref is REMOVED in the same
/// write-lock section it is read in — the single restore point.
#[must_use]
pub fn take_for_run(session_key: &str) -> Option<SessionMoaPref> {
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    let pref = guard.get(session_key).cloned()?;
    if pref.one_shot {
        guard.remove(session_key);
    }
    Some(pref)
}

/// Drop the session's activation (`moa off`).
pub fn clear_session_moa(session_key: &str) {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_key);
}

/// Refill a consumed one-shot when run construction failed BEFORE MoA could
/// engage (preset deleted / slot unresolvable) — the user's single activation
/// must not burn on a run that never used it. Only fills an EMPTY slot: an
/// activation written meanwhile wins. Post-construction failures/cancels do
/// NOT restore (structural zero-leak, round-1 surpass item ④ unchanged).
pub fn restore_one_shot(session_key: &str, pref: SessionMoaPref) {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .entry(session_key.to_string())
        .or_insert(pref);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_survives_take_for_run() {
        let key = "test:moa:sticky";
        set_session_moa(key, Some("deep".to_string()), false);
        let taken = take_for_run(key).unwrap();
        assert_eq!(taken.preset.as_deref(), Some("deep"));
        assert!(!taken.one_shot);
        // Sticky: still present for the next run.
        assert!(take_for_run(key).is_some());
        clear_session_moa(key);
        assert!(take_for_run(key).is_none());
    }

    #[test]
    fn one_shot_consumed_atomically() {
        let key = "test:moa:oneshot";
        set_session_moa(key, None, true);
        let taken = take_for_run(key).unwrap();
        assert!(taken.one_shot);
        // Consumed: a second run sees nothing — no restore step can leak.
        assert!(take_for_run(key).is_none());
        assert!(get_session_moa(key).is_none());
    }

    #[test]
    fn status_read_does_not_consume() {
        let key = "test:moa:status";
        set_session_moa(key, None, true);
        assert!(get_session_moa(key).is_some());
        assert!(get_session_moa(key).is_some());
        clear_session_moa(key);
    }

    #[test]
    fn restore_one_shot_refills_empty_slot_only() {
        let key = "test:moa:restore";
        set_session_moa(key, Some("deep".to_string()), true);
        let pref = take_for_run(key).unwrap();
        assert!(get_session_moa(key).is_none());
        // Build failed → restore: the one-shot survives for the next turn.
        restore_one_shot(key, pref.clone());
        assert_eq!(get_session_moa(key).unwrap().preset.as_deref(), Some("deep"));
        assert!(get_session_moa(key).unwrap().one_shot);
        // A newer activation written meanwhile must NOT be clobbered.
        set_session_moa(key, Some("newer".to_string()), false);
        restore_one_shot(key, pref);
        assert_eq!(get_session_moa(key).unwrap().preset.as_deref(), Some("newer"));
        clear_session_moa(key);
    }

    #[test]
    fn restore_after_sticky_take_is_a_noop() {
        let key = "test:moa:sticky-restore";
        // Sticky pref: take_for_run reads WITHOUT consuming — the slot stays
        // occupied for the next run.
        set_session_moa(key, Some("deep".to_string()), false);
        let pref = take_for_run(key).unwrap();
        assert!(!pref.one_shot);
        assert!(get_session_moa(key).is_some());
        // A build-failure path that wrongly skipped the `one_shot` gate and
        // called restore anyway must be a no-op on the occupied slot.
        restore_one_shot(key, pref);
        let stored = get_session_moa(key).unwrap();
        assert_eq!(stored.preset.as_deref(), Some("deep"));
        assert!(!stored.one_shot);
        // Same call with a TAMPERED pref: proves the no-op is real (an
        // identical refill above would also pass under a buggy overwrite).
        restore_one_shot(
            key,
            SessionMoaPref {
                preset: Some("tampered".to_string()),
                one_shot: true,
            },
        );
        let stored = get_session_moa(key).unwrap();
        assert_eq!(stored.preset.as_deref(), Some("deep"));
        assert!(!stored.one_shot);
        clear_session_moa(key);
    }
}
