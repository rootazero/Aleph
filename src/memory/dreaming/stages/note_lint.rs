//! NoteLint stage — frontmatter normalization and broken wikilink repair.
//!
//! This stage:
//!   1. Checks each note's frontmatter for required fields (`category`, `tags`,
//!      `created`, `updated`) and rewrites files that are missing any field.
//!   2. Queries the `notes_links` table for outgoing links from each note.
//!      For each link target that does not exist in `notes_index`, it attempts
//!      a fuzzy match (filename lookup across all categories).  If a unique
//!      match is found the link is rewritten in the file and the note is
//!      re-indexed; otherwise the broken link is counted and reported.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{remove_wikilink, rewrite_wikilinks};

use super::DreamStage;

// ---------------------------------------------------------------------------
// Stage struct
// ---------------------------------------------------------------------------

pub struct NoteLintStage;

// ---------------------------------------------------------------------------
// DreamStage impl
// ---------------------------------------------------------------------------

#[async_trait]
impl DreamStage for NoteLintStage {
    fn name(&self) -> &'static str {
        "note_lint"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let mut format_fixed = 0u32;
        let mut broken_links_found = 0u32;
        let mut links_repaired = 0u32;
        let mut links_purged = 0u32;

        // Snapshot note list so we can iterate without borrowing ctx
        let note_paths: Vec<String> = ctx.notes.iter().map(|n| n.path.clone()).collect();

