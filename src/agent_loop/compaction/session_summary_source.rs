//! Zero-cost compaction via existing session summaries.
//!
//! When the context window fills up, instead of making a side-channel LLM API
//! call for summarization, this module checks whether `SessionCompactor` has
//! already written hierarchical summaries (d0/d1/d2) to SQLite. If so, those
//! are assembled within a token budget and used directly — zero API cost.

use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::MemoryBackend;
use crate::providers::message::UnifiedMessage;

use super::super::context_compactor::{CompactResult, CompactStrategy};

/// Source of zero-cost compaction from pre-existing session summaries.
///
/// Checks SQLite for `aleph://session/<session_id>/d*/…` facts and
/// assembles them into a single summary message within a token budget.
pub struct SessionSummarySource {
    database: MemoryBackend,
    session_id: String,
}

impl SessionSummarySource {
    /// Create a new source for the given session.
    pub fn new(database: MemoryBackend, session_id: impl Into<String>) -> Self {
        Self {
            database,
            session_id: session_id.into(),
        }
    }

    /// Try to reuse existing session summaries stored in SQLite.
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
            .get_raw_by_path_prefix(&path_prefix, "default", 50)
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
            "[Context Summary (from session memory)]\n{}",
            assembled
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

/// Estimate token count using the 3.5 chars/token heuristic.
fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() as f64 / 3.5).ceil() as usize
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
}
