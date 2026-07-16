//! Process-global per-session MoA activation state.
//!
//! Mirrors [`session_model_handle`](super::session_model_handle): a
//! process-global lock-guarded map keyed by the canonical `SessionKey`
//! string; written by the `moa` tool / the `/moa` one-shot intercept, read
//! (and for one-shots, consumed) at run construction in `harness_bridge`.
//! In-memory by design — soft UX state that resets on restart.
//!
//! One-shot restore is a single mechanism: [`take_for_run`] consumes a
//! `one_shot` pref atomically under the write lock — removing it when the
//! slot carries no stash, or reinstating the stashed sticky when one-shot
//! displaced one — so success, error and cancel paths all resolve through
//! this one path (hermes needed three divergent restore implementations;
//! this is surpass item ④ in the spec).

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
    /// A sticky pref displaced by a one-shot arm, reinstated when the one-shot
    /// is consumed by [`take_for_run`]. `None` for sticky prefs and for
    /// one-shots armed over an empty or one-shot slot. Boxed to keep the
    /// (self-referential) struct small.
    pub restore: Option<Box<SessionMoaPref>>,
}

static SESSION_MOA: OnceLock<RwLock<HashMap<String, SessionMoaPref>>> = OnceLock::new();

fn map() -> &'static RwLock<HashMap<String, SessionMoaPref>> {
    SESSION_MOA.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Record (or overwrite) the session's MoA activation. Clears any stash
/// (a fresh explicit activation is not a one-shot displacement).
pub fn set_session_moa(session_key: &str, preset: Option<String>, one_shot: bool) {
    map().write().unwrap_or_else(|e| e.into_inner()).insert(
        session_key.to_string(),
        SessionMoaPref {
            preset,
            one_shot,
            restore: None,
        },
    );
}

