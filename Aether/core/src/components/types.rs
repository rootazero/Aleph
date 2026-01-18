//! Shared types for component implementations.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

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
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.into();
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
