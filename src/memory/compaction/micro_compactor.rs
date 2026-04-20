//! MicroCompactor — zero-LLM-cost tool output pruning via 3D compressibility scoring.
//!
//! Scores each tool result by age × size × importance and replaces the most
//! compressible entries with compact placeholders until context pressure drops
//! below the target threshold.

use std::future::Future;
use std::pin::Pin;

use crate::memory::compaction::types::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel, TokenEstimate,
};
use crate::agent_loop::tool_result_store::extract_persisted_ref;
use crate::memory::session_compactor::context_window::estimate_tokens;

// =============================================================================
// Importance
// =============================================================================

/// Semantic importance classification for a tool output.
///
/// Used as a penalty factor in compressibility scoring — higher importance means
/// lower compressibility (the output is harder to safely discard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Importance {
    /// Low-value outputs that are safe to prune: search results, file reads
    /// that have already been digested by the assistant.
    Low,
    /// General tool call outputs with moderate retention value.
    Medium,
    /// High-value outputs that must be retained: errors, user feedback,
    /// config or vault changes.
    High,
}

/// Classify the importance of a tool output based on tool name, content, and
/// how many turns ago the call was made.
///
/// Rules (highest precedence first):
/// 1. **High** — tool name contains a memory/config/vault/feedback keyword,
///    OR content contains an error/panic/failure keyword.
/// 2. **Low** — tool name is a read/search family AND `turn_age >= 5`.
/// 3. **Medium** — everything else.
pub fn classify_importance(tool_name: &str, content: &str, turn_age: u32) -> Importance {
    let name_lower = tool_name.to_ascii_lowercase();
    let content_lower = content.to_ascii_lowercase();

    // High-importance: memory/config/vault mutations or error outputs
    let high_name = name_lower.contains("memory")
        || name_lower.contains("config")
        || name_lower.contains("vault")
        || name_lower.contains("user_feedback");

    let high_content = content_lower.contains("error")
        || content_lower.contains("panic")
        || content_lower.contains("failure")
        || content_lower.contains("failed");

    if high_name || high_content {
        return Importance::High;
    }

    // Low-importance: read/search family that is old enough to have been digested
    let low_name = name_lower.contains("read_file")
        || name_lower.contains("search")
        || name_lower.contains("glob")
        || name_lower.contains("grep")
        || name_lower.contains("ls")
        || name_lower.contains("list_files")
        || name_lower.contains("web_fetch")
        || name_lower.contains("web_search");

    if low_name && turn_age >= 5 {
        return Importance::Low;
    }

    Importance::Medium
}

// =============================================================================
// ToolOutputEntry
// =============================================================================

/// A scored snapshot of a single tool result message.
///
/// `compressibility()` returns a value in `[0.0, 1.0]` — higher means more
/// beneficial to compact.
#[derive(Debug, Clone)]
pub struct ToolOutputEntry {
    /// How many turns ago this tool result occurred (0 = most recent turn).
    pub turn_age: u32,
    /// Estimated token size of the output content.
    pub token_size: usize,
    /// Semantic importance classification.
    pub importance: Importance,
    /// Name of the tool that produced this result.
    pub tool_name: String,
    /// Index of the message in the context's message list.
    pub message_index: usize,
}

impl ToolOutputEntry {
    /// Compute a compressibility score in `[0.0, 1.0]`.
    ///
    /// Formula:
    /// ```text
    /// score = (age_score * 0.4 + size_score * 0.4) * (1.0 - importance_penalty)
    /// ```
    /// where
    /// - `age_score  = min(turn_age / 20.0, 1.0)`
    /// - `size_score = min(token_size / 3000.0, 1.0)`
    /// - `importance_penalty` = 0.0 (Low) | 0.3 (Medium) | 0.7 (High)
    pub fn compressibility(&self) -> f64 {
        let age_score = (self.turn_age as f64 / 20.0).min(1.0);
        let size_score = (self.token_size as f64 / 3000.0).min(1.0);
        let importance_penalty = match self.importance {
            Importance::Low => 0.0,
            Importance::Medium => 0.3,
            Importance::High => 0.7,
        };
        (age_score * 0.4 + size_score * 0.4) * (1.0 - importance_penalty)
    }
}

// =============================================================================
// Placeholder formatting
// =============================================================================