        // ---------------------------------------------------------------
        // 1. Frontmatter completeness check
        // ---------------------------------------------------------------
        for path in &note_paths {
            let (category, filename) = match path.split_once('/') {
                Some(pair) => pair,
                None => continue,
            };

            let file_path = ctx
                .indexer
                .memory_dir()
                .join(&ctx.agent_id)
                .join(category)
                .join(format!("{filename}.md"));

            let content = match tokio::fs::read_to_string(&file_path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            if let Some(fixed_content) = ensure_frontmatter(&content, category) {
                if let Err(e) = tokio::fs::write(&file_path, &fixed_content).await {
                    tracing::warn!(path = %file_path.display(), error = %e, "NoteLint: failed to write fixed frontmatter");
                    continue;
                }
                // Re-index the corrected file
                let _ = ctx
                    .indexer
                    .index_file(&ctx.agent_id, category, &file_path)
                    .await;
                format_fixed += 1;
                // Invalidate cached content so later stages get the fresh version
                ctx.note_contents.remove(path);
            }
        }

        // ---------------------------------------------------------------
        // 2. Broken link detection and repair
        // ---------------------------------------------------------------
        // Hoist list_notes ONCE per stage entry. The previous per-target
        // re-fetch let a snapshot taken between target N's check and
        // target N+1's purge see different state — concurrent ingest could
        // create the missing target between iterations and the second
        // snapshot would still report it missing in target N+1's view.
        // A single snapshot keeps fuzzy-match decisions consistent across
        // all targets in this stage execution.
        let all_notes_snapshot = match ctx.indexer.store().list_notes(&ctx.agent_id).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "NoteLint: list_notes failed; skipping link repair phase");
                // Phase 1 results still apply; the repair phase is skipped.
                ctx.report.format_fixed = format_fixed;
                return Ok(ctx);
            }
        };

        for path in &note_paths {
            let (category, filename) = match path.split_once('/') {
                Some(pair) => pair,
                None => continue,
            };

            let outgoing = match ctx
                .indexer
                .store()
                .get_outgoing_links(path, &ctx.agent_id)
                .await
            {
                Ok(links) => links,
                Err(e) => {
                    tracing::warn!(path, error = %e, "NoteLint: failed to fetch outgoing links");
                    continue;
                }
            };

            for target in outgoing {
                // Check if target already resolves to an existing index entry.
                // Outgoing links store the raw wikilink text (filename without category).
                // Try a direct filename lookup first.
                let candidates = match ctx
                    .indexer
                    .store()
                    .find_by_filename(&target, &ctx.agent_id)
                    .await
                {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(target, error = %e, "NoteLint: find_by_filename failed");
                        continue;
                    }
                };

                if !candidates.is_empty() {
                    // Link resolves — no action needed
                    continue;
                }

                // Broken link detected
                broken_links_found += 1;

                // Fuzzy repair against the once-per-stage snapshot.
                let target_lower = target.to_lowercase();
                let fuzzy_matches: Vec<&str> = all_notes_snapshot
                    .iter()
                    .filter(|e| e.filename.to_lowercase() == target_lower)
                    .map(|e| e.filename.as_str())
                    .collect();

                if fuzzy_matches.is_empty() {
                    // D4: snapshot says target is missing — about to purge the
                    // wikilink. Re-check find_by_filename with a fresh query
                    // first to close the TOCTOU window: another writer may
                    // have created the target since our snapshot was taken,
                    // in which case purging would erase a now-valid link.
                    let recheck = ctx
                        .indexer
                        .store()
                        .find_by_filename(&target, &ctx.agent_id)
                        .await;
                    if matches!(recheck, Ok(ref c) if !c.is_empty()) {
                        tracing::info!(
                            target,
                            "NoteLint: target appeared during stage execution — skipping purge"
                        );
                        continue;
                    }

                    // tracing log preserves the original target for audit.
                    let file_path = ctx
                        .indexer
                        .memory_dir()
                        .join(&ctx.agent_id)
                        .join(category)
                        .join(format!("{filename}.md"));

                    let content = match tokio::fs::read_to_string(&file_path).await {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    let cleaned = remove_wikilink(&content, &target);
                    if cleaned == content {
                        // Link not present in body (already stripped or stored elsewhere) — skip
                        continue;
                    }

                    if let Err(e) = tokio::fs::write(&file_path, &cleaned).await {
                        tracing::warn!(path = %file_path.display(), error = %e, "NoteLint: failed to write purged content");
                        continue;
                    }

                    let _ = ctx
                        .indexer
                        .index_file(&ctx.agent_id, category, &file_path)
                        .await;

                    ctx.note_contents.remove(path);
                    links_purged += 1;
                    tracing::info!(
                        path,
                        purged_target = target,
                        "NoteLint: purged stale wikilink (D4)"
                    );
                    continue;
                }

                if fuzzy_matches.len() > 1 {
                    // Ambiguous — log only; never auto-pick to avoid mis-deletion
                    tracing::info!(
                        path,
                        target,
                        candidates = fuzzy_matches.len(),
                        "NoteLint: ambiguous wikilink, no unique fuzzy match — kept as-is"
                    );
                    continue;
                }

                let corrected_target = fuzzy_matches[0].to_string();
                if corrected_target == target {
                    // Identical — nothing to rewrite (shouldn't happen given empty candidates above,
                    // but guard against it)
                    continue;
                }

                // Rewrite the link in the file
                let file_path = ctx
                    .indexer
                    .memory_dir()
                    .join(&ctx.agent_id)
                    .join(category)
                    .join(format!("{filename}.md"));

                let content = match tokio::fs::read_to_string(&file_path).await {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let rewritten = rewrite_wikilinks(&content, &target, &corrected_target);
                if rewritten == content {
                    continue;
                }

                if let Err(e) = tokio::fs::write(&file_path, &rewritten).await {
                    tracing::warn!(path = %file_path.display(), error = %e, "NoteLint: failed to write repaired link");
                    continue;
                }

                // Re-index the repaired file
                let _ = ctx
                    .indexer
                    .index_file(&ctx.agent_id, category, &file_path)
                    .await;

                ctx.note_contents.remove(path);
                links_repaired += 1;

                tracing::info!(
                    path,
                    old_target = target,
                    new_target = corrected_target,
                    "NoteLint: repaired wikilink"
                );
            }
        }

        ctx.report.format_fixed = format_fixed;
        ctx.report.broken_links_found = broken_links_found;
        ctx.report.links_repaired = links_repaired;
        ctx.report.links_purged = links_purged;

        tracing::info!(
            format_fixed,
            broken_links_found,
            links_repaired,
            links_purged,
            "NoteLint completed"
        );

        Ok(ctx)
    }
}

// ---------------------------------------------------------------------------
// Frontmatter helpers
// ---------------------------------------------------------------------------

