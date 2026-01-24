//! Shared types for component implementations.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::agent_loop::RequestContext;
use crate::dispatcher::ToolRegistry;
use crate::event::EventBus;

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

    /// User's original request (from LoopState)
    #[serde(default)]
    pub original_request: String,

    /// Request context (attachments, selected files, clipboard, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<RequestContext>,

    /// Session start timestamp (from LoopState, unix timestamp)
    #[serde(default)]
    pub started_at: i64,

    /// Whether session needs compaction (for SessionCompactor integration)
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
    pub fn with_context(mut self, context: RequestContext) -> Self {
        self.context = Some(context);
        self
    }
}

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

/// Session part - fine-grained execution records
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionPart {
    UserInput(UserInputPart),
    AiResponse(AiResponsePart),
    ToolCall(ToolCallPart),
    Reasoning(ReasoningPart),
    PlanCreated(PlanPart),
    SubAgentCall(SubAgentPart),
    Summary(SummaryPart),
    /// Marker for compaction boundary - used by filter_compacted() to find
    /// the point where old context was summarized
    CompactionMarker(CompactionMarker),
    /// System reminder for context injection (aligns with OpenCode's <system-reminder>)
    SystemReminder(SystemReminderPart),
    /// Step boundary - start
    StepStart(StepStartPart),
    /// Step boundary - finish
    StepFinish(StepFinishPart),
    /// Filesystem snapshot
    Snapshot(SnapshotPart),
    /// File change record
    Patch(PatchPart),
    /// Incremental streaming text
    StreamingText(StreamingTextPart),
}

impl SessionPart {
    pub fn type_name(&self) -> &'static str {
        match self {
            SessionPart::UserInput(_) => "user_input",
            SessionPart::AiResponse(_) => "ai_response",
            SessionPart::ToolCall(_) => "tool_call",
            SessionPart::Reasoning(_) => "reasoning",
            SessionPart::PlanCreated(_) => "plan_created",
            SessionPart::SubAgentCall(_) => "sub_agent_call",
            SessionPart::Summary(_) => "summary",
            SessionPart::CompactionMarker(_) => "compaction_marker",
            SessionPart::SystemReminder(_) => "system_reminder",
            SessionPart::StepStart(_) => "step_start",
            SessionPart::StepFinish(_) => "step_finish",
            SessionPart::Snapshot(_) => "snapshot",
            SessionPart::Patch(_) => "patch",
            SessionPart::StreamingText(_) => "streaming_text",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputPart {
    pub text: String,
    pub context: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponsePart {
    pub content: String,
    pub reasoning: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallPart {
    pub id: String,
    pub tool_name: String,
    pub input: Value,
    pub status: ToolCallStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ToolCallStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningPart {
    pub content: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPart {
    pub plan_id: String,
    pub steps: Vec<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentPart {
    pub agent_id: String,
    pub prompt: String,
    pub result: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryPart {
    pub content: String,
    pub original_count: u32,
    pub compacted_at: i64,
}

/// Marker for compaction boundary
///
/// This marker is inserted into the session when compaction occurs,
/// allowing filter_compacted() to find the boundary and discard
/// old context that has been summarized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionMarker {
    /// When compaction occurred
    pub timestamp: i64,
    /// Whether this was automatic or user-triggered
    pub auto: bool,
    /// Unique marker identifier (optional for backward compatibility)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker_id: Option<String>,
    /// Number of parts that were compacted (optional for backward compatibility)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts_compacted: Option<usize>,
    /// Number of tokens freed by compaction (optional for backward compatibility)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_freed: Option<u64>,
}

impl CompactionMarker {
    /// Create a new basic compaction marker
    pub fn new(auto: bool) -> Self {
        Self {
            timestamp: chrono::Utc::now().timestamp(),
            auto,
            marker_id: None,
            parts_compacted: None,
            tokens_freed: None,
        }
    }

    /// Create a basic compaction marker with explicit timestamp
    pub fn with_timestamp(timestamp: i64, auto: bool) -> Self {
        Self {
            timestamp,
            auto,
            marker_id: None,
            parts_compacted: None,
            tokens_freed: None,
        }
    }

    /// Create a detailed compaction marker with full metadata
    pub fn with_details(
        auto: bool,
        marker_id: String,
        parts_compacted: usize,
        tokens_freed: u64,
    ) -> Self {
        Self {
            timestamp: chrono::Utc::now().timestamp(),
            auto,
            marker_id: Some(marker_id),
            parts_compacted: Some(parts_compacted),
            tokens_freed: Some(tokens_freed),
        }
    }
}

// =============================================================================
// Step Boundary Types (for execution tracking)
// =============================================================================

/// Step start marker - marks the beginning of an agent loop step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepStartPart {
    /// Step number in the current session
    pub step_id: usize,
    /// When the step started
    pub timestamp: i64,
    /// Associated file snapshot ID (for revert capability)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
}

impl StepStartPart {
    /// Create a new step start marker
    pub fn new(step_id: usize) -> Self {
        Self {
            step_id,
            timestamp: chrono::Utc::now().timestamp(),
            snapshot_id: None,
        }
    }

    /// Create with associated snapshot
    pub fn with_snapshot(step_id: usize, snapshot_id: String) -> Self {
        Self {
            step_id,
            timestamp: chrono::Utc::now().timestamp(),
            snapshot_id: Some(snapshot_id),
        }
    }
}

/// Reason why a step finished
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepFinishReason {
    /// Step completed successfully
    Completed,
    /// Step failed with an error
    Failed,
    /// User aborted the step
    UserAborted,
    /// Tool execution error
    ToolError,
    /// Maximum steps limit reached
    MaxStepsReached,
}

impl Default for StepFinishReason {
    fn default() -> Self {
        Self::Completed
    }
}

/// Token usage for a step (re-exported from event types or defined locally)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StepTokenUsage {
    /// Input tokens consumed
    pub input_tokens: u64,
    /// Output tokens generated
    pub output_tokens: u64,
}

impl StepTokenUsage {
    /// Create new token usage
    pub fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
        }
    }

    /// Total tokens used
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// Step finish marker - marks the end of an agent loop step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepFinishPart {
    /// Step number that finished
    pub step_id: usize,
    /// Reason for finishing
    pub reason: StepFinishReason,
    /// Token usage for this step (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<StepTokenUsage>,
    /// Duration of the step in milliseconds
    pub duration_ms: u64,
}

impl StepFinishPart {
    /// Create a new step finish marker
    pub fn new(step_id: usize, reason: StepFinishReason, duration_ms: u64) -> Self {
        Self {
            step_id,
            reason,
            tokens: None,
            duration_ms,
        }
    }

    /// Create with token usage
    pub fn with_tokens(
        step_id: usize,
        reason: StepFinishReason,
        duration_ms: u64,
        tokens: StepTokenUsage,
    ) -> Self {
        Self {
            step_id,
            reason,
            tokens: Some(tokens),
            duration_ms,
        }
    }
}

// =============================================================================
// Filesystem Snapshot Types (for session revert capability)
// =============================================================================

/// Individual file snapshot entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    /// File path (relative or absolute)
    pub path: String,
    /// Content hash (SHA256 or similar)
    pub hash: String,
}

impl FileSnapshot {
    /// Create a new file snapshot entry
    pub fn new(path: impl Into<String>, hash: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            hash: hash.into(),
        }
    }
}

/// Filesystem snapshot - captures file state at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotPart {
    /// Unique snapshot identifier
    pub snapshot_id: String,
    /// List of files with their hashes
    pub files: Vec<FileSnapshot>,
    /// When the snapshot was taken
    pub timestamp: i64,
}

