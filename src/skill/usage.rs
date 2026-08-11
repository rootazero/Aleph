//! Per-skill usage tracking — a `.usage.json` sidecar in the skills dir.
//!
//! Mirrors hermes-agent's `.usage.json` sidecar pattern, hardened for Aleph:
//!   * Cross-process safety via `utils::atomic_io::with_file_lock` on
//!     `<skills_dir>/.usage.json.lock`. Two concurrent skill reads in
//!     different processes will not lose counter increments.
//!   * Atomic writes via `utils::atomic_io::write_atomic`.
//!   * Best-effort throughout: every record/load failure degrades to a
//!     warn log and never propagates. A broken sidecar must never break
//!     the underlying tool call.
//!
//! New fields beyond the original sidecar (all `#[serde(default)]`, so
//! pre-existing `.usage.json` files deserialize without migration):
//!   * `patch_count` / `last_patched_at` — bumped when LLM mutates a skill
//!   * `created_at` — first time the sidecar saw this skill
//!   * `state` — `active` / `stale` / `archived` lifecycle marker
//!   * `pinned` — opt-out from any future auto lifecycle transitions
//!
//! The lifecycle fields are *telemetry only* in this layer. No code here
//! auto-transitions states. The transitions are owned by the dream
//! pipeline and split along the R7 (LLM Sovereignty) boundary:
//!
//!   * `Active → Stale`  — deterministic age-driven (system signal
//!     aggregation, no reasoning). Implemented in
//!     [`crate::memory::dreaming::stages::SkillLifecycleStage`].
//!   * `Stale → Archived` / consolidation / merge — reserved for a
//!     future LLM-driven curator stage that can weigh prefix clusters,
//!     usage trends, and consolidation candidates.
//!
//! Pinned skills (`pinned == true`) are exempt from *both* transitions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::utils::atomic_io::{with_file_lock, write_atomic};

/// Lifecycle state for a skill, mirroring hermes's three-state model.
///
/// `active` is the default for any skill we've seen. `stale` is set by
/// the rule-driven [`crate::memory::dreaming::stages::SkillLifecycleStage`]
/// when usage age crosses `DreamingConfig::skill_stale_after_days`.
/// `archived` is set only by a future LLM-driven curator stage; nothing
/// in this module or the lifecycle stage flips a skill to archived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillState {
    #[default]
    Active,
    Stale,
    Archived,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStats {
    #[serde(default)]
    pub use_count: u64,
    #[serde(default)]
    pub view_count: u64,
    #[serde(default)]
    pub patch_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_viewed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_patched_at: Option<String>,
    /// First time the sidecar observed this skill. Distinct from
    /// `last_used_at` so consumers can answer "never been touched" without
    /// a sentinel value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Lifecycle marker. Defaults to `active` for back-compat with old files.
    #[serde(default)]
    pub state: SkillState,
    /// Opt-out from future auto-archive. Default `false`.
    #[serde(default)]
    pub pinned: bool,
    /// Set when `state` flips to `archived`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
}

impl UsageStats {
    /// Newest activity timestamp across use / view / patch. Returns
    /// `None` if the skill has only been created and never touched.
    #[must_use]
    pub fn latest_activity_at(&self) -> Option<&str> {
        [
            self.last_used_at.as_deref(),
            self.last_viewed_at.as_deref(),
            self.last_patched_at.as_deref(),
        ]
        .into_iter()
        .flatten()
        .max()
    }

    /// Total observed activity events (use + view + patch).
    #[must_use]
    pub const fn activity_count(&self) -> u64 {
        self.use_count
            .saturating_add(self.view_count)
            .saturating_add(self.patch_count)
    }
}

/// Tracks skill usage in `<dir>/.usage.json`.
pub struct UsageStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl UsageStore {
    /// Create a store backed by `<skills_dir>/.usage.json`.
    pub fn new(skills_dir: impl AsRef<Path>) -> Self {
        let path = skills_dir.as_ref().join(".usage.json");
        let lock_path = skills_dir.as_ref().join(".usage.json.lock");
        Self { path, lock_path }
    }

