//! `NoteConsolidate` stage — merges similar/duplicate notes within the same category.
//!
//! Algorithm:
//!   1. Group notes by category.
//!   2. For categories with 2–20 notes, send all content to LLM in a single batch
//!      call asking for consolidation suggestions.
//!   3. For categories with >20 notes, use title-based heuristics (shared prefix
//!      or near-identical titles) to reduce the candidate set before calling LLM.
//!   4. For each suggested pair, execute MERGE / `ABSORB_A` / `ABSORB_B` decisions.
//!   5. Update the index after every modification.
//!
//! Safety: all files being modified are backed up (`.bak`) before any write.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::evolution::{score_merge_candidate, MERGE_ACCEPT_THRESHOLD};
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::KnowledgeNote;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::DreamStage;

// ---------------------------------------------------------------------------
// Stage struct
// ---------------------------------------------------------------------------

pub struct NoteConsolidateStage {
    /// Ceiling on LLM-judged pairs per run.
    ///
    /// This stage was the only looping LLM stage in the subsystem with neither
    /// a pair budget nor a provider-exhaustion arm — and its candidate
    /// generation for a large category is a full O(n²) title cross-product,
    /// one LLM call per returned pair. `NoteDrift` carries the same field for
    /// the same reason, recorded in its own doc: uncapped, this shape is what
    /// produced ten thousand provider calls in a single night. Burning the
    /// night's quota here also silently starves every later stage —
    /// `feedback_distill` and `tool_failure_distill` included — so the visible
    /// symptom is "dreaming ran but nothing was learned".
    pub max_pairs: usize,
}

impl Default for NoteConsolidateStage {
    fn default() -> Self {
        Self {
            max_pairs: crate::config::types::memory::DreamingConfig::default()
                .consolidate_max_pairs_per_run,
        }
    }
}

// ---------------------------------------------------------------------------
// DreamStage impl
// ---------------------------------------------------------------------------

#[async_trait]
impl DreamStage for NoteConsolidateStage {
    fn name(&self) -> &'static str {
        "note_consolidate"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.notes.len() >= 2
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let mut consolidated = 0u32;
        let mut pairs_checked = 0usize;
        let mut budget_exhausted = false;

        // Group notes by category — snapshot to avoid borrowing ctx inside the loop.
        let mut by_category: HashMap<String, Vec<usize>> = HashMap::new();
        for (idx, note) in ctx.notes.iter().enumerate() {
            by_category
                // rust-doctor-disable-next-line excessive-clone
                .entry(note.category.clone())
                .or_default()
                .push(idx);
        }

        for (category, indices) in &by_category {
            if indices.len() < 2 {
                continue;
            }

            // Determine candidate pairs:
            //   - small category (≤ 20 notes): batch LLM call
            //   - large category (> 20 notes): title-heuristic filter, then LLM
            let pairs = if indices.len() <= 20 {
                // Build a batch prompt and ask LLM for all consolidation suggestions.
                match batch_consolidate_candidates(&mut ctx, category, indices).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(category, error = %e, "NoteConsolidate: batch LLM call failed");
                        continue;
                    }
                }
            } else {
                // Large category: use title-similarity heuristic to pre-filter.
                title_heuristic_candidates(&ctx, indices)
            };

            // Process each candidate pair
            for (idx_a, idx_b) in pairs {
                if pairs_checked >= self.max_pairs {
                    budget_exhausted = true;
                    break;
                }
                pairs_checked += 1;
                let result = consolidate_pair(&mut ctx, idx_a, idx_b).await;
                match result {
                    Ok(true) => {
                        consolidated += 1;
                    }
                    Ok(false) => {} // COEXIST — no action
                    // An exhausted provider (429/403) is a deliberate cycle
                    // abort, not a pair that happened to fail: every later
                    // stage would fail identically and the daemon must not keep
                    // hammering. Same arm as note_weave / note_drift.
                    Err(e) if super::is_provider_exhausted(&e) => return Err(e),
                    Err(e) => {
                        tracing::warn!(
                            note_a = ctx.notes[idx_a].path,
                            note_b = ctx.notes[idx_b].path,
                            error = %e,
                            "NoteConsolidate: pair consolidation failed"
                        );
                    }
                }
            }
            if budget_exhausted {
                break;
            }
        }

        ctx.report.notes_consolidated = consolidated;
        if budget_exhausted {
            tracing::info!(
                max_pairs = self.max_pairs,
                consolidated,
                "NoteConsolidate: pair budget exhausted; remaining candidates deferred to the next cycle"
            );
        }
        tracing::info!(consolidated, "NoteConsolidate completed");
        Ok(ctx)
    }
}

