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
}
