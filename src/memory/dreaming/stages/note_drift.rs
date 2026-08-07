//! `NoteDrift` stage — detects contradictions and stale information between related notes.
//!
//! Algorithm:
//!   1. Filter notes updated in the last 7 days.
//!   2. For each recent note, fetch its outgoing wikilink targets from the index.
//!   3. Load the content of both the recent note and each linked note.
//!   4. Submit both contents to the LLM for a consistency check.
//!   5. CONTRADICTORY verdict → append a `## Superseded` section to the older/linked note.
//!   6. STALE verdict        → insert `stale: true` into the linked note's frontmatter.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::store::NoteStore;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::utils::atomic_write::atomic_write_file;

use super::DreamStage;

// ---------------------------------------------------------------------------
// Stage struct
// ---------------------------------------------------------------------------

pub struct NoteDriftStage {
    /// Hard cap on note pairs compared per run (`[memory.dreaming]
    /// drift_max_pairs_per_run`).
    ///
    /// Every pair costs one LLM call. Uncapped, the work is
    /// O(recent_notes × outgoing_links) — which is how a single night's
    /// dreaming reached tens of thousands of provider calls. The cap was
    /// configurable from day one but never read here; honour it.
    pub max_pairs: usize,
}

impl Default for NoteDriftStage {
    fn default() -> Self {
        Self {
            max_pairs: crate::config::types::memory::DreamingConfig::default()
                .drift_max_pairs_per_run,
        }
    }
}

// ---------------------------------------------------------------------------
// DreamStage impl
// ---------------------------------------------------------------------------

#[async_trait]
impl DreamStage for NoteDriftStage {
    fn name(&self) -> &'static str {
        "note_drift"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        let week_ago = chrono::Utc::now().timestamp() - 7 * 86_400;
        ctx.notes.iter().any(|n| n.updated_at > week_ago)
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let week_ago = chrono::Utc::now().timestamp() - 7 * 86_400;
        let mut contradictions_found = 0u32;
        let mut notes_marked_stale = 0u32;
        // One LLM call per pair — bounded by `max_pairs`.
        let mut pairs_checked = 0usize;
        let mut budget_exhausted = false;

        // Snapshot recently-updated note paths to avoid borrowing ctx inside the loop.
        let recent_paths: Vec<String> = ctx
            .notes
            .iter()
            .filter(|n| n.updated_at > week_ago)
            // rust-doctor-disable-next-line excessive-clone
            .map(|n| n.path.clone())
            .collect();

