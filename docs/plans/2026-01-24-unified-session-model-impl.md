# Unified Session Model Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Unify LoopState and ExecutionSession into a single message model, integrating filter_compacted(), real-time overflow detection, system reminder injection, and cache optimization.

**Architecture:** Three-phase migration - Bridge layer → Message builder refactor → LoopState deprecation. Each phase is independently deployable with feature flags for rollback.

**Tech Stack:** Rust, tokio, serde, chrono

---

## Phase 1: Bridge Layer (Tasks 1-4)

### Task 1: Enhance ExecutionSession with LoopState fields

**Files:**
- Modify: `core/src/components/types.rs:12-55`
- Test: `core/src/components/types.rs` (existing tests)

**Step 1: Write the failing test**

Add to `core/src/components/types.rs` in the `tests` module:

```rust
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
    assert_eq!(session.context.current_app, Some("Terminal".to_string()));
    assert!(!session.needs_compaction);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_execution_session_with_request_context -p aethecore --lib`
Expected: FAIL with "no method named `with_original_request`"

**Step 3: Write minimal implementation**

In `core/src/components/types.rs`, modify `ExecutionSession`:

```rust
use crate::agent_loop::RequestContext;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSession {
    // Existing fields
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

    // New fields from LoopState
    #[serde(default)]
    pub original_request: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<RequestContext>,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub needs_compaction: bool,
    #[serde(default)]
    pub last_compaction_index: usize,
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
            // New fields
            original_request: String::new(),
            context: None,
            started_at: now,
            needs_compaction: false,
            last_compaction_index: 0,
        }
    }

    pub fn with_original_request(mut self, request: impl Into<String>) -> Self {
        self.original_request = request.into();
        self
    }

    pub fn with_context(mut self, context: RequestContext) -> Self {
        self.context = Some(context);
        self
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_execution_session_with_request_context -p aethecore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/components/types.rs
git commit -m "$(cat <<'EOF'
feat(session): add LoopState fields to ExecutionSession

- Add original_request, context, started_at, needs_compaction, last_compaction_index
- Add builder methods with_original_request() and with_context()
- First step toward unified session model

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Add SystemReminderPart to SessionPart

**Files:**
- Modify: `core/src/components/types.rs:66-94`
- Test: `core/src/components/types.rs`

**Step 1: Write the failing test**

```rust
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
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_system_reminder_part -p aethecore --lib`
Expected: FAIL with "no variant named `SystemReminder`"

**Step 3: Write minimal implementation**

Add to `core/src/components/types.rs`:

```rust
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemReminderPart {
    pub content: String,
    pub reminder_type: ReminderType,
    pub timestamp: i64,
}

// Update SessionPart enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionPart {
    UserInput(UserInputPart),
    AiResponse(AiResponsePart),
    ToolCall(ToolCallPart),
    Reasoning(ReasoningPart),
    PlanCreated(PlanPart),
    SubAgentCall(SubAgentPart),
    Summary(SummaryPart),
    CompactionMarker(CompactionMarker),
    SystemReminder(SystemReminderPart),  // New
}

impl SessionPart {
    pub fn type_name(&self) -> &'static str {
        match self {
            // ... existing matches ...
            SessionPart::SystemReminder(_) => "system_reminder",
        }
    }
}