/// Format a compact placeholder string that replaces a pruned tool output.
///
/// # Example output
/// ```text
/// [Tool output compacted: read_file]
/// - Size: 2500 tokens -> compacted
/// - Key fields: path, content, encoding
/// - Status: success
/// ```
pub fn format_compact_placeholder(
    tool_name: &str,
    original_tokens: usize,
    key_fields: Option<&[&str]>,
    success: bool,
    original_content: Option<&str>,
) -> String {
    let mut lines = vec![
        format!("[Tool output compacted: {tool_name}]"),
        format!("- Size: {original_tokens} tokens -> compacted"),
    ];

    if let Some(fields) = key_fields {
        if !fields.is_empty() {
            lines.push(format!("- Key fields: {}", fields.join(", ")));
        }
    }

    let status = if success { "success" } else { "error" };
    lines.push(format!("- Status: {status}"));

    if let Some(content) = original_content {
        if let Some(ref_line) = extract_persisted_ref(content) {
            lines.push(ref_line.to_string());
        }
    }

    lines.join("\n")
}

// =============================================================================
// MicroCompactorConfig
// =============================================================================

/// Configuration for [`MicroCompactor`].
#[derive(Debug, Clone)]
pub struct MicroCompactorConfig {
    /// Number of recent messages that must never be touched (the "fresh tail").
    pub fresh_tail_count: usize,
    /// Minimum compressibility score required for an entry to be a candidate.
    /// Entries below this threshold are skipped even if pressure is high.
    pub min_compressibility: f64,
}

impl Default for MicroCompactorConfig {
    fn default() -> Self {
        Self {
            fresh_tail_count: 6,
            min_compressibility: 0.1,
        }
    }
}

// =============================================================================
// MicroCompactor
// =============================================================================

/// Zero-LLM-cost compaction strategy that replaces old, large, low-importance
/// tool outputs with compact placeholders.
///
/// Scoring is purely deterministic (3D: age × size × importance), so it has
/// zero API cost and near-zero latency.
pub struct MicroCompactor {
    pub config: MicroCompactorConfig,
}

impl MicroCompactor {
    /// Create a new `MicroCompactor` with the given configuration.
    pub fn new(config: MicroCompactorConfig) -> Self {
        Self { config }
    }

