//! SmartCompactor - Unified Compaction Component
//!
//! This module provides a unified compaction component that combines:
//! - `SmartCompactionStrategy` for decision making
//! - `ToolTruncator` for truncating tool outputs
//! - `TurnProtector` for protecting recent conversation turns
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::compressor::{SmartCompactor, SmartCompactionStrategy};
//!
//! // Create with default settings
//! let compactor = SmartCompactor::new();
//!
//! // Or with custom strategy
//! let strategy = SmartCompactionStrategy::new()
//!     .with_compaction_threshold(0.90)
//!     .with_protected_turns(3);
//! let compactor = SmartCompactor::with_strategy(strategy);
//!
//! // Compact session parts
//! let result = compactor.compact(&session_parts, 0.92);
//! if result.marker.is_some() {
//!     println!("Compacted {} parts, freed ~{} tokens",
//!         result.parts_compacted, result.tokens_freed_estimate);
//! }
//! ```
//!
//! # Compaction Logic
//!
//! 1. If `token_usage < compaction_threshold`, returns parts unchanged
//! 2. For each part:
//!    - Protected turns (last N) are never modified
//!    - Protected tools (skill, plan) are never modified
//!    - Large tool outputs are truncated with summaries
//! 3. Returns compacted parts with a `CompactionMarker` containing stats

use crate::components::{CompactionMarker, SessionPart, ToolCallPart};

use super::{CompactionAction, SmartCompactionStrategy, ToolTruncator, TurnProtector};

/// Result of a compaction operation
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The compacted session parts
    pub parts: Vec<SessionPart>,
    /// Compaction marker with stats (None if no compaction occurred)
    pub marker: Option<CompactionMarker>,
    /// Number of parts that were compacted
    pub parts_compacted: usize,
    /// Estimated tokens freed by compaction
    pub tokens_freed_estimate: u64,
}

impl CompactionResult {
    /// Create a result indicating no compaction was needed
    fn unchanged(parts: Vec<SessionPart>) -> Self {
        Self {
            parts,
            marker: None,
            parts_compacted: 0,
            tokens_freed_estimate: 0,
        }
    }

    /// Create a result with compaction stats
    fn compacted(
        parts: Vec<SessionPart>,
        parts_compacted: usize,
        tokens_freed_estimate: u64,
    ) -> Self {
        let marker = if parts_compacted > 0 {
            Some(CompactionMarker::with_details(
                true, // auto-triggered
                uuid::Uuid::new_v4().to_string(),
                parts_compacted,
                tokens_freed_estimate,
            ))
        } else {
            None
        };

        Self {
            parts,
            marker,
            parts_compacted,
            tokens_freed_estimate,
        }
    }
}

/// SmartCompactor combines strategy, truncator, and turn protector
/// for intelligent session compaction.
#[derive(Debug, Clone)]
pub struct SmartCompactor {
    /// Strategy for making compaction decisions
    strategy: SmartCompactionStrategy,
    /// Truncator for tool outputs
    truncator: ToolTruncator,
    /// Protector for recent conversation turns
    turn_protector: TurnProtector,
}

impl Default for SmartCompactor {
    fn default() -> Self {
        Self::new()
    }
}

impl SmartCompactor {
    /// Create a new SmartCompactor with default settings
    ///
    /// Default configuration:
    /// - `tool_output_max_chars`: 2000
    /// - `protected_turns`: 2
    /// - `compaction_threshold`: 0.85
    /// - `protected_tools`: ["skill", "plan"]
    pub fn new() -> Self {
        let strategy = SmartCompactionStrategy::new();
        Self {
            truncator: ToolTruncator::new(strategy.tool_output_max_chars),
            turn_protector: TurnProtector::new(strategy.protected_turns),
            strategy,
        }
    }

    /// Create a SmartCompactor with a custom strategy
    ///
    /// The truncator and turn protector are automatically configured
    /// based on the strategy's settings.
    pub fn with_strategy(strategy: SmartCompactionStrategy) -> Self {
        Self {
            truncator: ToolTruncator::new(strategy.tool_output_max_chars),
            turn_protector: TurnProtector::new(strategy.protected_turns),
            strategy,
        }
    }

    /// Set a custom tool truncator
    ///
    /// Use this to override the default truncator (e.g., with a custom template).
    pub fn with_truncator(mut self, truncator: ToolTruncator) -> Self {
        self.truncator = truncator;
        self
    }

