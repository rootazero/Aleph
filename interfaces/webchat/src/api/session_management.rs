//! Session management API (Markdown-backed session storage).

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompressionStatus {
    Pending,
    Compressed,
    Archived,
}

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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionData {
    pub metadata: SessionMetadata,
    pub transcript: String,
    pub tool_outputs: String,
    pub attachments: Vec<AttachmentInfo>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionListFilters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_date: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression_status: Option<CompressionStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionMetadata>,
    pub total: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionSearchResult {
    pub session_id: String,
    pub agent_id: String,
    pub started_at: i64,
    pub snippet: String,
    pub score: f32,
    pub metadata: SessionMetadata,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionSearchResponse {
    pub results: Vec<SessionSearchResult>,
}

pub struct SessionsApi;

impl SessionsApi {
    pub async fn list(
        state: &DashboardState,
        filters: SessionListFilters,
    ) -> Result<SessionListResponse, String> {
        let params = serde_json::to_value(filters)
            .map_err(|e| format!("Failed to serialize filters: {e}"))?;
        let response = state.rpc_call("memory.sessions.list", params).await?;
        serde_json::from_value(response).map_err(|e| format!("Failed to parse session list: {e}"))
    }

    pub async fn get(state: &DashboardState, session_id: &str) -> Result<SessionData, String> {
        let response = state
            .rpc_call("memory.sessions.get", json!({"session_id": session_id}))
            .await?;
        serde_json::from_value(response).map_err(|e| format!("Failed to parse session data: {e}"))
    }

    pub async fn search(
        state: &DashboardState,
        query: &str,
        limit: Option<usize>,
    ) -> Result<SessionSearchResponse, String> {
        let response = state
            .rpc_call(
                "memory.sessions.search",
                json!({"query": query, "limit": limit.unwrap_or(20)}),
            )
            .await?;
        serde_json::from_value(response).map_err(|e| format!("Failed to parse search results: {e}"))
    }

    pub async fn update_compression(
        state: &DashboardState,
        session_id: &str,
        status: CompressionStatus,
    ) -> Result<(), String> {
        state
            .rpc_call(
                "memory.sessions.update_compression",
                json!({"session_id": session_id, "status": status}),
            )
            .await?;
        Ok(())
    }

    pub fn attachment_url(session_id: &str, attachment_id: &str) -> String {
        format!("/api/memory/sessions/attachment/{session_id}:{attachment_id}")
    }
}