impl SnapshotPart {
    /// Create a new empty snapshot
    pub fn new(snapshot_id: impl Into<String>) -> Self {
        Self {
            snapshot_id: snapshot_id.into(),
            files: Vec::new(),
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Create with files
    pub fn with_files(snapshot_id: impl Into<String>, files: Vec<FileSnapshot>) -> Self {
        Self {
            snapshot_id: snapshot_id.into(),
            files,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Add a file to the snapshot
    pub fn add_file(&mut self, path: impl Into<String>, hash: impl Into<String>) {
        self.files.push(FileSnapshot::new(path, hash));
    }
}

// =============================================================================
// File Change Types (for patches between snapshots)
// =============================================================================

/// Type of file change
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileChangeType {
    /// File was added
    Added,
    /// File was modified
    Modified,
    /// File was deleted
    Deleted,
}

/// Individual file change record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// File path
    pub path: String,
    /// Type of change
    pub change_type: FileChangeType,
    /// New content hash (for Added/Modified), None for Deleted
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl FileChange {
    /// Create a new file added change
    pub fn added(path: impl Into<String>, hash: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            change_type: FileChangeType::Added,
            content_hash: Some(hash.into()),
        }
    }

    /// Create a new file modified change
    pub fn modified(path: impl Into<String>, hash: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            change_type: FileChangeType::Modified,
            content_hash: Some(hash.into()),
        }
    }

    /// Create a new file deleted change
    pub fn deleted(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            change_type: FileChangeType::Deleted,
            content_hash: None,
        }
    }
}

/// Patch part - records file changes between snapshots
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchPart {
    /// Unique patch identifier
    pub patch_id: String,
    /// Base snapshot this patch applies to
    pub base_snapshot_id: String,
    /// List of file changes
    pub changes: Vec<FileChange>,
}

impl PatchPart {
    /// Create a new empty patch
    pub fn new(patch_id: impl Into<String>, base_snapshot_id: impl Into<String>) -> Self {
        Self {
            patch_id: patch_id.into(),
            base_snapshot_id: base_snapshot_id.into(),
            changes: Vec::new(),
        }
    }

    /// Create with changes
    pub fn with_changes(
        patch_id: impl Into<String>,
        base_snapshot_id: impl Into<String>,
        changes: Vec<FileChange>,
    ) -> Self {
        Self {
            patch_id: patch_id.into(),
            base_snapshot_id: base_snapshot_id.into(),
            changes,
        }
    }

