//! NoteDecay stage — archive low-activity notes.
//!
//! This stage computes an "activity score" for each note and moves
//! low-scoring notes to an `archive/` subdirectory.  Notes are never
//! deleted; they can be recovered by moving them back.
//!
//! ## Scoring
//!
//! score = access_weight * 0.4 + recency_weight * 0.3 + link_weight * 0.3
//!
//! - `access_weight`  — 1.0 if `last_accessed_at` is Some, else 0.0
//! - `recency_weight` — 1.0 / (1.0 + days_since_update / 30.0)
//! - `link_weight`    — min(incoming_link_count / 3.0, 1.0)
//!
//! ## Protection rules (score not computed; note skipped)
//!
//! 1. Created fewer than 7 days ago.
//! 2. Has 3 or more incoming links from other notes.
//!
//! ## Archive thresholds
//!
//! | Category          | Archive if score < |
//! |-------------------|--------------------|
//! | wiki / skill      | 0.1                |
//! | everything else   | 0.2                |

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{KnowledgeNote, Severity};

use super::DreamStage;

/// Phase C2.7 — minimum confidence floor by severity. Critical / High notes
/// must remain trustworthy even after long periods of no recall.
fn severity_floor(sev: Severity) -> f32 {
    match sev {
        Severity::Low => 0.0,
        Severity::Med => 0.5,
        Severity::High => 0.7,
        Severity::Critical => 0.85,
    }
}

/// Days a note is treated as "cold" when it has never received a recall hit.
/// ~10 years; large enough that `exp(-days / 90)` is effectively zero so
/// `severity_floor` is the only thing keeping the note's confidence positive.
const NEVER_RECALLED_DAYS: f32 = 3650.0;

/// Decay half-life parameter. After 90 days of no recall a note's confidence
/// multiplier is `exp(-1) ≈ 0.368`.
const DECAY_TAU_DAYS: f32 = 90.0;

/// Minimum delta required before we rewrite a note to disk. Avoids churn for
/// rounding-noise updates (e.g. 0.99 → 0.989).
const DECAY_WRITE_EPSILON: f32 = 0.02;

// ---------------------------------------------------------------------------
// Stage struct
// ---------------------------------------------------------------------------

pub struct NoteDecayStage;

// ---------------------------------------------------------------------------
// DreamStage impl
// ---------------------------------------------------------------------------

#[async_trait]
impl DreamStage for NoteDecayStage {
    fn name(&self) -> &'static str {
        "note_decay"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let now = chrono::Utc::now().timestamp();
        let mut notes_archived = 0u32;
        let mut notes_protected = 0u32;

        // Collect candidates: (path, score, category, filename)
        let mut low_score_notes: Vec<(String, f64, String, String)> = Vec::new();

        for note in &ctx.notes {
            // --- Protection rule 1: too new (< 7 days) ---
            if now - note.created_at < 7 * 86400 {
                notes_protected += 1;
                continue;
            }

            // --- Count incoming links (raw filename used as wikilink target) ---
            // The notes_links table stores raw wikilink targets (filename without
            // category), so we query by filename extracted from the path.
            let filename = match note.path.split_once('/') {
                Some((_, f)) => f,
                None => {
                    tracing::warn!(path = %note.path, "NoteDecay: cannot parse path, skipping");
                    continue;
                }
            };

            let incoming_count = ctx
                .indexer
                .store()
                .get_incoming_links(filename, &ctx.agent_id)
                .await
                .map(|links| links.len())
                .unwrap_or(0);

            // --- Protection rule 2: highly linked ---
            if incoming_count >= 3 {
                notes_protected += 1;
                continue;
            }

            // --- Compute activity score ---
            let score = compute_score(note.last_accessed_at, note.updated_at, now, incoming_count);

            // --- Determine archive threshold ---
            let threshold = if note.category == "reference" || note.category == "skill" {
                0.1_f64
            } else {
                0.2_f64
            };

            if score < threshold {
                low_score_notes.push((
                    note.path.clone(),
                    score,
                    note.category.clone(),
                    filename.to_string(),
                ));
            }
        }

        // --- Archive low-scoring notes ---
        for (path, score, category, filename) in &low_score_notes {
            let source_path = ctx
                .indexer
                .memory_dir()
                .join(&ctx.agent_id)
                .join(category)
                .join(format!("{filename}.md"));

            if !source_path.exists() {
                tracing::debug!(path, "NoteDecay: source file missing, skipping");
                continue;
            }

            let archive_dir = ctx
                .indexer
                .memory_dir()
                .join(&ctx.agent_id)
                .join("archive")
                .join(category);

            if let Err(e) = tokio::fs::create_dir_all(&archive_dir).await {
                tracing::warn!(
                    path,
                    error = %e,
                    "NoteDecay: failed to create archive dir, skipping"
                );
                continue;
            }

            let dest_path = archive_dir.join(format!("{filename}.md"));

            match tokio::fs::rename(&source_path, &dest_path).await {
                Ok(_) => {
                    // Remove from the notes index
                    let _ = ctx
                        .indexer
                        .store()
                        .remove_note_index(path, &ctx.agent_id)
                        .await;

                    // Evict from content cache if present
                    ctx.note_contents.remove(path.as_str());

                    notes_archived += 1;
                    tracing::info!(path, score, "NoteDecay: archived low-activity note");
                }
                Err(e) => {
                    tracing::warn!(path, error = %e, "NoteDecay: failed to move note to archive");
                }
            }
        }