impl PartId for SessionPart {
    fn part_id(&self) -> String {
        match self {
            // ... existing matches ...
            SessionPart::SystemReminder(p) => format!("reminder_{}", p.timestamp),
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_system_reminder_part -p aethecore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/components/types.rs
git commit -m "$(cat <<'EOF'
feat(session): add SystemReminderPart for context injection

- Add ReminderType enum (ContinueTask, MaxStepsWarning, TokenLimitWarning, etc.)
- Add SystemReminderPart struct
- Integrate into SessionPart enum
- Aligns with OpenCode's <system-reminder> mechanism

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Create SessionSync bridge module

**Files:**
- Create: `core/src/agent_loop/session_sync.rs`
- Modify: `core/src/agent_loop/mod.rs` (add module)
- Test: `core/src/agent_loop/session_sync.rs`

**Step 1: Write the failing test**

Create `core/src/agent_loop/session_sync.rs`:

```rust
//! Session synchronization bridge
//!
//! Bridges LoopState and ExecutionSession during migration.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::{LoopState, RequestContext};
    use crate::components::types::ExecutionSession;

    #[test]
    fn test_sync_loop_state_to_session() {
        let state = LoopState::new(
            "test-session".to_string(),
            "Find files".to_string(),
            RequestContext::empty(),
        );

        let session = SessionSync::to_execution_session(&state);

        assert_eq!(session.id, "test-session");
        assert_eq!(session.original_request, "Find files");
        assert_eq!(session.iteration_count, 0);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_sync_loop_state_to_session -p aethecore --lib`
Expected: FAIL with "unresolved import" or "cannot find value `SessionSync`"

**Step 3: Write minimal implementation**

```rust
//! Session synchronization bridge
//!
//! This module provides bidirectional sync between LoopState (legacy)
//! and ExecutionSession (unified model) during the migration period.

use crate::agent_loop::{Action, ActionResult, LoopState, LoopStep, RequestContext};
use crate::components::types::{
    AiResponsePart, ExecutionSession, SessionPart, SessionStatus,
    ToolCallPart, ToolCallStatus, UserInputPart,
};

/// Bridge for synchronizing LoopState <-> ExecutionSession
pub struct SessionSync;

impl SessionSync {
    /// Convert LoopState to ExecutionSession
    pub fn to_execution_session(state: &LoopState) -> ExecutionSession {
        let mut session = ExecutionSession::new();
        session.id = state.session_id.clone();
        session.original_request = state.original_request.clone();
        session.context = Some(state.context.clone());
        session.iteration_count = state.step_count as u32;
        session.total_tokens = state.total_tokens as u64;
        session.started_at = state.started_at.elapsed().as_secs() as i64;

        // Convert steps to parts
        session.parts = Self::steps_to_parts(&state.steps, &state.original_request);

        session
    }

    /// Convert LoopSteps to SessionParts
    fn steps_to_parts(steps: &[LoopStep], original_request: &str) -> Vec<SessionPart> {
        let mut parts = Vec::new();
        let now = chrono::Utc::now().timestamp();

        // Add initial user input
        parts.push(SessionPart::UserInput(UserInputPart {
            text: original_request.to_string(),
            context: None,
            timestamp: now,
        }));

        // Convert each step
        for step in steps {
            // AI reasoning/response
            if let Some(ref reasoning) = step.thinking.reasoning {
                parts.push(SessionPart::AiResponse(AiResponsePart {
                    content: reasoning.clone(),
                    reasoning: Some(reasoning.clone()),
                    timestamp: now,
                }));
            }

            // Tool calls
            if let Action::ToolCall { tool_name, arguments } = &step.action {
                let (status, output, error) = match &step.result {
                    ActionResult::ToolSuccess { output } => {
                        (ToolCallStatus::Completed, Some(output.clone()), None)
                    }
                    ActionResult::ToolError { error } => {
                        (ToolCallStatus::Failed, None, Some(error.clone()))
                    }
                    _ => (ToolCallStatus::Completed, None, None),
                };

                parts.push(SessionPart::ToolCall(ToolCallPart {
                    id: format!("call-{}", step.step_id),
                    tool_name: tool_name.clone(),
                    input: arguments.clone(),
                    status,
                    output,
                    error,
                    started_at: now,
                    completed_at: Some(now),
                }));
            }
        }

        parts
    }

    /// Sync ExecutionSession state back to LoopState
    pub fn sync_to_loop_state(session: &ExecutionSession, state: &mut LoopState) {
        state.total_tokens = session.total_tokens as usize;
        state.step_count = session.iteration_count as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::{LoopState, RequestContext};

    #[test]
    fn test_sync_loop_state_to_session() {
        let state = LoopState::new(
            "test-session".to_string(),
            "Find files".to_string(),
            RequestContext::empty(),
        );

        let session = SessionSync::to_execution_session(&state);

        assert_eq!(session.id, "test-session");
        assert_eq!(session.original_request, "Find files");
        assert_eq!(session.iteration_count, 0);
    }

    #[test]
    fn test_sync_creates_user_input_part() {
        let state = LoopState::new(
            "test".to_string(),
            "Hello world".to_string(),
            RequestContext::empty(),
        );

        let session = SessionSync::to_execution_session(&state);

        assert!(!session.parts.is_empty());
        match &session.parts[0] {
            SessionPart::UserInput(p) => assert_eq!(p.text, "Hello world"),
            _ => panic!("Expected UserInput part"),
        }
    }
}
```

**Step 4: Update mod.rs to include the module**

In `core/src/agent_loop/mod.rs`, add:

```rust
pub mod session_sync;
pub use session_sync::SessionSync;
```

**Step 5: Run test to verify it passes**

Run: `cargo test session_sync -p aethecore --lib`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/agent_loop/session_sync.rs core/src/agent_loop/mod.rs
git commit -m "$(cat <<'EOF'
feat(agent_loop): add SessionSync bridge for migration

- Create session_sync.rs module
- Implement LoopState -> ExecutionSession conversion
- Convert LoopSteps to SessionParts
- Bridge layer for Phase 1 migration

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Add feature flag for unified session

**Files:**
- Modify: `core/src/agent_loop/config.rs`
- Test: `core/src/agent_loop/config.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_unified_session_feature_flags() {
    let config = LoopConfig::default();

    assert!(!config.use_unified_session);
    assert!(!config.use_message_builder);
    assert!(!config.use_realtime_overflow);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_unified_session_feature_flags -p aethecore --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

In `core/src/agent_loop/config.rs`, add to `LoopConfig`:

```rust
/// Feature flags for unified session migration
#[derive(Debug, Clone)]
pub struct LoopConfig {
    // ... existing fields ...

    /// Use unified ExecutionSession model (Phase 1)
    pub use_unified_session: bool,
    /// Use new MessageBuilder for prompt construction (Phase 2)
    pub use_message_builder: bool,
    /// Enable real-time overflow detection (Phase 2)
    pub use_realtime_overflow: bool,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            // ... existing defaults ...
            use_unified_session: false,
            use_message_builder: false,
            use_realtime_overflow: false,
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_unified_session_feature_flags -p aethecore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/agent_loop/config.rs
git commit -m "$(cat <<'EOF'
feat(config): add feature flags for unified session migration

- Add use_unified_session flag (Phase 1)
- Add use_message_builder flag (Phase 2)
- Add use_realtime_overflow flag (Phase 2)
- All disabled by default for safe rollout

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2: Message Builder (Tasks 5-9)

### Task 5: Create MessageBuilder module

**Files:**
- Create: `core/src/agent_loop/message_builder.rs`
- Modify: `core/src/agent_loop/mod.rs`
- Test: `core/src/agent_loop/message_builder.rs`

**Step 1: Write the failing test**

Create `core/src/agent_loop/message_builder.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::types::*;

    #[test]
    fn test_parts_to_messages_user_input() {
        let builder = MessageBuilder::new(MessageBuilderConfig::default());

        let parts = vec![
            SessionPart::UserInput(UserInputPart {
                text: "Hello".to_string(),
                context: None,
                timestamp: 1000,
            }),
        ];

        let messages = builder.parts_to_messages(&parts);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Hello");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_parts_to_messages_user_input -p aethecore --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

```rust
//! Message builder for converting SessionParts to LLM messages
//!
//! This module handles the conversion of ExecutionSession parts
//! into the message format expected by LLM providers.

use crate::components::types::{
    SessionPart, ToolCallStatus, ExecutionSession,
};
use serde::{Deserialize, Serialize};

/// Configuration for message building
#[derive(Debug, Clone)]
pub struct MessageBuilderConfig {
    /// Maximum messages to include
    pub max_messages: usize,
    /// Enable system reminder injection
    pub inject_reminders: bool,
    /// Reminder injection threshold (iteration count)
    pub reminder_threshold: u32,
}

impl Default for MessageBuilderConfig {
    fn default() -> Self {
        Self {
            max_messages: 100,
            inject_reminders: true,
            reminder_threshold: 1,
        }
    }
}

/// Simple message structure for LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn tool_result(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_call_id: Some(id.into()),
            tool_calls: None,
        }
    }

    pub fn assistant_with_tool_call(tool_call: ToolCall) -> Self {
        Self {
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: Some(vec![tool_call]),
        }
    }
}

/// Message builder for SessionParts -> LLM messages
pub struct MessageBuilder {
    config: MessageBuilderConfig,
}

impl MessageBuilder {
    pub fn new(config: MessageBuilderConfig) -> Self {
        Self { config }
    }