    /// Set a custom turn protector
    ///
    /// Use this to override the default turn protector.
    pub fn with_turn_protector(mut self, turn_protector: TurnProtector) -> Self {
        self.turn_protector = turn_protector;
        self
    }

    /// Get a reference to the strategy
    pub fn strategy(&self) -> &SmartCompactionStrategy {
        &self.strategy
    }

    /// Get a reference to the truncator
    pub fn truncator(&self) -> &ToolTruncator {
        &self.truncator
    }

    /// Get a reference to the turn protector
    pub fn turn_protector(&self) -> &TurnProtector {
        &self.turn_protector
    }

    /// Compact session parts based on current token usage
    ///
    /// # Arguments
    ///
    /// * `parts` - The session parts to compact
    /// * `token_usage` - Current token usage ratio (0.0 to 1.0)
    ///
    /// # Returns
    ///
    /// A `CompactionResult` containing:
    /// - The compacted parts
    /// - A compaction marker (if compaction occurred)
    /// - Statistics about the compaction
    ///
    /// # Behavior
    ///
    /// - If `token_usage < compaction_threshold`, returns parts unchanged
    /// - Protected turns (last N) are never modified
    /// - Protected tools are never modified
    /// - Large tool outputs are truncated with summaries
    pub fn compact(&self, parts: &[SessionPart], token_usage: f32) -> CompactionResult {
        // Check if compaction is needed
        if !self.strategy.should_compact(token_usage) {
            return CompactionResult::unchanged(parts.to_vec());
        }

        // Calculate turn indices for all parts
        let turn_indices = self.turn_protector.calculate_turn_index(parts);
        let total_turns = self.turn_protector.count_turns(parts);

        let mut compacted_parts = Vec::with_capacity(parts.len());
        let mut parts_compacted = 0usize;
        let mut tokens_freed_estimate = 0u64;

        for (part_index, part) in parts.iter().enumerate() {
            // Get turn index for this part
            let turn_index = turn_indices
                .get(part_index)
                .map(|(_, ti)| *ti)
                .unwrap_or(0);

            // Evaluate what action to take
            let action = self.strategy.evaluate_part(part, turn_index, total_turns);

            match action {
                CompactionAction::Keep => {
                    compacted_parts.push(part.clone());
                }
                CompactionAction::Truncate { .. } => {
                    // Apply truncation to tool call output
                    if let SessionPart::ToolCall(tool_call) = part {
                        let (truncated, freed) = self.truncate_tool_call(tool_call);
                        compacted_parts.push(SessionPart::ToolCall(truncated));
                        if freed > 0 {
                            parts_compacted += 1;
                            tokens_freed_estimate += freed;
                        }
                    } else {
                        // Non-tool parts don't get truncated, keep as-is
                        compacted_parts.push(part.clone());
                    }
                }
                CompactionAction::RemoveOutput => {
                    // Remove output from tool call
                    if let SessionPart::ToolCall(tool_call) = part {
                        let (cleaned, freed) = self.remove_tool_output(tool_call);
                        compacted_parts.push(SessionPart::ToolCall(cleaned));
                        if freed > 0 {
                            parts_compacted += 1;
                            tokens_freed_estimate += freed;
                        }
                    } else {
                        compacted_parts.push(part.clone());
                    }
                }
                CompactionAction::Summarize { .. } => {
                    // For now, Summarize is treated same as Keep
                    // Future: could merge multiple parts into a Summary
                    compacted_parts.push(part.clone());
                }
            }
        }

        CompactionResult::compacted(compacted_parts, parts_compacted, tokens_freed_estimate)
    }

    /// Truncate a tool call's output and return estimated freed tokens
    fn truncate_tool_call(&self, tool_call: &ToolCallPart) -> (ToolCallPart, u64) {
        let mut truncated = tool_call.clone();

        if let Some(ref output) = tool_call.output {
            if self.truncator.should_truncate(output) {
                let result = self.truncator.truncate(output, &tool_call.tool_name);

                // Estimate tokens freed (rough estimate: 4 chars per token)
                let chars_freed = output.len().saturating_sub(result.content.len());
                let tokens_freed = (chars_freed / 4) as u64;

                truncated.output = Some(result.content);

                return (truncated, tokens_freed);
            }
        }

        (truncated, 0)
    }

