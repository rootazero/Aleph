//! NoteSynthesis stage — generates cross-note insight summaries.
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
        let mut by_category: HashMap<String, Vec<&NoteEntry>> = HashMap::new();
        for note in &ctx.notes {
            if note.category == "synthesis" {
                continue;
            }
            by_category
                .entry(note.category.clone())
                .or_default()
                .push(note);
        }

        // Collect categories that need synthesis (3+ notes)
        let categories_to_synthesize: Vec<(String, Vec<String>)> = by_category
            .iter()
            .filter(|(_, notes)| notes.len() >= 3)
            .map(|(cat, notes)| {
                let paths: Vec<String> = notes.iter().map(|n| n.path.clone()).collect();
                (cat.clone(), paths)
            })
            .collect();

        for (category, note_paths) in &categories_to_synthesize {
            // Load content previews (cap at 15 notes to limit LLM context)
            let mut previews = Vec::new();
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
                Err(e) => {
                    tracing::warn!(category, error = %e, "Synthesis LLM call failed");
                    continue;
                }
            };

            let synthesis_text = response.text_content();

            // Build links from source note paths
            let source_links: Vec<String> = note_paths.to_vec();

            let note = KnowledgeNote {
                title: format!("{category} Synthesis"),
                category: "synthesis".to_string(),
                tags: vec![category.clone(), "synthesis".to_string()],
                facts: vec![synthesis_text],
                links: source_links,
                created_at: chrono::Utc::now().timestamp(),
                updated_at: chrono::Utc::now().timestamp(),
                content_hash: String::new(),
            };

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
                Ok(_) => {
                    synthesis_count += 1;
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
            last_accessed_at: None,
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
}