    /// Convert SessionParts to LLM messages
    pub fn parts_to_messages(&self, parts: &[SessionPart]) -> Vec<Message> {
        let mut messages = Vec::new();

        for part in parts {
            match part {
                SessionPart::UserInput(p) => {
                    messages.push(Message::user(&p.text));
                }

                SessionPart::AiResponse(p) => {
                    messages.push(Message::assistant(&p.content));
                }

                SessionPart::ToolCall(p) => {
                    // Tool call request
                    let tool_call = ToolCall {
                        id: p.id.clone(),
                        name: p.tool_name.clone(),
                        arguments: serde_json::to_string(&p.input).unwrap_or_default(),
                    };
                    messages.push(Message::assistant_with_tool_call(tool_call));

                    // Tool result
                    match p.status {
                        ToolCallStatus::Completed => {
                            let output = p.output.as_deref().unwrap_or("");
                            messages.push(Message::tool_result(&p.id, output));
                        }
                        ToolCallStatus::Failed => {
                            let error = p.error.as_deref().unwrap_or("Unknown error");
                            messages.push(Message::tool_result(&p.id, format!("Error: {}", error)));
                        }
                        ToolCallStatus::Pending | ToolCallStatus::Running => {
                            messages.push(Message::tool_result(&p.id, "[Tool execution was interrupted]"));
                        }
                        ToolCallStatus::Aborted => {
                            messages.push(Message::tool_result(&p.id, "[Tool execution was aborted]"));
                        }
                    }
                }

                SessionPart::Summary(p) => {
                    // Summary becomes a Q&A pair (like OpenCode)
                    messages.push(Message::user("What did we do so far?"));
                    messages.push(Message::assistant(&p.content));
                }

                SessionPart::SystemReminder(_) => {
                    // Handled separately in inject_reminders
                }

                _ => {
                    // Skip other part types for now
                }
            }
        }

        messages
    }