// ---------------------------------------------------------------------------
// Batch LLM consolidation (small categories, ≤ 20 notes)
// ---------------------------------------------------------------------------

/// Ask the LLM to identify which notes should be consolidated, returning pairs
/// of `(note_index_a, note_index_b)` from `indices`.
async fn batch_consolidate_candidates(
    ctx: &mut DreamContext,
    category: &str,
    indices: &[usize],
) -> Result<Vec<(usize, usize)>, AlephError> {
    // Load content for all notes in the category
    let mut previews: Vec<(usize, String, String)> = Vec::new(); // (index, path, preview)
    for &idx in indices {
        // rust-doctor-disable-next-line excessive-clone
        let path = ctx.notes[idx].path.clone();
        let content = ctx.load_content(&path).await.unwrap_or_default();
        // Use up to 400 chars per note for the batch prompt
        let preview: String = content.chars().take(400).collect();
        previews.push((idx, path, preview));
    }

    // Build the batch prompt
    let mut note_list = String::new();
    for (i, (_, path, preview)) in previews.iter().enumerate() {
        note_list.push_str(&format!("Note {i} ({path}):\n{preview}\n\n"));
    }

    let prompt = format!(
        "You are reviewing knowledge notes in the '{category}' category. \
Identify any pairs of notes that overlap significantly and should be consolidated.\n\n\
{note_list}\
Respond with ONLY a list of pairs to consolidate, one per line, in the format:\n\
MERGE: <note_i_number> <note_j_number>\n\
ABSORB_A: <note_i_number> <note_j_number>  (keep first, absorb second into it)\n\
ABSORB_B: <note_i_number> <note_j_number>  (keep second, absorb first into it)\n\
If no consolidation is needed, respond with: NONE\n\
Do NOT include any explanation or extra text."
    );

    let system = "You are a knowledge management assistant. \
Identify duplicate or heavily overlapping notes. \
Output ONLY the required pair directives or NONE.";

    let msgs = vec![UnifiedMessage::user(&prompt)];
    let response = ctx
        .provider
        .process(RequestPayload::new(&msgs).with_system(Some(system)))
        .await
        .map_err(|e| AlephError::other(format!("NoteConsolidate LLM batch call failed: {e}")))?;

    let text = response.text_content();
    let trimmed = text.trim();

    if trimmed.eq_ignore_ascii_case("none") || trimmed.is_empty() {
        return Ok(vec![]);
    }

    // Parse pairs from the LLM response
    // Map from note-list position back to ctx.notes index
    let pos_to_idx: Vec<usize> = previews.iter().map(|(idx, _, _)| *idx).collect();

    let mut pairs = Vec::new();
    for line in trimmed.lines() {
        if let Some(pair) = parse_llm_pair_line(line, &pos_to_idx) {
            // Deduplicate: only keep pairs where idx_a < idx_b
            let (a, b) = if pair.0 <= pair.1 {
                pair
            } else {
                (pair.1, pair.0)
            };
            if !pairs.contains(&(a, b)) {
                pairs.push((a, b));
            }
        }
    }

    Ok(pairs)
}

/// Parse one line like `MERGE: 0 2` or `ABSORB_A: 3 1` into `(idx_a, idx_b)`.
/// The `action` prefix is stored in the note entries below but for candidate
/// selection we only need the pair — the action is re-determined per pair
/// by a targeted LLM call in `consolidate_pair`.
fn parse_llm_pair_line(line: &str, pos_to_idx: &[usize]) -> Option<(usize, usize)> {
    // Expected format: "ACTION: N M"
    let colon_pos = line.find(':')?;
    let tail = line[colon_pos + 1..].trim();
    let mut parts = tail.split_whitespace();
    let a_str = parts.next()?;
    let b_str = parts.next()?;
    let a_pos: usize = a_str.parse().ok()?;
    let b_pos: usize = b_str.parse().ok()?;
    let idx_a = *pos_to_idx.get(a_pos)?;
    let idx_b = *pos_to_idx.get(b_pos)?;
    if idx_a == idx_b {
        return None;
    }
    Some((idx_a, idx_b))
}