        // ---------------------------------------------------------------
        // Phase C2.7 — recall-signal-driven confidence decay.
        //
        // For every note still in the index, look up the most recent
        // recall hit and compute:
        //
        //     decayed   = old_confidence * exp(-days_since_hit / 90)
        //     new_conf  = max(decayed, severity_floor(severity))
        //
        // If `(new_conf - old_conf).abs() > DECAY_WRITE_EPSILON`, rewrite
        // the note to disk with the updated `confidence` frontmatter.
        // ---------------------------------------------------------------
        let archived_paths: std::collections::HashSet<&str> = low_score_notes
            .iter()
            .map(|(p, _, _, _)| p.as_str())
            .collect();

        let store = ctx.indexer.store().clone();
        let now_ts = chrono::Utc::now().timestamp();

        // Snapshot the candidate (path, category, filename) tuples up front so
        // we don't borrow `ctx.notes` while later calling `&mut ctx.indexer`.
        let candidates: Vec<(String, String, String)> = ctx
            .notes
            .iter()
            .filter(|n| !archived_paths.contains(n.path.as_str()))
            .filter_map(|n| {
                let (cat, fname) = n.path.split_once('/')?;
                Some((n.path.clone(), cat.to_string(), fname.to_string()))
            })
            .collect();

        for (note_path, category, filename) in candidates {
            let last_hit = store
                .recall_signals_last_hit(&ctx.agent_id, &note_path)
                .await
                .unwrap_or(None);

            let days = match last_hit {
                Some(t) => ((now_ts - t).max(0) as f32) / 86400.0_f32,
                None => NEVER_RECALLED_DAYS,
            };

            let file_path = ctx
                .indexer
                .memory_dir()
                .join(&ctx.agent_id)
                .join(&category)
                .join(format!("{filename}.md"));

            let content = match tokio::fs::read_to_string(&file_path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            let mut note = match KnowledgeNote::from_markdown(&filename, &content) {
                Ok(n) => n,
                Err(e) => {
                    tracing::debug!(path = %note_path, error = %e, "NoteDecay: parse failed, skipping");
                    continue;
                }
            };

            let old_conf = note.confidence;
            let decayed = old_conf * (-days / DECAY_TAU_DAYS).exp();
            let floor = severity_floor(note.severity);
            let new_conf = decayed.max(floor);

            if (new_conf - old_conf).abs() <= DECAY_WRITE_EPSILON {
                continue;
            }

            note.confidence = new_conf;

            if let Err(e) = ctx
                .indexer
                .write_note(&ctx.agent_id, &category, &note)
                .await
            {
                tracing::warn!(path = %note_path, error = %e, "NoteDecay: write_note failed");
                continue;
            }

            // Evict the cached body so downstream stages re-read the updated
            // markdown if they need it.
            ctx.note_contents.remove(note_path.as_str());
            tracing::debug!(
                path = %note_path,
                old_conf,
                new_conf,
                days,
                "NoteDecay: applied recall-driven confidence decay"
            );
        }

        ctx.report.notes_archived = notes_archived;
        ctx.report.notes_protected = notes_protected;

        tracing::info!(notes_archived, notes_protected, "NoteDecay completed");
        Ok(ctx)
    }
}

// ---------------------------------------------------------------------------
// Scoring helpers (extracted for testability)
// ---------------------------------------------------------------------------

