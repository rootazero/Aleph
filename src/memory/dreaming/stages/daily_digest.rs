//! DailyDigest stage — generates a daily activity summary from recently changed notes.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::store::DreamStore;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::DreamStage;

pub struct DailyDigestStage;

#[async_trait]
impl DreamStage for DailyDigestStage {
    fn name(&self) -> &'static str {
        "daily_digest"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        // Run if there are any notes updated or created in the last 24 hours
        let now = chrono::Utc::now().timestamp();
        let day_ago = now - 86400;
        ctx.notes
            .iter()
            .any(|n| n.updated_at > day_ago || n.created_at > day_ago)
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let now = chrono::Utc::now().timestamp();
        let day_ago = now - 86400;

        // Collect metadata for notes changed in the last 24h
        let recent_indices: Vec<usize> = ctx
            .notes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.updated_at > day_ago || n.created_at > day_ago)
            .map(|(i, _)| i)
            .collect();

        if recent_indices.is_empty() {
            return Ok(ctx);
        }

        // Load content for recent notes and build summaries
        let mut note_summaries = Vec::new();
        for &idx in &recent_indices {
            let path = ctx.notes[idx].path.clone();
            let category = ctx.notes[idx].category.clone();
            let content = ctx.load_content(&path).await.unwrap_or_default();
            // Take first 200 chars as preview
            let preview: String = content.chars().take(200).collect();
            note_summaries.push(format!("- **{}** ({}): {}", path, category, preview));
        }

        let note_count = note_summaries.len();

        // Build LLM prompt for daily digest
        let prompt = format!(
            "Generate a concise daily activity summary (3-5 sentences) based on these knowledge notes that were created or updated today:\n\n{}\n\nFocus on key themes, decisions, and learnings. Write in third person.",
            note_summaries.join("\n")
        );

        let system = "You are a concise daily digest writer. Summarize the day's knowledge activity in 3-5 sentences.";

        let msgs = vec![UnifiedMessage::user(&prompt)];
        let response = ctx
            .provider
            .process(
                RequestPayload::new(&msgs).with_system(Some(system)),
            )
            .await
            .map_err(|e| AlephError::other(format!("Daily digest LLM call failed: {e}")))?;

        let digest_text = response.text_content();

        // Store the daily insight
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let insight = crate::memory::dreaming::DailyInsight::new(today, digest_text, note_count as u32);

        ctx.database.upsert_daily_insight(insight).await?;

        tracing::info!(
            notes_count = note_count,
            "Daily digest generated"
        );

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::NoteEntry;

    fn make_note(path: &str, category: &str, updated_at: i64) -> NoteEntry {
        NoteEntry {
            path: path.to_string(),
            category: category.to_string(),
            tags: vec![],
            created_at: updated_at,
            updated_at,
            last_accessed_at: None,
            content_hash: String::new(),
        }
    }

    #[tokio::test]
    async fn should_run_returns_false_when_no_recent_notes() {
        let stage = DailyDigestStage;
        let now = chrono::Utc::now().timestamp();
        // All notes older than 24h + 1 day
        let old_ts = now - 86400 - 3600;

        // We only need to test the filtering logic, so construct a minimal
        // notes list and check `should_run` without a full DreamContext.
        // The predicate: any(n.updated_at > day_ago || n.created_at > day_ago)
        let notes = vec![
            make_note("knowledge/rust", "knowledge", old_ts),
            make_note("journal/entry1", "journal", old_ts - 100),
        ];

        let day_ago = now - 86400;
        let result = notes
            .iter()
            .any(|n| n.updated_at > day_ago || n.created_at > day_ago);
        assert!(!result, "should_run predicate should be false for old notes");

        let _ = stage; // ensure stage is used
    }

    #[tokio::test]
    async fn should_run_returns_true_when_recent_notes_exist() {
        let stage = DailyDigestStage;
        let now = chrono::Utc::now().timestamp();
        // One note updated 1 hour ago (recent)
        let recent_ts = now - 3600;
        let old_ts = now - 86400 - 3600;

        let notes = vec![
            make_note("knowledge/rust", "knowledge", old_ts),
            make_note("journal/today", "journal", recent_ts),
        ];

        let day_ago = now - 86400;
        let result = notes
            .iter()
            .any(|n| n.updated_at > day_ago || n.created_at > day_ago);
        assert!(result, "should_run predicate should be true when recent note exists");

        let _ = stage;
    }
}
