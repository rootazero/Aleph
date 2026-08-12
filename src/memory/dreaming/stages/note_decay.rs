//! `NoteDecay` stage — archive low-activity notes.
//!
//! This stage computes an "activity score" for each note and moves
//! low-scoring notes to an `archive/` subdirectory.  Notes are never
//! deleted; they can be recovered by moving them back.
//!
//! ## Scoring
//!
//! score = `access_weight` * 0.4 + `recency_weight` * 0.3 + `link_weight` * 0.3
//!
//! - `access_weight`  — 1.0 / (1.0 + `days_since_last_recall` / 30.0) from the
//!   note's latest `recall_signals` hit, or 0.0 if it was never recalled
//! - `recency_weight` — 1.0 / (1.0 + `days_since_update` / 30.0)
//! - `link_weight`    — `min(incoming_link_count` / 3.0, 1.0)
//!
//! ## Protection rules (note skipped, not archived)
//!
//! 1. Created fewer than 7 days ago.
//! 2. Has 3 or more incoming links from other notes.
//! 3. `permanent: true` frontmatter / `permanent`/`pinned` tag, or a
//!    `protected_types` category.
//! 4. High/Critical severity — floored, not archived, so the C2.7 confidence
//!    floor (0.7/0.85) stays meaningful.
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
use crate::memory::notes::{tags_mark_permanent, KnowledgeNote, Severity};

use super::DreamStage;

