# LLM Data Format Redesign - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance LLM data transmission format to improve context retention and tool selection accuracy.

**Architecture:** Add ExecutionContext as semantic backbone through execution chain, enhance ToolCallInfo/Result with purpose fields, add structured tool descriptions with differentiation.

**Tech Stack:** Rust, serde, serde_json

---

## Task 1: Add Knowledge and Entity Types

**Files:**
- Modify: `core/src/components/types.rs`
- Test: `core/src/components/types.rs` (inline tests)

**Step 1: Write the failing test**

Add to the `#[cfg(test)]` module at the end of `core/src/components/types.rs`:

```rust
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
    let entity = Entity::new("project", "Aleph");
    assert_eq!(entity.entity_type, "project");
    assert_eq!(entity.value, "Aleph");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_knowledge_creation --lib -- --nocapture`
Expected: FAIL with "cannot find value `Knowledge`"

**Step 3: Write minimal implementation**

Add after the `SummaryPart` struct (around line 153):

```rust
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
    pub fn new(key: impl Into<String>, value: impl Into<String>, source: impl Into<String>) -> Self {
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
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_knowledge_creation test_entity_creation --lib -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/components/types.rs
git commit -m "feat(types): add Knowledge and Entity types for ExecutionContext"
```

---

## Task 2: Add UserIntent and Goal Types

**Files:**
- Modify: `core/src/components/types.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_user_intent_creation() {
    let intent = UserIntent::new("Help me deploy the project")
        .understood_as("Deploy current project to remote server")
        .with_entity(Entity::new("project", "Aleph"))
        .with_expectation("Don't break existing service");

    assert_eq!(intent.raw_input, "Help me deploy the project");
    assert_eq!(intent.understood_as, Some("Deploy current project to remote server".to_string()));
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
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_user_intent_creation --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Add after Entity impl:

```rust
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
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_user_intent_creation test_goal_creation --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/components/types.rs
git commit -m "feat(types): add UserIntent and Goal types"
```

---

## Task 3: Add ExecutionContext

**Files:**
- Modify: `core/src/components/types.rs`

**Step 1: Write the failing test**

```rust
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
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_execution_context_creation --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Add after Goal impl:

```rust
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
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_execution_context --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/components/types.rs
git commit -m "feat(types): add ExecutionContext with decision trail and knowledge"
```

---

## Task 4: Enhance ToolCallInfo with Purpose

**Files:**
- Modify: `core/src/agents/rig/types.rs`

**Step 1: Write the failing test**

Add to the test module:

```rust
#[test]
fn test_tool_call_info_with_purpose() {
    let info = ToolCallInfo::new("call_123", "search_files", serde_json::json!({"pattern": "*.toml"}))
        .with_purpose("Find configuration files to determine build method")
        .with_expected_outcome("List of config file paths")
        .with_goal_relation(GoalRelation::GathersInformation);

    assert_eq!(info.purpose, Some("Find configuration files to determine build method".to_string()));
    assert_eq!(info.expected_outcome, Some("List of config file paths".to_string()));
    assert_eq!(info.goal_relation, Some(GoalRelation::GathersInformation));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_tool_call_info_with_purpose --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

First, add the GoalRelation enum after the imports:

```rust
/// How a tool call relates to the current goal
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GoalRelation {
    /// Directly achieves the goal
    DirectlyAchieves,
    /// Gathers information for subsequent decisions
    GathersInformation,
    /// Validates previous results
    Validates,
    /// Prepares for subsequent steps
    Prepares,
}
```

Then update the `ToolCallInfo` struct:

```rust
/// Information about a tool call from the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    /// Unique ID for this tool call
    pub id: String,

    /// Tool name to execute
    pub name: String,

    /// Arguments for the tool (JSON)
    pub arguments: Value,

    // === New fields for context retention ===
    /// Purpose of this call (LLM generated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,

    /// Expected outcome type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_outcome: Option<String>,

    /// Relation to current goal
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_relation: Option<GoalRelation>,
}