    /// Build messages from ExecutionSession with filtering and reminders
    pub fn build_messages(&self, session: &ExecutionSession, filtered_parts: &[SessionPart]) -> Vec<Message> {
        let mut messages = self.parts_to_messages(filtered_parts);

        if self.config.inject_reminders && session.iteration_count > self.config.reminder_threshold {
            self.inject_reminders(&mut messages, session);
        }

        messages
    }

    /// Inject system reminders into messages
    fn inject_reminders(&self, messages: &mut Vec<Message>, session: &ExecutionSession) {
        // Find last user message and wrap it
        if let Some(last_user_idx) = messages.iter().rposition(|m| m.role == "user") {
            let original = messages[last_user_idx].content.clone();
            messages[last_user_idx].content = format!(
                "<system-reminder>\n\
                The user sent the following message:\n\
                {}\n\
                Please address this message and continue with your tasks.\n\
                </system-reminder>",
                original
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::types::*;

    #[test]
    fn test_parts_to_messages_user_input() {
        let builder = MessageBuilder::new(MessageBuilderConfig::default());

        let parts = vec![
            SessionPart::UserInput(UserInputPart {
                text: "Hello".to_string(),
                context: None,
                timestamp: 1000,
            }),
        ];

        let messages = builder.parts_to_messages(&parts);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Hello");
    }

    #[test]
    fn test_parts_to_messages_tool_call() {
        let builder = MessageBuilder::new(MessageBuilderConfig::default());

        let parts = vec![
            SessionPart::ToolCall(ToolCallPart {
                id: "call-1".to_string(),
                tool_name: "search".to_string(),
                input: serde_json::json!({"query": "test"}),
                status: ToolCallStatus::Completed,
                output: Some("Found 3 results".to_string()),
                error: None,
                started_at: 1000,
                completed_at: Some(2000),
            }),
        ];

        let messages = builder.parts_to_messages(&parts);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "assistant");
        assert!(messages[0].tool_calls.is_some());
        assert_eq!(messages[1].role, "tool");
        assert_eq!(messages[1].content, "Found 3 results");
    }

    #[test]
    fn test_inject_reminders() {
        let builder = MessageBuilder::new(MessageBuilderConfig::default());

        let parts = vec![
            SessionPart::UserInput(UserInputPart {
                text: "Do something".to_string(),
                context: None,
                timestamp: 1000,
            }),
        ];

        let mut session = ExecutionSession::new();
        session.iteration_count = 5;

        let messages = builder.build_messages(&session, &parts);

        assert!(messages[0].content.contains("<system-reminder>"));
        assert!(messages[0].content.contains("Do something"));
    }

    #[test]
    fn test_summary_to_qa_pair() {
        let builder = MessageBuilder::new(MessageBuilderConfig::default());

        let parts = vec![
            SessionPart::Summary(SummaryPart {
                content: "We searched for files and found config.toml".to_string(),
                original_count: 5,
                compacted_at: 1000,
            }),
        ];

        let messages = builder.parts_to_messages(&parts);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "What did we do so far?");
        assert!(messages[1].content.contains("config.toml"));
    }
}
```

**Step 4: Update mod.rs**

In `core/src/agent_loop/mod.rs`, add:

```rust
pub mod message_builder;
pub use message_builder::{MessageBuilder, MessageBuilderConfig, Message};
```

**Step 5: Run test to verify it passes**

Run: `cargo test message_builder -p aethecore --lib`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/agent_loop/message_builder.rs core/src/agent_loop/mod.rs
git commit -m "$(cat <<'EOF'
feat(agent_loop): add MessageBuilder for SessionPart conversion

- Create message_builder.rs module
- Implement parts_to_messages() conversion
- Add system reminder injection
- Handle tool calls, summaries, and user input
- Aligns with OpenCode's toModelMessages()

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Integrate filter_compacted into MessageBuilder

**Files:**
- Modify: `core/src/agent_loop/message_builder.rs`
- Test: `core/src/agent_loop/message_builder.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_build_messages_with_filter_compacted() {
    use crate::components::session_compactor::SessionCompactor;

    let compactor = SessionCompactor::new(Default::default());
    let builder = MessageBuilder::with_compactor(
        MessageBuilderConfig::default(),
        Arc::new(compactor),
    );

    let mut session = ExecutionSession::new();
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Old message".to_string(),
        context: None,
        timestamp: 1000,
    }));
    session.parts.push(SessionPart::Summary(SummaryPart {
        content: "Summary of old work".to_string(),
        original_count: 5,
        compacted_at: 2000,
    }));
    session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
        timestamp: 2000,
        auto: true,
    }));
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "New message".to_string(),
        context: None,
        timestamp: 3000,
    }));

    let messages = builder.build_from_session(&session);

    // Should only include parts after compaction marker
    assert!(!messages.iter().any(|m| m.content.contains("Old message")));
    assert!(messages.iter().any(|m| m.content.contains("Summary of old work")));
    assert!(messages.iter().any(|m| m.content.contains("New message")));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_build_messages_with_filter_compacted -p aethecore --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Add to `message_builder.rs`:

```rust
use std::sync::Arc;
use crate::components::session_compactor::SessionCompactor;

pub struct MessageBuilder {
    config: MessageBuilderConfig,
    compactor: Option<Arc<SessionCompactor>>,
}

impl MessageBuilder {
    pub fn new(config: MessageBuilderConfig) -> Self {
        Self { config, compactor: None }
    }

    pub fn with_compactor(config: MessageBuilderConfig, compactor: Arc<SessionCompactor>) -> Self {
        Self { config, compactor: Some(compactor) }
    }

    /// Build messages from session, applying filter_compacted
    pub fn build_from_session(&self, session: &ExecutionSession) -> Vec<Message> {
        let filtered_parts = if let Some(ref compactor) = self.compactor {
            compactor.filter_compacted(session)
        } else {
            session.parts.clone()
        };

        self.build_messages(session, &filtered_parts)
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_build_messages_with_filter_compacted -p aethecore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/agent_loop/message_builder.rs
git commit -m "$(cat <<'EOF'
feat(message_builder): integrate filter_compacted

- Add with_compactor() constructor
- Implement build_from_session() with filtering
- Messages now respect compaction boundaries

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Add real-time overflow detection

**Files:**
- Create: `core/src/agent_loop/overflow.rs`
- Modify: `core/src/agent_loop/mod.rs`
- Test: `core/src/agent_loop/overflow.rs`

**Step 1: Write the failing test**

Create `core/src/agent_loop/overflow.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_overflow_under_limit() {
        let detector = OverflowDetector::new(OverflowConfig::default());

        let session = ExecutionSession::new().with_model("gpt-4-turbo");

        assert!(!detector.is_overflow(&session));
    }

    #[test]
    fn test_is_overflow_over_limit() {
        let detector = OverflowDetector::new(OverflowConfig::default());

        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");
        session.total_tokens = 150_000; // Over 128K * 0.8 threshold

        assert!(detector.is_overflow(&session));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test overflow -p aethecore --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

```rust
//! Real-time overflow detection
//!
//! Detects when session token usage exceeds model limits.

use crate::components::types::ExecutionSession;
use std::collections::HashMap;

/// Model limits configuration
#[derive(Debug, Clone)]
pub struct ModelLimit {
    pub context: u64,
    pub max_output: u64,
    pub reserve_ratio: f32,
}

impl ModelLimit {
    pub fn usable_tokens(&self) -> u64 {
        let output_reserve = self.max_output.min(32_000);
        ((self.context - output_reserve) as f32 * (1.0 - self.reserve_ratio)) as u64
    }
}

/// Overflow detection configuration
#[derive(Debug, Clone)]
pub struct OverflowConfig {
    pub model_limits: HashMap<String, ModelLimit>,
    pub default_limit: ModelLimit,
}

impl Default for OverflowConfig {
    fn default() -> Self {
        let mut limits = HashMap::new();

        limits.insert("gpt-4-turbo".to_string(), ModelLimit {
            context: 128_000,
            max_output: 4_096,
            reserve_ratio: 0.2,
        });
        limits.insert("gpt-4o".to_string(), ModelLimit {
            context: 128_000,
            max_output: 16_384,
            reserve_ratio: 0.2,
        });
        limits.insert("claude-3-opus".to_string(), ModelLimit {
            context: 200_000,
            max_output: 4_096,
            reserve_ratio: 0.2,
        });
        limits.insert("claude-3.5-sonnet".to_string(), ModelLimit {
            context: 200_000,
            max_output: 8_192,
            reserve_ratio: 0.2,
        });
        limits.insert("claude-sonnet-4".to_string(), ModelLimit {
            context: 200_000,
            max_output: 16_000,
            reserve_ratio: 0.2,
        });

        Self {
            model_limits: limits,
            default_limit: ModelLimit {
                context: 128_000,
                max_output: 4_096,
                reserve_ratio: 0.2,
            },
        }
    }
}

/// Real-time overflow detector
pub struct OverflowDetector {
    config: OverflowConfig,
}

impl OverflowDetector {
    pub fn new(config: OverflowConfig) -> Self {
        Self { config }
    }

    /// Check if session has exceeded token limits
    pub fn is_overflow(&self, session: &ExecutionSession) -> bool {
        let limit = self.get_model_limit(&session.model);
        session.total_tokens > limit.usable_tokens()
    }

    /// Get usage percentage (0-100)
    pub fn usage_percent(&self, session: &ExecutionSession) -> u8 {
        let limit = self.get_model_limit(&session.model);
        let usable = limit.usable_tokens();
        if usable == 0 {
            return 100;
        }
        ((session.total_tokens as f64 / usable as f64) * 100.0).min(100.0) as u8
    }

    fn get_model_limit(&self, model: &str) -> &ModelLimit {
        self.config.model_limits.get(model).unwrap_or(&self.config.default_limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_overflow_under_limit() {
        let detector = OverflowDetector::new(OverflowConfig::default());

        let session = ExecutionSession::new().with_model("gpt-4-turbo");

        assert!(!detector.is_overflow(&session));
    }

    #[test]
    fn test_is_overflow_over_limit() {
        let detector = OverflowDetector::new(OverflowConfig::default());

        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");
        // GPT-4-turbo: 128K context, 4K output, 20% reserve
        // Usable: (128000 - 4096) * 0.8 = ~99,123
        session.total_tokens = 100_000;

        assert!(detector.is_overflow(&session));
    }

    #[test]
    fn test_usage_percent() {
        let detector = OverflowDetector::new(OverflowConfig::default());

        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");
        session.total_tokens = 50_000;

        let percent = detector.usage_percent(&session);
        assert!(percent > 0 && percent < 100);
    }
}
```

**Step 4: Update mod.rs**

```rust
pub mod overflow;
pub use overflow::{OverflowDetector, OverflowConfig};
```

**Step 5: Run test to verify it passes**

Run: `cargo test overflow -p aethecore --lib`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/agent_loop/overflow.rs core/src/agent_loop/mod.rs
git commit -m "$(cat <<'EOF'
feat(agent_loop): add real-time overflow detection

- Create overflow.rs module
- Implement OverflowDetector with model-specific limits
- Add usage_percent() for warning thresholds
- Support GPT-4, Claude-3/4 model families

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Add limit warnings to MessageBuilder

**Files:**
- Modify: `core/src/agent_loop/message_builder.rs`
- Test: `core/src/agent_loop/message_builder.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_inject_token_limit_warning() {
    let detector = Arc::new(OverflowDetector::new(OverflowConfig::default()));
    let builder = MessageBuilder::with_overflow_detector(
        MessageBuilderConfig::default(),
        detector,
    );

    let mut session = ExecutionSession::new().with_model("gpt-4-turbo");
    session.total_tokens = 85_000; // ~85% of usable
    session.iteration_count = 5;

    let parts = vec![
        SessionPart::UserInput(UserInputPart {
            text: "Continue".to_string(),
            context: None,
            timestamp: 1000,
        }),
    ];

    let messages = builder.build_messages(&session, &parts);

    // Should contain token limit warning
    assert!(messages.iter().any(|m| m.content.contains("Context usage")));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_inject_token_limit_warning -p aethecore --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Update `message_builder.rs`:

```rust
use crate::agent_loop::overflow::OverflowDetector;

pub struct MessageBuilder {
    config: MessageBuilderConfig,
    compactor: Option<Arc<SessionCompactor>>,
    overflow_detector: Option<Arc<OverflowDetector>>,
}

impl MessageBuilder {
    pub fn with_overflow_detector(
        config: MessageBuilderConfig,
        detector: Arc<OverflowDetector>,
    ) -> Self {
        Self {
            config,
            compactor: None,
            overflow_detector: Some(detector),
        }
    }

    fn inject_limit_warnings(&self, messages: &mut Vec<Message>, session: &ExecutionSession) {
        if let Some(ref detector) = self.overflow_detector {
            let usage = detector.usage_percent(session);

            if usage >= 80 {
                let warning = format!(
                    "<system-reminder>\n\
                    Context usage is at {}%. Consider wrapping up or the session will be compacted.\n\
                    </system-reminder>",
                    usage
                );

                // Insert after last user message
                if let Some(idx) = messages.iter().rposition(|m| m.role == "user") {
                    messages.insert(idx + 1, Message::user(&warning));
                }
            }
        }
    }

    fn inject_reminders(&self, messages: &mut Vec<Message>, session: &ExecutionSession) {
        // ... existing reminder injection ...

        // Add limit warnings
        self.inject_limit_warnings(messages, session);
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_inject_token_limit_warning -p aethecore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/agent_loop/message_builder.rs
git commit -m "$(cat <<'EOF'
feat(message_builder): add token limit warnings

- Integrate OverflowDetector into MessageBuilder
- Inject warnings at 80% context usage
- Helps LLM prepare for potential compaction

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Add max steps warning

**Files:**
- Modify: `core/src/agent_loop/message_builder.rs`
- Test: `core/src/agent_loop/message_builder.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_inject_max_steps_warning() {
    let mut config = MessageBuilderConfig::default();
    config.max_iterations = 10;

    let builder = MessageBuilder::new(config);

    let mut session = ExecutionSession::new();
    session.iteration_count = 9; // Last step

    let parts = vec![
        SessionPart::UserInput(UserInputPart {
            text: "Continue".to_string(),
            context: None,
            timestamp: 1000,
        }),
    ];

    let messages = builder.build_messages(&session, &parts);

    assert!(messages.iter().any(|m| m.content.contains("LAST step")));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_inject_max_steps_warning -p aethecore --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Update `MessageBuilderConfig` and add method:

```rust
#[derive(Debug, Clone)]
pub struct MessageBuilderConfig {
    // ... existing fields ...
    pub max_iterations: u32,
}

impl Default for MessageBuilderConfig {
    fn default() -> Self {
        Self {
            max_messages: 100,
            inject_reminders: true,
            reminder_threshold: 1,
            max_iterations: 50,
        }
    }
}

impl MessageBuilder {
    fn inject_max_steps_warning(&self, messages: &mut Vec<Message>, session: &ExecutionSession) {
        let max = self.config.max_iterations;
        let current = session.iteration_count;

        if current == max - 1 {
            let warning = "<system-reminder>\n\
                This is your LAST step. You must either:\n\
                1. Complete the task and call `complete`\n\
                2. Ask the user for guidance\n\
                Do NOT start new tool calls that cannot finish in one step.\n\
                </system-reminder>";

            messages.push(Message::user(warning));
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_inject_max_steps_warning -p aethecore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/agent_loop/message_builder.rs
git commit -m "$(cat <<'EOF'
feat(message_builder): add max steps warning

- Add max_iterations config
- Inject warning on last step
- Aligns with OpenCode's max-steps.txt mechanism

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3: Integration and Cleanup (Tasks 10-12)

### Task 10: Integrate MessageBuilder into AgentLoop

**Files:**
- Modify: `core/src/agent_loop/mod.rs`
- Test: Integration test

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn test_agent_loop_uses_message_builder() {
    // This is an integration test that verifies the full flow
    let config = LoopConfig {
        use_unified_session: true,
        use_message_builder: true,
        ..Default::default()
    };

    // Create agent loop with new config
    // Verify it uses MessageBuilder instead of old prompt_builder
    // ...
}
```

**Step 2-5: Implement integration**

This task involves modifying the main `AgentLoop` to:
1. Create `ExecutionSession` alongside `LoopState`
2. Sync state changes using `SessionSync`
3. Use `MessageBuilder` when `use_message_builder` is true
4. Check overflow and trigger compaction in the loop

The implementation details are extensive - see design document for full specification.

**Step 6: Commit**

```bash
git commit -m "$(cat <<'EOF'
feat(agent_loop): integrate MessageBuilder and overflow detection

- Add ExecutionSession creation in run()
- Sync LoopState changes to ExecutionSession
- Use MessageBuilder when feature flag enabled
- Check overflow before each iteration
- Trigger compaction when needed

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Add cache optimization to PromptBuilder

**Files:**
- Modify: `core/src/thinker/prompt_builder.rs`
- Test: `core/src/thinker/prompt_builder.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_build_system_prompt_cached() {
    let builder = PromptBuilder::new(PromptConfig::default());

    let parts = builder.build_system_prompt_cached(&[]);

    assert_eq!(parts.len(), 2);
    assert!(parts[0].cache); // Static header should be cached
    assert!(!parts[1].cache); // Dynamic part should not be cached
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_build_system_prompt_cached -p aethecore --lib`
Expected: FAIL

**Step 3: Write minimal implementation**

Add to `prompt_builder.rs`:

```rust
#[derive(Debug, Clone)]
pub struct SystemPromptPart {
    pub content: String,
    pub cache: bool,
}

impl PromptBuilder {
    /// Build two-part system prompt for Anthropic cache optimization
    pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
        // Part 1: Static header (cacheable)
        let header = self.build_static_header();

        // Part 2: Dynamic content (tools, runtime, etc.)
        let dynamic = self.build_dynamic_content(tools);

        vec![
            SystemPromptPart { content: header, cache: true },
            SystemPromptPart { content: dynamic, cache: false },
        ]
    }

    fn build_static_header(&self) -> String {
        let mut prompt = String::new();
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");
        prompt
    }

    fn build_dynamic_content(&self, tools: &[ToolInfo]) -> String {
        // ... existing tool building logic ...
        self.build_system_prompt(tools) // Reuse existing for now
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_build_system_prompt_cached -p aethecore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/thinker/prompt_builder.rs
git commit -m "$(cat <<'EOF'
feat(prompt_builder): add cache optimization for Anthropic

- Add SystemPromptPart struct with cache flag
- Implement build_system_prompt_cached()
- Split static header (cacheable) from dynamic content
- Maximizes Anthropic prompt cache hit rate

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: Enable feature flags and cleanup

**Files:**
- Modify: `core/src/agent_loop/config.rs`
- Modify: `core/src/agent_loop/mod.rs`
- Delete: `core/src/agent_loop/session_sync.rs` (after Phase 3 complete)

**Step 1: Enable flags by default**

```rust
impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            // ... existing defaults ...
            use_unified_session: true,   // Now enabled
            use_message_builder: true,   // Now enabled
            use_realtime_overflow: true, // Now enabled
        }
    }
}
```

**Step 2: Run full test suite**

Run: `cargo test -p aethecore --lib`
Expected: All tests pass

**Step 3: Commit**

```bash
git commit -m "$(cat <<'EOF'
feat: enable unified session model by default

- Enable use_unified_session flag
- Enable use_message_builder flag
- Enable use_realtime_overflow flag
- All migration phases complete

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Verification Checklist

- [ ] All tests pass: `cargo test -p aethecore --lib`
- [ ] No new warnings: `cargo clippy -p aethecore`
- [ ] Format check: `cargo fmt -p aethecore --check`
- [ ] Build succeeds: `cargo build -p aethecore`
- [ ] Integration test: Manual test with real LLM call

---

## Rollback Plan

If issues are found after deployment:

1. Set feature flags to false in config
2. Restart application
3. System reverts to legacy LoopState behavior
4. Investigate and fix issues
5. Re-enable flags after fix

---

## References

- Design document: `docs/plans/2026-01-24-unified-session-model-design.md`
- OpenCode source: `/Users/zouguojun/Workspace/opencode`
- OpenCode key files:
  - `packages/opencode/src/session/message-v2.ts`
  - `packages/opencode/src/session/prompt.ts`
  - `packages/opencode/src/session/compaction.ts`