    fn load_map(&self) -> HashMap<String, UsageStats> {
        match std::fs::read(&self.path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(map) => map,
                Err(e) => {
                    tracing::warn!(error = %e, path = %self.path.display(), "corrupted usage sidecar; resetting");
                    HashMap::new()
                }
            },
            Err(_) => HashMap::new(),
        }
    }

    fn save_map(&self, map: &HashMap<String, UsageStats>) {
        match serde_json::to_vec_pretty(map) {
            Ok(bytes) => {
                if let Err(e) = write_atomic(&self.path, &bytes) {
                    tracing::warn!(error = %e, "skill usage: atomic write failed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "skill usage: serialize failed"),
        }
    }

    /// Run a read-modify-write cycle under the cross-process file lock.
    /// All public mutators route through this so concurrent updates from
    /// multiple processes serialize safely.
    fn mutate<F>(&self, skill: &str, mutator: F)
    where
        F: FnOnce(&mut UsageStats),
    {
        if skill.is_empty() {
            return;
        }
        if let Some(parent) = self.lock_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(error = %e, "skill usage: mkdir failed");
                return;
            }
        }
        let result = with_file_lock(&self.lock_path, |_| {
            let mut map = self.load_map();
            let entry = map.entry(skill.to_string()).or_default();
            if entry.created_at.is_none() {
                entry.created_at = Some(now_iso());
            }
            mutator(entry);
            self.save_map(&map);
            Ok(())
        });
        if let Err(e) = result {
            tracing::warn!(error = %e, skill, "skill usage: lock acquisition failed");
        }
    }

    /// Read stats for one skill, if any.
    #[must_use]
    pub fn get(&self, skill: &str) -> Option<UsageStats> {
        self.load_map().get(skill).cloned()
    }

    /// Snapshot the entire usage map. Used by status reporting.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, UsageStats> {
        self.load_map()
    }

    /// Increment the view counter for `skill`. Best-effort.
    pub fn record_view(&self, skill: &str) {
        self.mutate(skill, |entry| {
            entry.view_count = entry.view_count.saturating_add(1);
            entry.last_viewed_at = Some(now_iso());
        });
    }

    /// Increment the use counter for `skill`. Best-effort.
    pub fn record_use(&self, skill: &str) {
        self.mutate(skill, |entry| {
            entry.use_count = entry.use_count.saturating_add(1);
            entry.last_used_at = Some(now_iso());
        });
    }

    /// Increment the patch counter for `skill`. Best-effort.
    ///
    /// "Patch" covers any LLM-driven mutation of skill state — install
    /// (which can rewrite bin links), enable/disable toggle, scope change.
    /// Matches hermes's `bump_patch` semantics.
    pub fn record_patch(&self, skill: &str) {
        self.mutate(skill, |entry| {
            entry.patch_count = entry.patch_count.saturating_add(1);
            entry.last_patched_at = Some(now_iso());
        });
    }

    /// Set the lifecycle state. Setting `Archived` stamps `archived_at`;
    /// setting `Active` clears it. Setting `Stale` leaves `archived_at`
    /// alone so promotion → demotion → re-archive preserves history.
    pub fn set_state(&self, skill: &str, state: SkillState) {
        self.mutate(skill, |entry| {
            entry.state = state;
            match state {
                SkillState::Archived => entry.archived_at = Some(now_iso()),
                SkillState::Active => entry.archived_at = None,
                SkillState::Stale => {}
            }
        });
    }

    /// Pin or unpin a skill. Pinned skills must be skipped by any future
    /// auto-archive curator.
    pub fn set_pinned(&self, skill: &str, pinned: bool) {
        self.mutate(skill, |entry| {
            entry.pinned = pinned;
        });
    }

    /// Drop a skill's usage entry. Called when the skill is deleted so
    /// the sidecar does not accumulate orphan rows over time.
    pub fn forget(&self, skill: &str) {
        if skill.is_empty() {
            return;
        }
        if let Some(parent) = self.lock_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(error = %e, "skill usage: mkdir failed");
                return;
            }
        }
        let result = with_file_lock(&self.lock_path, |_| {
            let mut map = self.load_map();
            if map.remove(skill).is_some() {
                self.save_map(&map);
            }
            Ok(())
        });
        if let Err(e) = result {
            tracing::warn!(error = %e, skill, "skill usage: forget lock failed");
        }
    }
}

