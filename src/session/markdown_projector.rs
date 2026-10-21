//! Markdown Projector - Projects session events to Markdown files in real-time.
//!
//! This projector implements the Read Side of the Event Sourcing + CQRS architecture:
//! - Write Side: `session_events` table (append-only log)
//! - Read Side: Markdown files in `~/.aleph/raw/sessions/`
//!
//! ## Architecture
//!
//! ```text
//! SessionEventStore (append)
//!         ↓
//! SessionEventObserver (on_appended)
//!         ↓
//! MarkdownProjector
//!         ↓
//! MarkdownSessionStore (write)
//!         ↓
//! ~/.aleph/raw/sessions/{sha256(session_id)}/
//!   ├── transcript.md
//!   ├── tool_outputs.md
//!   └── metadata.json
//! ```

use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::session::{
    Attachment, CompressionStatus, MessageRole, RawSessionStore, SessionMetadata, ToolResult,
};
use crate::providers::message::ContentBlock;
use crate::routing::session_key::SessionKey;
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::session::observer::SessionEventObserver;
use crate::session::service::SessionId;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Markdown projector that subscribes to session events and writes Markdown files.
pub struct MarkdownProjector {
    /// Sender for event queue (non-blocking notification)
    sender: mpsc::UnboundedSender<ProjectionTask>,
}

/// Projection task queued for background processing.
struct ProjectionTask {
    session_id: SessionId,
    record: SessionEventRecord,
}

impl MarkdownProjector {
    /// Create a new projector with a background worker.
    pub fn new(store: Arc<MarkdownSessionStore>) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();

        // Spawn background worker
        tokio::spawn(projection_worker(store, receiver));

        Self { sender }
    }
}

impl SessionEventObserver for MarkdownProjector {
    /// Non-blocking: enqueue event and return immediately.
    fn on_appended(&self, id: &SessionId, record: &SessionEventRecord) {
        let task = ProjectionTask {
            session_id: id.clone(),
            record: record.clone(),
        };

        if let Err(e) = self.sender.send(task) {
            error!(
                session_id = %id,
                seq = record.seq,
                "Failed to enqueue projection task: {}",
                e
            );
        }
    }
}

/// Background worker that processes projection tasks.
async fn projection_worker(
    store: Arc<MarkdownSessionStore>,
    mut receiver: mpsc::UnboundedReceiver<ProjectionTask>,
) {
    info!("Markdown projector worker started");

    while let Some(task) = receiver.recv().await {
        if let Err(e) = process_task(&store, task).await {
            error!("Projection task failed: {}", e);
        }
    }

    warn!("Markdown projector worker stopped");
}

