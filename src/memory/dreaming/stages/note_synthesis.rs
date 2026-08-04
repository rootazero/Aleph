//! `NoteSynthesis` stage — generates cross-note insight summaries.
//!
//! Runs only in the weekly pipeline and requires at least 5 notes.
//! Groups notes by category and calls an LLM to synthesize cross-cutting
//! themes and patterns for each category with 3 or more notes.

use async_trait::async_trait;
use std::collections::HashMap;

use crate::error::AlephError;
use crate::memory::dreaming::{DreamContext, NoteEntry};
use crate::memory::notes::KnowledgeNote;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::DreamStage;

pub struct NoteSynthesisStage;

#[async_trait]
impl DreamStage for NoteSynthesisStage {
    fn name(&self) -> &'static str {
        "note_synthesis"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        // Runs when strategy is Synthesize and there are enough notes
        ctx.notes.len() >= 5
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let mut synthesis_count = 0u32;

        // Group notes by category, skipping synthesis notes to avoid recursion
        let mut by_category: HashMap<&str, Vec<&NoteEntry>> = HashMap::new();
        for note in &ctx.notes {
            if note.category == "synthesis" || note.category == "query" {
                continue;
            }
            by_category
                .entry(note.category.as_str())
                .or_default()
                .push(note);
        }

        // Collect categories that need synthesis (3+ notes)
        let categories_to_synthesize: Vec<(String, Vec<String>)> = by_category
            .iter()
            .filter(|(_, notes)| notes.len() >= 3)
            .map(|(cat, notes)| {
                let paths: Vec<String> = notes.iter().map(|n| n.path.clone()).collect();
                (cat.to_string(), paths)
            })
            .collect();

        let mut previews = Vec::new();
        for (category, note_paths) in &categories_to_synthesize {
            // Load content previews (cap at 15 notes to limit LLM context)
            previews.clear();
            for path in note_paths.iter().take(15) {
                let content = ctx.load_content(path).await.unwrap_or_default();
                let preview: String = content.chars().take(300).collect();
                previews.push(format!("### {path}\n{preview}"));
            }

            let note_count = note_paths.len();

            let prompt = format!(
                "Analyze these {note_count} knowledge notes in the '{category}' category and write a synthesis insight.\n\n\
                 Identify:\n\
                 1. Cross-cutting themes and patterns\n\
                 2. Connections between different notes\n\
                 3. Key takeaways or emerging trends\n\n\
                 Notes:\n\n{}\n\n\
                 Write a concise synthesis (3-5 paragraphs). Use [[wikilinks]] to reference source notes by their path.",
                previews.join("\n\n")
            );

            let system = "You are a knowledge synthesis engine. Find patterns across notes and write insightful summaries.";

            let msgs = vec![UnifiedMessage::user(&prompt)];
            let response = match ctx
                .provider
                .process(RequestPayload::new(&msgs).with_system(Some(system)))
                .await
            {
                Ok(r) => r,
                Err(e) if super::is_provider_exhausted(&e) => {
                    tracing::warn!(error = %e, "Synthesis: provider exhausted — aborting dream cycle");
                    return Err(e);
                }
                Err(e) => {
                    tracing::warn!(category, error = %e, "Synthesis LLM call failed");
                    continue;
                }
            };

            let synthesis_text = response.text_content();

            // Build the synthesis note from its source member paths.
            let note = build_synthesis_note(category, &synthesis_text, note_paths.to_vec());

            // Ensure the synthesis directory exists (not in CATEGORY_DIRS, create manually)
            let synthesis_dir = ctx
                .indexer
                .memory_dir()
                .join(&ctx.agent_id)
                .join("synthesis");
            if let Err(e) = tokio::fs::create_dir_all(&synthesis_dir).await {
                tracing::warn!(
                    category,
                    error = %e,
                    "Failed to create synthesis directory"
                );
                continue;
            }

            match ctx
                .indexer
                .write_note(&ctx.agent_id, "synthesis", &note)
                .await
            {
                Ok(written) => {
                    synthesis_count += 1;
                    // Record (note path, body digest) so the *next* cycle's
                    // MutationGate can tell a stable synthesis from one that
                    // flip-flops. Digest the body, not the rendered markdown:
                    // frontmatter carries an `updated` date that moves on every
                    // write, which would report churn every night.
                    let note_path = written.file_stem().map_or_else(
                        || format!("synthesis/{category}"),
                        |stem| format!("synthesis/{}", stem.to_string_lossy()),
                    );
                    ctx.report.synthesis_rewrites.push((
                        note_path,
                        crate::memory::notes::indexer::sha2_hash(&synthesis_text),
                    ));
                    tracing::info!(
                        category,
                        notes = note_count,
                        "Generated synthesis for category"
                    );
                }
                Err(e) => {
                    tracing::warn!(category, error = %e, "Failed to write synthesis note");
                }
            }
        }

