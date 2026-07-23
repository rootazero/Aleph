//! Execution session types


use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::parts::SessionPart;
use super::status::SessionStatus;

/// Execution session - tracks the state of an agentic loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSession {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub status: SessionStatus,
    pub iteration_count: u32,
    pub total_tokens: u64,
    pub parts: Vec<SessionPart>,
    pub recent_calls: Vec<ToolCallRecord>,
    pub model: String,
    pub created_at: i64,
    pub updated_at: i64,

    // =========================================================================
    // Unified session model fields (from LoopState)
    // =========================================================================
    /// User's original request (from `LoopState`)
    #[serde(default)]
    pub original_request: String,

    /// Request context (attachments, selected files, clipboard, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,

    /// Session start timestamp (from `LoopState`, unix timestamp)
    #[serde(default)]
    pub started_at: i64,

    /// Whether session needs compaction (for `SessionCompactor` integration)
    #[serde(default)]
    pub needs_compaction: bool,

    /// Last compaction index (step index up to which compaction was applied)
    #[serde(default)]
    pub last_compaction_index: usize,
}

impl Default for ExecutionSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionSession {
    #[must_use]
    pub fn new() -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            agent_id: "main".into(),
            status: SessionStatus::Running,
            iteration_count: 0,
            total_tokens: 0,
            parts: Vec::new(),
            recent_calls: Vec::new(),
            model: "default".into(),
            created_at: now,
            updated_at: now,
            // Unified session model fields
            original_request: String::new(),
            context: None,
            started_at: now,
            needs_compaction: false,
            last_compaction_index: 0,
        }
    }

    #[must_use]
    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.into();
        self
    }

    /// Set the original request (builder pattern)
    pub fn with_original_request(mut self, request: impl Into<String>) -> Self {
        self.original_request = request.into();
        self
    }

    /// Set the request context (builder pattern)
    #[must_use]
    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }
}

/// Tool call record for doom loop detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool: String,
    pub input: Value,
    pub timestamp: i64,
}

