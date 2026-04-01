//! Session status and reminder types

use serde::{Deserialize, Serialize};

/// Session execution status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionStatus {
    Running,
    Completed,
    Failed(String),
    Paused,
    Compacting,
}

/// Type of system reminder
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReminderType {
    /// Multi-step task context reminder
    ContinueTask,
    /// Approaching max steps warning
    MaxStepsWarning { current: usize, max: usize },
    /// Approaching token limit warning
    TokenLimitWarning { usage_percent: u8 },
    /// Plan mode reminder
    PlanMode { plan_file: String },
    /// Custom reminder (from plugins/skills)
    Custom { source: String },
}

/// System reminder part for context injection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemReminderPart {
    /// Reminder content
    pub content: String,
    /// Type of reminder
    pub reminder_type: ReminderType,
    /// Timestamp when created
    pub timestamp: i64,
}
