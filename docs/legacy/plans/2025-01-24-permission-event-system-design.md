# Permission & Event System Design

> Design document for unified permission system and event bus architecture
>
> Date: 2025-01-24
> Status: Approved
> Reference: OpenCode (Claude Code open-source implementation)

## Overview

This document describes the design for a unified permission system and event bus architecture for Aleph, inspired by OpenCode's implementation but adapted to Aleph's Rust + Swift architecture.

## Goals

1. **Rule-based Permission System** - Replace confidence-based confirmation with declarative rules
2. **Structured User Interaction** - Replace simple callbacks with structured Question system
3. **Unified Event Bus** - Replace scattered callbacks with centralized event publishing
4. **FFI Stability** - Single event entry point for Swift integration

## Non-Goals

- Session share/revert functionality (Phase 2)
- Message part structure (Phase 2)

---

## Part 1: Permission System Architecture

### 1.1 Core Data Structures

```rust
/// Permission action
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    Allow,  // Auto-allow
    Deny,   // Auto-deny
    Ask,    // Requires user confirmation
}

/// Permission rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Permission type (e.g., "edit", "bash", "read")
    pub permission: String,
    /// Match pattern (e.g., "git *", "~/src/*", "*")
    pub pattern: String,
    /// Action to take
    pub action: PermissionAction,
}

pub type Ruleset = Vec<PermissionRule>;

/// Tool to permission type mapping
pub struct PermissionMapping {
    /// Edit tools → "edit" permission
    edit_tools: HashSet<String>,  // ["edit", "write", "patch", "file_write"]
}
```

### 1.2 Configuration Format

```json
{
  "permission": {
    "edit": "allow",
    "read": "allow",
    "bash": {
      "git *": "allow",
      "cargo *": "allow",
      "rm -rf *": "deny",
      "*": "ask"
    },
    "external_directory": {
      "~/Workspace/*": "allow",
      "*": "ask"
    }
  }
}
```

### 1.3 Permission Evaluator

```rust
impl PermissionEvaluator {
    /// Evaluate permission request
    ///
    /// Rule priority (later definitions win):
    /// 1. Global config (config.json)
    /// 2. Session-level overrides
    /// 3. Runtime approvals ("always" selections)
    pub fn evaluate(
        &self,
        permission: &str,
        pattern: &str,
        rulesets: &[&Ruleset],
    ) -> PermissionRule {
        let merged: Vec<_> = rulesets.iter().flat_map(|r| r.iter()).collect();

        // Find matching rule from end (later wins)
        for rule in merged.iter().rev() {
            if wildcard_match(permission, &rule.permission)
                && wildcard_match(pattern, &rule.pattern)
            {
                return rule.clone();
            }
        }

        // Default: ask
        PermissionRule {
            permission: permission.to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Ask,
        }
    }
}
```

### 1.4 Permission Request/Response

```rust
/// Permission request (sent to UI)
#[derive(Debug, Clone, Serialize)]
pub struct PermissionRequest {
    pub id: String,
    pub session_id: String,
    pub permission: String,
    pub patterns: Vec<String>,
    pub always_patterns: Vec<String>,
    pub metadata: HashMap<String, Value>,
    pub tool_call: Option<ToolCallRef>,
}

/// Permission response
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionReply {
    Once,
    Always,
    Reject,
    Correct { message: String },
}
```

### 1.5 Permission Errors

```rust
#[derive(Debug, Clone, thiserror::Error)]
pub enum PermissionError {
    #[error("User rejected permission for this tool call")]
    Rejected,

    #[error("User rejected with feedback: {message}")]
    Corrected { message: String },

    #[error("Permission denied by rule: {permission} on {pattern}")]
    Denied {
        permission: String,
        pattern: String,
        rule: PermissionRule,
    },
}
```

---

## Part 2: Question System

### 2.1 Data Structures

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionInfo {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default = "default_true")]
    pub custom: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuestionRequest {
    pub id: String,
    pub session_id: String,
    pub questions: Vec<QuestionInfo>,
    pub tool_call: Option<ToolCallRef>,
}

pub type Answer = Vec<String>;

#[derive(Debug, Clone, Deserialize)]
pub struct QuestionReply {
    pub answers: Vec<Answer>,
}
```

### 2.2 Comparison with Current Callback

| Current `LoopCallback` | New `QuestionManager` |
|------------------------|----------------------|
| `on_user_input_required(&str, Option<&[String]>) -> String` | `ask(QuestionRequest) -> Vec<Answer>` |
| Single question, simple options | Batch questions, structured options |
| Sync callback | Async Event + Reply |
| No timeout | Supports timeout |
| No multi-select | Multi-select + custom input |

---

## Part 3: Event Bus Architecture

### 3.1 Event Definitions (Nested by Category)

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "category", content = "payload")]
pub enum Event {
    Permission(PermissionEvent),
    Question(QuestionEvent),
    Loop(LoopEvent),
    Tool(ToolEvent),
    Session(SessionEvent),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum PermissionEvent {
    Asked(PermissionRequest),
    Replied { session_id: String, request_id: String, reply: PermissionReply },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum QuestionEvent {
    Asked(QuestionRequest),
    Replied { session_id: String, request_id: String, answers: Vec<Answer> },
    Rejected { session_id: String, request_id: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum LoopEvent {
    Started { session_id: String, request: String },
    StepStarted { session_id: String, step: usize },
    ThinkingStarted { session_id: String, step: usize },
    ThinkingStream { session_id: String, content: String },
    ThinkingDone { session_id: String, decision_type: String },
    ActionStarted { session_id: String, action_type: String },
    ActionDone { session_id: String, action_type: String, success: bool },
    GuardTriggered { session_id: String, violation: String },
    RetryScheduled { session_id: String, attempt: u32, delay_ms: u64, error: String },
    Completed { session_id: String, summary: String },
    Failed { session_id: String, reason: String },
    Aborted { session_id: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ToolEvent {
    Started { session_id: String, tool_name: String, call_id: String },
    Progress { session_id: String, call_id: String, progress: f32, message: String },
    Completed { session_id: String, call_id: String, success: bool, duration_ms: u64 },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum SessionEvent {
    Created { session_id: String },
    Updated { session_id: String },
    Compacting { session_id: String },
    Compacted { session_id: String },
    Archived { session_id: String },
}
```

