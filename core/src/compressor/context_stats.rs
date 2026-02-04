//! Context statistics and visualization
//!
//! This module provides context usage tracking and visualization,
//! similar to Claude Code's `/context` command.

use serde::{Deserialize, Serialize};

use crate::agent_loop::LoopState;

/// Context usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextStats {
    /// Total tokens used
    pub total_tokens: usize,
    /// Maximum tokens allowed
    pub max_tokens: usize,
    /// Number of steps in history
    pub step_count: usize,
    /// Number of compressed steps
    pub compressed_steps: usize,
    /// Size of history summary (characters)
    pub summary_size: usize,
    /// Estimated remaining tokens
    pub remaining_tokens: usize,
    /// Usage percentage (0-100)
    pub usage_percent: f32,
    /// Categories of token usage
    pub usage_breakdown: UsageBreakdown,
}

impl ContextStats {
    /// Create stats from loop state
    pub fn from_state(state: &LoopState, max_tokens: usize) -> Self {
        let compressed_steps = state.compressed_until_step;
        let summary_size = state.history_summary.len();
        let remaining = max_tokens.saturating_sub(state.total_tokens);
        let usage_percent = if max_tokens > 0 {
            (state.total_tokens as f32 / max_tokens as f32 * 100.0).min(100.0)
        } else {
            0.0
        };

        // Calculate usage breakdown
        let breakdown = UsageBreakdown::from_state(state);

        Self {
            total_tokens: state.total_tokens,
            max_tokens,
            step_count: state.steps.len(),
            compressed_steps,
            summary_size,
            remaining_tokens: remaining,
            usage_percent,
            usage_breakdown: breakdown,
        }
    }

    /// Get a formatted summary for display
    pub fn summary(&self) -> String {
        format!(
            "Context Usage: {}/{} tokens ({:.1}%)\n\
             Steps: {} total ({} compressed)\n\
             Remaining: {} tokens",
            self.total_tokens,
            self.max_tokens,
            self.usage_percent,
            self.step_count,
            self.compressed_steps,
            self.remaining_tokens
        )
    }

    /// Get a detailed report
    pub fn detailed_report(&self) -> String {
        format!(
            "╭─ Context Statistics ─────────────────────────╮\n\
             │ Total Tokens:     {:>8} / {:<8}         │\n\
             │ Usage:            {:>6.1}%                    │\n\
             │ Remaining:        {:>8}                    │\n\
             ├──────────────────────────────────────────────┤\n\
             │ Steps:            {:>8}                    │\n\
             │ Compressed:       {:>8}                    │\n\
             │ Summary Size:     {:>8} chars             │\n\
             ├── Breakdown ─────────────────────────────────┤\n\
             {}╰──────────────────────────────────────────────╯",
            self.total_tokens,
            self.max_tokens,
            self.usage_percent,
            self.remaining_tokens,
            self.step_count,
            self.compressed_steps,
            self.summary_size,
            self.usage_breakdown.format_lines()
        )
    }

    /// Get warning level based on usage
    pub fn warning_level(&self) -> WarningLevel {
        if self.usage_percent >= 90.0 {
            WarningLevel::Critical
        } else if self.usage_percent >= 75.0 {
            WarningLevel::Warning
        } else if self.usage_percent >= 50.0 {
            WarningLevel::Notice
        } else {
            WarningLevel::Normal
        }
    }

    /// Check if context is running low
    pub fn is_low(&self) -> bool {
        self.usage_percent >= 75.0
    }

    /// Check if context is critically low
    pub fn is_critical(&self) -> bool {
        self.usage_percent >= 90.0
    }
}

/// Breakdown of token usage by category
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageBreakdown {
    /// Tokens used by system prompt
    pub system_prompt: usize,
    /// Tokens used by user messages
    pub user_messages: usize,
    /// Tokens used by assistant responses
    pub assistant_responses: usize,
    /// Tokens used by tool calls/results
    pub tool_usage: usize,
    /// Tokens used by compressed history
    pub history_summary: usize,
}

impl UsageBreakdown {
    /// Create breakdown from loop state
    fn from_state(state: &LoopState) -> Self {
        let mut tool_usage = 0;
        let mut assistant_responses = 0;

        for step in &state.steps {
            // Estimate tokens based on step content
            if let Some(ref reasoning) = step.thinking.reasoning {
                assistant_responses += estimate_tokens(reasoning);
            }
            tool_usage += step.tokens_used / 2; // Rough estimate
        }

        Self {
            system_prompt: 500, // Typical system prompt size
            user_messages: estimate_tokens(&state.original_request),
            assistant_responses,
            tool_usage,
            history_summary: estimate_tokens(&state.history_summary),
        }
    }

    /// Format for display
    fn format_lines(&self) -> String {
        format!(
            "│   System Prompt:  {:>8}                    │\n\
             │   User Messages:  {:>8}                    │\n\
             │   Assistant:      {:>8}                    │\n\
             │   Tool Usage:     {:>8}                    │\n\
             │   History:        {:>8}                    │\n",
            self.system_prompt,
            self.user_messages,
            self.assistant_responses,
            self.tool_usage,
            self.history_summary
        )
    }

