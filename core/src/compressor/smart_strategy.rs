//! Smart Compaction Strategy for Intelligent Context Management
//!
//! This module provides a configurable strategy for intelligently compacting
//! session context. Unlike rule-based compression that operates on LoopStep,
//! this strategy works directly on SessionPart to enable fine-grained control
//! over what gets compacted.
//!
//! # Features
//!
//! - **Tool Output Truncation**: Large tool outputs are truncated with summaries
//! - **Turn Protection**: Recent conversation turns are never compacted
//! - **Token Budget Management**: Compaction triggers based on configurable thresholds
//! - **Tool Whitelisting**: Certain tools can be protected from compaction
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::compressor::SmartCompactionStrategy;
//!
//! let strategy = SmartCompactionStrategy::new()
//!     .with_tool_output_max_chars(2000)
//!     .with_protected_turns(2)
//!     .with_compaction_threshold(0.85)
//!     .add_protected_tool("skill")
//!     .add_protected_tool("plan");
//!
//! if strategy.should_compact(0.90) {
//!     for (index, part) in session.parts.iter().enumerate() {
//!         let action = strategy.evaluate_part(part, index, session.parts.len());
//!         // Apply action to part
//!     }
//! }
//! ```

use std::collections::HashSet;

use crate::components::{SessionPart, ToolCallPart};

/// Represents the action to take for a session part during compaction
#[derive(Debug, Clone, PartialEq)]
pub enum CompactionAction {
    /// Keep the part as-is, no modifications
    Keep,
    /// Truncate output, retain a summary
    Truncate {
        /// Maximum characters to retain
        max_chars: usize,
        /// Summary of truncated content
        summary: String,
    },
    /// Remove output entirely (keep call record)
    RemoveOutput,
    /// Merge multiple parts into a summary
    Summarize {
        /// Number of original parts being summarized
        original_count: usize,
    },
}

impl CompactionAction {
    /// Check if this action is Keep
    pub fn is_keep(&self) -> bool {
        matches!(self, CompactionAction::Keep)
    }

    /// Check if this action modifies the part
    pub fn modifies_part(&self) -> bool {
        !self.is_keep()
    }
}

/// Smart compaction strategy with configurable rules
///
/// This strategy provides fine-grained control over how session parts
/// are compacted based on configurable thresholds and rules.
#[derive(Debug, Clone)]
pub struct SmartCompactionStrategy {
    /// Maximum characters to retain for tool output
    pub tool_output_max_chars: usize,
    /// Number of recent conversation turns to protect
    pub protected_turns: usize,
    /// Token budget threshold (0.0-1.0) above which compaction triggers
    pub compaction_threshold: f32,
    /// Set of tool names that should never be compacted
    pub protected_tools: HashSet<String>,
}

impl Default for SmartCompactionStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl SmartCompactionStrategy {
    /// Create a new strategy with default values
    ///
    /// Default configuration:
    /// - `tool_output_max_chars`: 2000
    /// - `protected_turns`: 2
    /// - `compaction_threshold`: 0.85
    /// - `protected_tools`: ["skill", "plan"]
    pub fn new() -> Self {
        let mut protected_tools = HashSet::new();
        protected_tools.insert("skill".to_string());
        protected_tools.insert("plan".to_string());

        Self {
            tool_output_max_chars: 2000,
            protected_turns: 2,
            compaction_threshold: 0.85,
            protected_tools,
        }
    }

    /// Set maximum characters for tool output
    ///
    /// Tool outputs exceeding this limit will be truncated with a summary.
    pub fn with_tool_output_max_chars(mut self, max_chars: usize) -> Self {
        self.tool_output_max_chars = max_chars;
        self
    }

    /// Set number of protected turns
    ///
    /// The most recent N turns will never be compacted.
    pub fn with_protected_turns(mut self, turns: usize) -> Self {
        self.protected_turns = turns;
        self
    }