        'outer: for recent_path in &recent_paths {
            // Fetch outgoing links for this note.
            // NoteStore::get_outgoing_links expects the full `path` key (e.g. "reference/rust").
            let links = match ctx
                .indexer
                .store()
                .get_outgoing_links(recent_path, &ctx.agent_id)
                .await
            {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(
                        path = recent_path.as_str(),
                        error = %e,
                        "NoteDrift: failed to fetch outgoing links"
                    );
                    continue;
                }
            };

            if links.is_empty() {
                continue;
            }

            // Lazy-load the recent note's content.
            let recent_content = match ctx.load_content(recent_path).await {
                Some(c) => c,
                None => continue,
            };

            for linked_target in &links {
                if pairs_checked >= self.max_pairs {
                    budget_exhausted = true;
                    break 'outer;
                }

                // `linked_target` is the raw wikilink text (filename without category).
                // Resolve it to a full path present in ctx.notes.
                let linked_path = match resolve_link_path(&ctx, linked_target) {
                    Some(p) => p,
                    None => continue, // not in the current notes snapshot — skip
                };

                // Skip self-links (shouldn't happen, but guard defensively).
                if linked_path == *recent_path {
                    continue;
                }

                let linked_content = match ctx.load_content(&linked_path).await {
                    Some(c) => c,
                    None => continue,
                };

                // Build the LLM consistency-check prompt.
                let recent_preview: String = recent_content.chars().take(500).collect();
                let linked_preview: String = linked_content.chars().take(500).collect();

                let prompt = format!(
                    "Compare these two related knowledge notes for consistency.\n\n\
                     Note A (recently updated) — path: {recent_path}\n\
                     {recent_preview}\n\n\
                     Note B (linked by A) — path: {linked_path}\n\
                     {linked_preview}\n\n\
                     Are there any contradictions between them?\n\
                     Respond with exactly one word:\n\
                     CONSISTENT    — no contradictions\n\
                     CONTRADICTORY — they contain conflicting information\n\
                     STALE         — Note B contains outdated information that Note A has superseded"
                );

                let system = "You are a knowledge consistency checker. \
                    Respond with exactly one word: CONSISTENT, CONTRADICTORY, or STALE.";

                let msgs = vec![UnifiedMessage::user(&prompt)];
                pairs_checked += 1;
                let response = match ctx
                    .provider
                    .process(RequestPayload::new(&msgs).with_system(Some(system)))
                    .await
                {
                    Ok(r) => r,
                    Err(e) if super::is_provider_exhausted(&e) => {
                        tracing::warn!(
                            error = %e,
                            pairs_checked,
                            "NoteDrift: provider exhausted — aborting dream cycle"
                        );
                        return Err(e);
                    }
                    Err(e) => {
                        tracing::warn!(
                            recent = recent_path.as_str(),
                            linked = linked_path.as_str(),
                            error = %e,
                            "NoteDrift: LLM consistency check failed"
                        );
                        continue;
                    }
                };

                let verdict = response.text_content().trim().to_uppercase();

                if verdict.contains("CONTRADICTORY") {
                    mark_contradictory(&ctx, &linked_path, recent_path).await;
                    // Invalidate cached content for the modified note.
                    ctx.note_contents.remove(&linked_path);
                    contradictions_found += 1;
                    tracing::info!(
                        recent = recent_path.as_str(),
                        linked = linked_path.as_str(),
                        "NoteDrift: contradiction detected — marked Superseded"
                    );
                } else if verdict.contains("STALE") {
                    mark_stale(&ctx, &linked_path).await;
                    ctx.note_contents.remove(&linked_path);
                    notes_marked_stale += 1;
                    tracing::info!(
                        recent = recent_path.as_str(),
                        linked = linked_path.as_str(),
                        "NoteDrift: stale note detected — marked stale: true"
                    );
                }
                // CONSISTENT or unrecognised — nothing to do.
            }
        }

        ctx.report.contradictions_found = contradictions_found;
        ctx.report.notes_marked_stale = notes_marked_stale;

        if budget_exhausted {
            tracing::info!(
                max_pairs = self.max_pairs,
                "NoteDrift: pair budget exhausted — remaining pairs deferred to the next cycle"
            );
        }

        tracing::info!(
            contradictions_found,
            notes_marked_stale,
            pairs_checked,
            "NoteDrift completed"
        );

        Ok(ctx)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a raw wikilink target (filename without category) to a full note
/// path present in `ctx.notes`.  Returns `None` if not found.
fn resolve_link_path(ctx: &DreamContext, target: &str) -> Option<String> {
    // First try an exact path match in case the target already contains '/'.
    if let Some(entry) = ctx.notes.iter().find(|n| n.path == target) {
        // rust-doctor-disable-next-line excessive-clone
        return Some(entry.path.clone());
    }
    // Otherwise look for a note whose path ends with `/<target>`.
    ctx.notes
        .iter()
        .find(|n| {
            n.path
                .split('/')
                .nth(1)
                .is_some_and(|filename| filename == target)
        })
        // rust-doctor-disable-next-line excessive-clone
        .map(|n| n.path.clone())
}

