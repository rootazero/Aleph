//! Session recorder component - persists session state to SQLite.
//!
//! Subscribes to: All events (for recording)
//! Publishes: None (recording only)

use async_trait::async_trait;
use rusqlite::{params, Connection};
use std::path::Path;
use crate::sync_primitives::{Arc, Mutex};

use crate::event::{
    AlephEvent,
    // Event data types
    AiResponse,
    EventContext,
    EventHandler,
    EventType,
    HandlerError,
    InputEvent,
    TaskPlan,
    ToolCallError,
    ToolCallResult,
};

use super::{AiResponsePart, PlanPart, PlanStep, SessionPart, StepStatus, ToolCallPart, ToolCallStatus, UserInputPart};

// ============================================================================
// SessionRecorder Component
// ============================================================================

/// Session Recorder - persists execution state to SQLite
///
/// This component:
/// - Subscribes to ALL events (EventType::All)
/// - Converts events to SessionPart records
/// - Persists parts to SQLite database
/// - Tracks session metadata (iteration count, tokens, timestamps)
pub struct SessionRecorder {
    /// SQLite connection (thread-safe)
    conn: Arc<Mutex<Connection>>,
}

impl SessionRecorder {
    /// Create a new SessionRecorder with an in-memory database
    ///
    /// Useful for testing or ephemeral sessions.
    pub fn new_in_memory() -> Result<Self, RecorderError> {
        let conn =
            Connection::open_in_memory().map_err(|e| RecorderError::Database(e.to_string()))?;

        Self::init_schema(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Create a new SessionRecorder with a file-based database
    ///
    /// Creates the database file if it doesn't exist.
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, RecorderError> {
        let conn = Connection::open(db_path).map_err(|e| RecorderError::Database(e.to_string()))?;

        Self::init_schema(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Initialize the database schema
    ///
    /// Creates the following tables:
    /// - `sessions`: Session metadata (id, parent_id, agent_id, status, model, etc.)
    /// - `session_parts`: Fine-grained execution records (id, session_id, part_type, etc.)
    pub fn init_schema(conn: &Connection) -> Result<(), RecorderError> {
        // Create sessions table
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                parent_id TEXT,
                agent_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'running',
                model TEXT NOT NULL,
                iteration_count INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )
            "#,
            [],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        // Create session_parts table
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS session_parts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                part_type TEXT NOT NULL,
                part_data TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            )
            "#,
            [],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        // Create index on session_id for efficient lookups
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_session_parts_session_id ON session_parts(session_id)",
            [],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        // Create index on parent_id for hierarchical queries
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sessions_parent_id ON sessions(parent_id)",
            [],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        Ok(())
    }

    /// Create a new session record
    ///
    /// Inserts a new session with the given ID and model.
    /// Sets created_at and updated_at to current timestamp.
    pub fn create_session(&self, session_id: &str, model: &str) -> Result<(), RecorderError> {
        self.create_session_with_options(session_id, model, None, "main")
    }

    /// Create a new session record with full options
    pub fn create_session_with_options(
        &self,
        session_id: &str,
        model: &str,
        parent_id: Option<&str>,
        agent_id: &str,
    ) -> Result<(), RecorderError> {
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        conn.execute(
            r#"
            INSERT INTO sessions (id, parent_id, agent_id, status, model, iteration_count, total_tokens, created_at, updated_at)
            VALUES (?1, ?2, ?3, 'running', ?4, 0, 0, ?5, ?5)
            "#,
            params![session_id, parent_id, agent_id, model, now],
        ).map_err(|e| RecorderError::Database(e.to_string()))?;

        Ok(())
    }

    /// Update session metadata
    ///
    /// Updates the iteration count and updated_at timestamp.
    /// Optionally updates status and token count.
    pub fn update_session(&self, session_id: &str) -> Result<(), RecorderError> {
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        conn.execute(
            r#"
            UPDATE sessions
            SET iteration_count = iteration_count + 1, updated_at = ?1
            WHERE id = ?2
            "#,
            params![now, session_id],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        Ok(())
    }

    /// Update session with specific values
    pub fn update_session_full(
        &self,
        session_id: &str,
        status: Option<&str>,
        iteration_count: Option<u32>,
        total_tokens: Option<u64>,
    ) -> Result<(), RecorderError> {
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        // Build dynamic update query
        // ?1 = now, then optional fields, last param = session_id
        let mut updates: Vec<String> = vec!["updated_at = ?1".to_string()];
        let mut param_index = 2;

        if status.is_some() {
            updates.push(format!("status = ?{}", param_index));
            param_index += 1;
        }
        if iteration_count.is_some() {
            updates.push(format!("iteration_count = ?{}", param_index));
            param_index += 1;
        }
        if total_tokens.is_some() {
            updates.push(format!("total_tokens = ?{}", param_index));
            param_index += 1;
        }

        // session_id is always the last parameter
        let query = format!(
            "UPDATE sessions SET {} WHERE id = ?{}",
            updates.join(", "),
            param_index
        );

        // Execute with appropriate parameters
        match (status, iteration_count, total_tokens) {
            (Some(s), Some(ic), Some(tt)) => {
                conn.execute(&query, params![now, s, ic, tt, session_id])
            }
            (Some(s), Some(ic), None) => conn.execute(&query, params![now, s, ic, session_id]),
            (Some(s), None, Some(tt)) => conn.execute(&query, params![now, s, tt, session_id]),
            (Some(s), None, None) => conn.execute(&query, params![now, s, session_id]),
            (None, Some(ic), Some(tt)) => conn.execute(&query, params![now, ic, tt, session_id]),
            (None, Some(ic), None) => conn.execute(&query, params![now, ic, session_id]),
            (None, None, Some(tt)) => conn.execute(&query, params![now, tt, session_id]),
            (None, None, None) => conn.execute(&query, params![now, session_id]),
        }
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        Ok(())
    }

    /// Append a session part to the database
    ///
    /// Gets the next sequence number for the session and inserts the part.
    pub fn append_part(&self, session_id: &str, part: &SessionPart) -> Result<i64, RecorderError> {
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        // Get next sequence number for this session
        let sequence: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM session_parts WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap_or(1);

        // Serialize part to JSON
        let part_type = part.type_name();
        let part_data =
            serde_json::to_string(part).map_err(|e| RecorderError::Serialization(e.to_string()))?;

        // Insert the part
        conn.execute(
            r#"
            INSERT INTO session_parts (session_id, part_type, part_data, sequence, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![session_id, part_type, part_data, sequence, now],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        let id = conn.last_insert_rowid();

        Ok(id)
    }

    /// Convert an AlephEvent to a SessionPart
    ///
    /// Returns None for events that don't map to session parts.
    pub fn event_to_part(event: &AlephEvent) -> Option<SessionPart> {
        match event {
            AlephEvent::InputReceived(input) => Some(Self::input_to_part(input)),
            AlephEvent::ToolCallCompleted(result) => Some(Self::tool_result_to_part(result)),
            AlephEvent::ToolCallFailed(error) => Some(Self::tool_error_to_part(error)),
            AlephEvent::AiResponseGenerated(response) => Some(Self::ai_response_to_part(response)),
            AlephEvent::PlanCreated(plan) => Some(Self::plan_to_part(plan)),
            // Events that don't map to session parts
            AlephEvent::PlanRequested(_)
            | AlephEvent::ToolCallRequested(_)
            | AlephEvent::ToolCallStarted(_)
            | AlephEvent::ToolCallRetrying(_)
            | AlephEvent::LoopContinue(_)
            | AlephEvent::LoopStop(_)
            | AlephEvent::SessionCreated(_)
            | AlephEvent::SessionUpdated(_)
            | AlephEvent::SessionResumed(_)
            | AlephEvent::SessionCompacted(_)
            | AlephEvent::SubAgentStarted(_)
            | AlephEvent::SubAgentCompleted(_)
            | AlephEvent::UserQuestionAsked(_)
            | AlephEvent::UserResponseReceived(_)
            // Permission system events (handled separately)
            | AlephEvent::PermissionAsked(_)
            | AlephEvent::PermissionReplied { .. }
            // Question system events (handled separately)
            | AlephEvent::QuestionAsked(_)
            | AlephEvent::QuestionReplied { .. }
            | AlephEvent::QuestionRejected { .. }
            // Part update events are meta-events, not session parts themselves
            | AlephEvent::PartAdded(_)
            | AlephEvent::PartUpdated(_)
            | AlephEvent::PartRemoved(_) => None,
        }
    }

    /// Convert InputEvent to UserInputPart
    fn input_to_part(input: &InputEvent) -> SessionPart {
        SessionPart::UserInput(UserInputPart {
            text: input.text.clone(),
            context: input
                .context
                .as_ref()
                .map(|ctx| serde_json::to_string(ctx).unwrap_or_default()),
            timestamp: input.timestamp,
        })
    }

    /// Convert ToolCallResult to ToolCallPart
    fn tool_result_to_part(result: &ToolCallResult) -> SessionPart {
        SessionPart::ToolCall(ToolCallPart {
            id: result.call_id.clone(),
            tool_name: result.tool.clone(),
            input: result.input.clone(),
            status: ToolCallStatus::Completed,
            output: Some(result.output.clone()),
            error: None,
            started_at: result.started_at,
            completed_at: Some(result.completed_at),
        })
    }

    /// Convert ToolCallError to ToolCallPart
    fn tool_error_to_part(error: &ToolCallError) -> SessionPart {
        SessionPart::ToolCall(ToolCallPart {
            id: error.call_id.clone(),
            tool_name: error.tool.clone(),
            input: serde_json::Value::Null,
            status: ToolCallStatus::Failed,
            output: None,
            error: Some(error.error.clone()),
            started_at: chrono::Utc::now().timestamp(),
            completed_at: Some(chrono::Utc::now().timestamp()),
        })
    }

    /// Convert AiResponse to AiResponsePart
    fn ai_response_to_part(response: &AiResponse) -> SessionPart {
        SessionPart::AiResponse(AiResponsePart {
            content: response.content.clone(),
            reasoning: response.reasoning.clone(),
            timestamp: response.timestamp,
        })
    }

    /// Convert TaskPlan to PlanPart
    fn plan_to_part(plan: &TaskPlan) -> SessionPart {
        SessionPart::PlanCreated(PlanPart {
            plan_id: plan.id.clone(),
            steps: plan.steps.iter().map(|s| PlanStep {
                step_id: s.id.clone(),
                description: s.description.clone(),
                status: StepStatus::Pending,
                dependencies: s.depends_on.clone(),
            }).collect(),
            requires_confirmation: false,  // Default to false for now
            created_at: chrono::Utc::now().timestamp_millis(),
        })
    }

    /// Get all parts for a session
    pub fn get_session_parts(&self, session_id: &str) -> Result<Vec<SessionPart>, RecorderError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        let mut stmt = conn
            .prepare("SELECT part_data FROM session_parts WHERE session_id = ?1 ORDER BY sequence")
            .map_err(|e| RecorderError::Database(e.to_string()))?;

        let parts: Result<Vec<SessionPart>, _> = stmt
            .query_map(params![session_id], |row| {
                let data: String = row.get(0)?;
                Ok(data)
            })
            .map_err(|e| RecorderError::Database(e.to_string()))?
            .map(|r| {
                r.map_err(|e| RecorderError::Database(e.to_string()))
                    .and_then(|data| {
                        serde_json::from_str(&data)
                            .map_err(|e| RecorderError::Serialization(e.to_string()))
                    })
            })
            .collect();

        parts
    }

    /// Get session info
    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, RecorderError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        let result = conn.query_row(
            r#"
            SELECT id, parent_id, agent_id, status, model, iteration_count, total_tokens, created_at, updated_at
            FROM sessions WHERE id = ?1
            "#,
            params![session_id],
            |row| {
                Ok(SessionRecord {
                    id: row.get(0)?,
                    parent_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    status: row.get(3)?,
                    model: row.get(4)?,
                    iteration_count: row.get(5)?,
                    total_tokens: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        );

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(RecorderError::Database(e.to_string())),
        }
    }
}

// ============================================================================
// EventHandler Implementation
// ============================================================================

#[async_trait]
impl EventHandler for SessionRecorder {
    fn name(&self) -> &'static str {
        "SessionRecorder"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::All]
    }

    async fn handle(
        &self,
        event: &AlephEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // Get session ID from context
        let session_id = ctx.get_session_id().await;

        // Handle special session events
        match event {
            AlephEvent::SessionCreated(info) => {
                // Create new session record
                if let Err(e) = self.create_session_with_options(
                    &info.id,
                    &info.model,
                    info.parent_id.as_deref(),
                    &info.agent_id,
                ) {
                    tracing::error!(error = %e, "Failed to create session record");
                }
                return Ok(vec![]);
            }
            AlephEvent::LoopContinue(_) => {
                // Update session iteration count
                if let Some(ref sid) = session_id {
                    if let Err(e) = self.update_session(sid) {
                        tracing::error!(error = %e, "Failed to update session");
                    }
                }
                return Ok(vec![]);
            }
            AlephEvent::SessionUpdated(diff) => {
                // Apply session diff
                if let Err(e) = self.update_session_full(
                    &diff.session_id,
                    diff.status.as_deref(),
                    diff.iteration_count,
                    diff.total_tokens,
                ) {
                    tracing::error!(error = %e, "Failed to apply session diff");
                }
                return Ok(vec![]);
            }
            _ => {}
        }

        // Convert event to session part
        if let Some(part) = Self::event_to_part(event) {
            if let Some(ref sid) = session_id {
                if let Err(e) = self.append_part(sid, &part) {
                    tracing::error!(
                        error = %e,
                        event_type = ?event.event_type(),
                        "Failed to persist session part"
                    );
                }
            }
        }

        // SessionRecorder doesn't publish any events
        Ok(vec![])
    }
}

// ============================================================================
// Supporting Types
// ============================================================================

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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        AiResponse, ErrorKind, EventBus, InputContext, InputEvent, PlanStep, StepStatus, TaskPlan,
        TokenUsage, ToolCallError, ToolCallResult,
    };

    // ========================================================================
    // Construction Tests
    // ========================================================================

    #[test]
    fn test_create_in_memory() {
        let recorder = SessionRecorder::new_in_memory();
        assert!(recorder.is_ok());
    }

    #[test]
    fn test_create_file_based() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_sessions.db");

        let recorder = SessionRecorder::new(&db_path);
        assert!(recorder.is_ok());
        assert!(db_path.exists());
    }