    /// Add a change to the patch
    pub fn add_change(&mut self, change: FileChange) {
        self.changes.push(change);
    }
}

// =============================================================================
// Streaming Text Type (for incremental UI updates)
// =============================================================================

/// Streaming text part - supports incremental text updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingTextPart {
    /// Unique part identifier
    pub part_id: String,
    /// Current full content
    pub content: String,
    /// Whether streaming has completed
    pub is_complete: bool,
    /// Incremental content delta (for event push)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
}

impl StreamingTextPart {
    /// Create a new streaming text part
    pub fn new(part_id: impl Into<String>) -> Self {
        Self {
            part_id: part_id.into(),
            content: String::new(),
            is_complete: false,
            delta: None,
        }
    }

    /// Create with initial content
    pub fn with_content(part_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            part_id: part_id.into(),
            content: content.into(),
            is_complete: false,
            delta: None,
        }
    }

    /// Append delta to content
    pub fn append(&mut self, delta: &str) {
        self.content.push_str(delta);
        self.delta = Some(delta.to_string());
    }

    /// Mark as complete
    pub fn complete(&mut self) {
        self.is_complete = true;
        self.delta = None;
    }
}

// =============================================================================
// Part ID Trait and Update Types (for UI message flow)
// =============================================================================

/// Trait for getting unique part ID
pub trait PartId {
    /// Get the unique identifier for this part
    fn part_id(&self) -> String;
}

impl PartId for SessionPart {
    fn part_id(&self) -> String {
        match self {
            SessionPart::UserInput(p) => format!("user_input_{}", p.timestamp),
            SessionPart::AiResponse(p) => format!("ai_response_{}", p.timestamp),
            SessionPart::ToolCall(p) => p.id.clone(),
            SessionPart::Reasoning(p) => format!("reasoning_{}", p.timestamp),
            SessionPart::PlanCreated(p) => p.plan_id.clone(),
            SessionPart::SubAgentCall(p) => format!("subagent_{}", p.agent_id),
            SessionPart::Summary(p) => format!("summary_{}", p.compacted_at),
            SessionPart::CompactionMarker(p) => format!("compaction_marker_{}", p.timestamp),
            SessionPart::SystemReminder(p) => format!("reminder_{}", p.timestamp),
            SessionPart::StepStart(p) => format!("step_start_{}", p.step_id),
            SessionPart::StepFinish(p) => format!("step_finish_{}", p.step_id),
            SessionPart::Snapshot(p) => p.snapshot_id.clone(),
            SessionPart::Patch(p) => p.patch_id.clone(),
            SessionPart::StreamingText(p) => p.part_id.clone(),
        }
    }
}

/// Part event type for UI updates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartEventType {
    /// Part was added to the session
    Added,
    /// Part was updated (e.g., tool call status changed)
    Updated,
    /// Part was removed (e.g., compaction)
    Removed,
}

impl std::fmt::Display for PartEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartEventType::Added => write!(f, "added"),
            PartEventType::Updated => write!(f, "updated"),
            PartEventType::Removed => write!(f, "removed"),
        }
    }
}

/// Part update event data for UI rendering
///
/// This structure contains all information needed for the UI to render
/// a part update (add, update, or remove).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartUpdateData {
    /// Session ID this part belongs to
    pub session_id: String,
    /// Unique part identifier
    pub part_id: String,
    /// Part type name (e.g., "tool_call", "ai_response")
    pub part_type: String,
    /// Event type (Added, Updated, Removed)
    pub event_type: PartEventType,
    /// Serialized part data as JSON
    pub part_json: String,
    /// Delta content for streaming updates (text chunks)
    pub delta: Option<String>,
    /// Timestamp when the event occurred
    pub timestamp: i64,
}