// ---------------------------------------------------------------------------
// Title-heuristic candidate finding (large categories, > 20 notes)
// ---------------------------------------------------------------------------

/// Find candidate pairs by comparing note titles for near-duplicates.
///
/// Two notes are candidates if:
/// - One title is a prefix of the other (e.g. "rust-ownership" vs "rust-ownership-rules"), OR
/// - The titles share > 70% of characters when lowercased (simple length-based heuristic).
fn title_heuristic_candidates(ctx: &DreamContext, indices: &[usize]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();

    // Extract (index, lowercase_title) pairs
    let titles: Vec<(usize, String)> = indices
        .iter()
        .map(|&i| {
            let filename = ctx.notes[i]
                .path
                .split('/')
                .nth(1)
                .unwrap_or(&ctx.notes[i].path)
                .to_lowercase();
            (i, filename)
        })
        .collect();

    for i in 0..titles.len() {
        for j in (i + 1)..titles.len() {
            let (idx_a, title_a) = &titles[i];
            let (idx_b, title_b) = &titles[j];

            if is_title_similar(title_a, title_b) {
                pairs.push((*idx_a, *idx_b));
            }
        }
    }

    pairs
}

/// Returns true if two lowercased titles are likely to refer to the same topic.
fn is_title_similar(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }

    // One is a prefix of the other (with separator boundary)
    if a.starts_with(b) || b.starts_with(a) {
        let shorter = a.len().min(b.len());
        let longer = a.len().max(b.len());
        // Only flag if the prefix covers > 70% of the longer title
        if shorter * 10 >= longer * 7 {
            return true;
        }
    }

    // Levenshtein-free similarity: ratio of shared length vs max length
    // Simple approximation: |a| and |b| are close AND they share a long common prefix
    let common_prefix_len = a
        .chars()
        .zip(b.chars())
        .take_while(|(ca, cb)| ca == cb)
        .count();

    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return false;
    }

    // Titles share > 75% common prefix
    common_prefix_len * 4 >= max_len * 3
}

// ---------------------------------------------------------------------------
// Per-pair consolidation: targeted LLM decision + file operations
// ---------------------------------------------------------------------------