/// What "a skill was used" consists of, in one place.
///
/// A use is **two** writes — the `.usage.json` counter and the `.cooccur.json`
/// ring the dream pipeline mines into workflow proposals — and they have to
/// happen together or the second signal silently describes only part of the
/// activity.
///
/// This exists because the same event arrives on two faces and only one of
/// them used to record it: `skill_read` (the tool the model calls) did both
/// writes inline, while a user typing `/<skill>` expands the skill straight
/// into the prompt without ever touching the reader — so a skill used only by
/// slash command aged into `stale` while being used daily. Both faces now call
/// this function, so "what counts as a use" cannot drift between them.
pub fn record_use_in_dir(skills_dir: impl AsRef<Path>, skill_id: &str) {
    let dir = skills_dir.as_ref();
    UsageStore::new(dir).record_use(skill_id);
    super::cooccurrence::CoOccurrenceLog::new(dir).record(skill_id);
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn bump_and_reload_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = UsageStore::new(tmp.path());
        store.record_view("git");
        store.record_view("git");
        store.record_use("git");
        store.record_patch("git");
        let reloaded = UsageStore::new(tmp.path());
        let stats = reloaded.get("git").unwrap();
        assert_eq!(stats.view_count, 2);
        assert_eq!(stats.use_count, 1);
        assert_eq!(stats.patch_count, 1);
        assert!(stats.created_at.is_some());
        assert!(stats.last_used_at.is_some());
        assert!(stats.last_viewed_at.is_some());
        assert!(stats.last_patched_at.is_some());
        assert_eq!(stats.state, SkillState::Active);
        assert!(!stats.pinned);
    }

    #[test]
    fn unknown_skill_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let store = UsageStore::new(tmp.path());
        assert!(store.get("never").is_none());
    }

    #[test]
    fn empty_skill_name_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let store = UsageStore::new(tmp.path());
        store.record_use("");
        store.record_patch("");
        store.forget("");
        // No file should be created; no panic on empty key.
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn state_and_pin_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = UsageStore::new(tmp.path());
        store.record_use("alpha");
        store.set_state("alpha", SkillState::Stale);
        store.set_pinned("alpha", true);
        let stats = store.get("alpha").unwrap();
        assert_eq!(stats.state, SkillState::Stale);
        assert!(stats.pinned);
        assert!(
            stats.archived_at.is_none(),
            "stale must not stamp archived_at"
        );

        store.set_state("alpha", SkillState::Archived);
        let stats = store.get("alpha").unwrap();
        assert_eq!(stats.state, SkillState::Archived);
        assert!(stats.archived_at.is_some());

        store.set_state("alpha", SkillState::Active);
        let stats = store.get("alpha").unwrap();
        assert_eq!(stats.state, SkillState::Active);
        assert!(
            stats.archived_at.is_none(),
            "promoting back to active must clear archived_at"
        );
    }

    #[test]
    fn forget_drops_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let store = UsageStore::new(tmp.path());
        store.record_use("ghost");
        assert!(store.get("ghost").is_some());
        store.forget("ghost");
        assert!(store.get("ghost").is_none());
    }

    #[test]
    fn old_sidecar_format_loads_with_defaults() {
        // Simulate a sidecar written by the pre-hardening version: only
        // the four original fields, no state/pinned/created_at etc.
        let tmp = tempfile::tempdir().unwrap();
        let sidecar = tmp.path().join(".usage.json");
        std::fs::write(
            &sidecar,
            r#"{"legacy":{"use_count":3,"view_count":7,"last_used_at":"2025-01-01T00:00:00Z","last_viewed_at":"2025-01-02T00:00:00Z"}}"#,
        )
        .unwrap();
        let store = UsageStore::new(tmp.path());
        let stats = store.get("legacy").unwrap();
        assert_eq!(stats.use_count, 3);
        assert_eq!(stats.view_count, 7);
        assert_eq!(stats.patch_count, 0);
        assert_eq!(stats.state, SkillState::Active);
        assert!(!stats.pinned);
        assert!(
            stats.created_at.is_none(),
            "legacy rows should not synthesize created_at on read"
        );
    }

    #[test]
    fn latest_activity_picks_newest() {
        let s = UsageStats {
            last_used_at: Some("2026-01-01T00:00:00Z".into()),
            last_viewed_at: Some("2026-02-01T00:00:00Z".into()),
            last_patched_at: Some("2026-01-15T00:00:00Z".into()),
            ..Default::default()
        };
        assert_eq!(s.latest_activity_at(), Some("2026-02-01T00:00:00Z"));
    }

    #[test]
    fn activity_count_sums_all_three() {
        let s = UsageStats {
            use_count: 3,
            view_count: 5,
            patch_count: 2,
            ..Default::default()
        };
        assert_eq!(s.activity_count(), 10);
    }

    /// Cross-thread concurrent bumps must not lose counts. The file lock
    /// serializes read-modify-write across both threads (and processes).
    #[test]
    fn concurrent_bumps_do_not_lose_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let store = UsageStore::new(&path);
                barrier.wait();
                for _ in 0..25 {
                    store.record_use("hot");
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let store = UsageStore::new(&path);
        let stats = store.get("hot").unwrap();
        assert_eq!(stats.use_count, 100, "all 100 increments must be counted");
    }
}
