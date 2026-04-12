use crate::error::AlephError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Source of raw memory data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawMemorySource {
    SessionCompressed,
    Transcript,
    ToolOutput,
    Attachment,
}

impl RawMemorySource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SessionCompressed => "session_compressed",
            Self::Transcript => "transcript",
            Self::ToolOutput => "tool_output",
            Self::Attachment => "attachment",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "session_compressed" => Self::SessionCompressed,
            "transcript" => Self::Transcript,
            "tool_output" => Self::ToolOutput,
            "attachment" => Self::Attachment,
            _ => Self::ToolOutput,
        }
    }
}

/// A raw memory record — ephemeral data consumed by CompressionService.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMemory {
    pub id: String,
    pub content: String,
    pub source: RawMemorySource,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub path: Option<String>,
    pub layer: Option<String>,
    pub attachment_text: Option<String>,
    pub is_processed: bool,
    pub created_at: i64,
}

impl RawMemory {
    pub fn new(content: String, source: RawMemorySource) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content,
            source,
            agent_id: "default".to_string(),
            session_id: None,
            path: None,
            layer: None,
            attachment_text: None,
            is_processed: false,
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = agent_id.into();
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_layer(mut self, layer: impl Into<String>) -> Self {
        self.layer = Some(layer.into());
        self
    }

    pub fn with_attachment_text(mut self, text: impl Into<String>) -> Self {
        self.attachment_text = Some(text.into());
        self
    }
}

/// Storage trait for raw memory records.
#[async_trait]
pub trait RawMemoryStore: Send + Sync {
    /// Insert a raw memory record.
    async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError>;

    /// Get unprocessed raw memories for an agent, ordered by created_at ASC.
    async fn get_unprocessed_raw_memories(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError>;

    /// Mark raw memories as processed after CompressionService consumes them.
    async fn mark_raw_as_processed(&self, ids: &[String]) -> Result<usize, AlephError>;

    /// Count unprocessed raw memories for an agent.
    async fn count_unprocessed(&self, agent_id: &str) -> Result<usize, AlephError>;
}