    /// Set compaction threshold
    ///
    /// Compaction will trigger when context usage exceeds this threshold.
    /// Value should be between 0.0 and 1.0.
    pub fn with_compaction_threshold(mut self, threshold: f32) -> Self {
        self.compaction_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Replace the set of protected tools
    ///
    /// These tools will never have their output compacted.
    pub fn with_protected_tools(mut self, tools: HashSet<String>) -> Self {
        self.protected_tools = tools;
        self
    }

    /// Add a single tool to the protected set
    ///
    /// This tool will never have its output compacted.
    pub fn add_protected_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.protected_tools.insert(tool_name.into());
        self
    }

    /// Check if compaction should trigger based on current usage
    ///
    /// Returns true if `current_usage` exceeds `compaction_threshold`.
    pub fn should_compact(&self, current_usage: f32) -> bool {
        current_usage >= self.compaction_threshold
    }

    /// Evaluate a session part and determine the compaction action
    ///
    /// # Arguments
    ///
    /// * `part` - The session part to evaluate
    /// * `turn_index` - The index of this turn in the session (0-based)
    /// * `total_turns` - Total number of turns in the session
    ///
    /// # Returns
    ///
    /// The recommended `CompactionAction` for this part.
    ///
    /// # Logic
    ///
    /// 1. If turn is within protected window (last N turns), return Keep
    /// 2. If part is a ToolCall with protected tool name, return Keep
    /// 3. If part is a ToolCall with output exceeding max_chars, return Truncate
    /// 4. Otherwise return Keep (safe default)
    pub fn evaluate_part(
        &self,
        part: &SessionPart,
        turn_index: usize,
        total_turns: usize,
    ) -> CompactionAction {
        // Rule 1: Protect recent turns
        if self.is_protected_turn(turn_index, total_turns) {
            return CompactionAction::Keep;
        }

        // Rule 2 & 3: Check ToolCall parts
        if let SessionPart::ToolCall(tool_call) = part {
            return self.evaluate_tool_call(tool_call);
        }

        // Default: Keep (safe behavior)
        CompactionAction::Keep
    }

    /// Check if a turn index is within the protected window
    fn is_protected_turn(&self, turn_index: usize, total_turns: usize) -> bool {
        if total_turns == 0 || self.protected_turns == 0 {
            return false;
        }

        let protected_start = total_turns.saturating_sub(self.protected_turns);
        turn_index >= protected_start
    }

    /// Evaluate a tool call part for compaction
    fn evaluate_tool_call(&self, tool_call: &ToolCallPart) -> CompactionAction {
        // Rule 2: Protected tools are never compacted
        if self.protected_tools.contains(&tool_call.tool_name) {
            return CompactionAction::Keep;
        }

        // Rule 3: Truncate large outputs
        if let Some(ref output) = tool_call.output {
            if output.len() > self.tool_output_max_chars {
                let summary = self.generate_truncation_summary(output, &tool_call.tool_name);
                return CompactionAction::Truncate {
                    max_chars: self.tool_output_max_chars,
                    summary,
                };
            }
        }

        // Default: Keep
        CompactionAction::Keep
    }

    /// Generate a summary for truncated output
    fn generate_truncation_summary(&self, output: &str, tool_name: &str) -> String {
        let original_size = output.len();
        let truncated_size = self.tool_output_max_chars;

        // Extract first line or first N characters as preview
        let preview = output
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(50)
            .collect::<String>();

        format!(
            "[Truncated {}: {}B -> {}B] {}...",
            tool_name, original_size, truncated_size, preview
        )
    }

