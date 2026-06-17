//! `CorpusNarrative` stage — LLM-maintained overview.md + purpose.md.
//!
//! Reads the index + recent note previews + current overview/purpose, then asks
//! the LLM to (re)write a global synthesis (overview) and refine the corpus
//! purpose. Runs on the high-growth Synthesize path. R7/R9: the synthesis is
//! LLM semantic generation, not deterministic substitution.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::orientation::{OverviewMd, PurposeMd};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::DreamStage;

/// Minimum notes before a corpus-level narrative is worth generating.
const MIN_NOTES_FOR_NARRATIVE: usize = 5;
/// How many recent notes to preview into the prompt.
const PREVIEW_NOTES: usize = 40;

pub struct CorpusNarrativeStage;

#[async_trait]
impl DreamStage for CorpusNarrativeStage {
    fn name(&self) -> &'static str {
        "corpus_narrative"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.notes.len() >= MIN_NOTES_FOR_NARRATIVE
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // agent vault dir = memory_dir/{agent_id}
        let agent_dir = ctx.indexer.memory_dir().join(&ctx.agent_id);
        let overview_gen = OverviewMd::new(&agent_dir);
        let purpose_gen = PurposeMd::new(&agent_dir);
        let cur_overview = overview_gen.read().await;
        let cur_purpose = purpose_gen.read().await;

        // Recent-note previews (most-recently-updated first).
        let mut idx: Vec<usize> = (0..ctx.notes.len()).collect();
        idx.sort_by_key(|&i| std::cmp::Reverse(ctx.notes[i].updated_at));
        idx.truncate(PREVIEW_NOTES);
        let mut previews = Vec::new();
        for &i in &idx {
            let path = ctx.notes[i].path.clone();
            let cat = ctx.notes[i].category.clone();
            let content = ctx.load_content(&path).await.unwrap_or_default();
            let preview: String = content.chars().take(160).collect();
            previews.push(format!("- {path} ({cat}): {preview}"));
        }

        let system = "You maintain a personal knowledge vault. Produce two sections separated by a line containing exactly '===PURPOSE==='. \
First an OVERVIEW: a tight global synthesis (5-10 sentences) of what the corpus is about, its major themes, and how they connect. \
Then a PURPOSE: 3-6 bullet points capturing why this vault exists — the owner's goals and the key questions it should answer. \
If a current purpose is given, refine it minimally rather than rewriting wholesale.";
        let prompt = format!(
            "Current overview:\n{cur_overview}\n\nCurrent purpose:\n{cur_purpose}\n\nRecent notes ({} shown):\n{}\n\nWrite the new OVERVIEW, then '===PURPOSE===', then the PURPOSE.",
            previews.len(), previews.join("\n")
        );

        let msgs = vec![UnifiedMessage::user(&prompt)];
        let response = ctx
            .provider
            .process(RequestPayload::new(&msgs).with_system(Some(system)))
            .await
            .map_err(|e| AlephError::other(format!("corpus narrative LLM call failed: {e}")))?;
        let text = response.text_content();

        // Split on the sentinel; fall back to overview-only if absent.
        let (overview_body, purpose_body) = match text.split_once("===PURPOSE===") {
            Some((o, p)) => (o.trim().to_string(), p.trim().to_string()),
            None => (text.trim().to_string(), String::new()),
        };
        if !overview_body.is_empty() {
            overview_gen.write(&overview_body).await?;
        }
        // Idempotent purpose: only rewrite when the model produced a non-empty,
        // materially different body (avoid churn on every synthesize cycle).
        if !purpose_body.is_empty() && purpose_body != cur_purpose.trim() {
            purpose_gen.write(&purpose_body).await?;
        }
        tracing::info!(notes = ctx.notes.len(), "corpus narrative regenerated");
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn split_sentinel_separates_overview_and_purpose() {
        let text = "Overview body here.\n===PURPOSE===\n- goal one\n- goal two";
        let (o, p) = text.split_once("===PURPOSE===").unwrap();
        assert!(o.trim().ends_with("here."));
        assert!(p.contains("goal one"));
    }
    #[test]
    fn missing_sentinel_falls_back_to_overview_only() {
        let text = "Just an overview, no purpose marker.";
        assert!(text.split_once("===PURPOSE===").is_none());
    }
}