    /// Get total estimated tokens
    pub fn total(&self) -> usize {
        self.system_prompt
            + self.user_messages
            + self.assistant_responses
            + self.tool_usage
            + self.history_summary
    }
}

/// Warning level for context usage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningLevel {
    /// Normal usage (< 50%)
    Normal,
    /// Notice level (50-75%)
    Notice,
    /// Warning level (75-90%)
    Warning,
    /// Critical level (> 90%)
    Critical,
}

impl WarningLevel {
    /// Get display string
    pub fn as_str(&self) -> &'static str {
        match self {
            WarningLevel::Normal => "normal",
            WarningLevel::Notice => "notice",
            WarningLevel::Warning => "warning",
            WarningLevel::Critical => "critical",
        }
    }

    /// Get color code for terminal display
    pub fn color(&self) -> &'static str {
        match self {
            WarningLevel::Normal => "\x1b[32m",   // Green
            WarningLevel::Notice => "\x1b[33m",   // Yellow
            WarningLevel::Warning => "\x1b[38;5;208m", // Orange
            WarningLevel::Critical => "\x1b[31m", // Red
        }
    }
}

/// Compression focus options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum CompressionFocus {
    /// Default compression - balanced approach
    #[default]
    Balanced,
    /// Preserve tool outputs and file changes
    PreserveTools,
    /// Preserve reasoning and decision process
    PreserveReasoning,
    /// Aggressive compression - minimal retention
    Aggressive,
    /// Preserve conversation context
    PreserveConversation,
}


impl CompressionFocus {
    /// Get description of this focus
    pub fn description(&self) -> &'static str {
        match self {
            CompressionFocus::Balanced => "Balanced compression preserving key information",
            CompressionFocus::PreserveTools => "Preserve tool outputs and file changes",
            CompressionFocus::PreserveReasoning => "Preserve reasoning and decision process",
            CompressionFocus::Aggressive => "Aggressive compression for maximum token savings",
            CompressionFocus::PreserveConversation => "Preserve conversation flow and context",
        }
    }

    /// Get compression ratio hint (0.0-1.0, lower = more aggressive)
    pub fn compression_ratio(&self) -> f32 {
        match self {
            CompressionFocus::Balanced => 0.5,
            CompressionFocus::PreserveTools => 0.7,
            CompressionFocus::PreserveReasoning => 0.6,
            CompressionFocus::Aggressive => 0.2,
            CompressionFocus::PreserveConversation => 0.65,
        }
    }

    /// Get list of all focus options
    pub fn all() -> &'static [CompressionFocus] {
        &[
            CompressionFocus::Balanced,
            CompressionFocus::PreserveTools,
            CompressionFocus::PreserveReasoning,
            CompressionFocus::Aggressive,
            CompressionFocus::PreserveConversation,
        ]
    }
}

/// Estimate tokens from text (rough approximation: 4 chars per token)
fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::RequestContext;

    #[test]
    fn test_context_stats_from_state() {
        let mut state = LoopState::new(
            "test".to_string(),
            "Test request".to_string(),
            RequestContext::empty(),
        );
        state.total_tokens = 5000;

        let stats = ContextStats::from_state(&state, 10000);

        assert_eq!(stats.total_tokens, 5000);
        assert_eq!(stats.max_tokens, 10000);
        assert_eq!(stats.usage_percent, 50.0);
        assert_eq!(stats.remaining_tokens, 5000);
        assert_eq!(stats.warning_level(), WarningLevel::Notice);
    }

    #[test]
    fn test_warning_levels() {
        let mut state = LoopState::new(
            "test".to_string(),
            "request".to_string(),
            RequestContext::empty(),
        );

        // Normal
        state.total_tokens = 4000;
        let stats = ContextStats::from_state(&state, 10000);
        assert_eq!(stats.warning_level(), WarningLevel::Normal);

        // Notice
        state.total_tokens = 6000;
        let stats = ContextStats::from_state(&state, 10000);
        assert_eq!(stats.warning_level(), WarningLevel::Notice);

        // Warning
        state.total_tokens = 8000;
        let stats = ContextStats::from_state(&state, 10000);
        assert_eq!(stats.warning_level(), WarningLevel::Warning);

        // Critical
        state.total_tokens = 9500;
        let stats = ContextStats::from_state(&state, 10000);
        assert_eq!(stats.warning_level(), WarningLevel::Critical);
    }

    #[test]
    fn test_compression_focus() {
        assert_eq!(CompressionFocus::default(), CompressionFocus::Balanced);
        assert!(CompressionFocus::Aggressive.compression_ratio() < CompressionFocus::Balanced.compression_ratio());
        assert_eq!(CompressionFocus::all().len(), 5);
    }

    #[test]
    fn test_summary_format() {
        let stats = ContextStats {
            total_tokens: 5000,
            max_tokens: 10000,
            step_count: 10,
            compressed_steps: 5,
            summary_size: 200,
            remaining_tokens: 5000,
            usage_percent: 50.0,
            usage_breakdown: UsageBreakdown::default(),
        };

        let summary = stats.summary();
        assert!(summary.contains("5000/10000"));
        assert!(summary.contains("50.0%"));
        assert!(summary.contains("10 total"));
    }
}