    // ========================================================================
    // Session Management Tests
    // ========================================================================

    #[test]
    fn test_create_session() {
        let recorder = SessionRecorder::new_in_memory().unwrap();

        let result = recorder.create_session("session-001", "gpt-4");
        assert!(result.is_ok());

        // Verify session was created
        let session = recorder.get_session("session-001").unwrap();
        assert!(session.is_some());

        let session = session.unwrap();
        assert_eq!(session.id, "session-001");
        assert_eq!(session.model, "gpt-4");
        assert_eq!(session.agent_id, "main");
        assert_eq!(session.status, "running");
        assert_eq!(session.iteration_count, 0);
        assert_eq!(session.total_tokens, 0);
    }

    #[test]
    fn test_create_session_with_parent() {
        let recorder = SessionRecorder::new_in_memory().unwrap();

        // Create parent session
        recorder.create_session("parent-001", "gpt-4").unwrap();

        // Create child session
        let result = recorder.create_session_with_options(
            "child-001",
            "gpt-4",
            Some("parent-001"),
            "sub-agent",
        );
        assert!(result.is_ok());

        let session = recorder.get_session("child-001").unwrap().unwrap();
        assert_eq!(session.parent_id, Some("parent-001".to_string()));
        assert_eq!(session.agent_id, "sub-agent");
    }

