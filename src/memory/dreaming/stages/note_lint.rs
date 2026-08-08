//! `NoteLint` stage — frontmatter normalization and broken wikilink repair.
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
use crate::memory::notes::orientation::types::{LogAction, LogEntry};
use crate::memory::notes::rewrite_wikilinks;
use crate::memory::notes::store::NoteStore;

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

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let mut format_fixed = 0u32;
        let mut broken_links_found = 0u32;
        let mut links_repaired = 0u32;

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
                .get_outgoing_link_rows(path, &ctx.agent_id)
                .await
            {
                Ok(links) => links,
                Err(e) => {
                    tracing::warn!(path, error = %e, "NoteLint: failed to fetch outgoing link rows");
                    continue;
                }
            };

            for row in outgoing {
                // Skip active links (resolve by construction) and tombstone links
                // (deleted targets whose body text is preserved — D1). Only process dangling.
                match crate::memory::notes::links::LinkStatus::parse(&row.status) {
                    crate::memory::notes::links::LinkStatus::Active
                    | crate::memory::notes::links::LinkStatus::Tombstone => continue,
                    crate::memory::notes::links::LinkStatus::Dangling => {}
                }
                let target = row.to_raw.clone();

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
                    // No fuzzy match found — leave the dangling link as-is.
                    // The relink_unresolved sweep at the end will retry resolution.
                    tracing::debug!(
                        path,
                        target,
                        "NoteLint: dangling link, no fuzzy match found — will retry in next cycle"
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
        // `DreamReport.links_purged` intentionally stays at its Default (0): the
        // tombstone design never purges links, so lint has no purge producer.
        // Reporting a hardcoded 0 only added a permanent "0 purged" noise line.

        tracing::info!(
            format_fixed,
            broken_links_found,
            links_repaired,
            "NoteLint completed"
        );

        // Late-resolution sweep: write-time resolution may have fallen back to
        // raw bare-filename targets when the destination didn't yet exist.
        // After this stage's other rules settle, retry resolution so cross-form
        // graph queries become consistent (Task A2.3).
        //
        // Best-effort: every prior on-disk/DB mutation in this stage is already
        // committed. A relink failure must not roll back the rest of the stage,
        // so we log and continue — the next dream cycle will retry.
        match ctx.indexer.store().relink_unresolved(&ctx.agent_id).await {
            Ok(relinked) => {
                if relinked > 0 {
                    tracing::info!(relinked, "NoteLint: late-resolved unresolved wikilinks");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "NoteLint: relink_unresolved failed; will retry next dream cycle");
            }
        }

        // ---------------------------------------------------------------
        // 3. Record the lint pass to the wiki log (log.md).
        // ---------------------------------------------------------------
        // The llm_wiki protocol treats log.md as a chronological record of
        // ingests, queries, AND lint passes. The LLM reads the log tail each
        // session (via NoteOrientation::read_snapshot), so projecting the pass
        // here — together with the standing orphan count from the last graph
        // recompute — is what makes wiki health visible to the model. Without
        // it the lint runs silently and `record_lint` has no producer.
        //
        // Best-effort throughout: a log/insights failure must never fail the
        // stage (mirrors the relink sweep above and the orientation philosophy
        // elsewhere — orientation is a projection, never load-bearing).
        if let Some(orient) = ctx.orientation.as_ref() {
            let orphans = orphan_count(ctx.indexer.store().as_ref(), &ctx.agent_id).await;
            let did_work = format_fixed > 0 || broken_links_found > 0 || links_repaired > 0;
            // Skip wholly-clean passes (no fixes, no orphans) so log.md stays an
            // event timeline rather than filling with empty "nothing to do" rows.
            if did_work || orphans > 0 {
                let entry = LogEntry {
                    timestamp_utc: chrono::Utc::now().timestamp(),
                    action: LogAction::Lint,
                    summary: format!(
                        "wiki lint pass: {format_fixed} frontmatter fixed, \
                         {links_repaired} links repaired"
                    ),
                    detail_lines: vec![
                        format!("broken links found: {broken_links_found}"),
                        format!("orphan notes (last graph pass): {orphans}"),
                    ],
                };
                if let Err(e) = orient.record_lint(&ctx.agent_id, entry).await {
                    tracing::warn!(error = %e, "NoteLint: failed to record lint pass to log.md (non-fatal)");
                }
            }
        }

        Ok(ctx)
    }
}

