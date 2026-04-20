//! ContextCollapseStage — folds consecutive exploratory message groups into summaries.
//!
//! Detects consecutive rounds of file exploration (Read/Glob) or search sweeps
//! (Grep) and replaces them with a single summary message, freeing tokens for
//! more useful context.

use std::ops::Range;

use async_trait::async_trait;

use super::preflight::PreflightStage;
use super::pressure::estimate_tokens_smart;
use super::ContextPressure;
use crate::memory::session_compactor::context_window::partition_fresh_tail;
use crate::providers::message::UnifiedMessage;

/// Pressure ratio below which the stage is a no-op.
const ACTIVATION_THRESHOLD: f64 = 0.5;

/// Minimum tokens a group must contain to be worth folding.
const MIN_SAVINGS_TOKENS: usize = 500;

/// Minimum consecutive rounds of Read/Glob to qualify as file exploration.
const MIN_ROUNDS_FILE_EXPLORATION: usize = 3;

/// Minimum consecutive rounds of Grep to qualify as a search sweep.
const MIN_ROUNDS_SEARCH_SWEEP: usize = 2;

// =============================================================================
// Group types
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupType {
    /// 3+ consecutive Read/Glob rounds.
    FileExploration,
    /// 2+ consecutive Grep rounds.
    SearchSweep,
}

#[derive(Debug)]
struct MessageGroup {
    range: Range<usize>,
    group_type: GroupType,
    total_tokens: usize,
}

// =============================================================================
// Round detection helpers
// =============================================================================

/// Checks if messages starting at `pos` form a Read/Glob round (user + tool_result + assistant).
/// Returns the end index (exclusive) if so.
fn is_read_or_glob_round(messages: &[UnifiedMessage], pos: usize) -> Option<usize> {
    if pos + 2 >= messages.len() {
        return None;
    }
    if !messages[pos].is_user() {
        return None;
    }
    if let Some((name, _)) = messages[pos + 1].tool_result_info() {
        let lower = name.to_ascii_lowercase();
        if (lower == "read" || lower == "glob" || lower.contains("read_file"))
            && messages[pos + 2].is_assistant()
        {
            return Some(pos + 3);
        }
    }
    None
}

/// Checks if messages starting at `pos` form a Grep/search round (user + tool_result + assistant).
/// Returns the end index (exclusive) if so.
fn is_grep_round(messages: &[UnifiedMessage], pos: usize) -> Option<usize> {
    if pos + 2 >= messages.len() {
        return None;
    }
    if !messages[pos].is_user() {
        return None;
    }
    if let Some((name, _)) = messages[pos + 1].tool_result_info() {
        let lower = name.to_ascii_lowercase();
        if (lower == "grep" || lower == "search" || lower.contains("search"))
            && messages[pos + 2].is_assistant()
        {
            return Some(pos + 3);
        }
    }
    None
}

// =============================================================================
// Summary generation (pure local, no LLM)
// =============================================================================

fn generate_summary(messages: &[UnifiedMessage], group_type: GroupType) -> String {
    match group_type {
        GroupType::FileExploration => {
            let count = messages
                .iter()
                .filter(|m| m.tool_result_info().is_some())
                .count();
            let total_lines: usize = messages
                .iter()
                .filter_map(|m| m.tool_result_info().map(|(_, c)| c.lines().count()))
                .sum();
            format!(
                "Explored {} files ({} total lines of code)",
                count, total_lines
            )
        }
        GroupType::SearchSweep => {
            let count = messages
                .iter()
                .filter(|m| m.tool_result_info().is_some())
                .count();
            format!("Ran {} search queries across the codebase", count)
        }
    }
}

// =============================================================================
// Group detection
// =============================================================================

/// Scans messages in the compressible region and identifies consecutive groups.
fn detect_groups(messages: &[UnifiedMessage], partition: usize) -> Vec<MessageGroup> {
    let mut groups = Vec::new();
    let mut pos = 0;

    while pos < partition {
        // Try file exploration group
        if let Some(group) = try_detect_group(
            messages,
            partition,
            pos,
            is_read_or_glob_round,
            GroupType::FileExploration,
            MIN_ROUNDS_FILE_EXPLORATION,
        ) {
            pos = group.range.end;
            groups.push(group);
            continue;
        }

        // Try search sweep group
        if let Some(group) = try_detect_group(
            messages,
            partition,
            pos,
            is_grep_round,
            GroupType::SearchSweep,
            MIN_ROUNDS_SEARCH_SWEEP,
        ) {
            pos = group.range.end;
            groups.push(group);
            continue;
        }

        pos += 1;
    }

    groups
}