    #[test]
    fn test_update_session() {
        let recorder = SessionRecorder::new_in_memory().unwrap();

        recorder.create_session("session-001", "gpt-4").unwrap();

        // Update session
        let result = recorder.update_session("session-001");
        assert!(result.is_ok());

        // Verify iteration count increased
        let session = recorder.get_session("session-001").unwrap().unwrap();
        assert_eq!(session.iteration_count, 1);

        // Update again
        recorder.update_session("session-001").unwrap();
        let session = recorder.get_session("session-001").unwrap().unwrap();
        assert_eq!(session.iteration_count, 2);
    }

    #[test]
    fn test_update_session_full() {
        let recorder = SessionRecorder::new_in_memory().unwrap();

        recorder.create_session("session-001", "gpt-4").unwrap();

        // Update with all fields
        let result =
            recorder.update_session_full("session-001", Some("completed"), Some(5), Some(1000));
        assert!(result.is_ok());

        let session = recorder.get_session("session-001").unwrap().unwrap();
        assert_eq!(session.status, "completed");
        assert_eq!(session.iteration_count, 5);
        assert_eq!(session.total_tokens, 1000);
    }

    // ========================================================================
    // Part Persistence Tests
    // ========================================================================

    #[test]
    fn test_append_part() {
        let recorder = SessionRecorder::new_in_memory().unwrap();

        recorder.create_session("session-001", "gpt-4").unwrap();

        let part = SessionPart::UserInput(UserInputPart {
            text: "Hello, world!".to_string(),
            context: None,
            timestamp: chrono::Utc::now().timestamp(),
        });

        let result = recorder.append_part("session-001", &part);
        assert!(result.is_ok());

        // Verify part was stored
        let parts = recorder.get_session_parts("session-001").unwrap();
        assert_eq!(parts.len(), 1);

        if let SessionPart::UserInput(input) = &parts[0] {
            assert_eq!(input.text, "Hello, world!");
        } else {
            panic!("Expected UserInput part");
        }
    }