    /// Apply truncation to a tool call output
    ///
    /// Returns the truncated output string with a summary prefix.
    pub fn truncate_output(&self, output: &str, tool_name: &str) -> String {
        if output.len() <= self.tool_output_max_chars {
            return output.to_string();
        }

        let summary = self.generate_truncation_summary(output, tool_name);

        // Calculate how much of the original we can keep after the summary
        let summary_len = summary.len();
        let remaining_space = self.tool_output_max_chars.saturating_sub(summary_len + 1);

        if remaining_space > 0 {
            let truncated_content: String = output.chars().take(remaining_space).collect();
            format!("{}\n{}", summary, truncated_content)
        } else {
            summary
        }
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
    // Default Configuration Tests
    // =========================================================================

    #[test]
    fn test_new_with_defaults() {
        let strategy = SmartCompactionStrategy::new();

        assert_eq!(strategy.tool_output_max_chars, 2000);
        assert_eq!(strategy.protected_turns, 2);
        assert!((strategy.compaction_threshold - 0.85).abs() < 0.001);
        assert!(strategy.protected_tools.contains("skill"));
        assert!(strategy.protected_tools.contains("plan"));
    }

    #[test]
    fn test_default_trait() {
        let strategy = SmartCompactionStrategy::default();
        assert_eq!(strategy.tool_output_max_chars, 2000);
    }

    // =========================================================================
    // Builder Pattern Tests
    // =========================================================================

    #[test]
    fn test_with_tool_output_max_chars() {
        let strategy = SmartCompactionStrategy::new().with_tool_output_max_chars(5000);
        assert_eq!(strategy.tool_output_max_chars, 5000);
    }

    #[test]
    fn test_with_protected_turns() {
        let strategy = SmartCompactionStrategy::new().with_protected_turns(5);
        assert_eq!(strategy.protected_turns, 5);
    }

    #[test]
    fn test_with_compaction_threshold() {
        let strategy = SmartCompactionStrategy::new().with_compaction_threshold(0.90);
        assert!((strategy.compaction_threshold - 0.90).abs() < 0.001);
    }

    #[test]
    fn test_with_compaction_threshold_clamped() {
        // Test clamping to valid range
        let strategy = SmartCompactionStrategy::new().with_compaction_threshold(1.5);
        assert!((strategy.compaction_threshold - 1.0).abs() < 0.001);

        let strategy = SmartCompactionStrategy::new().with_compaction_threshold(-0.5);
        assert!((strategy.compaction_threshold - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_with_protected_tools() {
        let mut tools = HashSet::new();
        tools.insert("custom_tool".to_string());

        let strategy = SmartCompactionStrategy::new().with_protected_tools(tools);

        assert!(strategy.protected_tools.contains("custom_tool"));
        assert!(!strategy.protected_tools.contains("skill")); // Replaced default set
    }

    #[test]
    fn test_add_protected_tool() {
        let strategy = SmartCompactionStrategy::new().add_protected_tool("custom_tool");

        assert!(strategy.protected_tools.contains("custom_tool"));
        assert!(strategy.protected_tools.contains("skill")); // Default still present
        assert!(strategy.protected_tools.contains("plan"));
    }

    #[test]
    fn test_builder_chaining() {
        let strategy = SmartCompactionStrategy::new()
            .with_tool_output_max_chars(3000)
            .with_protected_turns(3)
            .with_compaction_threshold(0.75)
            .add_protected_tool("my_tool");

        assert_eq!(strategy.tool_output_max_chars, 3000);
        assert_eq!(strategy.protected_turns, 3);
        assert!((strategy.compaction_threshold - 0.75).abs() < 0.001);
        assert!(strategy.protected_tools.contains("my_tool"));
    }

    // =========================================================================
    // should_compact Tests
    // =========================================================================

    #[test]
    fn test_should_compact_below_threshold() {
        let strategy = SmartCompactionStrategy::new().with_compaction_threshold(0.85);
        assert!(!strategy.should_compact(0.80));
        assert!(!strategy.should_compact(0.50));
        assert!(!strategy.should_compact(0.0));
    }

    #[test]
    fn test_should_compact_at_threshold() {
        let strategy = SmartCompactionStrategy::new().with_compaction_threshold(0.85);
        assert!(strategy.should_compact(0.85));
    }

    #[test]
    fn test_should_compact_above_threshold() {
        let strategy = SmartCompactionStrategy::new().with_compaction_threshold(0.85);
        assert!(strategy.should_compact(0.90));
        assert!(strategy.should_compact(1.0));
    }

    // =========================================================================
    // Protected Turns Tests
    // =========================================================================

    #[test]
    fn test_is_protected_turn_recent_turns() {
        let strategy = SmartCompactionStrategy::new().with_protected_turns(2);

        // With 5 total turns and protected_turns=2, turns 3 and 4 are protected
        assert!(!strategy.is_protected_turn(0, 5)); // Not protected
        assert!(!strategy.is_protected_turn(1, 5)); // Not protected
        assert!(!strategy.is_protected_turn(2, 5)); // Not protected
        assert!(strategy.is_protected_turn(3, 5)); // Protected
        assert!(strategy.is_protected_turn(4, 5)); // Protected
    }

    #[test]
    fn test_is_protected_turn_zero_turns() {
        let strategy = SmartCompactionStrategy::new().with_protected_turns(2);
        assert!(!strategy.is_protected_turn(0, 0));
    }

    #[test]
    fn test_is_protected_turn_zero_protection() {
        let strategy = SmartCompactionStrategy::new().with_protected_turns(0);
        assert!(!strategy.is_protected_turn(0, 5));
        assert!(!strategy.is_protected_turn(4, 5));
    }

    #[test]
    fn test_is_protected_turn_all_protected() {
        let strategy = SmartCompactionStrategy::new().with_protected_turns(10);

        // All turns protected when protected_turns > total_turns
        assert!(strategy.is_protected_turn(0, 5));
        assert!(strategy.is_protected_turn(2, 5));
        assert!(strategy.is_protected_turn(4, 5));
    }

    // =========================================================================
    // evaluate_part Tests - Protected Turns
    // =========================================================================

    #[test]
    fn test_evaluate_part_protected_turn_returns_keep() {
        let strategy = SmartCompactionStrategy::new().with_protected_turns(2);

        let part = SessionPart::UserInput(UserInputPart {
            text: "Test input".to_string(),
            context: None,
            timestamp: 1000,
        });

        // Turn index 4 in a 5-turn session is protected
        let action = strategy.evaluate_part(&part, 4, 5);
        assert_eq!(action, CompactionAction::Keep);
    }

    // =========================================================================
    // evaluate_part Tests - Protected Tools
    // =========================================================================

    #[test]
    fn test_evaluate_part_protected_tool_returns_keep() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0) // Disable turn protection for this test
            .add_protected_tool("skill");

        let large_output = "x".repeat(5000); // Larger than default 2000

        let part = SessionPart::ToolCall(ToolCallPart {
            id: "call-1".to_string(),
            tool_name: "skill".to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output: Some(large_output),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        // Even with large output, skill tool is protected
        let action = strategy.evaluate_part(&part, 0, 5);
        assert_eq!(action, CompactionAction::Keep);
    }

    // =========================================================================
    // evaluate_part Tests - Truncation
    // =========================================================================

    #[test]
    fn test_evaluate_part_large_output_returns_truncate() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(100);

        let large_output = "x".repeat(500);

        let part = SessionPart::ToolCall(ToolCallPart {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({"path": "/test.txt"}),
            status: ToolCallStatus::Completed,
            output: Some(large_output),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        let action = strategy.evaluate_part(&part, 0, 5);

        match action {
            CompactionAction::Truncate { max_chars, summary } => {
                assert_eq!(max_chars, 100);
                assert!(summary.contains("Truncated"));
                assert!(summary.contains("read_file"));
            }
            _ => panic!("Expected Truncate action, got {:?}", action),
        }
    }

    #[test]
    fn test_evaluate_part_small_output_returns_keep() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(2000);

        let small_output = "Small output".to_string();

        let part = SessionPart::ToolCall(ToolCallPart {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output: Some(small_output),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        let action = strategy.evaluate_part(&part, 0, 5);
        assert_eq!(action, CompactionAction::Keep);
    }

    #[test]
    fn test_evaluate_part_no_output_returns_keep() {
        let strategy = SmartCompactionStrategy::new().with_protected_turns(0);

        let part = SessionPart::ToolCall(ToolCallPart {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({}),
            status: ToolCallStatus::Pending,
            output: None, // No output yet
            error: None,
            started_at: 1000,
            completed_at: None,
        });

        let action = strategy.evaluate_part(&part, 0, 5);
        assert_eq!(action, CompactionAction::Keep);
    }

    // =========================================================================
    // evaluate_part Tests - Non-ToolCall Parts
    // =========================================================================

    #[test]
    fn test_evaluate_part_user_input_returns_keep() {
        let strategy = SmartCompactionStrategy::new().with_protected_turns(0);

        let part = SessionPart::UserInput(UserInputPart {
            text: "Very long user input ".repeat(1000),
            context: None,
            timestamp: 1000,
        });

        // User input parts are not truncated
        let action = strategy.evaluate_part(&part, 0, 5);
        assert_eq!(action, CompactionAction::Keep);
    }

    #[test]
    fn test_evaluate_part_ai_response_returns_keep() {
        let strategy = SmartCompactionStrategy::new().with_protected_turns(0);

        let part = SessionPart::AiResponse(AiResponsePart {
            content: "Very long AI response ".repeat(1000),
            reasoning: None,
            timestamp: 1000,
        });

        // AI response parts are not truncated
        let action = strategy.evaluate_part(&part, 0, 5);
        assert_eq!(action, CompactionAction::Keep);
    }

    // =========================================================================
    // truncate_output Tests
    // =========================================================================

    #[test]
    fn test_truncate_output_small_input() {
        let strategy = SmartCompactionStrategy::new().with_tool_output_max_chars(2000);
        let output = "Small output";

        let result = strategy.truncate_output(output, "test_tool");
        assert_eq!(result, output); // Should be unchanged
    }

    #[test]
    fn test_truncate_output_large_input() {
        let strategy = SmartCompactionStrategy::new().with_tool_output_max_chars(200);
        let output = "x".repeat(1000);

        let result = strategy.truncate_output(&output, "test_tool");

        assert!(result.len() <= 200);
        assert!(result.contains("[Truncated"));
        assert!(result.contains("test_tool"));
    }

    #[test]
    fn test_generate_truncation_summary() {
        let strategy = SmartCompactionStrategy::new().with_tool_output_max_chars(100);
        let output = "First line of output\nSecond line\nThird line".to_string();

        let summary = strategy.generate_truncation_summary(&output, "read_file");

        assert!(summary.contains("Truncated read_file"));
        assert!(summary.contains("First line"));
        assert!(summary.contains("B -> 100B")); // Size info
    }

    // =========================================================================
    // CompactionAction Helper Tests
    // =========================================================================

    #[test]
    fn test_compaction_action_is_keep() {
        assert!(CompactionAction::Keep.is_keep());
        assert!(!CompactionAction::RemoveOutput.is_keep());
        assert!(!CompactionAction::Truncate {
            max_chars: 100,
            summary: String::new()
        }
        .is_keep());
        assert!(!CompactionAction::Summarize { original_count: 3 }.is_keep());
    }

    #[test]
    fn test_compaction_action_modifies_part() {
        assert!(!CompactionAction::Keep.modifies_part());
        assert!(CompactionAction::RemoveOutput.modifies_part());
        assert!(CompactionAction::Truncate {
            max_chars: 100,
            summary: String::new()
        }
        .modifies_part());
        assert!(CompactionAction::Summarize { original_count: 3 }.modifies_part());
    }

    // =========================================================================
    // Integration / Edge Case Tests
    // =========================================================================

    #[test]
    fn test_protected_turn_takes_priority_over_truncation() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(2)
            .with_tool_output_max_chars(100);

        let large_output = "x".repeat(5000);

        let part = SessionPart::ToolCall(ToolCallPart {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output: Some(large_output),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        // Turn 4 in 5 turns is protected, so Keep despite large output
        let action = strategy.evaluate_part(&part, 4, 5);
        assert_eq!(action, CompactionAction::Keep);

        // Turn 2 in 5 turns is NOT protected, so Truncate
        let action = strategy.evaluate_part(&part, 2, 5);
        assert!(matches!(action, CompactionAction::Truncate { .. }));
    }

    #[test]
    fn test_protected_tool_takes_priority_over_truncation() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(100)
            .add_protected_tool("important_tool");

        let large_output = "x".repeat(5000);

        let protected_part = SessionPart::ToolCall(ToolCallPart {
            id: "call-1".to_string(),
            tool_name: "important_tool".to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output: Some(large_output.clone()),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        let normal_part = SessionPart::ToolCall(ToolCallPart {
            id: "call-2".to_string(),
            tool_name: "normal_tool".to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output: Some(large_output),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        // Protected tool: Keep
        assert_eq!(strategy.evaluate_part(&protected_part, 0, 5), CompactionAction::Keep);

        // Normal tool: Truncate
        assert!(matches!(
            strategy.evaluate_part(&normal_part, 0, 5),
            CompactionAction::Truncate { .. }
        ));
    }
}