### 3.2 Event Bus Implementation

```rust
pub struct EventBus {
    broadcast_tx: broadcast::Sender<Event>,
    ffi_tx: mpsc::Sender<Event>,
}

impl EventBus {
    pub async fn publish(&self, event: Event) {
        let _ = self.broadcast_tx.send(event.clone());
        let _ = self.ffi_tx.send(event).await;
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.broadcast_tx.subscribe()
    }
}
```

### 3.3 FFI Bridge

```rust
#[uniffi::export(callback_interface)]
pub trait EventCallback: Send + Sync {
    fn on_event(&self, event_json: String);
}

#[uniffi::export]
pub fn reply_permission(request_id: String, reply_json: String) -> Result<(), String>;

#[uniffi::export]
pub fn reply_question(request_id: String, reply_json: String) -> Result<(), String>;
```

---

## Part 4: Module Structure

```
core/src/
├── permission/
│   ├── mod.rs
│   ├── action.rs
│   ├── rule.rs
│   ├── config.rs
│   ├── evaluator.rs
│   ├── manager.rs
│   └── error.rs
│
├── question/
│   ├── mod.rs
│   ├── types.rs
│   ├── manager.rs
│   └── error.rs
│
├── events/
│   ├── mod.rs
│   ├── event.rs
│   ├── permission.rs
│   ├── question.rs
│   ├── loop_event.rs
│   ├── tool.rs
│   ├── session.rs
│   └── bus.rs
│
├── agent_loop/
│   ├── callback.rs          # deprecated
│   ├── event_adapter.rs     # new
│   └── ...
│
└── ffi/
    ├── event_bridge.rs      # new
    └── ...
```

---

## Part 5: Migration Strategy

### Phase 1: Parallel Operation (Compatibility)

```
AgentLoop
   │
   ├──→ LoopCallback (existing)
   │        │
   │        └──→ CallbackToEventAdapter ──→ EventBus
   │
   └──→ New code uses PermissionManager / QuestionManager directly
```

### Phase 2: Gradual Replacement

- New features use Event pattern directly
- Old code runs through adapter
- Swift supports both patterns

### Phase 3: Complete Migration

- Remove LoopCallback trait
- Remove adapters
- All code uses Event pattern

### Files to Delete/Replace

| Old Code | New Code | Notes |
|----------|----------|-------|
| `dispatcher/confirmation.rs` | `permission/manager.rs` | Confidence → Rule-based |
| `dispatcher/async_confirmation.rs` | `permission/manager.rs` | Merge into unified manager |
| `LoopCallback` confirmation methods | `PermissionManager::ask()` | Structured requests |
| `LoopCallback::on_user_input_required` | `QuestionManager::ask()` | Structured Q&A |
| `clarification.rs` | Delete | Covered by Permission + Question |

---

## Part 6: Implementation Plan

### Phase A: Infrastructure (Week 1)
1. `events/` module - Event enum + EventBus
2. `permission/` module - Core data structures + evaluator
3. `question/` module - Core data structures

### Phase B: Managers (Week 2)
4. PermissionManager - Full request/response flow
5. QuestionManager - Full request/response flow
6. Config parsing - permission config support

### Phase C: Integration (Week 3)
7. CallbackToEventAdapter - Compatibility layer
8. FFI Bridge - UniFFI event interface
9. Swift EventHandler - Event dispatch

### Phase D: Cleanup (Week 4)
10. Delete `dispatcher/confirmation.rs`
11. Delete `dispatcher/async_confirmation.rs`
12. Delete `clarification.rs`
13. Mark LoopCallback deprecated

---

## Summary: Key Improvements

| Dimension | Before | After |
|-----------|--------|-------|
| **Permission Trigger** | Confidence < 0.7 | Rule matching (permission + pattern → action) |
| **Permission Memory** | None | "Always" option persists rules |
| **User Interaction** | Simple string return | Structured Q&A (multi-select, custom, batch) |
| **Event Communication** | Scattered callbacks (15+ methods) | Unified EventBus (single entry) |
| **FFI Boundary** | Multiple callback methods | `on_event(json)` + `reply_xxx()` |
| **Error Types** | `true/false` | `Rejected/Corrected/Denied` semantics |
| **Extensibility** | Change trait signature | Add Event variant |

---

## Appendix: OpenCode Alignment

| OpenCode Feature | Aleph Implementation |
|------------------|----------------------|
| `PermissionNext.ask()` | `PermissionManager::ask()` |
| `PermissionNext.reply()` | `PermissionManager::reply()` |
| `Question.ask()` | `QuestionManager::ask()` |
| `Bus.publish(Event.Asked, ...)` | `EventBus::publish(Event::Permission(...))` |
| `Ruleset` config | `permission` config section |
| `EDIT_TOOLS` mapping | `PermissionMapping.edit_tools` |
| `RejectedError/CorrectedError/DeniedError` | `PermissionError` enum |