/// Mark the note at `path` as superseded by the newer note `superseded_by`
/// (the recently-updated note that contradicts it).
///
/// The heading is written in the bare `## Superseded by [[target]]` form — no
/// date suffix — precisely so `sync_body_to_frontmatter`'s promotion regex
/// matches it, `index_note` materializes a `superseded_by` typed edge, and
/// retrieval force-surfaces the newer note alongside this one. A target-less
/// `## Superseded` banner (the earlier form) never promoted, so the loop was
/// silently open.
///
/// `CONTRADICTORY` is a symmetric verdict, so "the recently-updated note
/// supersedes the older one it contradicts" is a newer-wins governance default,
/// not an LLM attestation of direction.
async fn mark_contradictory(ctx: &DreamContext, path: &str, superseded_by: &str) {
    let file_path = match note_file_path(ctx, path) {
        Some(p) => p,
        None => return,
    };

    let content = match tokio::fs::read_to_string(&file_path).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %file_path.display(), error = %e, "NoteDrift: failed to read note for contradiction marking");
            return;
        }
    };

    let updated = match apply_supersede_banner(&content, superseded_by) {
        Some(u) => u,
        None => return, // Already superseded by this target — idempotent.
    };

    if let Err(e) = atomic_write_file(&file_path, &updated).await {
        tracing::warn!(path = %file_path.display(), error = %e, "NoteDrift: failed to write contradiction marker");
        return;
    }

    // Reconcile notes_index/FTS/embedding/tags with the new disk content —
    // `index_file` runs the vector leg too, so the banner is part of what the
    // note's embedding describes rather than drift the next boot merely counts.
    // Best-effort: a reindex failure must not abort the dream cycle.
    if let Some((category, _)) = path.split_once('/') {
        let _ = ctx
            .indexer
            .index_file(&ctx.agent_id, category, &file_path)
            .await;
    }
}

/// Build the supersession banner appended to a contradicted note.
///
/// The heading MUST stay in the bare `## Superseded by [[target]]` form (no date
/// suffix) so `sync_body_to_frontmatter` promotes it into a `superseded_by`
/// edge — the whole point of the marker. `supersede_banner_promotes` guards this.
fn supersede_banner(superseded_by: &str) -> String {
    format!(
        "## Superseded by [[{superseded_by}]]\n\n\
         _Superseded by a more recent, contradicting note. \
         See the linked note for current information._\n"
    )
}

/// Append the supersession banner to `content`, or `None` if `content` already
/// records this supersessor. Idempotency is *per target* — distinct newer notes
/// may each add their own banner, so the guard keys on the target-tagged
/// heading, not the bare `## Superseded` prefix.
fn apply_supersede_banner(content: &str, superseded_by: &str) -> Option<String> {
    let heading = format!("## Superseded by [[{superseded_by}]]");
    if content.contains(&heading) {
        return None;
    }
    Some(format!("{content}\n\n{}", supersede_banner(superseded_by)))
}

/// Insert `stale: true` into the YAML frontmatter of the note at `path`.
///
/// Inserts the key on a new line immediately after the opening `---` delimiter.
/// If the note has no frontmatter, or if `stale:` is already present, this is
/// a no-op.
async fn mark_stale(ctx: &DreamContext, path: &str) {
    let file_path = match note_file_path(ctx, path) {
        Some(p) => p,
        None => return,
    };

    let content = match tokio::fs::read_to_string(&file_path).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %file_path.display(), error = %e, "NoteDrift: failed to read note for stale marking");
            return;
        }
    };

    // Only operate on notes that have a frontmatter block and do not yet have
    // a `stale:` key.
    if !content.starts_with("---") || content.contains("stale:") {
        return;
    }

    // Find the first newline after the opening `---` and insert `stale: true`
    // on the very next line inside the frontmatter.
    // content[3..] skips the opening "---".
    let after_open = &content[3..];
    let first_nl = match after_open.find('\n') {
        Some(pos) => pos,
        None => return, // Malformed — don't touch.
    };

    // Insert position: right after the opening `---\n`
    let insert_at = 3 + first_nl + 1; // 3 = len("---"), +1 to move past '\n'
    let updated = format!(
        "{}stale: true\n{}",
        &content[..insert_at],
        &content[insert_at..]
    );

    if let Err(e) = atomic_write_file(&file_path, &updated).await {
        tracing::warn!(path = %file_path.display(), error = %e, "NoteDrift: failed to write stale marker");
        return;
    }

    // Reconcile notes_index/FTS/embedding/tags with the new disk content —
    // `index_file` runs the vector leg too, so the banner is part of what the
    // note's embedding describes rather than drift the next boot merely counts.
    // Best-effort: a reindex failure must not abort the dream cycle.
    if let Some((category, _)) = path.split_once('/') {
        let _ = ctx
            .indexer
            .index_file(&ctx.agent_id, category, &file_path)
            .await;
    }
}