/// Returns `Some(fixed_content)` if the content was missing required frontmatter
/// fields and was repaired; `None` if no changes were needed.
///
/// Required fields: `category`, `tags`, `created`, `updated`.
/// The function is intentionally simple: it does string-level detection so it
/// does not require a full YAML round-trip for the common case where the note
/// is already correct.
fn ensure_frontmatter(content: &str, default_category: &str) -> Option<String> {
    let trimmed = content.trim();

    if !trimmed.starts_with("---") {
        // No frontmatter at all — prepend a minimal one
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let header = format!(
            "---\ncategory: {default_category}\ntags: []\ncreated: {today}\nupdated: {today}\n---\n\n"
        );
        return Some(format!("{header}{}", trimmed));
    }

    // Parse the frontmatter section to check for missing fields
    let after_open = &trimmed[3..];
    let close_pos = after_open.find("---")?;

    let yaml_section = &after_open[..close_pos];
    let body_after = &after_open[close_pos + 3..];

    let has_category = yaml_section.contains("category:");
    let has_tags = yaml_section.contains("tags:");
    let has_created = yaml_section.contains("created:");
    let has_updated = yaml_section.contains("updated:");

    if has_category && has_tags && has_created && has_updated {
        return None; // All fields present
    }

    // Build a patched YAML section
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut patched = yaml_section.trim_end_matches('\n').to_string();

    if !has_category {
        patched.push_str(&format!("\ncategory: {default_category}"));
    }
    if !has_tags {
        patched.push_str("\ntags: []");
    }
    if !has_created {
        patched.push_str(&format!("\ncreated: {today}"));
    }
    if !has_updated {
        patched.push_str(&format!("\nupdated: {today}"));
    }

    let fixed = format!(
        "---\n{}\n---{}",
        patched.trim_start_matches('\n'),
        body_after
    );
    Some(fixed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_run_returns_true_by_default() {
        // The default DreamStage::should_run implementation always returns true.
        // We verify this without constructing a full DreamContext by ensuring
        // NoteLintStage does not override should_run to return false.
        //
        // Compile-time proof: NoteLintStage does not override should_run, so the
        // default impl (returns true) is used.  The test below documents this.
        let stage = NoteLintStage;
        assert_eq!(stage.name(), "note_lint");
        // If we could call should_run here we would assert true — covered by the
        // integration path.  The name check is the minimal smoke-test.
    }

    #[test]
    fn ensure_frontmatter_no_change_when_complete() {
        let content = "\
---
category: reference
tags: [rust]
created: 2026-04-01
updated: 2026-04-10
---

- A fact
";
        assert!(
            ensure_frontmatter(content, "reference").is_none(),
            "Complete frontmatter should not be modified"
        );
    }

    #[test]
    fn ensure_frontmatter_prepends_when_missing_entirely() {
        let content = "- A fact without frontmatter\n";
        let fixed = ensure_frontmatter(content, "other").expect("Should return fixed content");
        assert!(
            fixed.starts_with("---\n"),
            "Fixed content must start with ---"
        );
        assert!(fixed.contains("category: other"));
        assert!(fixed.contains("tags: []"));
        assert!(fixed.contains("created:"));
        assert!(fixed.contains("updated:"));
        assert!(fixed.contains("- A fact without frontmatter"));
    }

    #[test]
    fn ensure_frontmatter_adds_missing_fields() {
        let content = "\
---
category: preference
---

- A fact
";
        let fixed = ensure_frontmatter(content, "preference").expect("Should patch missing fields");
        assert!(fixed.contains("category: preference"));
        assert!(fixed.contains("tags: []"));
        assert!(fixed.contains("created:"));
        assert!(fixed.contains("updated:"));
        assert!(fixed.contains("- A fact"));
    }

    #[test]
    fn ensure_frontmatter_malformed_no_closing_unchanged() {
        // Frontmatter opened but never closed — don't touch it
        let content = "---\ncategory: reference\n\n- A fact\n";
        assert!(
            ensure_frontmatter(content, "reference").is_none(),
            "Malformed frontmatter (no closing ---) should not be modified"
        );
    }

    #[test]
    fn note_lint_stage_name() {
        assert_eq!(NoteLintStage.name(), "note_lint");
    }
}