impl PartUpdateData {
    /// Create a new PartUpdateData for an added part
    pub fn added(session_id: &str, part: &SessionPart) -> Self {
        Self {
            session_id: session_id.to_string(),
            part_id: part.part_id(),
            part_type: part.type_name().to_string(),
            event_type: PartEventType::Added,
            part_json: serde_json::to_string(part).unwrap_or_default(),
            delta: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create a new PartUpdateData for an updated part
    pub fn updated(session_id: &str, part: &SessionPart, delta: Option<String>) -> Self {
        Self {
            session_id: session_id.to_string(),
            part_id: part.part_id(),
            part_type: part.type_name().to_string(),
            event_type: PartEventType::Updated,
            part_json: serde_json::to_string(part).unwrap_or_default(),
            delta,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create a new PartUpdateData for a removed part
    pub fn removed(session_id: &str, part_id: &str, part_type: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            part_id: part_id.to_string(),
            part_type: part_type.to_string(),
            event_type: PartEventType::Removed,
            part_json: String::new(),
            delta: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create update for streaming text delta
    pub fn text_delta(session_id: &str, part_id: &str, part_type: &str, delta: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            part_id: part_id.to_string(),
            part_type: part_type.to_string(),
            event_type: PartEventType::Updated,
            part_json: String::new(),
            delta: Some(delta.to_string()),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}

// =============================================================================
// Knowledge and Entity Types (for ExecutionContext)
// =============================================================================

/// Knowledge fragment extracted from tool results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Knowledge {
    /// Knowledge key identifier
    pub key: String,
    /// Knowledge value
    pub value: String,
    /// Source of this knowledge (tool name or user input)
    pub source: String,
    /// Confidence level (0.0 - 1.0)
    pub confidence: f32,
    /// Timestamp when acquired
    pub acquired_at: i64,
}

impl Knowledge {
    /// Create a new knowledge fragment with default confidence
    pub fn new(
        key: impl Into<String>,
        value: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            source: source.into(),
            confidence: 0.9,
            acquired_at: chrono::Utc::now().timestamp(),
        }
    }

    /// Create with specific confidence
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }
}

/// Entity extracted from user input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// Entity type (e.g., "file", "project", "server")
    pub entity_type: String,
    /// Entity value
    pub value: String,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl Entity {
    /// Create a new entity
    pub fn new(entity_type: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            entity_type: entity_type.into(),
            value: value.into(),
            metadata: None,
        }
    }

    /// Add metadata
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// User intent - preserves raw input + structured understanding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserIntent {
    /// Raw user input (immutable)
    pub raw_input: String,
    /// Structured interpretation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub understood_as: Option<String>,
    /// Key entities extracted
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_entities: Vec<Entity>,
    /// Implicit expectations
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implicit_expectations: Vec<String>,
    /// Timestamp
    pub created_at: i64,
}

impl UserIntent {
    /// Create from raw input
    pub fn new(raw_input: impl Into<String>) -> Self {
        Self {
            raw_input: raw_input.into(),
            understood_as: None,
            key_entities: Vec::new(),
            implicit_expectations: Vec::new(),
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    /// Set structured understanding
    pub fn understood_as(mut self, interpretation: impl Into<String>) -> Self {
        self.understood_as = Some(interpretation.into());
        self
    }

    /// Add an entity
    pub fn with_entity(mut self, entity: Entity) -> Self {
        self.key_entities.push(entity);
        self
    }

    /// Add an implicit expectation
    pub fn with_expectation(mut self, expectation: impl Into<String>) -> Self {
        self.implicit_expectations.push(expectation.into());
        self
    }
}

/// Current goal in execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    /// Goal description
    pub description: String,
    /// Success criteria
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_criteria: Option<String>,
    /// Link to parent goal (for sub-goals)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_goal: Option<String>,
    /// Goal status
    pub status: GoalStatus,
    /// Created timestamp
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum GoalStatus {
    #[default]
    Pending,
    InProgress,
    Achieved,
    Failed(String),
    Superseded,
}

impl Goal {
    /// Create a new goal
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            success_criteria: None,
            parent_goal: None,
            status: GoalStatus::Pending,
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    /// Set success criteria
    pub fn with_success_criteria(mut self, criteria: impl Into<String>) -> Self {
        self.success_criteria = Some(criteria.into());
        self
    }

    /// Set parent goal
    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent_goal = Some(parent.into());
        self
    }
}

/// Decision record for tracking reasoning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// What was decided
    pub choice: String,
    /// Why this choice was made
    pub reasoning: String,
    /// Alternatives that were considered
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<String>,
    /// Timestamp
    pub timestamp: i64,
}

impl DecisionRecord {
    /// Create a new decision record
    pub fn new(
        choice: impl Into<String>,
        reasoning: impl Into<String>,
        alternatives: Vec<String>,
    ) -> Self {
        Self {
            choice: choice.into(),
            reasoning: reasoning.into(),
            alternatives,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }
}

/// Execution phase
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum ExecutionPhase {
    /// Understanding user intent
    #[default]
    Understanding,
    /// Planning execution steps
    Planning,
    /// Executing tools
    Executing,
    /// Validating results
    Validating,
    /// Summarizing for user
    Summarizing,
}

/// Context verbosity levels for prompt generation
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ContextVerbosity {
    /// First request: full context
    #[default]
    Full,
    /// Subsequent requests: incremental + key references only
    Incremental,
    /// Token-constrained: only core information
    Minimal,
}

/// Execution context - semantic backbone through execution chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Unique context ID
    pub id: String,
    /// Original user intent (immutable)
    pub original_intent: UserIntent,
    /// Current goal (may refine as task decomposes)
    pub current_goal: Goal,
    /// Decision trail (why these choices were made)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_trail: Vec<DecisionRecord>,
    /// Acquired knowledge (valuable results from tool calls)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acquired_knowledge: Vec<Knowledge>,
    /// Current execution phase
    pub phase: ExecutionPhase,
    /// Created timestamp
    pub created_at: i64,
    /// Last updated timestamp
    pub updated_at: i64,
}

impl ExecutionContext {
    /// Create a new execution context
    pub fn new(intent: UserIntent, goal: Goal) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            original_intent: intent,
            current_goal: goal,
            decision_trail: Vec::new(),
            acquired_knowledge: Vec::new(),
            phase: ExecutionPhase::Understanding,
            created_at: now,
            updated_at: now,
        }
    }