    /// Remove a tool call's output entirely and return estimated freed tokens
    fn remove_tool_output(&self, tool_call: &ToolCallPart) -> (ToolCallPart, u64) {
        let mut cleaned = tool_call.clone();

        let tokens_freed = if let Some(ref output) = tool_call.output {
            // Estimate tokens freed (rough estimate: 4 chars per token)
            (output.len() / 4) as u64
        } else {
            0
        };

        // Replace output with a placeholder
        cleaned.output = Some("[Output removed during compaction]".to_string());

        (cleaned, tokens_freed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{
        AiResponsePart, ToolCallStatus, UserInputPart,
    };
    use serde_json::json;

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn create_user_input(text: &str) -> SessionPart {
        SessionPart::UserInput(UserInputPart {
            text: text.to_string(),
            context: None,
            timestamp: 1000,
        })
    }

    fn create_ai_response(content: &str) -> SessionPart {
        SessionPart::AiResponse(AiResponsePart {
            content: content.to_string(),
            reasoning: None,
            timestamp: 1000,
        })
    }

    fn create_tool_call(name: &str, output: Option<String>) -> SessionPart {
        SessionPart::ToolCall(ToolCallPart {
            id: format!("call-{}", name),
            tool_name: name.to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output,
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        })
    }

    // =========================================================================
    // Construction Tests
    // =========================================================================

    #[test]
    fn test_new_default() {
        let compactor = SmartCompactor::new();

        assert_eq!(compactor.strategy().tool_output_max_chars, 2000);
        assert_eq!(compactor.strategy().protected_turns, 2);
        assert!((compactor.strategy().compaction_threshold - 0.85).abs() < 0.001);
        assert_eq!(compactor.truncator().max_chars(), 2000);
        assert_eq!(compactor.turn_protector().protected_turns(), 2);
    }

    #[test]
    fn test_default_trait() {
        let compactor = SmartCompactor::default();
        assert_eq!(compactor.strategy().tool_output_max_chars, 2000);
    }

    #[test]
    fn test_with_strategy() {
        let strategy = SmartCompactionStrategy::new()
            .with_tool_output_max_chars(5000)
            .with_protected_turns(3)
            .with_compaction_threshold(0.90);

        let compactor = SmartCompactor::with_strategy(strategy);

        assert_eq!(compactor.strategy().tool_output_max_chars, 5000);
        assert_eq!(compactor.strategy().protected_turns, 3);
        assert!((compactor.strategy().compaction_threshold - 0.90).abs() < 0.001);
        // Truncator and turn protector should be configured from strategy
        assert_eq!(compactor.truncator().max_chars(), 5000);
        assert_eq!(compactor.turn_protector().protected_turns(), 3);
    }

    #[test]
    fn test_with_custom_truncator() {
        let truncator = ToolTruncator::new(3000)
            .with_summary_template("[Custom: {tool_name}]");

        let compactor = SmartCompactor::new().with_truncator(truncator);

        assert_eq!(compactor.truncator().max_chars(), 3000);
        assert_eq!(compactor.truncator().summary_template(), "[Custom: {tool_name}]");
    }

    #[test]
    fn test_with_custom_turn_protector() {
        let protector = TurnProtector::new(5);

        let compactor = SmartCompactor::new().with_turn_protector(protector);

        assert_eq!(compactor.turn_protector().protected_turns(), 5);
    }

    // =========================================================================
    // Compact - No Compaction Needed
    // =========================================================================

    #[test]
    fn test_compact_below_threshold() {
        let compactor = SmartCompactor::new(); // threshold = 0.85

        let parts = vec![
            create_user_input("Hello"),
            create_ai_response("Hi!"),
            create_tool_call("search", Some("Large output".repeat(1000))),
        ];

        // 80% usage is below 85% threshold
        let result = compactor.compact(&parts, 0.80);

        assert!(result.marker.is_none());
        assert_eq!(result.parts_compacted, 0);
        assert_eq!(result.tokens_freed_estimate, 0);
        assert_eq!(result.parts.len(), parts.len());
    }

    #[test]
    fn test_compact_empty_parts() {
        let compactor = SmartCompactor::new();
        let parts: Vec<SessionPart> = vec![];

        let result = compactor.compact(&parts, 0.90);

        assert!(result.marker.is_none());
        assert_eq!(result.parts_compacted, 0);
        assert!(result.parts.is_empty());
    }

    // =========================================================================
    // Compact - Protected Turns
    // =========================================================================

    #[test]
    fn test_compact_protected_turns_not_modified() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(1)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        let large_output = "x".repeat(5000);

        // Turn 0: user + tool with large output
        // Turn 1: user + tool with large output (protected)
        let parts = vec![
            create_user_input("Turn 0"),
            create_tool_call("read_file", Some(large_output.clone())),
            create_user_input("Turn 1"),
            create_tool_call("read_file", Some(large_output.clone())),
        ];

        let result = compactor.compact(&parts, 0.90);

        // Turn 0 tool should be truncated
        // Turn 1 tool should NOT be truncated (protected)

        // Check that part 1 (turn 0 tool) was truncated
        if let SessionPart::ToolCall(tc) = &result.parts[1] {
            assert!(tc.output.as_ref().unwrap().len() <= 100);
        } else {
            panic!("Expected ToolCall at index 1");
        }

        // Check that part 3 (turn 1 tool) was NOT truncated
        if let SessionPart::ToolCall(tc) = &result.parts[3] {
            assert_eq!(tc.output.as_ref().unwrap().len(), 5000);
        } else {
            panic!("Expected ToolCall at index 3");
        }
    }

    // =========================================================================
    // Compact - Protected Tools
    // =========================================================================

    #[test]
    fn test_compact_protected_tools_not_modified() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0) // Disable turn protection
            .with_tool_output_max_chars(100)
            .add_protected_tool("skill");