    #[test]
    fn test_append_multiple_parts() {
        let recorder = SessionRecorder::new_in_memory().unwrap();

        recorder.create_session("session-001", "gpt-4").unwrap();

        // Add multiple parts
        let parts_to_add = vec![
            SessionPart::UserInput(UserInputPart {
                text: "First message".to_string(),
                context: None,
                timestamp: 1000,
            }),
            SessionPart::AiResponse(AiResponsePart {
                content: "First response".to_string(),
                reasoning: None,
                timestamp: 1001,
            }),
            SessionPart::UserInput(UserInputPart {
                text: "Second message".to_string(),
                context: None,
                timestamp: 1002,
            }),
        ];

        for part in &parts_to_add {
            recorder.append_part("session-001", part).unwrap();
        }

        // Verify parts are in order
        let parts = recorder.get_session_parts("session-001").unwrap();
        assert_eq!(parts.len(), 3);

        // Check sequence
        if let SessionPart::UserInput(input) = &parts[0] {
            assert_eq!(input.text, "First message");
        }
        if let SessionPart::AiResponse(response) = &parts[1] {
            assert_eq!(response.content, "First response");
        }
        if let SessionPart::UserInput(input) = &parts[2] {
            assert_eq!(input.text, "Second message");
        }
    }

    // ========================================================================
    // Event Conversion Tests
    // ========================================================================