/// Arm a one-shot pref. If an existing STICKY pref occupies the slot, stash
/// it into `restore` so [`take_for_run`] reinstates it after consuming this
/// one-shot. Arming over an empty or one-shot slot stashes nothing.
pub fn set_session_moa_one_shot(session_key: &str, preset: Option<String>) {
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    let restore = guard
        .get(session_key)
        .filter(|p| !p.one_shot)
        .cloned()
        .map(Box::new);
    guard.insert(
        session_key.to_string(),
        SessionMoaPref {
            preset,
            one_shot: true,
            restore,
        },
    );
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

/// Read for run construction. A `one_shot` pref is REMOVED — or, when it
/// carries a stashed sticky, REPLACED by that sticky — in the same write-lock
/// section it is read in (the single restore point).
#[must_use]
pub fn take_for_run(session_key: &str) -> Option<SessionMoaPref> {
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    let pref = guard.get(session_key).cloned()?;
    if pref.one_shot {
        match &pref.restore {
            Some(sticky) => {
                guard.insert(session_key.to_string(), (**sticky).clone());
            }
            None => {
                guard.remove(session_key);
            }
        }
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
/// must not burn on a run that never used it. Fills an EMPTY slot, and — the
/// displaced-sticky case — undoes `take_for_run`'s reinstatement: when the
/// slot holds exactly the sticky this one-shot had stashed (i.e. what
/// `take_for_run` put back while consuming it), the one-shot is re-stacked,
/// stash included. Any OTHER occupant is a genuinely newer activation and
/// wins. Post-construction failures/cancels do NOT restore (structural
/// zero-leak, round-1 surpass item ④ unchanged).
pub fn restore_one_shot(session_key: &str, pref: SessionMoaPref) {
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    let undo_reinstate = match guard.get(session_key) {
        None => true,
        Some(current) => pref.restore.as_deref() == Some(current),
    };
    if undo_reinstate {
        guard.insert(session_key.to_string(), pref);
    }
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
        assert_eq!(
            get_session_moa(key).unwrap().preset.as_deref(),
            Some("deep")
        );
        assert!(get_session_moa(key).unwrap().one_shot);
        // A newer activation written meanwhile must NOT be clobbered.
        set_session_moa(key, Some("newer".to_string()), false);
        restore_one_shot(key, pref);
        assert_eq!(
            get_session_moa(key).unwrap().preset.as_deref(),
            Some("newer")
        );
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
                restore: None,
            },
        );
        let stored = get_session_moa(key).unwrap();
        assert_eq!(stored.preset.as_deref(), Some("deep"));
        assert!(!stored.one_shot);
        clear_session_moa(key);
    }

    #[test]
    fn restore_one_shot_undoes_reinstated_sticky() {
        // MOA-2: one-shot displaced sticky A; take_for_run consumed it and
        // reinstated A. A build failure then restores — the one-shot must
        // re-stack over the reinstated A (undoing the consume), NOT no-op and
        // silently burn the user's single activation on a run that never
        // engaged MoA.
        let key = "test:moa:restore-over-reinstated";
        set_session_moa(key, Some("deep".to_string()), false); // sticky A
        set_session_moa_one_shot(key, Some("once".to_string()));
        let taken = take_for_run(key).unwrap();
        assert!(taken.one_shot);
        // take_for_run reinstated the sticky.
        assert_eq!(
            get_session_moa(key).unwrap().preset.as_deref(),
            Some("deep")
        );
        // Build failed before MoA engaged → restore re-stacks the one-shot.
        restore_one_shot(key, taken);
        let stored = get_session_moa(key).unwrap();
        assert!(stored.one_shot, "one-shot must survive the failed build");
        assert_eq!(stored.preset.as_deref(), Some("once"));
        // …and the stash is intact: the NEXT consume reinstates A again.
        let _ = take_for_run(key).unwrap();
        assert_eq!(
            get_session_moa(key).unwrap().preset.as_deref(),
            Some("deep")
        );
        clear_session_moa(key);
    }

    #[test]
    fn one_shot_over_sticky_reinstates_sticky_on_consume() {
        let key = "test:moa:f2:reinstate";
        set_session_moa(key, Some("deep".to_string()), false); // sticky
        set_session_moa_one_shot(key, None); // one-shot over sticky
                                             // Slot now holds the one-shot, carrying the stashed sticky.
        let taken = take_for_run(key).unwrap();
        assert!(taken.one_shot);
        assert!(taken.preset.is_none());
        // Consuming the one-shot reinstates the sticky for the next run.
        let after = get_session_moa(key).unwrap();
        assert!(!after.one_shot);
        assert_eq!(after.preset.as_deref(), Some("deep"));
        assert!(after.restore.is_none());
        clear_session_moa(key);
    }

    #[test]
    fn one_shot_over_empty_slot_removes_on_consume() {
        let key = "test:moa:f2:empty";
        set_session_moa_one_shot(key, Some("x".to_string()));
        let taken = take_for_run(key).unwrap();
        assert!(taken.one_shot);
        assert!(taken.restore.is_none());
        assert!(get_session_moa(key).is_none()); // nothing stashed → removed
    }

    #[test]
    fn one_shot_over_one_shot_stashes_nothing() {
        let key = "test:moa:f2:double-oneshot";
        set_session_moa_one_shot(key, Some("a".to_string()));
        set_session_moa_one_shot(key, Some("b".to_string()));
        let taken = take_for_run(key).unwrap();
        assert_eq!(taken.preset.as_deref(), Some("b"));
        assert!(taken.restore.is_none());
        assert!(get_session_moa(key).is_none());
    }

    #[test]
    fn explicit_clear_drops_stashed_sticky_too() {
        let key = "test:moa:f2:clear";
        set_session_moa(key, Some("deep".to_string()), false);
        set_session_moa_one_shot(key, None);
        clear_session_moa(key); // moa off wins — whole slot, stash included
        assert!(get_session_moa(key).is_none());
        assert!(take_for_run(key).is_none());
    }
}
