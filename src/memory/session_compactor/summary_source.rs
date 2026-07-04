//! Zero-cost compaction via existing session summaries.
//!
//! When the context window fills up, instead of making a side-channel LLM API
//! call for summarization, this module checks whether `SessionCompactor` has
//! already written hierarchical summaries (d0/d1/d2) to `SQLite`. If so, those
//! are assembled within a token budget and used directly — zero API cost.

use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::MemoryBackend;
use crate::providers::message::UnifiedMessage;

use crate::context::compact::compactor::{CompactResult, CompactStrategy};

/// Source of zero-cost compaction from pre-existing session summaries.
///
/// Checks `SQLite` for `aleph://session/<session_id>/d*/…` facts and
/// assembles them into a single summary message within a token budget.
pub struct SessionSummarySource {
    database: MemoryBackend,
    session_id: String,
    /// Agent that owns the session summaries. `get_raw_by_path_prefix` filters
    /// `WHERE agent_id = ?`, so this must be the real owning agent id — the
    /// same one `SessionCompactor::post_turn_compress` writes the d0/d1/d2
    /// facts under. A wrong (or placeholder) value matches nothing.
    agent_id: String,
}

impl SessionSummarySource {
    /// Create a new source for the given session and owning agent.
    pub fn new(
        database: MemoryBackend,
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            database,
            session_id: session_id.into(),
            agent_id: agent_id.into(),
        }
    }

    /// Try to reuse existing session summaries stored in `SQLite`.
    ///
    /// Returns `Some(CompactResult)` if usable summaries were found and the
    /// window was successfully replaced. Returns `None` if no summaries exist
    /// or the assembled text is empty, in which case the caller should fall
    /// back to the normal LLM-based compaction path.
    pub async fn try_reuse(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        window_start: usize,
        cut_end: usize,
    ) -> Option<CompactResult> {
        let path_prefix = format!("aleph://session/{}/", self.session_id);

        let mut raws = self
            .database
            .get_raw_by_path_prefix(&path_prefix, &self.agent_id, 50)
            .await
            .ok()?;

        if raws.is_empty() {
            return None;
        }

        // Sort highest-depth-first (d2 > d1 > d0) so the most condensed
        // summaries are preferred when the token budget is tight.
        raws.sort_by(|a, b| {
            let path_a = a.path.as_deref().unwrap_or("");
            let path_b = b.path.as_deref().unwrap_or("");
            extract_depth(path_b)
                .cmp(&extract_depth(path_a))
                // Secondary sort: newest first within the same depth.
                .then(b.created_at.cmp(&a.created_at))
        });

        let cut_end = cut_end.min(messages.len());
        if window_start >= cut_end {
            return None;
        }

        // Estimate the token cost of the window being replaced.
        let window_text: String = messages[window_start..cut_end]
            .iter()
            .map(|m| m.text_content())
            .collect::<Vec<_>>()
            .join("\n");
        let tokens_before = estimate_tokens(&window_text);

        // Allow 50 % of the original token count.
        let budget = tokens_before / 2;

        let mut assembled = String::new();
        let mut used_tokens: usize = 0;

        for raw in &raws {
            let fact_tokens = estimate_tokens(&raw.content);
            if used_tokens + fact_tokens > budget && !assembled.is_empty() {
                break;
            }
            if !assembled.is_empty() {
                assembled.push_str("\n\n");
            }
            assembled.push_str(&raw.content);
            used_tokens += fact_tokens;
        }

        if assembled.is_empty() {
            return None;
        }

        let summary_msg = UnifiedMessage::user(format!(
            "[Context Summary (from session memory)]\n{assembled}"
        ));

        messages.drain(window_start..cut_end);
        messages.insert(window_start, summary_msg);

        Some(CompactResult {
            tokens_before,
            tokens_after: used_tokens,
            strategy_used: CompactStrategy::SessionMemoryReuse,
        })
    }
}

// === Private helpers ===

/// Extract the numeric depth from a path segment beginning with `'d'`.
///
/// For example:
/// - `"aleph://session/abc/d2/1"` → `2`
/// - `"aleph://session/abc/d0/3"` → `0`
/// - `"invalid/path"` → `0`
fn extract_depth(path: &str) -> u32 {
    path.split('/')
        .find_map(|seg| {
            seg.strip_prefix('d')
                .and_then(|rest| rest.parse::<u32>().ok())
        })
        .unwrap_or(0)
}

/// Estimate token count using content-aware ratio detection.
///
/// Thin alias for [`crate::context::budget::pressure::estimate_tokens_smart`]
/// — the single source of truth for the prose-anchored, CJK/code-aware
/// char→token estimate. Kept as a local name so call sites read clearly.
fn estimate_tokens(text: &str) -> usize {
    crate::context::budget::pressure::estimate_tokens_smart(text)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_depth_parses_d0() {
        assert_eq!(extract_depth("aleph://session/abc/d0/3"), 0);
    }

    #[test]
    fn extract_depth_parses_d2() {
        assert_eq!(extract_depth("aleph://session/abc/d2/1"), 2);
    }

    #[test]
    fn extract_depth_returns_0_for_invalid() {
        assert_eq!(extract_depth("invalid/path"), 0);
    }

    #[tokio::test]
    async fn try_reuse_finds_summaries_for_the_owning_agent() {
        use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        let backend: MemoryBackend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        // Seed a d0 session summary the way `post_turn_compress` writes it.
        let raw = RawMemory::new(
            "Earlier turns: set up the project and ran the tests.".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("agent-x")
        .with_session("sess-1")
        .with_path("aleph://session/sess-1/d0/0");
        backend.insert_raw_memory(&raw).await.unwrap();

        let source = SessionSummarySource::new(backend, "sess-1", "agent-x");
        let mut messages = vec![
            UnifiedMessage::user("old one"),
            UnifiedMessage::assistant("old two"),
            UnifiedMessage::user("fresh tail"),
        ];
        let result = source
            .try_reuse(&mut messages, 0, 2)
            .await
            .expect("matching agent + session must reuse the stored summary");
        assert_eq!(result.strategy_used, CompactStrategy::SessionMemoryReuse);
        // window [0,2) collapses into one summary message; the fresh tail stays.
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn try_reuse_returns_none_for_a_different_agent() {
        use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        let backend: MemoryBackend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        let raw = RawMemory::new("summary".to_string(), RawMemorySource::SessionCompressed)
            .with_agent("agent-x")
            .with_session("sess-1")
            .with_path("aleph://session/sess-1/d0/0");
        backend.insert_raw_memory(&raw).await.unwrap();

        // Same session id, wrong owning agent — the agent_id filter must
        // exclude it. Regression guard for the old hardcoded "default".
        let source = SessionSummarySource::new(backend, "sess-1", "other-agent");
        let mut messages = vec![
            UnifiedMessage::user("old one"),
            UnifiedMessage::assistant("old two"),
            UnifiedMessage::user("fresh tail"),
        ];
        assert!(source.try_reuse(&mut messages, 0, 2).await.is_none());
    }

    #[test]
    fn estimate_tokens_is_cjk_aware() {
        // 纯 CJK：内容感知估算按 ~1.5 chars/token 计，token 数应显著高于
        // 平坦 3.5 的旧估算。50 个汉字 → 智能估算 ≈ 34，旧平坦估算 ≈ 15。
        let cjk = "记".repeat(50);
        let tokens = estimate_tokens(&cjk);
        assert!(
            tokens >= 30,
            "CJK text must be charged at the dense ratio, got {tokens}"
        );
    }
}
