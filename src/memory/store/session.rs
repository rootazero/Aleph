//! Session storage abstraction for Markdown-based raw memory.
//!
//! Replaces the legacy `raw_memories` SQLite table with Markdown files.
//! SQLite is retained only for IDs, indexes, and metadata.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::AlephError;

// ---------------------------------------------------------------------------
// Data Structures
// ---------------------------------------------------------------------------

/// Metadata for a session stored in Markdown format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub agent_id: String,
    pub workspace_id: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_seconds: Option<i64>,
    pub transcript_path: PathBuf,
    pub tool_outputs_path: PathBuf,
    pub message_count: i64,
    pub tool_call_count: i64,
    pub content_hash: Option<String>,
    pub compression_status: CompressionStatus,
    pub tags: Vec<String>,
}

/// Compression status for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionStatus {
    Pending,
    Compressed,
    Archived,
}

impl CompressionStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Compressed => "compressed",
            Self::Archived => "archived",
        }
    }

    /// Infallible labelled parse: pairs with [`Self::as_str`]. Unknown labels
    /// map to [`Self::Pending`] (the inferred-future-state default) rather than
    /// failing, because the on-disk column may carry a label written by a newer
    /// build than the one reading it — fail closed is fine, but failing *open*
    /// would lose the row. If you want strict parsing, derive `FromStr` here.
    #[must_use]
    pub fn from_label(s: &str) -> Self {
        match s {
            "compressed" => Self::Compressed,
            "archived" => Self::Archived,
            _ => Self::Pending,
        }
    }
}

/// Role in a conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

impl MessageRole {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Assistant => "Assistant",
            Self::System => "System",
        }
    }
}

/// Binary attachment data.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub original_name: Option<String>,
}

/// Reference to a stored attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub attachment_id: String,
    pub file_path: PathBuf,
    pub content_hash: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub uploaded_at: i64,
}

/// Information about an attachment stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub attachment_id: String,
    pub session_id: String,
    pub file_path: String,
    pub content_hash: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub uploaded_at: i64,
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// Session search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSearchResult {
    pub session_id: String,
    pub agent_id: String,
    pub started_at: i64,
    pub snippet: String,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// RawSessionStore Trait
// ---------------------------------------------------------------------------

/// Storage abstraction for Markdown-based session data.
///
/// Sessions are stored as:
/// - `~/.aleph/raw/sessions/{sha256(session_id)}/transcript.md`
/// - `~/.aleph/raw/sessions/{sha256(session_id)}/tool_outputs.md`
/// - `~/.aleph/raw/sessions/{sha256(session_id)}/metadata.json`
/// - Attachments in `~/.aleph/raw/attachments/{type}/{hash}.{ext}`
///
/// SQLite `metadata.db` stores only IDs, indexes, and derived data.
#[async_trait]
pub trait RawSessionStore: Send + Sync {
    /// Create a new session with its directory and initial files.
    async fn create_session(&self, metadata: SessionMetadata) -> Result<(), AlephError>;

    /// Append a message to the session transcript.
    async fn append_transcript(
        &self,
        session_id: &str,
        timestamp: i64,
        role: MessageRole,
        content: &str,
    ) -> Result<(), AlephError>;

    /// Append a tool execution result to the tool outputs file.
    async fn append_tool_output(
        &self,
        session_id: &str,
        timestamp: i64,
        tool_name: &str,
        args: &str,
        result: &ToolResult,
    ) -> Result<(), AlephError>;

    /// Store an attachment and return its reference.
    ///
    /// The attachment is written to a content-addressed file and registered
    /// in the SQLite metadata database.
    async fn store_attachment(
        &self,
        session_id: &str,
        attachment: Attachment,
    ) -> Result<AttachmentRef, AlephError>;

    /// Finalize a session (set end time, compute stats, update hash).
    async fn finalize_session(&self, session_id: &str, ended_at: i64) -> Result<(), AlephError>;

    /// Read the full transcript content for a session.
    async fn read_transcript(&self, session_id: &str) -> Result<String, AlephError>;

    /// Read the full tool outputs content for a session.
    async fn read_tool_outputs(&self, session_id: &str) -> Result<String, AlephError>;

    /// List sessions for an agent with pagination and filters.
    async fn list_sessions(
        &self,
        agent_id: Option<&str>,
        workspace_id: Option<&str>,
        start_date: Option<i64>,
        end_date: Option<i64>,
        compression_status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SessionMetadata>, AlephError>;

    /// Get metadata for a specific session.
    async fn get_session_metadata(&self, session_id: &str) -> Result<SessionMetadata, AlephError>;

    /// List all attachments for a session.
    async fn list_attachments(&self, session_id: &str) -> Result<Vec<AttachmentInfo>, AlephError>;

    /// Get a specific attachment's data.
    async fn get_attachment(
        &self,
        session_id: &str,
        attachment_id: &str,
    ) -> Result<Attachment, AlephError>;

    /// Search across session transcripts using FTS5 index.
    async fn search_transcripts(
        &self,
        query: &str,
        agent_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SessionSearchResult>, AlephError>;

    /// Update the compression status of a session.
    async fn update_compression_status(
        &self,
        session_id: &str,
        status: CompressionStatus,
    ) -> Result<(), AlephError>;

    /// Archive a session (move to archive directory and update metadata).
    async fn archive_session(&self, session_id: &str) -> Result<(), AlephError>;
}