        let compactor = SmartCompactor::with_strategy(strategy);

        let large_output = "x".repeat(5000);

        let parts = vec![
            create_user_input("Request"),
            create_tool_call("skill", Some(large_output.clone())),
            create_tool_call("read_file", Some(large_output.clone())),
        ];

        let result = compactor.compact(&parts, 0.90);

        // skill tool should NOT be truncated (protected tool)
        if let SessionPart::ToolCall(tc) = &result.parts[1] {
            assert_eq!(tc.output.as_ref().unwrap().len(), 5000);
        } else {
            panic!("Expected ToolCall at index 1");
        }

        // read_file tool should be truncated
        if let SessionPart::ToolCall(tc) = &result.parts[2] {
            assert!(tc.output.as_ref().unwrap().len() <= 100);
        } else {
            panic!("Expected ToolCall at index 2");
        }
    }

    // =========================================================================
    // Compact - Truncation
    // =========================================================================

    #[test]
    fn test_compact_truncates_large_outputs() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(200);

        let compactor = SmartCompactor::with_strategy(strategy);

        let parts = vec![
            create_user_input("Request"),
            create_tool_call("read_file", Some("x".repeat(1000))),
            create_tool_call("search", Some("Small output".to_string())),
        ];

        let result = compactor.compact(&parts, 0.90);

        assert!(result.marker.is_some());
        assert_eq!(result.parts_compacted, 1); // Only the large output was truncated
        assert!(result.tokens_freed_estimate > 0);

        // Check that the large output was truncated
        if let SessionPart::ToolCall(tc) = &result.parts[1] {
            assert!(tc.output.as_ref().unwrap().len() <= 200);
            assert!(tc.output.as_ref().unwrap().contains("Truncated"));
        } else {
            panic!("Expected ToolCall at index 1");
        }

        // Check that the small output was not modified
        if let SessionPart::ToolCall(tc) = &result.parts[2] {
            assert_eq!(tc.output.as_ref().unwrap(), "Small output");
        } else {
            panic!("Expected ToolCall at index 2");
        }
    }

    #[test]
    fn test_compact_tokens_freed_estimate() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        // Create output that will be truncated significantly
        let large_output = "x".repeat(1000);

        let parts = vec![
            create_user_input("Request"),
            create_tool_call("read_file", Some(large_output)),
        ];

        let result = compactor.compact(&parts, 0.90);

        // Should have freed approximately (1000 - 100) / 4 = 225 tokens
        assert!(result.tokens_freed_estimate > 200);
    }

    // =========================================================================
    // Compact - No Output / Already Small
    // =========================================================================

    #[test]
    fn test_compact_no_output_parts() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        let parts = vec![
            create_user_input("Request"),
            create_tool_call("read_file", None), // No output
        ];

        let result = compactor.compact(&parts, 0.90);

        // No parts should be compacted since there's no output to truncate
        assert_eq!(result.parts_compacted, 0);
    }

    #[test]
    fn test_compact_already_small_outputs() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(1000);

        let compactor = SmartCompactor::with_strategy(strategy);

        let parts = vec![
            create_user_input("Request"),
            create_tool_call("read_file", Some("Small output".to_string())),
        ];

        let result = compactor.compact(&parts, 0.90);

        // No parts should be compacted since output is already small
        assert_eq!(result.parts_compacted, 0);

        // Output should be unchanged
        if let SessionPart::ToolCall(tc) = &result.parts[1] {
            assert_eq!(tc.output.as_ref().unwrap(), "Small output");
        } else {
            panic!("Expected ToolCall at index 1");
        }
    }

    // =========================================================================
    // CompactionResult Tests
    // =========================================================================

    #[test]
    fn test_compaction_result_unchanged() {
        let parts = vec![create_user_input("Test")];
        let result = CompactionResult::unchanged(parts.clone());

        assert!(result.marker.is_none());
        assert_eq!(result.parts_compacted, 0);
        assert_eq!(result.tokens_freed_estimate, 0);
        assert_eq!(result.parts.len(), 1);
    }

    #[test]
    fn test_compaction_result_compacted() {
        let parts = vec![create_user_input("Test")];
        let result = CompactionResult::compacted(parts.clone(), 5, 1000);

        assert!(result.marker.is_some());
        assert_eq!(result.parts_compacted, 5);
        assert_eq!(result.tokens_freed_estimate, 1000);

        let marker = result.marker.unwrap();
        assert!(marker.auto);
        assert!(marker.marker_id.is_some());
        assert_eq!(marker.parts_compacted, Some(5));
        assert_eq!(marker.tokens_freed, Some(1000));
    }

    #[test]
    fn test_compaction_result_zero_compacted() {
        let parts = vec![create_user_input("Test")];
        let result = CompactionResult::compacted(parts.clone(), 0, 0);

        // No marker when no parts compacted
        assert!(result.marker.is_none());
        assert_eq!(result.parts_compacted, 0);
    }

    // =========================================================================
    // Integration Tests
    // =========================================================================

    #[test]
    fn test_complex_session_compaction() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(1)
            .with_tool_output_max_chars(200)
            .add_protected_tool("skill");

        let compactor = SmartCompactor::with_strategy(strategy);

        let large_output = "x".repeat(1000);
        let small_output = "OK".to_string();

        let parts = vec![
            // Turn 0 (NOT protected)
            create_user_input("Find config"),
            create_tool_call("search", Some(large_output.clone())),
            create_tool_call("read_file", Some(large_output.clone())),
            // Turn 1 (NOT protected)
            create_user_input("Update database"),
            create_tool_call("skill", Some(large_output.clone())), // Protected tool
            create_tool_call("edit", Some(small_output.clone())),  // Small output
            // Turn 2 (PROTECTED)
            create_user_input("Verify"),
            create_tool_call("test", Some(large_output.clone())), // Protected turn
        ];

        let result = compactor.compact(&parts, 0.90);

        // Expected:
        // - Turn 0 search: truncated (large, not protected)
        // - Turn 0 read_file: truncated (large, not protected)
        // - Turn 1 skill: NOT truncated (protected tool)
        // - Turn 1 edit: NOT truncated (already small)
        // - Turn 2 test: NOT truncated (protected turn)

        assert_eq!(result.parts.len(), 8);
        assert_eq!(result.parts_compacted, 2); // search and read_file

        // Verify turn 0 tools were truncated
        if let SessionPart::ToolCall(tc) = &result.parts[1] {
            assert!(tc.output.as_ref().unwrap().len() <= 200);
        }
        if let SessionPart::ToolCall(tc) = &result.parts[2] {
            assert!(tc.output.as_ref().unwrap().len() <= 200);
        }

        // Verify skill tool was NOT truncated
        if let SessionPart::ToolCall(tc) = &result.parts[4] {
            assert_eq!(tc.output.as_ref().unwrap().len(), 1000);
        }

        // Verify turn 2 test was NOT truncated
        if let SessionPart::ToolCall(tc) = &result.parts[7] {
            assert_eq!(tc.output.as_ref().unwrap().len(), 1000);
        }
    }

    #[test]
    fn test_non_tool_call_parts_preserved() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        let parts = vec![
            create_user_input(&"Very long user input ".repeat(100)),
            create_ai_response(&"Very long AI response ".repeat(100)),
            create_tool_call("read", Some("x".repeat(1000))),
        ];

        let result = compactor.compact(&parts, 0.90);

        // User input and AI response should be preserved unchanged
        if let SessionPart::UserInput(ui) = &result.parts[0] {
            assert_eq!(ui.text.len(), "Very long user input ".len() * 100);
        }
        if let SessionPart::AiResponse(ar) = &result.parts[1] {
            assert_eq!(ar.content.len(), "Very long AI response ".len() * 100);
        }

        // Only tool output should be truncated
        assert_eq!(result.parts_compacted, 1);
    }
}