    #[test]
    fn test_event_to_part_input() {
        let event = AlephEvent::InputReceived(InputEvent {
            text: "Hello".to_string(),
            topic_id: Some("topic-1".to_string()),
            context: Some(InputContext {
                app_name: Some("Terminal".to_string()),
                window_title: Some("bash".to_string()),
                selected_text: None,
            }),
            timestamp: 1234567890,
        });

        let part = SessionRecorder::event_to_part(&event);
        assert!(part.is_some());

        if let Some(SessionPart::UserInput(input)) = part {
            assert_eq!(input.text, "Hello");
            assert_eq!(input.timestamp, 1234567890);
            assert!(input.context.is_some());
        } else {
            panic!("Expected UserInput part");
        }
    }

    #[test]
    fn test_event_to_part_tool_completed() {
        let event = AlephEvent::ToolCallCompleted(ToolCallResult {
            call_id: "call-001".to_string(),
            tool: "web_search".to_string(),
            input: serde_json::json!({"query": "rust programming"}),
            output: "Search results...".to_string(),
            started_at: 1000,
            completed_at: 2000,
            token_usage: TokenUsage::default(),
            session_id: None,
        });

        let part = SessionRecorder::event_to_part(&event);
        assert!(part.is_some());

        if let Some(SessionPart::ToolCall(tool_call)) = part {
            assert_eq!(tool_call.id, "call-001");
            assert_eq!(tool_call.tool_name, "web_search");
            assert_eq!(tool_call.status, ToolCallStatus::Completed);
            assert_eq!(tool_call.output, Some("Search results...".to_string()));
            assert!(tool_call.error.is_none());
        } else {
            panic!("Expected ToolCall part");
        }
    }