/// Resolve the agent identity from the structured session key.
///
/// The wire form starts with the key family (`agent:`); the actual agent ID
/// is the second component for the common main/direct/group/task forms. Parse
/// the key instead of splitting the wire string so subagent and future key
/// variants keep their ownership semantics.
fn agent_id_for_session(session_id: &str) -> String {
    SessionKey::parse(session_id)
        .map(|key| key.agent_id().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
/// Ensure a session exists even when a UserMessage was persisted before RunStarted.
async fn ensure_session(
    store: &MarkdownSessionStore,
    session_id: &str,
    started_at: i64,
    workspace_id: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if store.get_session_metadata(session_id).await.is_ok() {
        return Ok(());
    }
    let agent_id = agent_id_for_session(session_id);
    store
        .create_session(SessionMetadata {
            session_id: session_id.to_string(),
            agent_id,
            workspace_id,
            started_at,
            ended_at: None,
            duration_seconds: None,
            transcript_path: PathBuf::from(format!("raw/sessions/{session_id}/transcript.md")),
            tool_outputs_path: PathBuf::from(format!("raw/sessions/{session_id}/tool_outputs.md")),
            message_count: 0,
            tool_call_count: 0,
            content_hash: None,
            compression_status: CompressionStatus::Pending,
            tags: vec![],
        })
        .await?;
    Ok(())
}

/// Persist image blocks carried by a user message as content-addressed attachments.
async fn persist_image_blocks(
    store: &MarkdownSessionStore,
    session_id: &str,
    blocks: &[serde_json::Value],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Prefer the explicit inbound marker so Image + marker is stored once.
    let mut marker_found = false;
    for block in blocks {
        let Some(attachment) = block
            .get("Json")
            .and_then(|value| value.get("value"))
            .and_then(|value| value.get("aleph_attachment"))
        else {
            continue;
        };
        let Some(data) = attachment.get("data").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(mime_type) = attachment
            .get("mime_type")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        store
            .store_attachment(
                session_id,
                Attachment {
                    data: BASE64.decode(data)?,
                    mime_type: mime_type.to_string(),
                    original_name: attachment
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                },
            )
            .await?;
        marker_found = true;
    }
    if marker_found {
        return Ok(());
    }

    // Backward-compatible fallback for events that only carry Image blocks.
    for block in blocks {
        if let Ok(ContentBlock::Image { data, mime_type }) =
            serde_json::from_value::<ContentBlock>(block.clone())
        {
            store
                .store_attachment(
                    session_id,
                    Attachment {
                        data: BASE64.decode(data)?,
                        mime_type,
                        original_name: None,
                    },
                )
                .await?;
        }
    }
    Ok(())
}

/// Process a single projection task.
async fn process_task(
    store: &MarkdownSessionStore,
    task: ProjectionTask,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let session_id = task.session_id.to_string();
    let event = &task.record.event;
    let timestamp = task.record.created_at_ms / 1000; // ms → s for transcript timestamps

    match event {
        SessionEvent::RunStarted {
            run_id,
            project_root,
            ..
        } => {
            // Create new session on first RunStarted event
            let agent_id = task.session_id.agent_id().to_string();
            let workspace_id = project_root.as_ref().map(|p| p.to_string());

            let metadata = SessionMetadata {
                session_id: session_id.clone(),
                agent_id: agent_id.clone(),
                workspace_id,
                started_at: timestamp,
                ended_at: None,
                duration_seconds: None,
                transcript_path: PathBuf::from(format!(
                    "raw/sessions/{}/transcript.md",
                    session_id
                )),
                tool_outputs_path: PathBuf::from(format!(
                    "raw/sessions/{}/tool_outputs.md",
                    session_id
                )),
                message_count: 0,
                tool_call_count: 0,
                content_hash: None,
                compression_status: CompressionStatus::Pending,
                tags: vec![],
            };

            store.create_session(metadata).await?;

            info!(
                session_id = %session_id,
                agent_id = %agent_id,
                run_id = %run_id,
                "Created Markdown session"
            );
        }

        SessionEvent::UserMessage { content, .. } => {
            ensure_session(store, &session_id, timestamp, None).await?;
            store
                .append_transcript(&session_id, timestamp, MessageRole::User, &content.text)
                .await?;
            persist_image_blocks(store, &session_id, &content.blocks).await?;
        }

        SessionEvent::AssistantMessage { content, .. } => {
            ensure_session(store, &session_id, timestamp, None).await?;
            store
                .append_transcript(
                    &session_id,
                    timestamp,
                    MessageRole::Assistant,
                    &content.text,
                )
                .await?;
        }

        SessionEvent::SystemMessage { content, .. } => {
            ensure_session(store, &session_id, timestamp, None).await?;
            store
                .append_transcript(&session_id, timestamp, MessageRole::System, content)
                .await?;
        }

        SessionEvent::ToolCallRequested { name, input, .. } => {
            ensure_session(store, &session_id, timestamp, None).await?;
            let result = ToolResult {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: 0,
            };
            store
                .append_tool_output(&session_id, timestamp, name, &input.to_string(), &result)
                .await?;
        }

        SessionEvent::ToolResult { output, .. } => {
            ensure_session(store, &session_id, timestamp, None).await?;
            let result = ToolResult {
                exit_code: Some(0),
                stdout: output.value.to_string(),
                stderr: String::new(),
                duration_ms: 0,
            };
            store
                .append_tool_output(&session_id, timestamp, "tool", "", &result)
                .await?;
        }

        SessionEvent::ToolError { error, .. } => {
            ensure_session(store, &session_id, timestamp, None).await?;
            let result = ToolResult {
                exit_code: Some(1),
                stdout: String::new(),
                stderr: error.clone(),
                duration_ms: 0,
            };
            store
                .append_tool_output(&session_id, timestamp, "tool", "", &result)
                .await?;
        }

        SessionEvent::Error { kind, message, .. } => {
            ensure_session(store, &session_id, timestamp, None).await?;
            let error_text = format!("[ERROR - {:?}] {}", kind, message);
            store
                .append_transcript(&session_id, timestamp, MessageRole::System, &error_text)
                .await?;
        }

        SessionEvent::RunFinished { .. } => {
            ensure_session(store, &session_id, timestamp, None).await?;
            store.finalize_session(&session_id, timestamp).await?;
            info!(session_id = %session_id, "Finalized Markdown session");
        }

        // Ignore internal events
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_projector_creates_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(MarkdownSessionStore::new(temp_dir.path()).await.unwrap());
        let projector = MarkdownProjector::new(store.clone());

        let session_id = crate::routing::session_key::SessionKey::ephemeral("test");
        let record = SessionEventRecord {
            seq: 1,
            event: SessionEvent::RunStarted {
                run_id: "test-run".to_string(),
                at: 1000,
                project_root: None,
                envelope: None,
                telecom_provenance: Default::default(),
            },
            created_at_ms: chrono::Utc::now().timestamp_millis(),
        };

        projector.on_appended(&session_id, &record);
        // The projector works through a background worker, so the write lands
        // when the worker gets to it — a fixed delay would be a race with the
        // scheduler. Wait for the effect instead of assuming a delay covers it.
        let session_dir = temp_dir.path().join("raw/sessions").join(
            crate::memory::store::markdown::session_directory_key(&session_id.to_string()),
        );
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while !session_dir.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the projector never created {session_dir:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        // Verify session directory exists
        assert!(session_dir.exists());
        let metadata = store
            .get_session_metadata(&session_id.to_string())
            .await
            .unwrap();
        assert_eq!(metadata.agent_id, "test");
    }
}
