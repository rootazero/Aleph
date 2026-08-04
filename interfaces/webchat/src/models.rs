//! UI Domain Models
//!
//! These models mirror the structures in `shared_ui_logic` to ensure
//! seamless integration when we replace mock data with real SDK calls.

use serde::{Deserialize, Serialize};

/// Trace node representing a step in Agent's thinking process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceNode {
    /// Unique identifier
    pub id: String,
    /// Node type (thinking, `tool_call`, observation, etc.)
    pub node_type: TraceNodeType,
    /// Timestamp
    pub timestamp: f64,
    /// Duration in milliseconds
    pub duration_ms: Option<u64>,
    /// Content/message
    pub content: String,
    /// Status
    pub status: TraceStatus,
    /// Child nodes
    pub children: Vec<Self>,
}

/// Trace node type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TraceNodeType {
    /// Agent thinking
    Thinking,
    /// Tool call
    ToolCall,
    /// Tool result
    ToolResult,
    /// Observation
    Observation,
    /// Decision
    Decision,
}

/// Trace status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatus {
    /// Pending
    Pending,
    /// In progress
    InProgress,
    /// Success
    Success,
    /// Failed
    Failed,
}

/// Memory statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    /// Total number of facts
    pub count: u64,
    /// Total size in bytes
    pub size_bytes: u64,
    /// Number of apps
    pub apps_count: u32,
}

/// Memory search item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchItem {
    /// Fact ID
    pub id: String,
    /// Content
    pub content: String,
    /// Relevance score
    pub score: f32,
    /// Timestamp
    pub timestamp: f64,
}

/// Tool metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMetrics {
    /// Tool name
    pub name: String,
    /// Total calls
    pub total_calls: u64,
    /// Successful calls
    pub success_count: u64,
    /// Failed calls
    pub failed_count: u64,
    /// Average duration in milliseconds
    pub avg_duration_ms: f64,
}

impl TraceNode {
    /// Get CSS class for status
    #[must_use]
    pub const fn status_class(&self) -> &'static str {
        match self.status {
            TraceStatus::Pending => "bg-surface-sunken",
            TraceStatus::InProgress => "bg-info-subtle",
            TraceStatus::Success => "bg-success-subtle",
            TraceStatus::Failed => "bg-danger-subtle",
        }
    }
}