    /// Add knowledge to the context
    pub fn add_knowledge(&mut self, knowledge: Knowledge) {
        self.acquired_knowledge.push(knowledge);
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Add a decision record
    pub fn add_decision(
        &mut self,
        choice: impl Into<String>,
        reasoning: impl Into<String>,
        alternatives: Vec<String>,
    ) {
        self.decision_trail
            .push(DecisionRecord::new(choice, reasoning, alternatives));
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Update current goal
    pub fn set_goal(&mut self, goal: Goal) {
        self.current_goal = goal;
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Update execution phase
    pub fn set_phase(&mut self, phase: ExecutionPhase) {
        self.phase = phase;
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Get knowledge by key
    pub fn get_knowledge(&self, key: &str) -> Option<&Knowledge> {
        self.acquired_knowledge.iter().find(|k| k.key == key)
    }

    /// Generate context string based on verbosity level
    pub fn to_prompt(&self, verbosity: ContextVerbosity) -> String {
        match verbosity {
            ContextVerbosity::Full => self.to_full_prompt(),
            ContextVerbosity::Incremental => self.to_incremental_prompt(),
            ContextVerbosity::Minimal => self.to_minimal_prompt(),
        }
    }

    /// Full context for first request
    fn to_full_prompt(&self) -> String {
        let mut parts = Vec::new();

        // Original intent
        parts.push(format!(
            "**User Original Intent**: {}",
            self.original_intent.raw_input
        ));
        if let Some(ref understood) = self.original_intent.understood_as {
            parts.push(format!("**Understood As**: {}", understood));
        }

        // Implicit expectations
        if !self.original_intent.implicit_expectations.is_empty() {
            parts.push(format!(
                "**Implicit Expectations**: {}",
                self.original_intent.implicit_expectations.join("; ")
            ));
        }

        // Current goal
        parts.push(format!(
            "**Current Goal**: {}",
            self.current_goal.description
        ));
        if let Some(ref criteria) = self.current_goal.success_criteria {
            parts.push(format!("**Success Criteria**: {}", criteria));
        }

        // Acquired knowledge
        if !self.acquired_knowledge.is_empty() {
            let knowledge_lines: Vec<String> = self
                .acquired_knowledge
                .iter()
                .map(|k| {
                    format!(
                        "- {}: {} (source: {}, confidence: {:.0}%)",
                        k.key,
                        k.value,
                        k.source,
                        k.confidence * 100.0
                    )
                })
                .collect();
            parts.push(format!(
                "**Acquired Information**:\n{}",
                knowledge_lines.join("\n")
            ));
        }

        // Decision history
        if !self.decision_trail.is_empty() {
            let decision_lines: Vec<String> = self
                .decision_trail
                .iter()
                .enumerate()
                .map(|(i, d)| format!("{}. {} - {}", i + 1, d.choice, d.reasoning))
                .collect();
            parts.push(format!(
                "**Decision History**:\n{}",
                decision_lines.join("\n")
            ));
        }

        parts.join("\n\n")
    }

    /// Incremental context (recent changes only)
    fn to_incremental_prompt(&self) -> String {
        let mut parts = Vec::new();

        // Current goal only
        parts.push(format!("**Goal**: {}", self.current_goal.description));

        // Recent knowledge (last 3 items)
        let recent_knowledge: Vec<String> = self
            .acquired_knowledge
            .iter()
            .rev()
            .take(3)
            .map(|k| format!("{}={}", k.key, k.value))
            .collect();
        if !recent_knowledge.is_empty() {
            parts.push(format!("**Recent Info**: {}", recent_knowledge.join(", ")));
        }

        // Last decision
        if let Some(last_decision) = self.decision_trail.last() {
            parts.push(format!("**Last Decision**: {}", last_decision.choice));
        }

        parts.join("\n")
    }

    /// Generate context summary for LLM prompt (minimal version)
    pub fn to_minimal_prompt(&self) -> String {
        let knowledge_str = self
            .acquired_knowledge
            .iter()
            .filter(|k| k.confidence >= 0.8)
            .map(|k| format!("{}={}", k.key, k.value))
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "Goal: {}\nKnown: {}",
            self.current_goal.description,
            if knowledge_str.is_empty() {
                "(none)".to_string()
            } else {
                knowledge_str
            }
        )
    }
}

/// Tool call record for doom loop detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool: String,
    pub input: Value,
    pub timestamp: i64,
}

/// Complexity level for planning decisions
#[derive(Debug, Clone, PartialEq)]
pub enum Complexity {
    Simple,
    NeedsPlan,
}

/// LLM decision after tool execution
#[derive(Debug, Clone)]
pub enum Decision {
    CallTool(crate::event::ToolCallRequest),
    Stop(crate::event::StopReason),
    AskUser(crate::event::UserQuestion),
}

/// Component context - shared state for all event handlers
pub struct ComponentContext {
    pub session: Arc<RwLock<ExecutionSession>>,
    pub tools: Arc<ToolRegistry>,
    pub bus: EventBus,
    pub abort_signal: Arc<std::sync::atomic::AtomicBool>,
    pub session_id: String,
}

impl ComponentContext {
    pub fn new(
        session: Arc<RwLock<ExecutionSession>>,
        tools: Arc<ToolRegistry>,
        bus: EventBus,
        abort_signal: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        Self {
            session,
            tools,
            bus,
            abort_signal,
            session_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_knowledge_creation() {
        let knowledge = Knowledge::new("db_path", "./config/db.toml", "search_files");
        assert_eq!(knowledge.key, "db_path");
        assert_eq!(knowledge.value, "./config/db.toml");
        assert_eq!(knowledge.source, "search_files");
        assert!(knowledge.confidence >= 0.0 && knowledge.confidence <= 1.0);
    }

    #[test]
    fn test_entity_creation() {
        let entity = Entity::new("project", "Aether");
        assert_eq!(entity.entity_type, "project");
        assert_eq!(entity.value, "Aether");
    }

    #[test]
    fn test_user_intent_creation() {
        let intent = UserIntent::new("Help me deploy the project")
            .understood_as("Deploy current project to remote server")
            .with_entity(Entity::new("project", "Aether"))
            .with_expectation("Don't break existing service");

        assert_eq!(intent.raw_input, "Help me deploy the project");
        assert_eq!(
            intent.understood_as,
            Some("Deploy current project to remote server".to_string())
        );
        assert_eq!(intent.key_entities.len(), 1);
        assert_eq!(intent.implicit_expectations.len(), 1);
    }

    #[test]
    fn test_goal_creation() {
        let goal = Goal::new("Find project config files")
            .with_success_criteria("Located Cargo.toml and verified build target")
            .with_parent("Deploy project");

        assert_eq!(goal.description, "Find project config files");
        assert!(goal.success_criteria.is_some());
        assert!(goal.parent_goal.is_some());
    }

    #[test]
    fn test_execution_context_creation() {
        let intent = UserIntent::new("Deploy the project");
        let goal = Goal::new("Find configuration");

        let ctx = ExecutionContext::new(intent, goal);

        assert_eq!(ctx.original_intent.raw_input, "Deploy the project");
        assert_eq!(ctx.current_goal.description, "Find configuration");
        assert!(ctx.decision_trail.is_empty());
        assert!(ctx.acquired_knowledge.is_empty());
        assert_eq!(ctx.phase, ExecutionPhase::Understanding);
    }

    #[test]
    fn test_execution_context_add_knowledge() {
        let intent = UserIntent::new("Test");
        let goal = Goal::new("Test goal");
        let mut ctx = ExecutionContext::new(intent, goal);

        ctx.add_knowledge(Knowledge::new("key", "value", "test_tool"));

        assert_eq!(ctx.acquired_knowledge.len(), 1);
        assert_eq!(ctx.acquired_knowledge[0].key, "key");
    }

    #[test]
    fn test_execution_context_add_decision() {
        let intent = UserIntent::new("Test");
        let goal = Goal::new("Test goal");
        let mut ctx = ExecutionContext::new(intent, goal);

        ctx.add_decision(
            "Use search_files tool",
            "Need to find config location first",
            vec!["read_file".to_string(), "list_dir".to_string()],
        );

        assert_eq!(ctx.decision_trail.len(), 1);
        assert_eq!(ctx.decision_trail[0].choice, "Use search_files tool");
    }

    #[test]
    fn test_context_verbosity_prompt_generation() {
        let intent = UserIntent::new("Deploy project").understood_as("Deploy to server");
        let goal = Goal::new("Find config");
        let mut ctx = ExecutionContext::new(intent, goal);
        ctx.add_knowledge(Knowledge::new("project_type", "rust", "analysis").with_confidence(0.95));
        ctx.add_decision("Analyze project first", "Need to understand structure", vec![]);

        let minimal = ctx.to_prompt(ContextVerbosity::Minimal);
        assert!(minimal.contains("Find config"));
        assert!(minimal.contains("project_type=rust"));

        let full = ctx.to_prompt(ContextVerbosity::Full);
        assert!(full.contains("Deploy project"));
        assert!(full.contains("Deploy to server"));
        assert!(full.contains("Decision History"));
    }

    #[test]
    fn test_part_id_trait() {
        // Test ToolCallPart ID extraction
        let tool_call = SessionPart::ToolCall(ToolCallPart {
            id: "call-123".to_string(),
            tool_name: "search".to_string(),
            input: serde_json::json!({}),
            status: ToolCallStatus::Running,
            output: None,
            error: None,
            started_at: 1000,
            completed_at: None,
        });
        assert_eq!(tool_call.part_id(), "call-123");
        assert_eq!(tool_call.type_name(), "tool_call");

        // Test PlanPart ID extraction
        let plan = SessionPart::PlanCreated(PlanPart {
            plan_id: "plan-456".to_string(),
            steps: vec!["Step 1".to_string()],
            timestamp: 2000,
        });
        assert_eq!(plan.part_id(), "plan-456");
        assert_eq!(plan.type_name(), "plan_created");

        // Test UserInputPart ID (uses timestamp)
        let input = SessionPart::UserInput(UserInputPart {
            text: "Hello".to_string(),
            context: None,
            timestamp: 3000,
        });
        assert_eq!(input.part_id(), "user_input_3000");
        assert_eq!(input.type_name(), "user_input");
    }

    #[test]
    fn test_part_update_data_creation() {
        let tool_call = SessionPart::ToolCall(ToolCallPart {
            id: "call-789".to_string(),
            tool_name: "web_fetch".to_string(),
            input: serde_json::json!({"url": "https://example.com"}),
            status: ToolCallStatus::Completed,
            output: Some("Page content".to_string()),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        // Test added event
        let added = PartUpdateData::added("session-1", &tool_call);
        assert_eq!(added.session_id, "session-1");
        assert_eq!(added.part_id, "call-789");
        assert_eq!(added.part_type, "tool_call");
        assert_eq!(added.event_type, PartEventType::Added);
        assert!(added.delta.is_none());
        assert!(!added.part_json.is_empty());

        // Test updated event with delta
        let updated = PartUpdateData::updated("session-1", &tool_call, Some("output chunk".to_string()));
        assert_eq!(updated.event_type, PartEventType::Updated);
        assert_eq!(updated.delta, Some("output chunk".to_string()));

        // Test text delta event
        let delta = PartUpdateData::text_delta("session-1", "resp-1", "ai_response", "Hello, ");
        assert_eq!(delta.part_id, "resp-1");
        assert_eq!(delta.part_type, "ai_response");
        assert_eq!(delta.event_type, PartEventType::Updated);
        assert_eq!(delta.delta, Some("Hello, ".to_string()));
        assert!(delta.part_json.is_empty()); // text_delta doesn't include full part

        // Test removed event
        let removed = PartUpdateData::removed("session-1", "call-789", "tool_call");
        assert_eq!(removed.part_id, "call-789");
        assert_eq!(removed.event_type, PartEventType::Removed);
        assert!(removed.part_json.is_empty());
    }

    #[test]
    fn test_part_event_type_display() {
        assert_eq!(format!("{}", PartEventType::Added), "added");
        assert_eq!(format!("{}", PartEventType::Updated), "updated");
        assert_eq!(format!("{}", PartEventType::Removed), "removed");
    }

    #[test]
    fn test_system_reminder_part() {
        let reminder = SessionPart::SystemReminder(SystemReminderPart {
            content: "Continue with your tasks".to_string(),
            reminder_type: ReminderType::ContinueTask,
            timestamp: 1000,
        });

        assert_eq!(reminder.type_name(), "system_reminder");
        assert!(reminder.part_id().starts_with("reminder_"));
    }

    #[test]
    fn test_execution_session_with_request_context() {
        use crate::agent_loop::RequestContext;

        let ctx = RequestContext {
            current_app: Some("Terminal".to_string()),
            working_directory: Some("/tmp".to_string()),
            ..Default::default()
        };

        let session = ExecutionSession::new()
            .with_original_request("Find files")
            .with_context(ctx);

        assert_eq!(session.original_request, "Find files");
        assert!(session.context.is_some());
        assert_eq!(session.context.as_ref().unwrap().current_app, Some("Terminal".to_string()));
        assert!(!session.needs_compaction);
    }

    // =========================================================================
    // Tests for new SessionPart types (step boundaries, snapshots, streaming)
    // =========================================================================

    #[test]
    fn test_step_start_part() {
        let step = StepStartPart::new(1);
        assert_eq!(step.step_id, 1);
        assert!(step.timestamp > 0);
        assert!(step.snapshot_id.is_none());

        let step_with_snapshot = StepStartPart::with_snapshot(2, "snap-123".to_string());
        assert_eq!(step_with_snapshot.step_id, 2);
        assert_eq!(step_with_snapshot.snapshot_id, Some("snap-123".to_string()));
    }

    #[test]
    fn test_step_finish_part() {
        let finish = StepFinishPart::new(1, StepFinishReason::Completed, 500);
        assert_eq!(finish.step_id, 1);
        assert_eq!(finish.reason, StepFinishReason::Completed);
        assert_eq!(finish.duration_ms, 500);
        assert!(finish.tokens.is_none());

        let finish_with_tokens = StepFinishPart::with_tokens(
            2,
            StepFinishReason::Failed,
            1000,
            StepTokenUsage::new(100, 50),
        );
        assert_eq!(finish_with_tokens.step_id, 2);
        assert_eq!(finish_with_tokens.reason, StepFinishReason::Failed);
        assert_eq!(finish_with_tokens.tokens.as_ref().unwrap().total(), 150);
    }

    #[test]
    fn test_step_finish_reason_variants() {
        assert_eq!(StepFinishReason::default(), StepFinishReason::Completed);
        assert_ne!(StepFinishReason::Failed, StepFinishReason::Completed);
        assert_ne!(StepFinishReason::UserAborted, StepFinishReason::ToolError);
        assert_ne!(StepFinishReason::MaxStepsReached, StepFinishReason::Failed);
    }

    #[test]
    fn test_step_token_usage() {
        let usage = StepTokenUsage::new(100, 50);
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total(), 150);

        let default = StepTokenUsage::default();
        assert_eq!(default.total(), 0);
    }

    #[test]
    fn test_file_snapshot() {
        let file = FileSnapshot::new("/src/main.rs", "abc123");
        assert_eq!(file.path, "/src/main.rs");
        assert_eq!(file.hash, "abc123");
    }

    #[test]
    fn test_snapshot_part() {
        let mut snapshot = SnapshotPart::new("snap-001");
        assert_eq!(snapshot.snapshot_id, "snap-001");
        assert!(snapshot.files.is_empty());
        assert!(snapshot.timestamp > 0);

        snapshot.add_file("/src/main.rs", "hash1");
        snapshot.add_file("/Cargo.toml", "hash2");
        assert_eq!(snapshot.files.len(), 2);

        let files = vec![
            FileSnapshot::new("/a.rs", "h1"),
            FileSnapshot::new("/b.rs", "h2"),
        ];
        let snapshot2 = SnapshotPart::with_files("snap-002", files);
        assert_eq!(snapshot2.files.len(), 2);
    }

    #[test]
    fn test_file_change() {
        let added = FileChange::added("/new.rs", "hash1");
        assert_eq!(added.change_type, FileChangeType::Added);
        assert_eq!(added.content_hash, Some("hash1".to_string()));

        let modified = FileChange::modified("/existing.rs", "hash2");
        assert_eq!(modified.change_type, FileChangeType::Modified);
        assert_eq!(modified.content_hash, Some("hash2".to_string()));

        let deleted = FileChange::deleted("/old.rs");
        assert_eq!(deleted.change_type, FileChangeType::Deleted);
        assert!(deleted.content_hash.is_none());
    }

    #[test]
    fn test_patch_part() {
        let mut patch = PatchPart::new("patch-001", "snap-000");
        assert_eq!(patch.patch_id, "patch-001");
        assert_eq!(patch.base_snapshot_id, "snap-000");
        assert!(patch.changes.is_empty());

        patch.add_change(FileChange::added("/new.rs", "h1"));
        patch.add_change(FileChange::modified("/main.rs", "h2"));
        assert_eq!(patch.changes.len(), 2);

        let changes = vec![
            FileChange::added("/a.rs", "h1"),
            FileChange::deleted("/b.rs"),
        ];
        let patch2 = PatchPart::with_changes("patch-002", "snap-001", changes);
        assert_eq!(patch2.changes.len(), 2);
    }

    #[test]
    fn test_streaming_text_part() {
        let mut stream = StreamingTextPart::new("stream-001");
        assert_eq!(stream.part_id, "stream-001");
        assert!(stream.content.is_empty());
        assert!(!stream.is_complete);
        assert!(stream.delta.is_none());

        stream.append("Hello, ");
        assert_eq!(stream.content, "Hello, ");
        assert_eq!(stream.delta, Some("Hello, ".to_string()));

        stream.append("World!");
        assert_eq!(stream.content, "Hello, World!");
        assert_eq!(stream.delta, Some("World!".to_string()));

        stream.complete();
        assert!(stream.is_complete);
        assert!(stream.delta.is_none());
    }

    #[test]
    fn test_streaming_text_with_content() {
        let stream = StreamingTextPart::with_content("stream-002", "Initial content");
        assert_eq!(stream.content, "Initial content");
        assert!(!stream.is_complete);
    }

    #[test]
    fn test_compaction_marker_constructors() {
        let marker = CompactionMarker::new(true);
        assert!(marker.auto);
        assert!(marker.timestamp > 0);
        assert!(marker.marker_id.is_none());
        assert!(marker.parts_compacted.is_none());
        assert!(marker.tokens_freed.is_none());

        let marker2 = CompactionMarker::with_timestamp(1000, false);
        assert_eq!(marker2.timestamp, 1000);
        assert!(!marker2.auto);

        let marker3 = CompactionMarker::with_details(true, "m-001".to_string(), 10, 5000);
        assert!(marker3.auto);
        assert_eq!(marker3.marker_id, Some("m-001".to_string()));
        assert_eq!(marker3.parts_compacted, Some(10));
        assert_eq!(marker3.tokens_freed, Some(5000));
    }

    #[test]
    fn test_new_session_part_type_names() {
        let step_start = SessionPart::StepStart(StepStartPart::new(1));
        assert_eq!(step_start.type_name(), "step_start");

        let step_finish = SessionPart::StepFinish(StepFinishPart::new(1, StepFinishReason::Completed, 100));
        assert_eq!(step_finish.type_name(), "step_finish");

        let snapshot = SessionPart::Snapshot(SnapshotPart::new("s-001"));
        assert_eq!(snapshot.type_name(), "snapshot");

        let patch = SessionPart::Patch(PatchPart::new("p-001", "s-000"));
        assert_eq!(patch.type_name(), "patch");

        let streaming = SessionPart::StreamingText(StreamingTextPart::new("st-001"));
        assert_eq!(streaming.type_name(), "streaming_text");
    }

    #[test]
    fn test_new_session_part_ids() {
        let step_start = SessionPart::StepStart(StepStartPart::new(5));
        assert_eq!(step_start.part_id(), "step_start_5");

        let step_finish = SessionPart::StepFinish(StepFinishPart::new(5, StepFinishReason::Completed, 100));
        assert_eq!(step_finish.part_id(), "step_finish_5");

        let snapshot = SessionPart::Snapshot(SnapshotPart::new("snap-123"));
        assert_eq!(snapshot.part_id(), "snap-123");

        let patch = SessionPart::Patch(PatchPart::new("patch-456", "snap-123"));
        assert_eq!(patch.part_id(), "patch-456");

        let streaming = SessionPart::StreamingText(StreamingTextPart::new("stream-789"));
        assert_eq!(streaming.part_id(), "stream-789");
    }
}