impl ToolCallInfo {
    /// Create a new tool call info
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            purpose: None,
            expected_outcome: None,
            goal_relation: None,
        }
    }

    /// Set the purpose of this tool call
    pub fn with_purpose(mut self, purpose: impl Into<String>) -> Self {
        self.purpose = Some(purpose.into());
        self
    }

    /// Set the expected outcome
    pub fn with_expected_outcome(mut self, outcome: impl Into<String>) -> Self {
        self.expected_outcome = Some(outcome.into());
        self
    }

    /// Set the goal relation
    pub fn with_goal_relation(mut self, relation: GoalRelation) -> Self {
        self.goal_relation = Some(relation);
        self
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_tool_call_info_with_purpose --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/agents/rig/types.rs
git commit -m "feat(types): add purpose and goal_relation to ToolCallInfo"
```

---

## Task 5: Enhance ToolCallResult with Summary

**Files:**
- Modify: `core/src/agents/rig/types.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_tool_call_result_with_summary() {
    use crate::components::types::Knowledge;

    let result = ToolCallResult::success("call_123", "search_files", "Found 3 files", 150)
        .with_summary("Located config files: Cargo.toml, .env, settings.json")
        .with_goal_contribution("Config file locations confirmed")
        .with_knowledge(Knowledge::new("config_path", "./Cargo.toml", "search_files"));

    assert_eq!(result.summary, Some("Located config files: Cargo.toml, .env, settings.json".to_string()));
    assert_eq!(result.goal_contribution, Some("Config file locations confirmed".to_string()));
    assert_eq!(result.extracted_knowledge.len(), 1);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_tool_call_result_with_summary --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Update the `ToolCallResult` struct:

```rust
/// Result of executing a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Tool call ID (matches ToolCallInfo.id)
    pub tool_call_id: String,

    /// Tool name
    pub name: String,

    /// Result content (string or JSON)
    pub content: String,

    /// Whether execution was successful
    pub success: bool,

    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    // === New fields for context retention ===
    /// Result summary (human-readable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Contribution to goal
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_contribution: Option<String>,

    /// Extracted knowledge fragments
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extracted_knowledge: Vec<crate::components::types::Knowledge>,
}

impl ToolCallResult {
    /// Create a successful result
    pub fn success(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            name: name.into(),
            content: content.into(),
            success: true,
            error: None,
            duration_ms,
            summary: None,
            goal_contribution: None,
            extracted_knowledge: Vec::new(),
        }
    }

    /// Create a failed result
    pub fn failure(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        error: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            name: name.into(),
            content: String::new(),
            success: false,
            error: Some(error.into()),
            duration_ms,
            summary: None,
            goal_contribution: None,
            extracted_knowledge: Vec::new(),
        }
    }

    /// Set result summary
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Set goal contribution
    pub fn with_goal_contribution(mut self, contribution: impl Into<String>) -> Self {
        self.goal_contribution = Some(contribution.into());
        self
    }

    /// Add extracted knowledge
    pub fn with_knowledge(mut self, knowledge: crate::components::types::Knowledge) -> Self {
        self.extracted_knowledge.push(knowledge);
        self
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_tool_call_result_with_summary --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/agents/rig/types.rs
git commit -m "feat(types): add summary and knowledge extraction to ToolCallResult"
```

---

## Task 6: Add Structured Tool Description Types

**Files:**
- Modify: `core/src/dispatcher/types.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_capability_creation() {
    let cap = Capability::new("search", "file names", "project directory", "list of paths");
    assert_eq!(cap.action, "search");
    assert_eq!(cap.target, "file names");
}

#[test]
fn test_tool_diff_creation() {
    let diff = ToolDiff::new(
        "search_content",
        "matches file name/path",
        "matches file content",
        "know file name",
        "know content",
    );
    assert_eq!(diff.other_tool, "search_content");
    assert_eq!(diff.choose_this_when, "know file name");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_capability_creation --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Add after the `ToolDefinition` impl block:

```rust
// =============================================================================
// Structured Tool Description Types (for LLM tool selection)
// =============================================================================

/// Capability description for structured tool definitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// Action verb (e.g., "search", "read", "write")
    pub action: String,
    /// Target of action (e.g., "file names", "file content")
    pub target: String,
    /// Scope limitation (e.g., "project directory", "current file")
    pub scope: String,
    /// Output type (e.g., "list of paths", "file content string")
    pub output: String,
}

impl Capability {
    /// Create a new capability
    pub fn new(
        action: impl Into<String>,
        target: impl Into<String>,
        scope: impl Into<String>,
        output: impl Into<String>,
    ) -> Self {
        Self {
            action: action.into(),
            target: target.into(),
            scope: scope.into(),
            output: output.into(),
        }
    }

    /// Format for LLM prompt
    pub fn to_prompt(&self) -> String {
        format!(
            "{} {} within {} → {}",
            self.action, self.target, self.scope, self.output
        )
    }
}

/// Tool differentiation for distinguishing similar tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDiff {
    /// The other tool being compared
    pub other_tool: String,
    /// What this tool does (brief)
    pub this_tool: String,
    /// What the other tool does (brief)
    pub other_is: String,
    /// When to choose this tool
    pub choose_this_when: String,
    /// When to choose the other tool
    pub choose_other_when: String,
}