    #[test]
    fn test_event_to_part_tool_failed() {
        let event = AlephEvent::ToolCallFailed(ToolCallError {
            call_id: "call-002".to_string(),
            tool: "file_read".to_string(),
            error: "File not found".to_string(),
            error_kind: ErrorKind::NotFound,
            is_retryable: false,
            attempts: 1,
            session_id: None,
        });

        let part = SessionRecorder::event_to_part(&event);
        assert!(part.is_some());

        if let Some(SessionPart::ToolCall(tool_call)) = part {
            assert_eq!(tool_call.id, "call-002");
            assert_eq!(tool_call.status, ToolCallStatus::Failed);
            assert_eq!(tool_call.error, Some("File not found".to_string()));
        } else {
            panic!("Expected ToolCall part");
        }
    }

    #[test]
    fn test_event_to_part_ai_response() {
        let event = AlephEvent::AiResponseGenerated(AiResponse {
            content: "Here is my response".to_string(),
            reasoning: Some("I thought about it carefully".to_string()),
            is_final: true,
            timestamp: 1234567890,
        });

        let part = SessionRecorder::event_to_part(&event);
        assert!(part.is_some());

        if let Some(SessionPart::AiResponse(response)) = part {
            assert_eq!(response.content, "Here is my response");
            assert_eq!(
                response.reasoning,
                Some("I thought about it carefully".to_string())
            );
            assert_eq!(response.timestamp, 1234567890);
        } else {
            panic!("Expected AiResponse part");
        }
    }

    #[test]
    fn test_event_to_part_plan_created() {
        let event = AlephEvent::PlanCreated(TaskPlan {
            id: "plan-001".to_string(),
            steps: vec![
                PlanStep {
                    id: "step-1".to_string(),
                    description: "First step".to_string(),
                    tool: "search".to_string(),
                    parameters: serde_json::json!({}),
                    depends_on: vec![],
                    status: StepStatus::Pending,
                },
                PlanStep {
                    id: "step-2".to_string(),
                    description: "Second step".to_string(),
                    tool: "process".to_string(),
                    parameters: serde_json::json!({}),
                    depends_on: vec!["step-1".to_string()],
                    status: StepStatus::Pending,
                },
            ],
            parallel_groups: vec![],
            current_step_index: 0,
        });

        let part = SessionRecorder::event_to_part(&event);
        assert!(part.is_some());

        if let Some(SessionPart::PlanCreated(plan)) = part {
            assert_eq!(plan.plan_id, "plan-001");
            assert_eq!(plan.steps.len(), 2);
            assert_eq!(plan.steps[0].description, "First step");
            assert_eq!(plan.steps[1].description, "Second step");
        } else {
            panic!("Expected PlanCreated part");
        }
    }

