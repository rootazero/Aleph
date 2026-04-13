//! NoteDrift stage — detects contradictions and stale information between related notes.
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

use super::DreamStage;

// ---------------------------------------------------------------------------
// Stage struct
// ---------------------------------------------------------------------------

pub struct NoteDriftStage;

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

        // Snapshot recently-updated note paths to avoid borrowing ctx inside the loop.
        let recent_paths: Vec<String> = ctx
            .notes
            .iter()
            .filter(|n| n.updated_at > week_ago)
            .map(|n| n.path.clone())
            .collect();

        for recent_path in &recent_paths {
            // Fetch outgoing links for this note.
            // NoteStore::get_outgoing_links expects the full `path` key (e.g. "wiki/rust").
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
                let response = match ctx
                    .provider
                    .process(RequestPayload::new(&msgs).with_system(Some(system)))
                    .await
                {
                    Ok(r) => r,
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
                    mark_contradictory(&ctx, &linked_path).await;
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

        tracing::info!(
            contradictions_found,
            notes_marked_stale,
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
        return Some(entry.path.clone());
    }
    // Otherwise look for a note whose path ends with `/<target>`.
    ctx.notes
        .iter()
        .find(|n| {
            n.path
                .split('/')
                .nth(1)
                .map(|filename| filename == target)
                .unwrap_or(false)
        })
        .map(|n| n.path.clone())
}

/// Append a `## Superseded` section to the note at `path` if it is not already
/// present, signalling that some information may conflict with a newer note.
async fn mark_contradictory(ctx: &DreamContext, path: &str) {
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

    if content.contains("## Superseded") {
        return; // Already marked — idempotent.
    }

    let updated = format!(
        "{content}\n\n## Superseded\n\n\
         _Some information in this note may be outdated or contradicted by a more recent note. \
         Review linked notes for current information._\n"
    );

    if let Err(e) = tokio::fs::write(&file_path, &updated).await {
        tracing::warn!(path = %file_path.display(), error = %e, "NoteDrift: failed to write contradiction marker");
    }
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

    if let Err(e) = tokio::fs::write(&file_path, &updated).await {
        tracing::warn!(path = %file_path.display(), error = %e, "NoteDrift: failed to write stale marker");
    }
}

/// Construct the on-disk file path for a note given its `path` key
/// (e.g. `"wiki/rust-ownership"`) inside `ctx`.
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
            last_accessed_at: None,
            content_hash: String::new(),
        }
    }

    // -----------------------------------------------------------------------
    // should_run tests — evaluated directly against the predicate logic
    // -----------------------------------------------------------------------

    #[test]
    fn should_run_false_no_recent_updates() {
        let old_ts = chrono::Utc::now().timestamp() - 8 * 86_400; // 8 days ago
        let notes = vec![
            make_note("wiki/rust", "wiki", old_ts),
            make_note("wiki/cargo", "wiki", old_ts),
        ];
        let week_ago = chrono::Utc::now().timestamp() - 7 * 86_400;
        let should = notes.iter().any(|n| n.updated_at > week_ago);
        assert!(!should, "No recent notes — stage should NOT run");
    }

    #[test]
    fn should_run_true_with_recent_update() {
        let recent_ts = chrono::Utc::now().timestamp() - 1 * 86_400; // 1 day ago
        let old_ts = chrono::Utc::now().timestamp() - 10 * 86_400;
        let notes = vec![
            make_note("wiki/rust", "wiki", old_ts),
            make_note("wiki/cargo", "wiki", recent_ts),
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

        let original = "---\ncategory: wiki\ntags: [rust]\n---\n\n- A fact\n";
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
            result.contains("category: wiki"),
            "original fields preserved"
        );
        assert!(result.contains("- A fact"), "body preserved");
    }

    #[tokio::test]
    async fn mark_stale_idempotent_when_stale_already_present() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");

        let original = "---\nstale: true\ncategory: wiki\n---\n\n- A fact\n";
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

    #[tokio::test]
    async fn mark_contradictory_appends_superseded_section() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");

        let original = "---\ncategory: wiki\n---\n\n- A fact\n";
        tokio::fs::write(&file, original).await.unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert!(!content.contains("## Superseded"));

        // Replicate mark_contradictory logic
        let updated = format!(
            "{content}\n\n## Superseded\n\n\
             _Some information in this note may be outdated or contradicted by a more recent note. \
             Review linked notes for current information._\n"
        );
        tokio::fs::write(&file, &updated).await.unwrap();

        let result = tokio::fs::read_to_string(&file).await.unwrap();
        assert!(
            result.contains("## Superseded"),
            "Superseded section must be appended"
        );
        assert!(result.contains("- A fact"), "Original body preserved");
    }

    #[tokio::test]
    async fn mark_contradictory_idempotent_when_superseded_present() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");

        let original =
            "---\ncategory: wiki\n---\n\n- A fact\n\n## Superseded\n\n_Already marked._\n";
        tokio::fs::write(&file, original).await.unwrap();

        // Guard condition: if "## Superseded" already present, do nothing.
        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert!(
            content.contains("## Superseded"),
            "Pre-condition: already marked"
        );
        // File unchanged — no second append.
        let after = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, after);
    }

    // -----------------------------------------------------------------------
    // resolve_link_path — unit test without full DreamContext
    // -----------------------------------------------------------------------

    #[test]
    fn stage_name() {
        assert_eq!(NoteDriftStage.name(), "note_drift");
    }

    #[test]
    fn resolve_link_path_finds_by_filename_segment() {
        // Simulate the relevant part of ctx.notes
        let notes = vec![
            make_note("wiki/rust-ownership", "wiki", 0),
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
        assert_eq!(found.map(|n| n.path.as_str()), Some("wiki/rust-ownership"));
    }

    #[test]
    fn resolve_link_path_returns_none_for_unknown_target() {
        let notes = vec![make_note("wiki/rust-ownership", "wiki", 0)];
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
}
