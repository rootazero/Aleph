//! In-process selection table — "what did the user select on this canvas".
//!
//! Semantics (spec §3): the value is **the most recent selection pushed for a
//! canvas** — multiple clients last-write-wins, which is exactly enough to
//! answer the model's "what is the user pointing at" question. Selection is
//! short-lived BY NATURE and deliberately owes no sidecar: unlike a run
//! registry (where "empty after restart" is a lie about work that happened,
//! §4.13b), an empty selection is a legitimate current value, and the Panel
//! re-establishes it on the next pointer interaction.
//!
//! Shape: `OnceLock<Mutex<_>>` process-global with a hard ceiling and
//! evict-oldest, mirroring `gateway/security/artifact_caps.rs`. The table
//! logic lives on a private struct so the eviction bound is testable on a
//! local instance without racing parallel tests through the global table;
//! the struct is module-private, so the global below stays the only
//! production instance (§3.15③: a second writer to a process-global table is
//! a compile error here, not a discipline).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use aleph_protocol::canvas::MAX_SHAPES;

/// Hard ceiling on live selection entries (one per canvas).
///
/// `canvas.selection.set` is caller-driven and the canvas id space is open,
/// so expiry-free growth would be unbounded (CWE-400). At the ceiling the
/// least-recently-written entry is dropped — same bound, same reasoning as
/// `artifact_caps::MAX_LIVE_CAPS`.
const MAX_LIVE: usize = 4096;

/// One canvas's latest pushed selection, stamped for evict-oldest.
struct Entry {
    shape_ids: Vec<String>,
    /// Monotonic write counter — unlike an `Instant`, ties are impossible,
    /// so the eviction victim is deterministic (§0: an aggregate's "which
    /// key won" must not depend on `HashMap` iteration order).
    stamp: u64,
}

/// Selection state, `canvas_id -> Entry`. Module-private on purpose — see
/// the module doc; only [`set`]/[`get`] below reach the global instance.
#[derive(Default)]
struct SelectionTable {
    entries: HashMap<String, Entry>,
    next_stamp: u64,
}

impl SelectionTable {
    fn set(&mut self, canvas_id: &str, mut shape_ids: Vec<String>) {
        // Absent and empty are THE SAME answer for a selection (no third
        // surface distinguishes them), so an empty push frees the slot
        // instead of spending one on a value `get` synthesizes anyway.
        if shape_ids.is_empty() {
            self.entries.remove(canvas_id);
            return;
        }
        // A selection can never name more live shapes than a document may
        // hold; the excess is unaddressable noise, and truncating here keeps
        // the per-entry dimension (ids) bounded alongside the entry count.
        shape_ids.truncate(MAX_SHAPES);
        let stamp = self.next_stamp;
        self.next_stamp += 1;
        if self.entries.len() >= MAX_LIVE && !self.entries.contains_key(canvas_id) {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.stamp)
                .map(|(k, _)| k.clone());
            if let Some(victim) = victim {
                self.entries.remove(&victim);
            }
        }
        self.entries
            .insert(canvas_id.to_string(), Entry { shape_ids, stamp });
    }

    fn get(&self, canvas_id: &str) -> Vec<String> {
        self.entries
            .get(canvas_id)
            .map(|e| e.shape_ids.clone())
            .unwrap_or_default()
    }
}

/// Process-wide selection table.
fn table() -> &'static Mutex<SelectionTable> {
    static TABLE: OnceLock<Mutex<SelectionTable>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(SelectionTable::default()))
}

/// Record the latest selection pushed for `canvas_id` (last write wins).
///
/// Callers gate visibility BEFORE writing (the RPC face checks
/// `canvas_visible` first); this table is storage, not an authority.
pub fn set(canvas_id: &str, shape_ids: Vec<String>) {
    table()
        .lock()
        .unwrap_or_else(|e| { tracing::error!(
                reason = %e,
                "canvas lock table poisoned: a previous holder panicked mid-insert; recovering"
            ); e.into_inner() })
        .set(canvas_id, shape_ids);
}

