//! Context statistics and visualization (stubbed)
//!
//! Previously depended on LoopState from the old OTAF agent loop.

use serde::{Deserialize, Serialize};

/// Context usage statistics (stubbed)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextStats {
    pub total_tokens: usize,
    pub max_tokens: usize,
    pub step_count: usize,
    pub compressed_steps: usize,
    pub summary_size: usize,
    pub remaining_tokens: usize,
    pub usage_percent: f32,
    pub usage_breakdown: UsageBreakdown,
}

impl ContextStats {
    /// Get a formatted summary for display
    pub fn summary(&self) -> String {
        format!(
            "Context Usage: {}/{} tokens ({:.1}%)",
            self.total_tokens, self.max_tokens, self.usage_percent,
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
    pub system_prompt: usize,
    pub user_messages: usize,
    pub assistant_responses: usize,
    pub tool_usage: usize,
    pub history_summary: usize,
}

/// Warning level for context usage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningLevel {
    Normal,
    Notice,
    Warning,
    Critical,
}

/// Compression focus options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum CompressionFocus {
    #[default]
    Balanced,
    PreserveTools,
    PreserveReasoning,
    Aggressive,
    PreserveConversation,
}

impl CompressionFocus {
    pub fn description(&self) -> &'static str {
        match self {
            CompressionFocus::Balanced => "Balanced compression preserving key information",
            CompressionFocus::PreserveTools => "Preserve tool outputs and file changes",
            CompressionFocus::PreserveReasoning => "Preserve reasoning and decision process",
            CompressionFocus::Aggressive => "Aggressive compression for maximum token savings",
            CompressionFocus::PreserveConversation => "Preserve conversation flow and context",
        }
    }

    pub fn compression_ratio(&self) -> f32 {
        match self {
            CompressionFocus::Balanced => 0.5,
            CompressionFocus::PreserveTools => 0.7,
            CompressionFocus::PreserveReasoning => 0.6,
            CompressionFocus::Aggressive => 0.2,
            CompressionFocus::PreserveConversation => 0.65,
        }
    }

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