/// Attempts to detect a group of consecutive rounds starting at `start`.
fn try_detect_group(
    messages: &[UnifiedMessage],
    partition: usize,
    start: usize,
    round_detector: fn(&[UnifiedMessage], usize) -> Option<usize>,
    group_type: GroupType,
    min_rounds: usize,
) -> Option<MessageGroup> {
    let mut end = start;
    let mut rounds = 0;

    while end < partition {
        if let Some(next_end) = round_detector(messages, end) {
            if next_end <= partition {
                end = next_end;
                rounds += 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if rounds >= min_rounds {
        let total_tokens: usize = messages[start..end]
            .iter()
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum();

        Some(MessageGroup {
            range: start..end,
            group_type,
            total_tokens,
        })
    } else {
        None
    }
}

// =============================================================================
// ContextCollapseStage
// =============================================================================

/// Folds consecutive exploratory message groups into compact summaries.
pub struct ContextCollapseStage;

impl Default for ContextCollapseStage {
    fn default() -> Self {
        Self
    }
}

impl ContextCollapseStage {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PreflightStage for ContextCollapseStage {
    fn name(&self) -> &'static str {
        "context_collapse"
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
        if partition < 6 {
            return 0;
        }

        let groups = detect_groups(messages, partition);
        if groups.is_empty() {
            return 0;
        }

        let mut total_freed: usize = 0;

        // Process groups back-to-front for stable indices
        for group in groups.into_iter().rev() {
            if group.total_tokens < MIN_SAVINGS_TOKENS {
                continue;
            }

            let summary = generate_summary(&messages[group.range.clone()], group.group_type);
            let summary_tokens = estimate_tokens_smart(&summary);

            if summary_tokens >= group.total_tokens {
                continue;
            }

            let freed = group.total_tokens - summary_tokens;
            let collapsed = UnifiedMessage::user(format!("[Context collapsed] {summary}"));

            messages.splice(group.range, std::iter::once(collapsed));
            total_freed += freed;
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
            used_tokens: 300,
            budget_tokens: 1000,
            ratio: 0.3,
            overhead_tokens: 100,
            available_for_messages: 900,
        }
    }

    /// Build N consecutive Read rounds with substantial content.
    fn make_read_rounds(n: usize) -> Vec<UnifiedMessage> {
        let mut msgs = vec![];
        for i in 0..n {
            msgs.push(UnifiedMessage::user(format!("read file {}", i)));
            msgs.push(UnifiedMessage::tool_result(
                format!("c{}", i),
                "Read",
                "fn foo() {}\n".repeat(50),
                false,
            ));
            msgs.push(UnifiedMessage::assistant(format!("I read file {}", i)));
        }
        msgs
    }

    #[tokio::test]
    async fn folds_file_exploration_group() {
        let stage = ContextCollapseStage::new();

        let mut msgs = make_read_rounds(4);
        let original_len = msgs.len(); // 12
                                       // Add fresh tail so partition covers the rounds
        msgs.push(UnifiedMessage::user("now summarize"));

        let freed = stage.prepare(&mut msgs, &high_pressure(), 1).await;

        assert!(freed > 0, "should have freed tokens, got {freed}");
        assert!(
            msgs.len() < original_len,
            "message count should decrease: was {original_len}, now {}",
            msgs.len()
        );

        // The collapsed message should be present
        let has_collapsed = msgs
            .iter()
            .any(|m| m.text_content().contains("[Context collapsed]"));
        assert!(has_collapsed, "should contain [Context collapsed] marker");
    }

    #[tokio::test]
    async fn preserves_write_groups() {
        let stage = ContextCollapseStage::new();

        let mut msgs = vec![];
        for i in 0..4 {
            msgs.push(UnifiedMessage::user(format!("write file {}", i)));
            msgs.push(UnifiedMessage::tool_result(
                format!("w{}", i),
                "Write",
                "fn bar() {}\n".repeat(50),
                false,
            ));
            msgs.push(UnifiedMessage::assistant(format!("I wrote file {}", i)));
        }
        msgs.push(UnifiedMessage::user("done"));

        let original_len = msgs.len();
        let freed = stage.prepare(&mut msgs, &high_pressure(), 1).await;

        assert_eq!(freed, 0, "Write rounds should not be folded");
        assert_eq!(msgs.len(), original_len, "message count should not change");
    }

    #[tokio::test]
    async fn skips_below_threshold() {
        let stage = ContextCollapseStage::new();

        let mut msgs = make_read_rounds(4);
        msgs.push(UnifiedMessage::user("now summarize"));

        let original_len = msgs.len();
        let freed = stage.prepare(&mut msgs, &low_pressure(), 1).await;

        assert_eq!(freed, 0, "should not fire below activation threshold");
        assert_eq!(
            msgs.len(),
            original_len,
            "message count should not change at low pressure"
        );
    }
}