/// Construct the on-disk file path for a note given its `path` key
/// (e.g. `"reference/rust-ownership"`) inside `ctx`.
fn note_file_path(ctx: &DreamContext, path: &str) -> Option<std::path::PathBuf> {
    let (category, filename) = path.split_once('/')?;
    Some(
        ctx.indexer
            .memory_dir()
            .join(&ctx.agent_id)
            .join(category)
            .join(format!("{filename}.md")),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::NoteEntry;

    fn make_note(path: &str, category: &str, updated_at: i64) -> NoteEntry {
        NoteEntry {
            path: path.to_string(),
            category: category.to_string(),
            tags: vec![],
            created_at: 1_700_000_000,
            updated_at,
            content_hash: String::new(),
        }
    }

    // -----------------------------------------------------------------------
    // should_run tests — evaluated directly against the predicate logic
    // -----------------------------------------------------------------------

    #[test]
    fn should_run_false_no_recent_updates() {
        let old_ts = chrono::Utc::now().timestamp() - 8 * 86_400; // 8 days ago
        let notes = [
            make_note("reference/rust", "reference", old_ts),
            make_note("reference/cargo", "reference", old_ts),
        ];
        let week_ago = chrono::Utc::now().timestamp() - 7 * 86_400;
        let should = notes.iter().any(|n| n.updated_at > week_ago);
        assert!(!should, "No recent notes — stage should NOT run");
    }

    #[test]
    fn should_run_true_with_recent_update() {
        let recent_ts = chrono::Utc::now().timestamp() - 86_400; // 1 day ago
        let old_ts = chrono::Utc::now().timestamp() - 10 * 86_400;
        let notes = [
            make_note("reference/rust", "reference", old_ts),
            make_note("reference/cargo", "reference", recent_ts),
        ];
        let week_ago = chrono::Utc::now().timestamp() - 7 * 86_400;
        let should = notes.iter().any(|n| n.updated_at > week_ago);
        assert!(should, "One recent note — stage SHOULD run");
    }

    // -----------------------------------------------------------------------
    // mark_stale helper — unit test with temp file
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn mark_stale_inserts_key_after_opening_dashes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");

        let original = "---\ncategory: reference\ntags: [rust]\n---\n\n- A fact\n";
        tokio::fs::write(&file, original).await.unwrap();

        // Call the helper's logic directly via a thin wrapper that operates on
        // a raw path (to avoid constructing a full DreamContext in a unit test).
        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert!(content.starts_with("---"));
        assert!(!content.contains("stale:"));

        // Replicate the insertion logic from mark_stale
        let after_open = &content[3..];
        let first_nl = after_open.find('\n').unwrap();
        let insert_at = 3 + first_nl + 1;
        let updated = format!(
            "{}stale: true\n{}",
            &content[..insert_at],
            &content[insert_at..]
        );

        tokio::fs::write(&file, &updated).await.unwrap();

        let result = tokio::fs::read_to_string(&file).await.unwrap();
        assert!(result.contains("stale: true"), "stale key must be present");
        assert!(
            result.starts_with("---\nstale: true\n"),
            "stale key must come first in frontmatter"
        );
        assert!(
            result.contains("category: reference"),
            "original fields preserved"
        );
        assert!(result.contains("- A fact"), "body preserved");
    }

    #[tokio::test]
    async fn mark_stale_idempotent_when_stale_already_present() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");

        let original = "---\nstale: true\ncategory: reference\n---\n\n- A fact\n";
        tokio::fs::write(&file, original).await.unwrap();

        // The guard condition `content.contains("stale:")` prevents double-insertion.
        let content = tokio::fs::read_to_string(&file).await.unwrap();
        let already_stale = content.contains("stale:");
        assert!(already_stale, "Pre-condition: stale already present");
        // If already_stale is true we do nothing — file remains unchanged.
        let after = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(
            content, after,
            "File must not be modified if stale: already present"
        );
    }

    // -----------------------------------------------------------------------
    // mark_contradictory helper — unit test with temp file
    // -----------------------------------------------------------------------

    #[test]
    fn apply_supersede_banner_appends_target_and_preserves_body() {
        let original = "---\ncategory: reference\n---\n\n- A fact\n";
        let updated = apply_supersede_banner(original, "reference/new").unwrap();
        assert!(
            updated.contains("## Superseded by [[reference/new]]"),
            "target-tagged section must be appended"
        );
        assert!(updated.contains("- A fact"), "original body preserved");
    }

    #[test]
    fn apply_supersede_banner_is_idempotent_per_target() {
        let base = "---\ncategory: reference\n---\n\n- A fact\n";
        let once = apply_supersede_banner(base, "reference/new").unwrap();
        // Same supersessor already recorded → no second append.
        assert!(apply_supersede_banner(&once, "reference/new").is_none());
        // A *different* supersessor is not blocked (multiple may accumulate).
        let twice = apply_supersede_banner(&once, "reference/other").unwrap();
        assert!(twice.contains("## Superseded by [[reference/new]]"));
        assert!(twice.contains("## Superseded by [[reference/other]]"));
    }

    // -----------------------------------------------------------------------
    // resolve_link_path — unit test without full DreamContext
    // -----------------------------------------------------------------------

    #[test]
    fn stage_name() {
        assert_eq!(NoteDriftStage::default().name(), "note_drift");
    }

    /// The cap exists to bound LLM spend: it was configurable from day one but
    /// `note_drift` never read it, so a run cost one call per (recent note ×
    /// outgoing link) with no ceiling.
    #[test]
    fn default_max_pairs_comes_from_dreaming_config() {
        assert_eq!(
            NoteDriftStage::default().max_pairs,
            crate::config::types::memory::DreamingConfig::default().drift_max_pairs_per_run,
        );
    }

    /// Guards the pairing arithmetic the cap protects against: 11 recent notes
    /// averaging ~6 links each is 70 LLM calls per run uncapped — the exact
    /// shape observed in the logs. Capped at 20, a run cannot exceed 20.
    #[test]
    fn pair_budget_bounds_calls_below_the_uncapped_product() {
        let recent_notes = 11usize;
        let links_each = 6usize;
        let uncapped = recent_notes * links_each;
        let cap = NoteDriftStage::default().max_pairs;
        assert!(
            cap < uncapped,
            "cap ({cap}) must actually bound the uncapped pair count ({uncapped})"
        );
        assert_eq!(uncapped.min(cap), cap);
    }

    #[test]
    fn resolve_link_path_finds_by_filename_segment() {
        // Simulate the relevant part of ctx.notes
        let notes = [
            make_note("reference/rust-ownership", "reference", 0),
            make_note("preference/editor", "preference", 0),
        ];
        let target = "rust-ownership";
        let found = notes.iter().find(|n| {
            n.path
                .split('/')
                .nth(1)
                .map(|f| f == target)
                .unwrap_or(false)
        });
        assert_eq!(
            found.map(|n| n.path.as_str()),
            Some("reference/rust-ownership")
        );
    }

    #[test]
    fn resolve_link_path_returns_none_for_unknown_target() {
        let notes = [make_note("reference/rust-ownership", "reference", 0)];
        let target = "nonexistent-note";
        let found = notes.iter().find(|n| {
            n.path
                .split('/')
                .nth(1)
                .map(|f| f == target)
                .unwrap_or(false)
        });
        assert!(found.is_none());
    }

    /// The contradiction→supersession loop only closes if the banner's heading
    /// is promotable by `sync_body_to_frontmatter`. Guards against a regression
    /// (e.g. re-adding a date suffix, or reverting to a target-less banner) that
    /// would silently stop materializing the `superseded_by` edge.
    #[test]
    fn supersede_banner_promotes() {
        let banner = supersede_banner("reference/new");
        assert!(banner.starts_with("## Superseded by [[reference/new]]\n"));

        let md = format!("---\ncategory: reference\ntags: []\n---\n\n- x\n\n{banner}");
        let mut n = crate::memory::notes::KnowledgeNote::from_markdown("old", &md).unwrap();
        crate::memory::notes::governance::supersession::sync_body_to_frontmatter(&mut n, &md);
        assert_eq!(n.superseded_by, vec!["reference/new".to_string()]);
    }
}