    /// Scan messages in the compaction window, score each tool result, and
    /// return entries sorted by compressibility (highest first).
    ///
    /// Messages in the fresh tail are always skipped.
    pub fn scan_tool_outputs(&self, ctx: &CompactionContext) -> Vec<ToolOutputEntry> {
        let messages = &ctx.messages;
        let tail_start = if messages.len() <= self.config.fresh_tail_count {
            // All messages are fresh — nothing to scan.
            return Vec::new();
        } else {
            messages.len() - self.config.fresh_tail_count
        };

        let total_turns = messages.len() as u32;
        let ratio = ctx.token_estimate_ratio;

        let mut entries: Vec<ToolOutputEntry> = messages[..tail_start]
            .iter()
            .enumerate()
            .filter_map(|(idx, msg)| {
                let (tool_name, content) = msg.tool_result_info()?;
                let turn_age = total_turns.saturating_sub(idx as u32);
                let token_size = estimate_tokens(&content, ratio);
                let importance = classify_importance(tool_name, &content, turn_age);

                Some(ToolOutputEntry {
                    turn_age,
                    token_size,
                    importance,
                    tool_name: tool_name.to_owned(),
                    message_index: idx,
                })
            })
            .collect();

        // Sort highest compressibility first so execute() can prune greedily.
        entries.sort_by(|a, b| {
            b.compressibility()
                .partial_cmp(&a.compressibility())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        entries
    }
}

impl CompactionStrategy for MicroCompactor {
    fn name(&self) -> &str {
        "micro_compactor"
    }

    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate {
        let entries = self.scan_tool_outputs(ctx);
        let estimated_savings: usize = entries
            .iter()
            .filter(|e| e.compressibility() >= self.config.min_compressibility)
            .map(|e| {
                // Placeholder is ~10 tokens; savings = original - placeholder
                e.token_size.saturating_sub(10)
            })
            .sum();

        TokenEstimate {
            estimated_savings,
            confidence: 0.75,
        }
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a mut CompactionContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>> {
        Box::pin(async move {
            let pressure_before = ctx.pressure.ratio;
            let ratio = ctx.token_estimate_ratio;

            let entries = self.scan_tool_outputs(ctx);
            let candidates: Vec<ToolOutputEntry> = entries
                .into_iter()
                .filter(|e| e.compressibility() >= self.config.min_compressibility)
                .collect();

            let mut freed_tokens: usize = 0;
            let mut compacted_count: usize = 0;

            for entry in &candidates {
                // Stop once pressure drops below 0.70.
                let budget = ctx.pressure.budget_tokens.max(1) as f64;
                let current_pressure = pressure_before - (freed_tokens as f64 / budget);
                if current_pressure < 0.70 {
                    break;
                }

                let msg = &mut ctx.messages[entry.message_index];
                let original_content = match msg.tool_result_info() {
                    Some((_, c)) => c,
                    None => continue,
                };

                let original_tokens = estimate_tokens(&original_content, ratio);
                let placeholder = format_compact_placeholder(
                    &entry.tool_name,
                    original_tokens,
                    None,
                    true,
                    Some(&original_content),
                );
                let placeholder_tokens = estimate_tokens(&placeholder, ratio);

                msg.replace_tool_result_content(placeholder);

                let saved = original_tokens.saturating_sub(placeholder_tokens);
                freed_tokens += saved;
                compacted_count += 1;
            }

            // Compute approximate pressure_after from freed tokens.
            let total = ctx.pressure.budget_tokens.max(1) as f64;
            let pressure_after = (pressure_before - freed_tokens as f64 / total).max(0.0);

            Ok(CompactionResult {
                freed_tokens,
                compacted_count,
                strategy_name: self.name().to_owned(),
                pressure_before,
                pressure_after,
            })
        })
    }

    fn is_applicable(&self, ctx: &CompactionContext) -> bool {
        ctx.pressure_level >= PressureLevel::Preventive
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importance_classification() {
        // Low: read_file family with age >= 5
        assert_eq!(
            classify_importance("read_file", "file contents here", 5),
            Importance::Low
        );
        // Medium: read_file but age < 5 (not old enough)
        assert_eq!(
            classify_importance("read_file", "file contents", 3),
            Importance::Medium
        );
        // Low: search family with age >= 5
        assert_eq!(
            classify_importance("search", "results", 10),
            Importance::Low
        );
        // Medium: unknown tool, no error keywords
        assert_eq!(
            classify_importance("custom_tool", "ok", 1),
            Importance::Medium
        );
        // High: memory tool name
        assert_eq!(classify_importance("memory", "saved", 1), Importance::High);
        // High: content contains error keyword
        assert_eq!(
            classify_importance("run_code", "error: panicked", 1),
            Importance::High
        );
    }

    #[test]
    fn compressibility_scoring() {
        // Old + large + low importance → high compressibility (> 0.7)
        let entry = ToolOutputEntry {
            turn_age: 20,
            token_size: 3000,
            importance: Importance::Low,
            tool_name: "read_file".into(),
            message_index: 0,
        };
        assert!(
            entry.compressibility() > 0.7,
            "expected > 0.7 but got {}",
            entry.compressibility()
        );

        // New + small + high importance → low compressibility (< 0.1)
        let entry2 = ToolOutputEntry {
            turn_age: 1,
            token_size: 100,
            importance: Importance::High,
            tool_name: "memory".into(),
            message_index: 5,
        };
        assert!(
            entry2.compressibility() < 0.1,
            "expected < 0.1 but got {}",
            entry2.compressibility()
        );
    }

    #[test]
    fn compact_placeholder_format() {
        let placeholder = format_compact_placeholder(
            "read_file",
            2500,
            Some(&["path", "content", "encoding"]),
            true,
            None,
        );
        assert!(
            placeholder.contains("[Tool output compacted: read_file]"),
            "missing header: {placeholder}"
        );
        assert!(
            placeholder.contains("2500 tokens"),
            "missing token count: {placeholder}"
        );
        assert!(
            placeholder.contains("path, content, encoding"),
            "missing key fields: {placeholder}"
        );
        assert!(
            placeholder.contains("success"),
            "missing status: {placeholder}"
        );
    }

    #[test]
    fn compact_placeholder_preserves_disk_ref() {
        let ref_line = "[Full output persisted: /tmp/aleph/tool_results/sess/call_1_bash.txt (12000 tokens, bash)]";
        let original_content = format!("some output\n{ref_line}\nmore text");
        let placeholder =
            format_compact_placeholder("bash", 12000, None, true, Some(&original_content));
        assert!(
            placeholder.contains(ref_line),
            "placeholder must preserve the disk ref line: {placeholder}"
        );
    }
}