    #[test]
    fn test_event_to_part_returns_none_for_internal_events() {
        // ToolCallRequested should not create a part
        let event = AlephEvent::ToolCallRequested(crate::event::ToolCallRequest {
            tool: "test".to_string(),
            parameters: serde_json::json!({}),
            plan_step_id: None,
        });
        assert!(SessionRecorder::event_to_part(&event).is_none());

        // LoopContinue should not create a part
        let event = AlephEvent::LoopContinue(crate::event::LoopState {
            session_id: "test".to_string(),
            iteration: 1,
            total_tokens: 100,
            last_tool: None,
            model: "gpt-4-turbo".to_string(),
        });
        assert!(SessionRecorder::event_to_part(&event).is_none());
    }

    // ========================================================================
    // EventHandler Implementation Tests
    // ========================================================================

    #[test]
    fn test_handler_name() {
        let recorder = SessionRecorder::new_in_memory().unwrap();
        assert_eq!(recorder.name(), "SessionRecorder");
    }

    #[test]
    fn test_handler_subscriptions() {
        let recorder = SessionRecorder::new_in_memory().unwrap();
        let subs = recorder.subscriptions();

        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0], EventType::All);
    }

    #[tokio::test]
    async fn test_handler_creates_session_on_session_created() {
        let recorder = SessionRecorder::new_in_memory().unwrap();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let event = AlephEvent::SessionCreated(crate::event::SessionInfo {
            id: "session-from-event".to_string(),
            parent_id: None,
            agent_id: "main".to_string(),
            model: "claude-3".to_string(),
            created_at: chrono::Utc::now().timestamp(),
        });

        let result = recorder.handle(&event, &ctx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty()); // Should not publish any events

        // Verify session was created
        let session = recorder.get_session("session-from-event").unwrap();
        assert!(session.is_some());
        assert_eq!(session.unwrap().model, "claude-3");
    }

    #[tokio::test]
    async fn test_handler_persists_input_event() {
        let recorder = SessionRecorder::new_in_memory().unwrap();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // Create session first
        recorder.create_session("test-session", "gpt-4").unwrap();
        ctx.set_session_id("test-session".to_string()).await;

        // Handle input event
        let event = AlephEvent::InputReceived(InputEvent {
            text: "Test input".to_string(),
            topic_id: None,
            context: None,
            timestamp: chrono::Utc::now().timestamp(),
        });

        let result = recorder.handle(&event, &ctx).await;
        assert!(result.is_ok());

        // Verify part was persisted
        let parts = recorder.get_session_parts("test-session").unwrap();
        assert_eq!(parts.len(), 1);
    }

    #[tokio::test]
    async fn test_handler_updates_session_on_loop_continue() {
        let recorder = SessionRecorder::new_in_memory().unwrap();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // Create session
        recorder.create_session("test-session", "gpt-4").unwrap();
        ctx.set_session_id("test-session".to_string()).await;

        // Handle loop continue event
        let event = AlephEvent::LoopContinue(crate::event::LoopState {
            session_id: "test-session".to_string(),
            iteration: 1,
            total_tokens: 100,
            last_tool: None,
            model: "gpt-4-turbo".to_string(),
        });

        recorder.handle(&event, &ctx).await.unwrap();

        // Verify iteration count increased
        let session = recorder.get_session("test-session").unwrap().unwrap();
        assert_eq!(session.iteration_count, 1);
    }
}
