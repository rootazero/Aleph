//! MicrocompactStage — content-addressed dedup for tool results.
//!
//! When the same tool produces identical content multiple times in a conversation,
//! older occurrences are replaced with compact references, freeing tokens for
//! more useful context.

use std::collections::HashMap;

use async_trait::async_trait;

use super::preflight::PreflightStage;
use super::pressure::estimate_tokens_smart;
use super::ContextPressure;
use crate::memory::session_compactor::context_window::partition_fresh_tail;
use crate::providers::message::UnifiedMessage;

/// Pressure ratio below which the stage is a no-op.
const ACTIVATION_THRESHOLD: f64 = 0.3;

/// FNV-1a hash — lightweight, no external crate needed.
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Tracks the newest index for a given key hash.
///
/// The key already contains the content hash, so we only need to track
/// the newest occurrence index for deduplication.
struct CacheEntry {
    newest_index: usize,
}

/// Deduplicates identical tool results, replacing older duplicates with compact
/// references that preserve provenance while freeing tokens.
pub struct MicrocompactStage;

impl Default for MicrocompactStage {
    fn default() -> Self {
        Self
    }
}

impl MicrocompactStage {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PreflightStage for MicrocompactStage {
    fn name(&self) -> &'static str {
        "microcompact"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        if pressure.ratio < ACTIVATION_THRESHOLD {
            return 0;
        }

        let partition = partition_fresh_tail(messages, fresh_tail_count);
        if partition == 0 {
            return 0;
        }

        // --- Pass 1: scan tool results, track newest index per key ---
        // Key: (tool_name_hash, full_content_hash) — eliminates prefix collision risk
        let mut cache: HashMap<(u64, u64), CacheEntry> = HashMap::new();

        for (idx, msg) in messages[..partition].iter().enumerate() {
            if let Some((name, content)) = msg.tool_result_info() {
                let content_hash = simple_hash(&content);
                let key = (simple_hash(name), content_hash);

                let entry = cache.entry(key).or_insert(CacheEntry { newest_index: idx });

                // Update if this occurrence is newer (same key means same content)
                if idx > entry.newest_index {
                    entry.newest_index = idx;
                }
            }
        }

        // --- Pass 2: replace older duplicates with compact references ---
        let mut total_freed: usize = 0;

        for (idx, msg) in messages.iter_mut().enumerate().take(partition) {
            if !msg.is_tool_result() {
                continue;
            }

            // Extract info as owned values before mutating
            let (name, content) = match msg.tool_result_info() {
                Some((n, c)) => (n.to_owned(), c),
                None => continue,
            };

            let content_hash = simple_hash(&content);
            let key = (simple_hash(&name), content_hash);

            if let Some(entry) = cache.get(&key) {
                if idx < entry.newest_index {
                    let old_tokens = estimate_tokens_smart(&content);
                    let replacement = format!(
                        "[cached: {name} result, {old_tokens} tokens, same content appears later]"
                    );
                    let new_tokens = estimate_tokens_smart(&replacement);
                    msg.replace_tool_result_content(replacement);
                    total_freed += old_tokens.saturating_sub(new_tokens);
                }
            }
        }

        total_freed
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn high_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 800,
            budget_tokens: 1000,
            ratio: 0.8,
            overhead_tokens: 100,
            available_for_messages: 900,
        }
    }

    fn low_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 200,
            budget_tokens: 1000,
            ratio: 0.2,
            overhead_tokens: 100,
            available_for_messages: 900,
        }
    }

    #[tokio::test]
    async fn deduplicates_identical_tool_results() {
        let stage = MicrocompactStage::new();
        let long_content = "x".repeat(200);

        let mut messages = vec![
            UnifiedMessage::tool_result("call1", "Read", long_content.clone(), false),
            UnifiedMessage::assistant("I see the content"),
            UnifiedMessage::tool_result("call2", "Read", long_content.clone(), false),
            UnifiedMessage::assistant("Same content again"),
        ];

        let freed = stage.prepare(&mut messages, &high_pressure(), 0).await;

        // The older (index 0) should be replaced with a compact reference
        assert!(freed > 0, "should have freed tokens, got {freed}");

        let (_, content0) = messages[0].tool_result_info().unwrap();
        assert!(
            content0.contains("[cached:"),
            "older result should be replaced, got: {content0}"
        );

        // The newer (index 2) should be untouched
        let (_, content2) = messages[2].tool_result_info().unwrap();
        assert_eq!(content2, long_content, "newer result should be preserved");
    }

    #[tokio::test]
    async fn skips_at_low_pressure() {
        let stage = MicrocompactStage::new();
        let content = "same content here".to_string();

        let mut messages = vec![
            UnifiedMessage::tool_result("call1", "Read", content.clone(), false),
            UnifiedMessage::tool_result("call2", "Read", content.clone(), false),
        ];

        let freed = stage.prepare(&mut messages, &low_pressure(), 0).await;
        assert_eq!(freed, 0, "should not fire at low pressure");

        // Both should be untouched
        let (_, c0) = messages[0].tool_result_info().unwrap();
        let (_, c1) = messages[1].tool_result_info().unwrap();
        assert_eq!(c0, content);
        assert_eq!(c1, content);
    }

    #[tokio::test]
    async fn preserves_different_content() {
        let stage = MicrocompactStage::new();

        let mut messages = vec![
            UnifiedMessage::tool_result("call1", "Read", "content A".to_string(), false),
            UnifiedMessage::assistant("got it"),
            UnifiedMessage::tool_result("call2", "Read", "content B".to_string(), false),
            UnifiedMessage::assistant("different"),
        ];

        let freed = stage.prepare(&mut messages, &high_pressure(), 0).await;
        assert_eq!(freed, 0, "different content should not be deduped");

        let (_, c0) = messages[0].tool_result_info().unwrap();
        let (_, c2) = messages[2].tool_result_info().unwrap();
        assert_eq!(c0, "content A");
        assert_eq!(c2, "content B");
    }
}