impl ToolDiff {
    /// Create a new tool differentiation
    pub fn new(
        other_tool: impl Into<String>,
        this_tool: impl Into<String>,
        other_is: impl Into<String>,
        choose_this_when: impl Into<String>,
        choose_other_when: impl Into<String>,
    ) -> Self {
        Self {
            other_tool: other_tool.into(),
            this_tool: this_tool.into(),
            other_is: other_is.into(),
            choose_this_when: choose_this_when.into(),
            choose_other_when: choose_other_when.into(),
        }
    }

    /// Format for LLM prompt
    pub fn to_prompt(&self) -> String {
        format!(
            "vs {}: this={}, that={}. Choose this when: {}",
            self.other_tool, self.this_tool, self.other_is, self.choose_this_when
        )
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_capability_creation test_tool_diff_creation --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/dispatcher/types.rs
git commit -m "feat(types): add Capability and ToolDiff for structured tool descriptions"
```

---

## Task 7: Add StructuredToolMeta to UnifiedTool

**Files:**
- Modify: `core/src/dispatcher/types.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_unified_tool_with_structured_meta() {
    let tool = UnifiedTool::new(
        "builtin:search_files",
        "search_files",
        "Search for files by name pattern",
        ToolSource::Builtin,
    )
    .with_capability(Capability::new("search", "file names", "project", "file paths"))
    .with_not_suitable_for("searching file content")
    .with_differentiation(ToolDiff::new(
        "search_content",
        "matches names",
        "matches content",
        "know file name",
        "know content",
    ))
    .with_use_when("user mentions specific file name");

    assert!(tool.structured_meta.is_some());
    let meta = tool.structured_meta.unwrap();
    assert_eq!(meta.capabilities.len(), 1);
    assert_eq!(meta.not_suitable_for.len(), 1);
    assert_eq!(meta.differentiation.len(), 1);
    assert_eq!(meta.use_when.len(), 1);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_unified_tool_with_structured_meta --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Add the `StructuredToolMeta` struct:

```rust
/// Structured metadata for enhanced tool descriptions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructuredToolMeta {
    /// Core capabilities (precise enumeration)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,

    /// Explicitly unsuitable scenarios (prevent misuse)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_suitable_for: Vec<String>,

    /// Differentiation from similar tools
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differentiation: Vec<ToolDiff>,

    /// Typical use cases (positive examples)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub use_when: Vec<String>,
}

impl StructuredToolMeta {
    /// Check if this metadata is empty
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
            && self.not_suitable_for.is_empty()
            && self.differentiation.is_empty()
            && self.use_when.is_empty()
    }

    /// Format for LLM prompt
    pub fn to_prompt(&self) -> String {
        let mut parts = Vec::new();

        if !self.capabilities.is_empty() {
            let caps = self
                .capabilities
                .iter()
                .map(|c| c.to_prompt())
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(format!("Can: {}", caps));
        }

        if !self.not_suitable_for.is_empty() {
            parts.push(format!("NOT for: {}", self.not_suitable_for.join(", ")));
        }

        if !self.differentiation.is_empty() {
            let diffs = self
                .differentiation
                .iter()
                .map(|d| d.to_prompt())
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(diffs);
        }

        if !self.use_when.is_empty() {
            parts.push(format!("Use when: {}", self.use_when.join("; ")));
        }

        parts.join(" | ")
    }
}
```

Add the field to `UnifiedTool`:

```rust
// Add after the `was_renamed` field:
    /// Structured metadata for enhanced tool descriptions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_meta: Option<StructuredToolMeta>,
```

Update `UnifiedTool::new()` to initialize it:

```rust
// Add in new():
            structured_meta: None,
```

Add builder methods:

```rust
    /// Builder method: add a capability
    pub fn with_capability(mut self, capability: Capability) -> Self {
        let meta = self.structured_meta.get_or_insert_with(StructuredToolMeta::default);
        meta.capabilities.push(capability);
        self
    }

    /// Builder method: add a not-suitable-for scenario
    pub fn with_not_suitable_for(mut self, scenario: impl Into<String>) -> Self {
        let meta = self.structured_meta.get_or_insert_with(StructuredToolMeta::default);
        meta.not_suitable_for.push(scenario.into());
        self
    }

    /// Builder method: add a differentiation
    pub fn with_differentiation(mut self, diff: ToolDiff) -> Self {
        let meta = self.structured_meta.get_or_insert_with(StructuredToolMeta::default);
        meta.differentiation.push(diff);
        self
    }

    /// Builder method: add a use-when scenario
    pub fn with_use_when(mut self, scenario: impl Into<String>) -> Self {
        let meta = self.structured_meta.get_or_insert_with(StructuredToolMeta::default);
        meta.use_when.push(scenario.into());
        self
    }
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_unified_tool_with_structured_meta --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/dispatcher/types.rs
git commit -m "feat(types): add StructuredToolMeta to UnifiedTool for enhanced descriptions"
```

---

## Task 8: Add ContextVerbosity and Prompt Generation

**Files:**
- Modify: `core/src/components/types.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_context_verbosity_prompt_generation() {
    let intent = UserIntent::new("Deploy project")
        .understood_as("Deploy to server");
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
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_context_verbosity_prompt_generation --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Add after `ExecutionPhase`:

```rust
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
```

Add to `ExecutionContext` impl:

```rust
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
                .map(|k| format!("- {}: {} (source: {}, confidence: {:.0}%)",
                    k.key, k.value, k.source, k.confidence * 100.0))
                .collect();
            parts.push(format!("**Acquired Information**:\n{}", knowledge_lines.join("\n")));
        }

        // Decision history
        if !self.decision_trail.is_empty() {
            let decision_lines: Vec<String> = self
                .decision_trail
                .iter()
                .enumerate()
                .map(|(i, d)| format!("{}. {} - {}", i + 1, d.choice, d.reasoning))
                .collect();
            parts.push(format!("**Decision History**:\n{}", decision_lines.join("\n")));
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
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_context_verbosity_prompt_generation --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/components/types.rs
git commit -m "feat(types): add ContextVerbosity and prompt generation methods"
```

---

## Task 9: Run Full Test Suite

**Step 1: Run all tests**

Run: `cargo test --lib 2>&1 | tail -50`
Expected: All new tests pass, existing tests unchanged

**Step 2: Run clippy**

Run: `cargo clippy --lib -- -D warnings 2>&1 | tail -30`
Expected: No warnings

**Step 3: Commit if needed**

Fix any issues and commit:

```bash
git add -A
git commit -m "fix: address clippy warnings and test issues"
```

---

## Task 10: Final Integration Commit

**Step 1: Verify all changes**

Run: `git log --oneline -10`

**Step 2: Create summary commit if needed**

If multiple fix commits were made, optionally squash or add a summary:

```bash
git add -A
git commit -m "feat(llm-data-format): complete Phase 1 - core data structures

Added:
- ExecutionContext with UserIntent, Goal, Knowledge, DecisionRecord
- Enhanced ToolCallInfo with purpose, expected_outcome, goal_relation
- Enhanced ToolCallResult with summary, goal_contribution, extracted_knowledge
- StructuredToolMeta with Capability, ToolDiff for tool descriptions
- ContextVerbosity for token-optimized prompt generation

Resolves context retention and tool selection accuracy issues.
"
```

---

## Summary

| Task | Files Modified | New Types Added |
|------|----------------|-----------------|
| 1 | components/types.rs | Knowledge, Entity |
| 2 | components/types.rs | UserIntent, Goal, GoalStatus |
| 3 | components/types.rs | ExecutionContext, DecisionRecord, ExecutionPhase |
| 4 | agents/rig/types.rs | GoalRelation (+ ToolCallInfo fields) |
| 5 | agents/rig/types.rs | (ToolCallResult fields) |
| 6 | dispatcher/types.rs | Capability, ToolDiff |
| 7 | dispatcher/types.rs | StructuredToolMeta (+ UnifiedTool field) |
| 8 | components/types.rs | ContextVerbosity (+ prompt methods) |

**Next Phase:** Update message generation in `message_history.rs` to use these new types.