/// The most recent selection pushed for `canvas_id`; empty when none (or
/// when the entry was evicted — indistinguishable on purpose, see module doc).
#[must_use]
pub fn get(canvas_id: &str) -> Vec<String> {
    table()
        .lock()
        .unwrap_or_else(|e| { tracing::error!(
                reason = %e,
                "canvas lock table poisoned: a previous holder panicked mid-insert; recovering"
            ); e.into_inner() })
        .get(canvas_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests of the global face share one process-wide table with every
    /// parallel test, so each uses a unique canvas id. Eviction is exercised
    /// on a LOCAL instance below — filling the global to its ceiling would
    /// evict sibling tests' entries mid-flight.
    fn unique_canvas() -> String {
        format!("cv-{}", uuid::Uuid::new_v4().simple())
    }

    #[test]
    fn set_then_get_round_trips_and_the_last_write_wins() {
        let id = unique_canvas();
        set(&id, vec!["a".into(), "b".into()]);
        assert_eq!(get(&id), vec!["a".to_string(), "b".to_string()]);
        set(&id, vec!["c".into()]);
        assert_eq!(get(&id), vec!["c".to_string()], "last push wins");
    }

    #[test]
    fn an_unknown_canvas_reads_as_an_empty_selection() {
        assert!(get(&unique_canvas()).is_empty());
    }

    #[test]
    fn an_empty_push_clears_the_selection_and_frees_the_slot() {
        let id = unique_canvas();
        set(&id, vec!["a".into()]);
        set(&id, Vec::new());
        assert!(get(&id).is_empty());
        // The slot really is freed, not parked holding an empty vec.
        let mut local = SelectionTable::default();
        local.set("cv-x", vec!["a".into()]);
        local.set("cv-x", Vec::new());
        assert!(local.entries.is_empty(), "empty push must remove the entry");
    }

    #[test]
    fn an_oversized_selection_is_truncated_to_the_shape_cap() {
        let mut local = SelectionTable::default();
        let ids: Vec<String> = (0..MAX_SHAPES + 10).map(|i| format!("s{i}")).collect();
        local.set("cv-big", ids);
        assert_eq!(local.get("cv-big").len(), MAX_SHAPES);
    }

    #[test]
    fn the_table_evicts_the_oldest_entry_at_capacity() {
        let mut local = SelectionTable::default();
        for i in 0..MAX_LIVE {
            local.set(&format!("cv-{i}"), vec!["s".into()]);
        }
        assert_eq!(local.entries.len(), MAX_LIVE);
        local.set("cv-one-more", vec!["s".into()]);
        assert_eq!(local.entries.len(), MAX_LIVE, "the ceiling holds");
        assert!(
            local.get("cv-0").is_empty(),
            "the oldest write is the victim"
        );
        assert_eq!(local.get("cv-one-more"), vec!["s".to_string()]);
        assert_eq!(local.get("cv-1"), vec!["s".to_string()]);
    }

    #[test]
    fn rewriting_an_entry_refreshes_its_age() {
        let mut local = SelectionTable::default();
        for i in 0..MAX_LIVE {
            local.set(&format!("cv-{i}"), vec!["s".into()]);
        }
        // cv-0 is the oldest — rewrite it, making cv-1 the oldest.
        local.set("cv-0", vec!["fresh".into()]);
        local.set("cv-one-more", vec!["s".into()]);
        assert_eq!(
            local.get("cv-0"),
            vec!["fresh".to_string()],
            "a rewritten entry must not be the eviction victim"
        );
        assert!(local.get("cv-1").is_empty(), "the true oldest goes instead");
    }

    #[test]
    fn a_rewrite_at_capacity_does_not_evict_anyone() {
        let mut local = SelectionTable::default();
        for i in 0..MAX_LIVE {
            local.set(&format!("cv-{i}"), vec!["s".into()]);
        }
        local.set("cv-5", vec!["updated".into()]);
        assert_eq!(local.entries.len(), MAX_LIVE);
        for i in 0..MAX_LIVE {
            assert!(
                !local.get(&format!("cv-{i}")).is_empty(),
                "cv-{i} must survive a rewrite of an existing key"
            );
        }
    }
}