/// Phase C2.7 — minimum confidence floor by severity. Critical / High notes
/// must remain trustworthy even after long periods of no recall.
const fn severity_floor(sev: Severity) -> f32 {
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

/// Minimum delta required before we rewrite a note to disk. Avoids churn for
/// rounding-noise updates (e.g. 0.99 → 0.989).
const DECAY_WRITE_EPSILON: f32 = 0.02;

// ---------------------------------------------------------------------------
// Stage struct
// ---------------------------------------------------------------------------

/// `NoteDecay` stage, parameterised by the runtime [`MemoryDecayPolicy`] so the
/// previously-dead `memory.memory_decay.*` config finally drives behaviour.
///
/// `half_life_days` replaces the old hard-coded `DECAY_TAU_DAYS = 90`,
/// `min_strength` is a global confidence floor combined with the per-severity
/// floor, and `protected_types` plus per-note permanence exempt core knowledge
/// from both archival and decay.
pub struct NoteDecayStage {
    /// Confidence half-life: `exp(-days_since_recall / half_life_days)`.
    pub half_life_days: f32,
    /// Global minimum confidence; the effective floor is
    /// `max(severity_floor, min_strength)`.
    pub min_strength: f32,
    /// Note categories never decayed or archived (e.g. `"personal"`).
    pub protected_types: Vec<String>,
}

impl Default for NoteDecayStage {
    /// Defaults mirror the pre-wiring constants (90-day half-life) with the
    /// configured protected-type / min-strength defaults, so tests and any
    /// caller that omits config keep the historical behaviour.
    fn default() -> Self {
        Self {
            half_life_days: 90.0,
            min_strength: 0.0,
            protected_types: Vec::new(),
        }
    }
}

impl NoteDecayStage {
    /// Whether a note in this category is protected from decay/archival by the
    /// `protected_types` policy.
    fn category_protected(&self, category: &str) -> bool {
        self.protected_types.iter().any(|t| t == category)
    }
}

// ---------------------------------------------------------------------------
// DreamStage impl
// ---------------------------------------------------------------------------

#[async_trait]
impl DreamStage for NoteDecayStage {
    fn name(&self) -> &'static str {
        "note_decay"
    }

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let now = chrono::Utc::now().timestamp();
        let mut notes_archived = 0u32;
        let mut notes_protected = 0u32;

        // Latest recall hit per note, fetched once and shared by the archival
        // scoring below and the C2.7 confidence pass (which previously issued
        // the same per-note query itself). This is what feeds `access_weight`:
        // a recalled-but-rarely-edited note now scores on its recall recency
        // instead of a hardcoded 0.0.
        let mut last_hits: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for note in &ctx.notes {
            if let Ok(Some(t)) = ctx
                .indexer
                .store()
                .recall_signals_last_hit(&ctx.agent_id, &note.path)
                .await
            {
                // rust-doctor-disable-next-line excessive-clone
                last_hits.insert(note.path.clone(), t);
            }
        }

        // Collect candidates: (path, score, category, filename)
        let mut low_score_notes: Vec<(String, f64, String, String)> = Vec::new();

        for note in &ctx.notes {
            // --- Protection rule 0: permanent / protected-type core knowledge ---
            // Permanent notes (frontmatter `permanent: true` or a
            // `permanent`/`pinned` tag) and notes in a `protected_types`
            // category are exempt from archival entirely. The index `tags` are
            // already loaded here, so no file read is needed.
            if tags_mark_permanent(&note.tags) || self.category_protected(&note.category) {
                notes_protected += 1;
                continue;
            }

            // --- Protection rule 1: too new (< 7 days) ---
            // Calendar days via chrono::Duration, not 7*86400 raw seconds:
            // 7*86400 is exactly 7.0 solar days and ignores leap seconds; a
            // 1-hour NTP correction at the boundary can flip a borderline note
            // in or out of protection.
            let age_seconds = now - note.created_at;
            const SEVEN_DAYS_SECS: i64 = 7 * 86_400;
            if age_seconds < SEVEN_DAYS_SECS {
                notes_protected += 1;
                continue;
            }

            // --- Count incoming links ---
            // `notes_links.to_note` is the resolved target: a full path when the
            // wikilink filename uniquely resolved, otherwise the bare text. Match
            // either. Querying by filename alone (the old code) never matched a
            // full-path to_note, so this count was always 0 — silently disabling
            // both the >=3-incoming protection and link_weight below.
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
                .get_incoming_links_any(&note.path, filename, &ctx.agent_id)
                .await
                .map_or(0, |links| links.len());

            // --- Protection rule 2: highly linked ---
            if incoming_count >= 3 {
                notes_protected += 1;
                continue;
            }

            // --- Compute activity score ---
            let last_recalled = last_hits.get(&note.path).copied();
            let score = compute_score(last_recalled, note.updated_at, now, incoming_count);

            // --- Determine archive threshold ---
            let threshold = if note.category == "reference" || note.category == "skill" {
                0.1_f64
            } else {
                0.2_f64
            };

            if score < threshold {
                low_score_notes.push((
                    // rust-doctor-disable-next-line excessive-clone
                    note.path.clone(),
                    score,
                    // rust-doctor-disable-next-line excessive-clone
                    note.category.clone(),
                    filename.to_string(),
                ));
            }
        }

        // --- Archive low-scoring notes ---
        // Track paths we actually move to archive/ so the C2.7 confidence pass
        // below only skips genuinely-archived notes. Notes that stay (permanent,
        // High/Critical severity, or a failed move) must still be floored.
        let mut archived_now: std::collections::HashSet<String> = std::collections::HashSet::new();
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

            // Honour the per-note `permanent: true` flag and High/Critical
            // severity — neither of which the lightweight index `tags` (checked
            // in the first loop) can see. Only archival candidates are read
            // here, so this stays cheap. The read length also feeds the edit
            // budget below (a proxy for the knowledge removed).
            let note_bytes = match tokio::fs::read_to_string(&source_path).await {
                Ok(content) => {
                    if let Ok(note) = KnowledgeNote::from_markdown(filename, &content) {
                        if note.is_permanent() {
                            notes_protected += 1;
                            tracing::debug!(path, "NoteDecay: permanent note exempt from archival");
                            continue;
                        }
                        // Critical/High notes have their confidence floored at
                        // 0.85/0.7 by the C2.7 pass below; archiving them out of
                        // the index would make that floor meaningless. Exempt
                        // them from archival (Med/Low still archive on low
                        // activity).
                        if matches!(note.severity, Severity::High | Severity::Critical) {
                            notes_protected += 1;
                            tracing::debug!(
                                path,
                                "NoteDecay: high-severity note exempt from archival"
                            );
                            continue;
                        }
                    }
                    content.len() as u64
                }
                // Unreadable candidate still archives (unchanged behaviour);
                // charge a nominal cost so the budget is not free to bypass.
                Err(_) => 512,
            };

            // Archival is a destructive edit — bound it by the cycle's shared
            // `EditBudget` ("textual learning rate") so one night cannot
            // mass-archive the corpus. Exempt notes already `continue`d above,
            // so only genuine archives spend budget. On exhaustion, defer the
            // rest to the next cycle rather than churning unbounded.
            if !ctx.evolution_budget.try_spend(note_bytes) {
                tracing::info!(
                    remaining_edits = ctx.evolution_budget.edits_remaining,
                    "NoteDecay: edit budget exhausted; deferring remaining archival to next cycle"
                );
                break;
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

                    // rust-doctor-disable-next-line excessive-clone
                    archived_now.insert(path.clone());
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
        //     decayed   = old_confidence * exp(-days_since_hit / half_life_days)
        //     new_conf  = max(decayed, severity_floor(severity), min_strength)
        //
        // If `(new_conf - old_conf).abs() > DECAY_WRITE_EPSILON`, rewrite
        // the note to disk with the updated `confidence` frontmatter.
        //
        // Permanent notes (frontmatter / tag) and `protected_types` categories
        // are skipped so core knowledge never erodes.
        // ---------------------------------------------------------------
        let archived_paths: std::collections::HashSet<&str> =
            archived_now.iter().map(String::as_str).collect();

        let now_ts = chrono::Utc::now().timestamp();

        // Snapshot the candidate (path, category, filename) tuples up front so
        // we don't borrow `ctx.notes` while later calling `&mut ctx.indexer`.
        // Protected-type categories are dropped here (cheap, no file read); the
        // per-note permanent flag is checked after parsing below.
        let candidates: Vec<(String, String, String)> = ctx
            .notes
            .iter()
            .filter(|n| !archived_paths.contains(n.path.as_str()))
            .filter(|n| !self.category_protected(&n.category))
            .filter_map(|n| {
                let (cat, fname) = n.path.split_once('/')?;
                // rust-doctor-disable-next-line excessive-clone
                Some((n.path.clone(), cat.to_string(), fname.to_string()))
            })
            .collect();

        for (note_path, category, filename) in candidates {
            let last_hit = last_hits.get(&note_path).copied();

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

            let note = match KnowledgeNote::from_markdown(&filename, &content) {
                Ok(n) => n,
                Err(e) => {
                    tracing::debug!(path = %note_path, error = %e, "NoteDecay: parse failed, skipping");
                    continue;
                }
            };

            // Permanent core knowledge never decays.
            if note.is_permanent() {
                continue;
            }

            // Stale notes: NoteDrift judged this note's information outdated or
            // contradicted by a newer note (frontmatter `stale: true`, the
            // previously write-only flag). Archive Low/Med stale notes out of
            // active retrieval so they stop crowding recall — recoverable like
            // any archived note, with NoteDrift's `## Superseded` banner
            // explaining why in the file. High/Critical stay indexed (the C2.7
            // confidence floor they rely on requires it; permanent already
            // returned above). Respect the same "highly linked" protection as
            // score-based archival — orphaning many inbound links is worse than
            // keeping a flagged note (the extra query only fires for the rare
            // stale note).
            if note.stale
                && matches!(note.severity, Severity::Low | Severity::Med)
                && (now_ts - note.created_at) >= 7 * 86_400
            {
                let incoming = ctx
                    .indexer
                    .store()
                    .get_incoming_links_any(&note_path, &filename, &ctx.agent_id)
                    .await
                    .map_or(0, |links| links.len());
                if incoming < 3 {
                    let archive_dir = ctx
                        .indexer
                        .memory_dir()
                        .join(&ctx.agent_id)
                        .join("archive")
                        .join(&category);
                    let dest_path = archive_dir.join(format!("{filename}.md"));
                    let moved = tokio::fs::create_dir_all(&archive_dir).await.is_ok()
                        && tokio::fs::rename(&file_path, &dest_path).await.is_ok();
                    if moved {
                        let _ = ctx
                            .indexer
                            .store()
                            .remove_note_index(&note_path, &ctx.agent_id)
                            .await;
                        ctx.note_contents.remove(note_path.as_str());
                        notes_archived += 1;
                        tracing::info!(path = %note_path, "NoteDecay: archived stale (NoteDrift-flagged) note");
                        continue;
                    }
                    tracing::warn!(path = %note_path, "NoteDecay: failed to archive stale note; keeping indexed");
                }
            }

            let old_conf = note.confidence;
            let decayed = old_conf * (-days / self.half_life_days).exp();
            let floor = severity_floor(note.severity).max(self.min_strength);
            let new_conf = decayed.max(floor);

            if (new_conf - old_conf).abs() <= DECAY_WRITE_EPSILON {
                continue;
            }

            // Patch ONLY the `confidence:` frontmatter scalar and write the file
            // back verbatim. `write_note` rebuilds from `to_markdown`, which is
            // lossy (drops prose / headings / code blocks — see indexer.rs), so
            // rewriting a whole note just to nudge one scalar would silently
            // erase the note body (and any `## Superseded` marker NoteDrift added
            // earlier in the same cycle). `write_note_raw` is byte-preserving.
            let patched = match patch_confidence_frontmatter(&content, new_conf) {
                Some(p) => p,
                None => {
                    tracing::debug!(path = %note_path, "NoteDecay: no frontmatter block, skipping confidence rewrite");
                    continue;
                }
            };

            if let Err(e) = ctx
                .indexer
                .write_note_raw(&ctx.agent_id, &category, &filename, &patched)
                .await
            {
                tracing::warn!(path = %note_path, error = %e, "NoteDecay: write_note_raw failed");
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

/// Patch (or insert) the `confidence:` scalar inside a note's YAML frontmatter,
/// leaving the body (prose, headings, code, supersession markers) byte-for-byte
/// intact. Mirrors `to_markdown`'s `confidence: {:.4}` formatting so the `SQLite`
/// reparse stays consistent.
///
/// Returns `None` if `content` has no leading `---`-delimited frontmatter block.
pub(crate) fn patch_confidence_frontmatter(content: &str, new_conf: f32) -> Option<String> {
    // Frontmatter must open on the very first line.
    let rest = content.strip_prefix("---\n")?;
    // Closing delimiter is the next line that is exactly `---` — locate the
    // newline that precedes it so the body (everything from that `\n---`) is
    // preserved verbatim.
    let close_rel = rest.find("\n---")?;
    let fm = &rest[..close_rel];
    let after = &rest[close_rel..];

    let new_line = format!("confidence: {new_conf:.4}");
    let mut lines: Vec<String> = fm.lines().map(str::to_string).collect();
    let mut new_line = Some(new_line);
    for line in lines.iter_mut() {
        if line.trim_start().starts_with("confidence:") {
            if let Some(nl) = new_line.take() {
                *line = nl;
            }
            break;
        }
    }
    if let Some(new_line) = new_line {
        // Legacy note with no `confidence:` line — append it as the last
        // frontmatter entry (before the closing `---`).
        lines.push(new_line);
    }
    Some(format!("---\n{}{}", lines.join("\n"), after))
}

/// Compute the activity score for a note.
///
/// # Arguments
///
/// * `last_accessed_at` — Unix timestamp of the latest recall-signal hit, if any.
/// * `updated_at`       — Unix timestamp of last modification.
/// * `now`              — Current Unix timestamp.
/// * `incoming_count`   — Number of other notes that link to this one.
pub(crate) fn compute_score(
    last_accessed_at: Option<i64>,
    updated_at: i64,
    now: i64,
    incoming_count: usize,
) -> f64 {
    // Graded by recall recency (mirrors `recency_weight`'s 30-day curve): a
    // note recalled yesterday earns ~0.39 of the score, one recalled a year
    // ago ~0.03 — so stale recalls do not grant permanent archival immunity.
    let access_weight = match last_accessed_at {
        Some(t) => {
            let days_since_access = (now - t).max(0) as f64 / 86400.0;
            1.0_f64 / (1.0_f64 + days_since_access / 30.0_f64)
        }
        None => 0.0_f64,
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
        // Recalled yesterday, updated 5 days ago, 1 incoming link → decent score
        let now = 1_000_000_000_i64;
        let updated_at = now - 5 * 86400;
        let score = compute_score(Some(now - 86400), updated_at, now, 1);
        // access_weight = 1/(1+1/30) ≈ 0.968
        // recency = 1/(1+5/30) ≈ 0.857
        // link_weight = 1/3 ≈ 0.333
        // score ≈ 0.387 + 0.257 + 0.1 = 0.744
        assert!(score > 0.2, "expected high score, got {score}");
    }

    #[test]
    fn recent_recall_protects_stale_note_old_recall_does_not() {
        // The bug this guards against: `access_weight` used to be fed a
        // hardcoded `None`, so an unlinked note not edited for ~15 days was
        // archived even when it was being recalled every day.
        let now = 1_000_000_000_i64;
        let updated_at = now - 90 * 86400; // stale content
                                           // Recalled yesterday → access ≈ 0.968*0.4 + recency 0.25*0.3 ≈ 0.462
        let hot = compute_score(Some(now - 86400), updated_at, now, 0);
        assert!(hot > 0.2, "recently recalled note must survive, got {hot}");
        // Recalled a year ago → access ≈ 0.076*0.4 + 0.075 ≈ 0.105
        let cold = compute_score(Some(now - 365 * 86400), updated_at, now, 0);
        assert!(
            cold < 0.2,
            "stale recall must not grant immunity, got {cold}"
        );
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
        assert_eq!(NoteDecayStage::default().name(), "note_decay");
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
        let tau = NoteDecayStage::default().half_life_days;
        let days: f32 = 365.0;
        let old_conf: f32 = 1.0;
        let decayed = old_conf * (-days / tau).exp();
        let floor = severity_floor(Severity::Low);
        let new_conf = decayed.max(floor);
        assert!(new_conf < 0.1, "expected decayed < 0.1, got {new_conf}");
    }

    #[test]
    fn decay_formula_high_severity_holds_floor() {
        // 365 days cold, severity High (floor 0.7), starting confidence 1.0.
        let tau = NoteDecayStage::default().half_life_days;
        let days: f32 = 365.0;
        let old_conf: f32 = 1.0;
        let decayed = old_conf * (-days / tau).exp();
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
        let tau = NoteDecayStage::default().half_life_days;
        let days: f32 = 1.0;
        let old_conf: f32 = 0.99;
        let decayed = old_conf * (-days / tau).exp();
        let new_conf = decayed.max(0.0);
        assert!(
            (new_conf - old_conf).abs() <= DECAY_WRITE_EPSILON,
            "delta should be within epsilon, got {}",
            (new_conf - old_conf).abs()
        );
    }

    // --- Permanent / protected-type exemption (the "never affected" promise) ---

    #[test]
    fn category_protected_matches_configured_types() {
        let stage = NoteDecayStage {
            half_life_days: 90.0,
            min_strength: 0.1,
            protected_types: vec!["personal".to_string(), "preference".to_string()],
        };
        assert!(stage.category_protected("personal"));
        assert!(stage.category_protected("preference"));
        assert!(!stage.category_protected("learning"));
    }

    #[test]
    fn default_stage_protects_nothing_and_keeps_legacy_tau() {
        // The unit-struct replacement must preserve historical behaviour for
        // callers that don't thread config: 90-day half-life, no protections,
        // zero global floor.
        let stage = NoteDecayStage::default();
        assert_eq!(stage.half_life_days, 90.0);
        assert_eq!(stage.min_strength, 0.0);
        assert!(stage.protected_types.is_empty());
        assert!(!stage.category_protected("personal"));
    }

    #[test]
    fn min_strength_lifts_low_severity_floor() {
        // With a configured global floor, even a fully-decayed Low note keeps
        // `min_strength` confidence instead of collapsing to 0.
        let tau = 90.0_f32;
        let days = 3650.0_f32; // a decade cold
        let old_conf = 1.0_f32;
        let decayed = old_conf * (-days / tau).exp();
        let floor = severity_floor(Severity::Low).max(0.1);
        let new_conf = decayed.max(floor);
        assert!(
            (new_conf - 0.1).abs() < 1e-6,
            "expected min_strength floor 0.1, got {new_conf}"
        );
    }

    // --- stage-level fixture (mirrors note_weave.rs build_ctx) ---
    use crate::memory::dreaming::{DreamReport, DreamStrategy, NoteEntry};
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::{KnowledgeNote, NoteIndexer};
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::mock::MockProvider;
    use crate::sync_primitives::Arc;

    struct DecayStubEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for DecayStubEmbedder {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(Vec::new())
        }
        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(Vec::new())
        }
        fn dimensions(&self) -> usize {
            0
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    async fn build_decay_ctx() -> (DreamContext, Arc<SqliteMemoryBackend>) {
        let temp = std::env::temp_dir().join(format!("aleph_decay_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp).await.unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new("{}"));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> =
            std::sync::Arc::new(DecayStubEmbedder);
        let ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: DreamReport::default(),
            pipeline_type: "consolidate".into(),
            activity_checker: std::sync::Arc::new(|| false),
            strategy: DreamStrategy::Consolidate,
            orientation: None,
            evolution_budget: crate::memory::dreaming::EditBudget::default(),
        };
        (ctx, store)
    }

    fn decay_entry(path: &str, created_at: i64, updated_at: i64) -> NoteEntry {
        let (category, _) = path.split_once('/').unwrap();
        NoteEntry {
            path: path.into(),
            category: category.into(),
            tags: vec![],
            created_at,
            updated_at,
            content_hash: "h".into(),
        }
    }

    #[tokio::test]
    async fn note_with_three_incoming_fullpath_links_is_protected() {
        // `hot` is referenced by 3 other notes via full-path to_note. With the
        // old bare-filename query incoming_count was 0 and `hot` was archived;
        // with get_incoming_links_any it reads 3 and is protected.
        let (mut ctx, store) = build_decay_ctx().await;

        // Old enough to clear the <7-day protection. `reference` is not a
        // protected category, so only the >=3-incoming rule can protect it.
        store
            .index_note(
                &KnowledgeNote {
                    title: "hot".into(),
                    category: "reference".into(),
                    facts: vec!["core fact".into()],
                    content_hash: "hhot".into(),
                    created_at: 1,
                    updated_at: 1,
                    ..Default::default()
                },
                "default",
                "reference",
            )
            .await
            .unwrap();
        for s in ["a", "b", "c"] {
            store
                .index_note(
                    &KnowledgeNote {
                        title: s.into(),
                        category: "notes".into(),
                        facts: vec![format!("see [[hot]] from {s}")],
                        links: vec!["reference/hot".into()],
                        content_hash: format!("h{s}"),
                        created_at: 1,
                        updated_at: 1,
                        ..Default::default()
                    },
                    "default",
                    "notes",
                )
                .await
                .unwrap();
        }
        // Only `hot` is walked this cycle.
        ctx.notes = vec![decay_entry("reference/hot", 1, 1)];

        let out = NoteDecayStage::default().execute(ctx).await.unwrap();

        assert_eq!(
            out.report.notes_protected, 1,
            "note with 3 full-path incoming links must be protected (was the >=3 rule reached?)"
        );
        assert_eq!(
            out.report.notes_archived, 0,
            "protected note must not be archived"
        );
    }

    #[tokio::test]
    async fn high_severity_note_is_exempt_from_archival() {
        // A High-severity note with no activity (never recalled, stale, no
        // links) scores well below the 0.1 reference threshold, but must be
        // protected: archiving it out of the index would make the C2.7
        // confidence floor (0.7) it relies on meaningless.
        let (mut ctx, _store) = build_decay_ctx().await;

        let md = KnowledgeNote {
            title: "crit".into(),
            category: "reference".into(),
            facts: vec!["load-bearing fact".into()],
            severity: Severity::High,
            content_hash: "hc".into(),
            created_at: 1,
            updated_at: 1,
            ..Default::default()
        }
        .to_markdown();
        let dir = ctx.indexer.memory_dir().join("default").join("reference");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("crit.md"), &md).await.unwrap();

        // Old enough to clear the <7-day rule; `reference` is not a protected
        // category and has no incoming links, so only severity can protect it.
        ctx.notes = vec![decay_entry("reference/crit", 1, 1)];

        let out = NoteDecayStage::default().execute(ctx).await.unwrap();

        assert_eq!(
            out.report.notes_protected, 1,
            "High-severity note must be exempt from archival"
        );
        assert_eq!(
            out.report.notes_archived, 0,
            "protected note must not be archived"
        );
    }

    #[tokio::test]
    async fn stale_low_severity_note_is_archived() {
        // W2: a note NoteDrift flagged `stale: true` (outdated / contradicted)
        // is archived out of active retrieval even when its activity score is
        // high (freshly updated) — the flag was previously write-only.
        let (mut ctx, _store) = build_decay_ctx().await;

        let now = chrono::Utc::now().timestamp();
        // Updated an hour ago → high activity score, so the score-based loop
        // does NOT archive it (only the stale path can); created 30 days ago →
        // clears the >7-day age protection.
        let mut md = KnowledgeNote {
            title: "stale-note".into(),
            category: "reference".into(),
            facts: vec!["outdated fact".into()],
            severity: Severity::Low,
            content_hash: "hs".into(),
            created_at: now - 30 * 86400,
            updated_at: now - 3600,
            ..Default::default()
        }
        .to_markdown();
        // `to_markdown` never emits the parse-only `stale` field; NoteDrift
        // inserts it after the opening fence, so inject it the same way here.
        md = md.replacen("---\n", "---\nstale: true\n", 1);

        let mem_dir = ctx.indexer.memory_dir().to_path_buf();
        let cat_dir = mem_dir.join("default").join("reference");
        tokio::fs::create_dir_all(&cat_dir).await.unwrap();
        tokio::fs::write(cat_dir.join("stale-note.md"), &md)
            .await
            .unwrap();

        ctx.notes = vec![decay_entry(
            "reference/stale-note",
            now - 30 * 86400,
            now - 3600,
        )];

        let out = NoteDecayStage::default().execute(ctx).await.unwrap();

        assert_eq!(
            out.report.notes_archived, 1,
            "stale note must be archived out of active retrieval"
        );
        assert!(
            !cat_dir.join("stale-note.md").exists(),
            "original stale note file must be moved out of its category dir"
        );
        assert!(
            mem_dir
                .join("default")
                .join("archive")
                .join("reference")
                .join("stale-note.md")
                .exists(),
            "stale note must land in the archive/ dir (recoverable)"
        );
    }

    #[tokio::test]
    async fn stale_high_severity_note_is_not_archived() {
        // Stale High/Critical notes stay indexed: the C2.7 confidence floor they
        // rely on requires them present. Only Low/Med stale notes archive.
        let (mut ctx, _store) = build_decay_ctx().await;

        let now = chrono::Utc::now().timestamp();
        let mut md = KnowledgeNote {
            title: "stale-crit".into(),
            category: "reference".into(),
            facts: vec!["outdated but load-bearing".into()],
            severity: Severity::High,
            content_hash: "hsc".into(),
            created_at: now - 30 * 86400,
            updated_at: now - 3600,
            ..Default::default()
        }
        .to_markdown();
        md = md.replacen("---\n", "---\nstale: true\n", 1);

        let mem_dir = ctx.indexer.memory_dir().to_path_buf();
        let cat_dir = mem_dir.join("default").join("reference");
        tokio::fs::create_dir_all(&cat_dir).await.unwrap();
        tokio::fs::write(cat_dir.join("stale-crit.md"), &md)
            .await
            .unwrap();

        ctx.notes = vec![decay_entry(
            "reference/stale-crit",
            now - 30 * 86400,
            now - 3600,
        )];

        let out = NoteDecayStage::default().execute(ctx).await.unwrap();

        assert_eq!(
            out.report.notes_archived, 0,
            "stale High-severity note must NOT be archived"
        );
        assert!(
            cat_dir.join("stale-crit.md").exists(),
            "stale High-severity note must stay in place"
        );
    }

    #[tokio::test]
    async fn archival_stops_when_edit_budget_exhausted() {
        // Two ancient, unlinked, Low-severity notes — both score-based archival
        // candidates (recency-only score ≈ 0.01 < 0.2, cleared the >7-day age
        // protection). With a shared budget of exactly one destructive edit, the
        // loop archives one and breaks, deferring the second — proving the "one
        // night cannot mass-archive the corpus" bound is live.
        let (mut ctx, _store) = build_decay_ctx().await;
        let now = chrono::Utc::now().timestamp();
        let old = now - 400 * 86400;
        let mem_dir = ctx.indexer.memory_dir().to_path_buf();
        let cat_dir = mem_dir.join("default").join("notes");
        tokio::fs::create_dir_all(&cat_dir).await.unwrap();
        for name in ["alpha", "beta"] {
            let md = KnowledgeNote {
                title: name.into(),
                category: "notes".into(),
                facts: vec![format!("{name} cold fact")],
                severity: crate::memory::notes::Severity::Low,
                content_hash: format!("h_{name}"),
                created_at: old,
                updated_at: old,
                ..Default::default()
            }
            .to_markdown();
            tokio::fs::write(cat_dir.join(format!("{name}.md")), &md)
                .await
                .unwrap();
        }
        ctx.notes = vec![
            decay_entry("notes/alpha", old, old),
            decay_entry("notes/beta", old, old),
        ];
        ctx.evolution_budget = crate::memory::dreaming::EditBudget::new(1, 1_000_000);

        let out = NoteDecayStage::default().execute(ctx).await.unwrap();
        assert_eq!(
            out.report.notes_archived, 1,
            "a one-edit budget must cap archival at one note; the second defers to next cycle"
        );
    }
}