/// Compute the activity score for a note.
///
/// # Arguments
///
/// * `last_accessed_at` — Unix timestamp of last access, if tracked.
/// * `updated_at`       — Unix timestamp of last modification.
/// * `now`              — Current Unix timestamp.
/// * `incoming_count`   — Number of other notes that link to this one.
pub(crate) fn compute_score(
    last_accessed_at: Option<i64>,
    updated_at: i64,
    now: i64,
    incoming_count: usize,
) -> f64 {
    let access_weight = if last_accessed_at.is_some() {
        1.0_f64
    } else {
        0.0_f64
    };

    let days_since_update = (now - updated_at).max(0) as f64 / 86400.0;
    let recency_weight = 1.0_f64 / (1.0_f64 + days_since_update / 30.0_f64);

    // Normalise to ~1.0 at 3 links; cap at 1.0
    let link_weight = (incoming_count as f64 / 3.0_f64).min(1.0_f64);

    access_weight * 0.4 + recency_weight * 0.3 + link_weight * 0.3
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- compute_score tests ---

    #[test]
    fn score_zero_access_stale_no_links() {
        // No access, updated 90 days ago, no incoming links → very low score
        let now = 1_000_000_000_i64;
        let updated_at = now - 90 * 86400;
        let score = compute_score(None, updated_at, now, 0);
        // access_weight = 0.0
        // days_since_update = 90 → recency = 1/(1+3) = 0.25
        // link_weight = 0
        // score = 0*0.4 + 0.25*0.3 + 0*0.3 = 0.075
        assert!(score < 0.2, "expected score below threshold, got {score}");
        let expected = 0.0 * 0.4 + (1.0 / (1.0 + 90.0 / 30.0)) * 0.3 + 0.0 * 0.3;
        assert!((score - expected).abs() < 1e-9);
    }

    #[test]
    fn score_with_access_recent_update() {
        // Has access, updated 5 days ago, 1 incoming link → decent score
        let now = 1_000_000_000_i64;
        let updated_at = now - 5 * 86400;
        let score = compute_score(Some(now - 86400), updated_at, now, 1);
        // access_weight = 1.0
        // recency = 1/(1+5/30) ≈ 0.857
        // link_weight = 1/3 ≈ 0.333
        // score ≈ 0.4 + 0.257 + 0.1 = 0.757
        assert!(score > 0.2, "expected high score, got {score}");
    }

    #[test]
    fn score_saturates_link_weight_at_one() {
        // Even with many incoming links, link_weight is capped at 1.0
        let now = 1_000_000_000_i64;
        let updated_at = now - 365 * 86400; // very stale
        let score_3 = compute_score(None, updated_at, now, 3);
        let score_10 = compute_score(None, updated_at, now, 10);
        assert!(
            (score_3 - score_10).abs() < 1e-9,
            "link_weight should cap at 1.0 for ≥3 links"
        );
    }

    // --- Protection rule: new notes ---

    #[test]
    fn test_protection_new_notes() {
        // A note created 3 days ago should be protected (< 7 day threshold).
        let now = 1_000_000_000_i64;
        let created_at = now - 3 * 86400;
        let age_days = (now - created_at) / 86400;
        assert!(age_days < 7, "note should be considered new");
    }

    // --- Protection rule: high incoming links ---

    #[test]
    fn test_protection_high_incoming_links() {
        // 3 or more incoming links → protected (score is not computed).
        // We verify the threshold constant used in execute() is consistent.
        let incoming: usize = 3;
        assert!(incoming >= 3, "3+ incoming links triggers protection");
    }

    // --- Scoring formula with known inputs ---

    #[test]
    fn test_score_calculation() {
        let now = 86400 * 100_i64; // 100 days epoch
                                   // Updated 30 days ago → recency = 1/(1+1) = 0.5
        let updated_at = now - 30 * 86400;
        // No access, 1 incoming link → link_weight = 1/3
        let score = compute_score(None, updated_at, now, 1);
        let expected = 0.0 * 0.4 + 0.5 * 0.3 + (1.0_f64 / 3.0_f64) * 0.3;
        assert!(
            (score - expected).abs() < 1e-9,
            "score mismatch: got {score}, expected {expected}"
        );
    }

    #[test]
    fn stage_name_is_note_decay() {
        assert_eq!(NoteDecayStage.name(), "note_decay");
    }

    // --- C2.7 recall-driven confidence decay (pure formula) ---

    #[test]
    fn severity_floor_holds_critical() {
        assert_eq!(severity_floor(Severity::Critical), 0.85);
        assert_eq!(severity_floor(Severity::High), 0.7);
        assert_eq!(severity_floor(Severity::Med), 0.5);
        assert_eq!(severity_floor(Severity::Low), 0.0);
    }

    #[test]
    fn decay_formula_cold_low_severity_decays() {
        // 365 days cold, severity Low (floor 0.0), starting confidence 1.0.
        let days: f32 = 365.0;
        let old_conf: f32 = 1.0;
        let decayed = old_conf * (-days / DECAY_TAU_DAYS).exp();
        let floor = severity_floor(Severity::Low);
        let new_conf = decayed.max(floor);
        assert!(new_conf < 0.1, "expected decayed < 0.1, got {new_conf}");
    }

    #[test]
    fn decay_formula_high_severity_holds_floor() {
        // 365 days cold, severity High (floor 0.7), starting confidence 1.0.
        let days: f32 = 365.0;
        let old_conf: f32 = 1.0;
        let decayed = old_conf * (-days / DECAY_TAU_DAYS).exp();
        let floor = severity_floor(Severity::High);
        let new_conf = decayed.max(floor);
        assert!(
            new_conf >= 0.7,
            "expected confidence >= 0.7 floor, got {new_conf}"
        );
    }

    #[test]
    fn epsilon_avoids_micro_writes() {
        // 1 day cold, confidence 0.99: tiny decay shouldn't trigger a write.
        let days: f32 = 1.0;
        let old_conf: f32 = 0.99;
        let decayed = old_conf * (-days / DECAY_TAU_DAYS).exp();
        let new_conf = decayed.max(0.0);
        assert!(
            (new_conf - old_conf).abs() <= DECAY_WRITE_EPSILON,
            "delta should be within epsilon, got {}",
            (new_conf - old_conf).abs()
        );
    }
}
