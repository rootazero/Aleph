//! `SkillLifecycle` stage — mechanical aging of skill state.
//!
//! Ports the deterministic half of hermes-agent's `curator.py` lifecycle
//! rules (`DEFAULT_STALE_AFTER_DAYS = 30`) into a dream pipeline stage.
//!
//! Per R7 (LLM Sovereignty), this stage handles only the **mechanical**
//! aging decision: when a skill's most recent `use / view / patch`
//! timestamp is older than `skill_stale_after_days`, the lifecycle marker
//! flips from `Active` to `Stale`. That is signal aggregation, not
//! reasoning.
//!
//! The harder `Stale → Archived` and consolidation decisions are
//! deliberately deferred to a future LLM-driven stage. The stale set this
//! stage observes is logged, not stashed: it used to be written into a
//! `#[serde(skip)]` `DreamReport.extra` bag "for the curator stage when it
//! lands", which no code has ever read — a channel kept open for a consumer
//! that does not exist. The list a curator would need is re-derivable from
//! `UsageStore` at the moment it runs, which is the only moment it is true.
//!
//! ## Invariants
//!
//! - `pinned == true` skills are **never** transitioned (matches hermes
//!   `Pinned skills bypass all auto-transitions`).
//! - Only `Active → Stale` flips here. Never `Stale → Active` (re-use
//!   already revives via `UsageStore::record_use`) and never any flip
//!   to `Archived`.
//! - Never deletes; the sidecar `.usage.json` remains the source of
//!   truth and `set_state(Active)` always restores.
//! - Best-effort: a missing or unparseable `last_*_at` timestamp simply
//!   skips that skill (the next dream cycle will retry).

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::skill::{default_skill_dirs, SkillState, UsageStore};

use super::DreamStage;

pub struct SkillLifecycleStage {
    pub stale_after_days: u32,
}

impl Default for SkillLifecycleStage {
    fn default() -> Self {
        Self {
            stale_after_days:
                crate::config::types::memory::defaults::default_skill_stale_after_days(),
        }
    }
}

#[async_trait]
impl DreamStage for SkillLifecycleStage {
    fn name(&self) -> &'static str {
        "skill_lifecycle"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let dirs = default_skill_dirs();
        if dirs.is_empty() {
            tracing::debug!("SkillLifecycle: no skill dirs registered; nothing to age");
            return Ok(ctx);
        }

        let now = Utc::now();
        let mut transitioned = 0u32;
        let mut pinned_skipped = 0u32;
        let mut already_stale = 0u32;
        let mut archive_candidates: Vec<String> = Vec::new();

        for dir in &dirs {
            let store = UsageStore::new(dir);
            for (id, stats) in store.snapshot() {
                if stats.pinned {
                    pinned_skipped += 1;
                    continue;
                }
                // Prefer the newest activity timestamp; fall back to
                // `created_at` so a skill that was installed but never
                // touched still ages from its install time.
                let anchor = stats.latest_activity_at().or(stats.created_at.as_deref());
                let anchor_ts = match anchor.and_then(parse_iso_utc) {
                    Some(t) => t,
                    None => continue,
                };
                let age_days = age_in_days(now, anchor_ts);

                match classify(stats.state, age_days, self.stale_after_days) {
                    LifecycleVerdict::PromoteToStale => {
                        store.set_state(&id, SkillState::Stale);
                        transitioned += 1;
                        tracing::info!(
                            skill = %id,
                            age_days,
                            "SkillLifecycle: Active → Stale"
                        );
                    }
                    LifecycleVerdict::AlreadyStaleCandidate => {
                        already_stale += 1;
                        archive_candidates.push(id);
                    }
                    LifecycleVerdict::Keep => {}
                }
            }
        }

        tracing::info!(
            transitioned,
            pinned_skipped,
            already_stale,
            archive_candidates = %archive_candidates.join(","),
            "SkillLifecycle completed"
        );
        Ok(ctx)
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (testable without filesystem)
// ---------------------------------------------------------------------------

/// Outcome of evaluating a single skill against the aging policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleVerdict {
    /// Skill is `Active` and has crossed the staleness threshold.
    PromoteToStale,
    /// Skill is already `Stale`; record as an archive candidate for a
    /// future LLM-driven curator stage.
    AlreadyStaleCandidate,
    /// No change needed (active and fresh, or already archived).
    Keep,
}

pub(crate) const fn classify(
    state: SkillState,
    age_days: i64,
    stale_after_days: u32,
) -> LifecycleVerdict {
    let threshold = stale_after_days as i64;
    match state {
        SkillState::Active if age_days >= threshold => LifecycleVerdict::PromoteToStale,
        SkillState::Stale => LifecycleVerdict::AlreadyStaleCandidate,
        _ => LifecycleVerdict::Keep,
    }
}