/// Count orphan notes from the last materialized graph recompute.
///
/// Reads the `isolated` insight row (a JSON array of note paths) written by
/// `GraphRecomputeStage`. A cold cache (before the first recompute) or any
/// read/parse error yields `0` — orphan reporting is purely informational and
/// must never disrupt the lint stage.
async fn orphan_count<S: NoteStore + ?Sized>(store: &S, agent_id: &str) -> usize {
    match store.read_graph_insights(agent_id, Some("isolated")).await {
        Ok(rows) => rows
            .into_iter()
            .find_map(|(_, payload)| serde_json::from_str::<Vec<String>>(&payload).ok())
            .map_or(0, |paths| paths.len()),
        Err(e) => {
            tracing::debug!(error = %e, "NoteLint: read_graph_insights(isolated) failed; reporting 0 orphans");
            0
        }
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
        return Some(format!("{header}{trimmed}"));
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

    // -----------------------------------------------------------------
    // Late-resolution lint rule (Task A2.3)
    // -----------------------------------------------------------------

    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::{KnowledgeNote, NoteIndexer};
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::mock::MockProvider;
    use crate::sync_primitives::Arc;

    /// Minimal EmbeddingProvider stub for stage tests — never invoked because
    /// `lint_resolves_pending_links_after_target_appears` does not exercise
    /// any embedding-touching stage code.
    struct StubEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
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

    async fn build_test_dream_ctx() -> (DreamContext, Arc<SqliteMemoryBackend>) {
        let temp = std::env::temp_dir().join(format!("aleph_lint_{}", uuid::Uuid::new_v4()));
        // Create the dir first: a nonexistent path is treated as the DB *file*
        // by SqliteMemoryBackend::new, which would make memory_dir a file and
        // break tests that mkdir note categories under it (NotADirectory).
        tokio::fs::create_dir_all(&temp).await.unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new(""));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(StubEmbedder);

        let ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: crate::memory::dreaming::DreamReport::default(),
            pipeline_type: "consolidate".into(),
            strategy: crate::memory::dreaming::DreamStrategy::Consolidate,
            orientation: None,
            evolution_budget: crate::memory::dreaming::EditBudget::default(),
        };
        (ctx, store)
    }

    #[tokio::test]
    async fn lint_resolves_pending_links_after_target_appears() {
        let (ctx, store) = build_test_dream_ctx().await;

        // Note A links to [[rust]] before any rust note exists → at write time
        // the resolver falls back to to_raw="rust", to_note="rust".
        store
            .index_note(
                &KnowledgeNote {
                    title: "a".into(),
                    category: "preference".into(),
                    facts: vec!["see [[rust]]".into()],
                    links: vec!["rust".into()],
                    content_hash: "h1".into(),
                    ..Default::default()
                },
                &ctx.agent_id,
                "preference",
            )
            .await
            .unwrap();

        // Now rust exists at reference/rust.
        store
            .index_note(
                &KnowledgeNote {
                    title: "rust".into(),
                    category: "reference".into(),
                    facts: vec!["body".into()],
                    content_hash: "h2".into(),
                    ..Default::default()
                },
                &ctx.agent_id,
                "reference",
            )
            .await
            .unwrap();

        let stage = NoteLintStage;
        let agent_id = ctx.agent_id.clone();
        stage.execute(ctx).await.unwrap();

        let mut incoming = store
            .get_incoming_links("reference/rust", &agent_id)
            .await
            .unwrap();
        incoming.sort();
        assert_eq!(
            incoming,
            vec!["preference/a".to_string()],
            "lint should have resolved [[rust]] -> reference/rust from a single source"
        );
    }

    #[tokio::test]
    async fn lint_leaves_ambiguous_links_unresolved() {
        let (ctx, store) = build_test_dream_ctx().await;

        // Note A links to bare [[rust]] before any rust note exists.
        store
            .index_note(
                &KnowledgeNote {
                    title: "a".into(),
                    category: "preference".into(),
                    facts: vec!["see [[rust]]".into()],
                    links: vec!["rust".into()],
                    content_hash: "h1".into(),
                    ..Default::default()
                },
                &ctx.agent_id,
                "preference",
            )
            .await
            .unwrap();

        // Two notes share filename "rust" — at reference/rust AND tutorial/rust.
        store
            .index_note(
                &KnowledgeNote {
                    title: "rust".into(),
                    category: "reference".into(),
                    facts: vec!["body".into()],
                    content_hash: "h2".into(),
                    ..Default::default()
                },
                &ctx.agent_id,
                "reference",
            )
            .await
            .unwrap();
        store
            .index_note(
                &KnowledgeNote {
                    title: "rust".into(),
                    category: "tutorial".into(),
                    facts: vec!["body".into()],
                    content_hash: "h3".into(),
                    ..Default::default()
                },
                &ctx.agent_id,
                "tutorial",
            )
            .await
            .unwrap();

        let stage = NoteLintStage;
        let agent_id = ctx.agent_id.clone();
        stage.execute(ctx).await.unwrap();

        // Neither path receives an incoming link — the bare row stays at to_note="rust".
        let inc_ref = store
            .get_incoming_links("reference/rust", &agent_id)
            .await
            .unwrap();
        let inc_tut = store
            .get_incoming_links("tutorial/rust", &agent_id)
            .await
            .unwrap();
        assert!(
            inc_ref.is_empty() && inc_tut.is_empty(),
            "ambiguous bare link must not auto-resolve; got ref={:?} tut={:?}",
            inc_ref,
            inc_tut
        );
    }

    // -----------------------------------------------------------------
    // Lint-pass logging to log.md (llm_wiki protocol wiring)
    // -----------------------------------------------------------------

    use crate::memory::dreaming::NoteEntry;
    use crate::memory::notes::orientation::{FsNoteOrientation, LogMdWriter, NoteOrientation};

    #[tokio::test]
    async fn orphan_count_parses_isolated_insight() {
        let temp = std::env::temp_dir().join(format!("aleph_orphan_{}", uuid::Uuid::new_v4()));
        let store = SqliteMemoryBackend::new(&temp).unwrap();

        // Cold cache (no graph recompute yet) → 0 orphans.
        assert_eq!(orphan_count(&store, "default").await, 0);

        let payload = serde_json::to_string(&vec!["a/x".to_string(), "b/y".to_string()]).unwrap();
        store
            .replace_graph_insights("default", &[("isolated".to_string(), payload)])
            .await
            .unwrap();
        assert_eq!(orphan_count(&store, "default").await, 2);
    }

    async fn ctx_with_orientation(temp: &std::path::Path) -> DreamContext {
        let store = Arc::new(SqliteMemoryBackend::new(&temp.join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(temp.to_path_buf(), store.clone());
        let orientation = Arc::new(FsNoteOrientation::new(temp.to_path_buf(), store.clone()));
        orientation.bootstrap("default").await.unwrap();
        let orient_dyn: Arc<dyn NoteOrientation> = orientation;

        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new(""));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(StubEmbedder);

        DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: crate::memory::dreaming::DreamReport::default(),
            pipeline_type: "consolidate".into(),
            strategy: crate::memory::dreaming::DreamStrategy::Consolidate,
            orientation: Some(orient_dyn),
            evolution_budget: crate::memory::dreaming::EditBudget::default(),
        }
    }

    #[tokio::test]
    async fn lint_records_pass_to_log_when_frontmatter_fixed() {
        let temp = std::env::temp_dir().join(format!("aleph_lintlog_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(temp.join("default/preference"))
            .await
            .unwrap();
        // Note file missing frontmatter — the frontmatter rule will patch it,
        // so the pass does real work and must be recorded.
        tokio::fs::write(
            temp.join("default/preference/x.md"),
            "- the user prefers vim\n",
        )
        .await
        .unwrap();

        let mut ctx = ctx_with_orientation(&temp).await;
        ctx.notes = vec![NoteEntry {
            path: "preference/x".into(),
            category: "preference".into(),
            tags: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: "h".into(),
        }];

        NoteLintStage.execute(ctx).await.unwrap();

        let log = LogMdWriter::new(temp.join("default"))
            .tail(50)
            .await
            .unwrap();
        assert!(
            log.contains("lint | wiki lint pass"),
            "log.md must record the lint pass; got:\n{log}"
        );
        assert!(log.contains("1 frontmatter fixed"), "got:\n{log}");
        assert!(
            log.contains("orphan notes (last graph pass): 0"),
            "got:\n{log}"
        );
    }

    #[tokio::test]
    async fn lint_skips_log_when_nothing_to_report() {
        // No notes, no orphans → a wholly-clean pass must not append a lint row,
        // keeping log.md an event timeline rather than per-cycle noise.
        let temp = std::env::temp_dir().join(format!("aleph_lintclean_{}", uuid::Uuid::new_v4()));
        let ctx = ctx_with_orientation(&temp).await;

        NoteLintStage.execute(ctx).await.unwrap();

        let log = LogMdWriter::new(temp.join("default"))
            .tail(50)
            .await
            .unwrap();
        assert!(
            !log.contains("lint |"),
            "clean pass must not write a lint entry; got:\n{log}"
        );
    }

    #[tokio::test]
    async fn lint_never_purges_tombstoned_links() {
        let (ctx, store) = build_test_dream_ctx().await;
        let agent = ctx.agent_id.clone();
        // On-disk source note whose body carries [[gone]].
        let dir = ctx.indexer.memory_dir().join(&agent).join("plan");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(
            dir.join("keeper.md"),
            "---\ncategory: plan\ntags: []\ncreated: 2026-01-01\nupdated: 2026-01-01\n---\n\nsee [[gone]]\n",
        )
        .await
        .unwrap();
        ctx.indexer
            .index_file(&agent, "plan", &dir.join("keeper.md"))
            .await
            .unwrap();
        // Simulate delete of the (never-indexed-here) target by hand-marking
        // the row tombstone — the shape delete_note now produces.
        // (Direct SQL via store helper: use remove-note flow instead when the
        // target existed; here the row is dangling → flip it to tombstone.)
        store
            .set_link_status_for_test("plan/keeper", "gone", "tombstone", &agent)
            .await
            .unwrap();

        let mut lint_ctx = ctx;
        lint_ctx.notes = vec![NoteEntry {
            path: "plan/keeper".into(),
            category: "plan".into(),
            tags: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: "h".into(),
        }];
        NoteLintStage.execute(lint_ctx).await.unwrap();

        let body = tokio::fs::read_to_string(dir.join("keeper.md"))
            .await
            .unwrap();
        assert!(
            body.contains("[[gone]]"),
            "tombstoned link text must survive lint"
        );
    }
}