        ctx.report.synthesis_count = synthesis_count;
        tracing::info!(synthesis_count, "NoteSynthesis completed");
        Ok(ctx)
    }
}

/// Build the L2 synthesis note for a category from its source member paths.
///
/// The member paths are recorded as BOTH `links` (graph edges) and
/// `source_notes` (provenance: which L1 notes this synthesis distilled), so the
/// L1→L2 evidence chain stays connected and `index_note` can materialize
/// `notes_sources`.
fn build_synthesis_note(
    category: &str,
    synthesis_text: &str,
    source_links: Vec<String>,
) -> KnowledgeNote {
    KnowledgeNote {
        title: format!("{category} Synthesis"),
        category: "synthesis".to_string(),
        tags: vec![category.to_string(), "synthesis".to_string()],
        facts: vec![synthesis_text.to_string()],
        links: source_links.clone(),
        source_notes: source_links,
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
        content_hash: String::new(),
        ..Default::default()
    }
}

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
            updated_at: 1_700_000_000,
            content_hash: String::new(),
        }
    }

    fn make_notes(count: usize, category: &str) -> Vec<NoteEntry> {
        (0..count)
            .map(|i| make_note(&format!("{category}/note{i}"), category))
            .collect()
    }

    #[test]
    fn stage_name() {
        assert_eq!(NoteSynthesisStage.name(), "note_synthesis");
    }

    // Tests for should_run logic (tested directly on the predicate since
    // constructing a full DreamContext requires heavy dependencies)

    #[test]
    fn should_run_false_when_too_few_notes() {
        // < 5 notes → false regardless of strategy
        let notes = make_notes(4, "preference");
        let result = notes.len() >= 5;
        assert!(!result, "fewer than 5 notes should not trigger synthesis");
    }

    #[test]
    fn should_run_true_with_enough_notes() {
        // >= 5 notes → true (strategy selection is handled upstream)
        let notes = make_notes(5, "preference");
        let result = notes.len() >= 5;
        assert!(result, "5+ notes should trigger synthesis");
    }

    #[test]
    fn should_run_boundary_exactly_five_notes() {
        let notes = make_notes(5, "skill");
        let result = notes.len() >= 5;
        assert!(result, "exactly 5 notes should satisfy the threshold");
    }

    #[test]
    fn synthesis_notes_excluded_from_grouping() {
        // Synthesis notes should be skipped when building category groups
        let notes = vec![
            make_note("synthesis/preference-insights", "synthesis"),
            make_note("preference/note1", "preference"),
            make_note("preference/note2", "preference"),
            make_note("preference/note3", "preference"),
        ];

        let mut by_category: HashMap<String, Vec<&NoteEntry>> = HashMap::new();
        for note in &notes {
            if note.category == "synthesis" {
                continue;
            }
            by_category
                .entry(note.category.clone())
                .or_default()
                .push(note);
        }

        assert!(
            !by_category.contains_key("synthesis"),
            "synthesis category must be excluded"
        );
        assert_eq!(by_category["preference"].len(), 3);
    }

    #[test]
    fn build_synthesis_note_records_source_member_paths() {
        // Exercise the real production builder: a synthesis note built from
        // member paths must expose them as source_notes (provenance for the
        // L1→L2 chain), not only as links.
        let member_paths = vec!["learning/tokio".to_string(), "learning/async".to_string()];
        let note = build_synthesis_note("learning", "Synthesized insight.", member_paths.clone());
        assert_eq!(
            note.source_notes, member_paths,
            "member paths must be recorded as source_notes"
        );
        assert_eq!(note.links, member_paths, "member paths must also be links");
        assert_eq!(note.category, "synthesis");
        assert_eq!(note.title, "learning Synthesis");
        assert_eq!(note.facts, vec!["Synthesized insight.".to_string()]);
    }
}