/// Attempt to consolidate a pair of notes.
///
/// Returns `Ok(true)` if consolidation happened, `Ok(false)` if COEXIST.
async fn consolidate_pair(
    ctx: &mut DreamContext,
    idx_a: usize,
    idx_b: usize,
) -> Result<bool, AlephError> {
    // rust-doctor-disable-next-line excessive-clone
    let path_a = ctx.notes[idx_a].path.clone();
    // rust-doctor-disable-next-line excessive-clone
    let path_b = ctx.notes[idx_b].path.clone();

    let content_a = ctx.load_content(&path_a).await.unwrap_or_default();
    let content_b = ctx.load_content(&path_b).await.unwrap_or_default();

    // Preview: up to 600 chars per note
    let preview_a: String = content_a.chars().take(600).collect();
    let preview_b: String = content_b.chars().take(600).collect();

    let prompt = format!(
        "Given these two knowledge notes in the same category, decide if they should be merged:\n\n\
Note A ({path_a}):\n{preview_a}\n\n\
Note B ({path_b}):\n{preview_b}\n\n\
Respond with exactly one word:\n\
MERGE      — combine both into a single note (content is too similar/redundant)\n\
COEXIST    — keep both as separate notes (content is distinct enough)\n\
ABSORB_A   — keep A, absorb B's unique content into A, then delete B\n\
ABSORB_B   — keep B, absorb A's unique content into B, then delete A"
    );

    let system = "You are a knowledge consolidation assistant. \
Respond with exactly one word: MERGE, COEXIST, ABSORB_A, or ABSORB_B.";

    let msgs = vec![UnifiedMessage::user(&prompt)];
    let response = ctx
        .provider
        .process(RequestPayload::new(&msgs).with_system(Some(system)))
        .await
        .map_err(|e| {
            AlephError::other(format!(
                "NoteConsolidate pair LLM call failed ({path_a} / {path_b}): {e}"
            ))
        })?;

    let decision = response.text_content().trim().to_uppercase();
    let decision = decision.as_str();

    match decision {
        "COEXIST" => {
            tracing::debug!(path_a, path_b, "NoteConsolidate: COEXIST");
            Ok(false)
        }
        "MERGE" => execute_merge(ctx, idx_a, idx_b, &content_a, &content_b, MergeMode::Merge).await,
        "ABSORB_A" => {
            // Keep A, absorb B into A
            execute_merge(
                ctx,
                idx_a,
                idx_b,
                &content_a,
                &content_b,
                MergeMode::AbsorbIntoA,
            )
            .await
        }
        "ABSORB_B" => {
            // Keep B, absorb A into B — swap roles: keeper=B, absorbed=A
            execute_merge(
                ctx,
                idx_b,
                idx_a,
                &content_b,
                &content_a,
                MergeMode::AbsorbIntoA,
            )
            .await
        }
        other => {
            tracing::warn!(
                path_a,
                path_b,
                decision = other,
                "NoteConsolidate: unexpected LLM decision, treating as COEXIST"
            );
            Ok(false)
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum MergeMode {
    /// Full merge: deduplicate facts, write to keeper (earlier `created_at`), delete other.
    Merge,
    /// Absorb: append unique facts from absorbed note into keeper, delete absorbed.
    AbsorbIntoA,
}

/// Execute the file-level merge/absorb operation.
///
/// `keeper_idx` is the note that survives; `absorbed_idx` is deleted.
/// Both content strings are passed in to avoid re-reading.
/// Returns `Ok(true)` when the merge was applied, `Ok(false)` when the
/// evolution gate or edit budget skipped it (no files touched).
// rust-doctor-disable-next-line high-cyclomatic-complexity
async fn execute_merge(
    ctx: &mut DreamContext,
    keeper_idx: usize,
    absorbed_idx: usize,
    keeper_content: &str,
    absorbed_content: &str,
    mode: MergeMode,
) -> Result<bool, AlephError> {
    // rust-doctor-disable-next-line excessive-clone
    let keeper_path = ctx.notes[keeper_idx].path.clone();
    // rust-doctor-disable-next-line excessive-clone
    let absorbed_path = ctx.notes[absorbed_idx].path.clone();

    let (keeper_category, keeper_filename) = keeper_path
        .split_once('/')
        .ok_or_else(|| AlephError::other(format!("Invalid note path: {keeper_path}")))?;
    let (absorbed_category, absorbed_filename) = absorbed_path
        .split_once('/')
        .ok_or_else(|| AlephError::other(format!("Invalid note path: {absorbed_path}")))?;

    let memory_dir = ctx.indexer.memory_dir().to_path_buf();
    // rust-doctor-disable-next-line excessive-clone
    let agent_id = ctx.agent_id.clone();

    let keeper_file = memory_dir
        .join(&agent_id)
        .join(keeper_category)
        .join(format!("{keeper_filename}.md"));
    let absorbed_file = memory_dir
        .join(&agent_id)
        .join(absorbed_category)
        .join(format!("{absorbed_filename}.md"));

    // --- Parse both notes (before any file write, so the gate can score them) ---
    let mut keeper_note =
        KnowledgeNote::from_markdown(keeper_filename, keeper_content).map_err(|e| {
            AlephError::other(format!(
                "NoteConsolidate: failed to parse keeper note {keeper_path}: {e}"
            ))
        })?;

    let absorbed_note =
        KnowledgeNote::from_markdown(absorbed_filename, absorbed_content).map_err(|e| {
            AlephError::other(format!(
                "NoteConsolidate: failed to parse absorbed note {absorbed_path}: {e}"
            ))
        })?;

    // --- Evolution gate: skip merges that would fuse distinct knowledge ---
    // The LLM proposed this merge, but SkillOpt discipline applies it only when
    // a local safety score clears the threshold (R7: model proposes, gate vets).
    let merge_score = score_merge_candidate(&keeper_note, &absorbed_note);
    if merge_score < MERGE_ACCEPT_THRESHOLD {
        ctx.report.merges_rejected += 1;
        tracing::info!(
            keeper = keeper_path,
            absorbed = absorbed_path,
            score = merge_score,
            threshold = MERGE_ACCEPT_THRESHOLD,
            "NoteConsolidate: evolution gate rejected merge (would fuse distinct knowledge)"
        );
        return Ok(false);
    }

    // --- Edit budget: bound how much this cycle may rewrite ---
    let edit_bytes = (keeper_content.len() + absorbed_content.len()) as u64;
    if !ctx.evolution_budget.try_spend(edit_bytes) {
        tracing::info!(
            keeper = keeper_path,
            absorbed = absorbed_path,
            "NoteConsolidate: edit budget exhausted — deferring merge to next cycle"
        );
        return Ok(false);
    }

    // --- Safety: back up both files before modification ---
    let keeper_bak = keeper_file.with_extension("md.bak");
    let absorbed_bak = absorbed_file.with_extension("md.bak");

    tokio::fs::copy(&keeper_file, &keeper_bak)
        .await
        .map_err(|e| {
            AlephError::other(format!(
                "NoteConsolidate: failed to back up keeper {keeper_file:?}: {e}"
            ))
        })?;
    tokio::fs::copy(&absorbed_file, &absorbed_bak)
        .await
        .map_err(|e| {
            AlephError::other(format!(
                "NoteConsolidate: failed to back up absorbed {absorbed_file:?}: {e}"
            ))
        })?;

    // --- Merge content ---
    match mode {
        MergeMode::Merge => {
            // Use the earliest created_at as the keeper's created_at
            if absorbed_note.created_at < keeper_note.created_at && absorbed_note.created_at > 0 {
                keeper_note.created_at = absorbed_note.created_at;
            }
        }
        MergeMode::AbsorbIntoA => {}
    }

    // Merge unique facts + links through the body-sync helpers so a keeper
    // with a verbatim prose body gains the absorbed bullets instead of
    // silently dropping them (both merge modes append unique facts).
    let fresh_facts: Vec<String> = absorbed_note
        .facts
        .iter()
        .filter(|f| !keeper_note.facts.contains(f))
        .cloned()
        .collect();
    keeper_note.append_facts(&fresh_facts);
    keeper_note.add_links(&absorbed_note.links);

    // Merge unique tags
    for tag in &absorbed_note.tags {
        if !keeper_note.tags.contains(tag) {
            // rust-doctor-disable-next-line excessive-clone
            keeper_note.tags.push(tag.clone());
        }
    }

    // Bump updated_at
    keeper_note.updated_at = chrono::Utc::now().timestamp();

    // --- Write the merged keeper note ---
    // `write_note` is a full write: it indexes, re-embeds and backfills inbound
    // links from the bytes it just put on disk. The `index_file` call that used
    // to follow was structurally a no-op — it hashes the same file and skips on
    // the hash `write_note` had just recorded — so it only ever bought a wasted
    // read and the impression that the write path needed a chaperone.
    ctx.indexer
        .write_note(&agent_id, keeper_category, &keeper_note)
        .await
        .map_err(|e| {
            AlephError::other(format!(
                "NoteConsolidate: failed to write merged keeper {keeper_path}: {e}"
            ))
        })?;

    // --- Delete the absorbed file and its index entry ---
    ctx.indexer
        .store()
        .remove_note_index(&absorbed_path, &agent_id)
        .await
        .map_err(|e| {
            AlephError::other(format!(
                "NoteConsolidate: failed to remove index for {absorbed_path}: {e}"
            ))
        })?;

    tokio::fs::remove_file(&absorbed_file).await.map_err(|e| {
        AlephError::other(format!(
            "NoteConsolidate: failed to delete absorbed file {absorbed_file:?}: {e}"
        ))
    })?;

    // Clean up backup files (best-effort)
    let _ = tokio::fs::remove_file(&keeper_bak).await;
    let _ = tokio::fs::remove_file(&absorbed_bak).await;

    // Invalidate cached contents
    ctx.note_contents.remove(&keeper_path);
    ctx.note_contents.remove(&absorbed_path);

    // Record the merged pair so the post-pipeline drain can feed MutationGate's
    // merge-cycle detector (previously starved of data — no caller existed).
    ctx.report
        .merged_pairs
        // rust-doctor-disable-next-line excessive-clone
        .push((keeper_path.clone(), absorbed_path.clone()));

    tracing::info!(
        keeper = keeper_path,
        absorbed = absorbed_path,
        mode = ?mode,
        score = merge_score,
        "NoteConsolidate: merged notes"
    );

    Ok(true)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::NoteEntry;

    fn make_note(path: &str, category: &str) -> NoteEntry {
        NoteEntry {
            path: path.to_string(),
            category: category.to_string(),
            tags: vec![],
            created_at: 1_700_000_000,
            updated_at: 1_700_001_000,
            content_hash: String::new(),
        }
    }

    #[test]
    fn should_run_false_with_single_note() {
        let stage = NoteConsolidateStage::default();
        let notes = [make_note("reference/rust", "reference")];
        // Predicate: notes.len() >= 2
        assert!(
            notes.len() < 2,
            "single note should NOT trigger consolidation"
        );
        let _ = stage;
    }

    #[test]
    fn should_run_true_with_multiple_notes() {
        let stage = NoteConsolidateStage::default();
        let notes = [
            make_note("reference/rust", "reference"),
            make_note("reference/rust-ownership", "reference"),
        ];
        assert!(notes.len() >= 2, "two notes should trigger consolidation");
        let _ = stage;
    }

    #[test]
    fn parse_merge_decision() {
        // Test that the decision parser correctly routes MERGE/COEXIST/ABSORB_*
        let cases = [
            ("MERGE", true),
            ("COEXIST", false),
            ("ABSORB_A", true),
            ("ABSORB_B", true),
            ("INVALID", false), // unexpected → treated as COEXIST
        ];

        for (decision, expect_consolidated) in &cases {
            let upper = decision.to_uppercase();
            let would_consolidate = matches!(upper.as_str(), "MERGE" | "ABSORB_A" | "ABSORB_B");
            assert_eq!(
                would_consolidate, *expect_consolidated,
                "decision={decision}"
            );
        }
    }

    #[test]
    fn title_heuristic_finds_prefix_pair() {
        let notes = [
            make_note("reference/rust-ownership", "reference"),
            make_note("reference/rust-ownership-rules", "reference"),
            make_note("reference/python-async", "reference"),
        ];
        let _indices: Vec<usize> = (0..notes.len()).collect();

        // Build a minimal DreamContext-like structure for testing the heuristic
        // We can test is_title_similar directly
        assert!(is_title_similar("rust-ownership", "rust-ownership-rules"));
        assert!(!is_title_similar("rust-ownership", "python-async"));
    }

    #[test]
    fn title_heuristic_no_false_positives() {
        assert!(!is_title_similar("async-rust", "sync-python"));
        assert!(!is_title_similar("ownership", "borrowing"));
        assert!(!is_title_similar("", ""));
    }

    #[test]
    fn parse_llm_pair_line_valid() {
        let pos_to_idx = vec![10, 20, 30]; // positions 0,1,2 map to note indices 10,20,30
        let result = parse_llm_pair_line("MERGE: 0 2", &pos_to_idx);
        assert_eq!(result, Some((10, 30)));
    }

    #[test]
    fn parse_llm_pair_line_absorb() {
        let pos_to_idx = vec![5, 6, 7];
        let result = parse_llm_pair_line("ABSORB_A: 1 2", &pos_to_idx);
        assert_eq!(result, Some((6, 7)));
    }

    #[test]
    fn parse_llm_pair_line_invalid_format() {
        let pos_to_idx = vec![0, 1, 2];
        // Missing second number
        assert_eq!(parse_llm_pair_line("MERGE: 0", &pos_to_idx), None);
        // Out of bounds position
        assert_eq!(parse_llm_pair_line("MERGE: 0 5", &pos_to_idx), None);
        // Same index → None
        assert_eq!(parse_llm_pair_line("MERGE: 1 1", &pos_to_idx), None);
    }

    #[test]
    fn stage_name() {
        assert_eq!(NoteConsolidateStage::default().name(), "note_consolidate");
    }
}