fn parse_iso_utc(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn age_in_days(now: DateTime<Utc>, anchor: DateTime<Utc>) -> i64 {
    (now - anchor).num_days().max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_name_is_skill_lifecycle() {
        assert_eq!(SkillLifecycleStage::default().name(), "skill_lifecycle");
    }

    #[test]
    fn default_threshold_matches_hermes() {
        // hermes-agent curator.DEFAULT_STALE_AFTER_DAYS = 30
        assert_eq!(SkillLifecycleStage::default().stale_after_days, 30);
    }

    #[test]
    fn active_under_threshold_keeps() {
        assert_eq!(classify(SkillState::Active, 29, 30), LifecycleVerdict::Keep);
    }

    #[test]
    fn active_at_threshold_promotes() {
        assert_eq!(
            classify(SkillState::Active, 30, 30),
            LifecycleVerdict::PromoteToStale
        );
    }

    #[test]
    fn active_well_past_threshold_promotes() {
        assert_eq!(
            classify(SkillState::Active, 365, 30),
            LifecycleVerdict::PromoteToStale
        );
    }

    #[test]
    fn stale_always_surfaces_as_candidate() {
        assert_eq!(
            classify(SkillState::Stale, 0, 30),
            LifecycleVerdict::AlreadyStaleCandidate
        );
        assert_eq!(
            classify(SkillState::Stale, 9999, 30),
            LifecycleVerdict::AlreadyStaleCandidate
        );
    }

    #[test]
    fn archived_never_changes_state() {
        // Archived is a terminal state for this stage; the LLM curator
        // owns un-archival.
        assert_eq!(
            classify(SkillState::Archived, 9999, 30),
            LifecycleVerdict::Keep
        );
    }

    #[test]
    fn age_in_days_never_negative() {
        // Future-stamped activity (clock skew, manual edits) must not
        // produce negative ages that would underflow comparison.
        let now = Utc::now();
        let future = now + chrono::Duration::days(7);
        assert_eq!(age_in_days(now, future), 0);
    }

    #[test]
    fn parse_iso_utc_round_trips_rfc3339() {
        let t = parse_iso_utc("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(t.timestamp(), 1_767_225_600);
    }

    #[test]
    fn parse_iso_utc_rejects_garbage() {
        assert!(parse_iso_utc("not a timestamp").is_none());
    }

    /// End-to-end fs check: a skill with `created_at` 60d ago and
    /// `pinned = false` flips to `Stale`; a pinned twin does not.
    #[tokio::test]
    async fn lifecycle_stage_promotes_old_unpinned_skill_via_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let store = UsageStore::new(tmp.path());
        // First touch creates the sidecar row with `created_at = now`.
        store.record_use("old_unpinned");
        store.record_use("old_pinned");
        store.set_pinned("old_pinned", true);

        // Re-write the sidecar with an ancient `created_at` AND clear
        // the activity timestamps so `latest_activity_at()` falls back
        // to `created_at` (otherwise the freshly recorded `last_used_at`
        // makes the skills look brand new).
        let path = tmp.path().join(".usage.json");
        let bytes = tokio::fs::read(&path).await.unwrap();
        let mut map: std::collections::HashMap<String, crate::skill::UsageStats> =
            serde_json::from_slice(&bytes).unwrap();
        let ancient = (Utc::now() - chrono::Duration::days(60)).to_rfc3339();
        for stats in map.values_mut() {
            stats.created_at = Some(ancient.clone());
            stats.last_used_at = None;
            stats.last_viewed_at = None;
            stats.last_patched_at = None;
        }
        tokio::fs::write(&path, serde_json::to_vec_pretty(&map).unwrap())
            .await
            .unwrap();

        // Re-implement the per-dir loop from `execute()` without needing
        // a full DreamContext — the fs side-effect is what we assert.
        let stage = SkillLifecycleStage {
            stale_after_days: 30,
        };
        let store = UsageStore::new(tmp.path());
        let now = Utc::now();
        let mut promoted = 0u32;
        for (id, stats) in store.snapshot() {
            if stats.pinned {
                continue;
            }
            let anchor = stats
                .latest_activity_at()
                .or(stats.created_at.as_deref())
                .and_then(parse_iso_utc);
            let Some(anchor_ts) = anchor else { continue };
            if let LifecycleVerdict::PromoteToStale = classify(
                stats.state,
                age_in_days(now, anchor_ts),
                stage.stale_after_days,
            ) {
                store.set_state(&id, SkillState::Stale);
                promoted += 1;
            }
        }

        assert_eq!(promoted, 1, "only the unpinned old skill should flip");
        let unpinned = store.get("old_unpinned").unwrap();
        assert_eq!(unpinned.state, SkillState::Stale);
        let pinned = store.get("old_pinned").unwrap();
        assert_eq!(
            pinned.state,
            SkillState::Active,
            "pinned skill must not transition"
        );
    }
}
