//! Supporting types for SessionRecorder.

/// Session record from database
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub status: String,
    pub model: String,
    pub iteration_count: u32,
    pub total_tokens: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Error type for SessionRecorder operations
#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Lock error: {0}")]
    Lock(String),

    #[error("Serialization error: {0}")]
    Serialization(String),
}
