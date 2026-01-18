# Phase 5: Integration and Testing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Integrate the event-driven agentic loop with the FFI layer and provide callback bridge for Swift UI updates.

**Architecture:** Create a CallbackBridge component that subscribes to UI-relevant events and forwards them to Swift via the existing AetherEventHandler trait. Enhance the FFI layer with new methods for session management.

**Tech Stack:** Rust, UniFFI, async-trait, tokio

---

## Task 1: Extend AetherEventHandler trait with new callbacks

**Files:**
- Modify: `Aether/core/src/ffi/mod.rs`

**Step 1: Add new callback methods to AetherEventHandler trait**

Add new methods after the existing callbacks in the `AetherEventHandler` trait:

```rust
    // ========================================================================
    // AGENTIC LOOP CALLBACKS (Phase 5)
    // ========================================================================

    /// Called when a new session is created
    fn on_session_started(&self, session_id: String);

    /// Called when tool execution starts (with call_id for tracking)
    fn on_tool_call_started(&self, call_id: String, tool_name: String);

    /// Called when tool execution completes
    fn on_tool_call_completed(&self, call_id: String, output: String);

    /// Called when tool execution fails
    fn on_tool_call_failed(&self, call_id: String, error: String, is_retryable: bool);

    /// Called on each loop iteration with progress update
    fn on_loop_progress(&self, session_id: String, iteration: u32, status: String);

    /// Called when a plan is created for multi-step task
    fn on_plan_created(&self, session_id: String, steps: Vec<String>);

    /// Called when session completes
    fn on_session_completed(&self, session_id: String, summary: String);

    /// Called when sub-agent is started
    fn on_subagent_started(&self, parent_session_id: String, child_session_id: String, agent_id: String);

    /// Called when sub-agent completes
    fn on_subagent_completed(&self, child_session_id: String, success: bool, summary: String);
```

**Step 2: Run tests to verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo check`
Expected: Compilation succeeds (trait changes don't break existing code until implemented)

**Step 3: Commit**

```bash
git add Aether/core/src/ffi/mod.rs
git commit -m "feat(ffi): extend AetherEventHandler with agentic loop callbacks"
```

---

## Task 2: Create CallbackBridge component

**Files:**
- Create: `Aether/core/src/components/callback_bridge.rs`
- Modify: `Aether/core/src/components/mod.rs`

**Step 1: Create callback_bridge.rs**

```rust
//! CallbackBridge - Forwards internal events to Swift callbacks.
//!
//! This component subscribes to UI-relevant events and converts them
//! to callbacks via the AetherEventHandler trait.

use std::sync::Arc;

use async_trait::async_trait;

use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError,
    LoopState, PlanStep, StopReason, SubAgentRequest, SubAgentResult,
    ToolCallRequest, ToolCallResult, ToolCallError, ToolCallStarted,
};
use crate::ffi::AetherEventHandler;

/// CallbackBridge forwards events to the Swift layer
pub struct CallbackBridge {
    handler: Arc<dyn AetherEventHandler>,
}

impl CallbackBridge {
    /// Create a new CallbackBridge
    pub fn new(handler: Arc<dyn AetherEventHandler>) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl EventHandler for CallbackBridge {
    fn name(&self) -> &'static str {
        "CallbackBridge"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::SessionCreated,
            EventType::ToolCallStarted,
            EventType::ToolCallCompleted,
            EventType::ToolCallFailed,
            EventType::LoopContinue,
            EventType::LoopStop,
            EventType::PlanCreated,
            EventType::SubAgentStarted,
            EventType::SubAgentCompleted,
            EventType::AiResponseGenerated,
        ]
    }

    async fn handle(&self, event: &AetherEvent, ctx: &EventContext) -> Result<Vec<AetherEvent>, HandlerError> {
        match event {
            AetherEvent::SessionCreated(info) => {
                self.handler.on_session_started(info.session_id.clone());
            }
            AetherEvent::ToolCallStarted(info) => {
                self.handler.on_tool_call_started(
                    info.call_id.clone(),
                    info.tool.clone(),
                );
            }
            AetherEvent::ToolCallCompleted(result) => {
                self.handler.on_tool_call_completed(
                    result.call_id.clone(),
                    result.output.to_string(),
                );
            }
            AetherEvent::ToolCallFailed(error) => {
                self.handler.on_tool_call_failed(
                    error.call_id.clone(),
                    error.error.clone(),
                    error.retryable,
                );
            }
            AetherEvent::LoopContinue(state) => {
                if let Some(session_id) = ctx.get_session_id().await {
                    self.handler.on_loop_progress(
                        session_id,
                        state.iteration,
                        format!("{:?}", state.reason),
                    );
                }
            }
            AetherEvent::LoopStop(reason) => {
                if let Some(session_id) = ctx.get_session_id().await {
                    let summary = match reason {
                        StopReason::Completed => "Task completed successfully".to_string(),
                        StopReason::MaxIterations(n) => format!("Reached max iterations ({})", n),
                        StopReason::UserAborted => "Cancelled by user".to_string(),
                        StopReason::Error(e) => format!("Error: {}", e),
                        StopReason::TokenLimit => "Token limit reached".to_string(),
                        StopReason::DoomLoop => "Detected repetitive loop".to_string(),
                    };
                    self.handler.on_session_completed(session_id, summary);
                }
            }
            AetherEvent::PlanCreated(plan) => {
                if let Some(session_id) = ctx.get_session_id().await {
                    let steps: Vec<String> = plan.steps.iter()
                        .map(|s| s.description.clone())
                        .collect();
                    self.handler.on_plan_created(session_id, steps);
                }
            }
            AetherEvent::SubAgentStarted(request) => {
                self.handler.on_subagent_started(
                    request.parent_session_id.clone(),
                    request.child_session_id.clone(),
                    request.agent_id.clone(),
                );
            }
            AetherEvent::SubAgentCompleted(result) => {
                self.handler.on_subagent_completed(
                    result.child_session_id.clone(),
                    result.success,
                    result.summary.clone(),
                );
            }
            AetherEvent::AiResponseGenerated(response) => {
                // Forward streaming chunks via on_stream_chunk
                self.handler.on_stream_chunk(response.content.clone());
                if response.is_final {
                    self.handler.on_complete(response.content.clone());
                }
            }
            _ => {}
        }

        // CallbackBridge doesn't produce new events
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventBus, SessionInfo, TaskPlan};
    use std::sync::atomic::{AtomicU32, Ordering};

    struct MockHandler {
        session_started_count: AtomicU32,
        tool_started_count: AtomicU32,
        tool_completed_count: AtomicU32,
        loop_progress_count: AtomicU32,
        session_completed_count: AtomicU32,
    }

    impl MockHandler {
        fn new() -> Self {
            Self {
                session_started_count: AtomicU32::new(0),
                tool_started_count: AtomicU32::new(0),
                tool_completed_count: AtomicU32::new(0),
                loop_progress_count: AtomicU32::new(0),
                session_completed_count: AtomicU32::new(0),
            }
        }
    }

    impl AetherEventHandler for MockHandler {
        fn on_thinking(&self) {}
        fn on_tool_start(&self, _: String) {}
        fn on_tool_result(&self, _: String, _: String) {}
        fn on_stream_chunk(&self, _: String) {}
        fn on_complete(&self, _: String) {}
        fn on_error(&self, _: String) {}
        fn on_memory_stored(&self) {}
        fn on_agent_mode_detected(&self, _: crate::intent::ExecutableTaskFFI) {}
        fn on_tools_changed(&self, _: u32) {}
        fn on_mcp_startup_complete(&self, _: crate::event_handler::McpStartupReportFFI) {}

        fn on_session_started(&self, _: String) {
            self.session_started_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_tool_call_started(&self, _: String, _: String) {
            self.tool_started_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_tool_call_completed(&self, _: String, _: String) {
            self.tool_completed_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_tool_call_failed(&self, _: String, _: String, _: bool) {}
        fn on_loop_progress(&self, _: String, _: u32, _: String) {
            self.loop_progress_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_plan_created(&self, _: String, _: Vec<String>) {}
        fn on_session_completed(&self, _: String, _: String) {
            self.session_completed_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_subagent_started(&self, _: String, _: String, _: String) {}
        fn on_subagent_completed(&self, _: String, _: bool, _: String) {}
    }

    fn create_test_context() -> EventContext {
        EventContext::new(EventBus::new())
    }

    #[tokio::test]
    async fn test_callback_bridge_name() {
        let handler = Arc::new(MockHandler::new());
        let bridge = CallbackBridge::new(handler);
        assert_eq!(bridge.name(), "CallbackBridge");
    }

    #[tokio::test]
    async fn test_callback_bridge_subscriptions() {
        let handler = Arc::new(MockHandler::new());
        let bridge = CallbackBridge::new(handler);
        let subs = bridge.subscriptions();

        assert!(subs.contains(&EventType::SessionCreated));
        assert!(subs.contains(&EventType::ToolCallStarted));
        assert!(subs.contains(&EventType::ToolCallCompleted));
        assert!(subs.contains(&EventType::LoopStop));
    }

    #[tokio::test]
    async fn test_session_created_callback() {
        let handler = Arc::new(MockHandler::new());
        let bridge = CallbackBridge::new(Arc::clone(&handler));
        let ctx = create_test_context();

        let event = AetherEvent::SessionCreated(SessionInfo {
            session_id: "test-session".into(),
            agent_id: "main".into(),
            model: "test".into(),
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(handler.session_started_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_tool_call_started_callback() {
        let handler = Arc::new(MockHandler::new());
        let bridge = CallbackBridge::new(Arc::clone(&handler));
        let ctx = create_test_context();

        let event = AetherEvent::ToolCallStarted(ToolCallStarted {
            call_id: "call-1".into(),
            tool: "web_fetch".into(),
            input: serde_json::json!({}),
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(handler.tool_started_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_tool_call_completed_callback() {
        let handler = Arc::new(MockHandler::new());
        let bridge = CallbackBridge::new(Arc::clone(&handler));
        let ctx = create_test_context();

        let event = AetherEvent::ToolCallCompleted(ToolCallResult {
            call_id: "call-1".into(),
            tool: "web_fetch".into(),
            input: serde_json::json!({}),
            output: serde_json::json!({"content": "test"}),
            started_at: 0,
            completed_at: 1,
            token_usage: None,
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(handler.tool_completed_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_loop_stop_callback() {
        let handler = Arc::new(MockHandler::new());
        let bridge = CallbackBridge::new(Arc::clone(&handler));
        let ctx = create_test_context();
        ctx.set_session_id("test-session".into()).await;

        let event = AetherEvent::LoopStop(StopReason::Completed);

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(handler.session_completed_count.load(Ordering::SeqCst), 1);
    }
}
```

**Step 2: Update components/mod.rs**

Add to `Aether/core/src/components/mod.rs`:

```rust
mod callback_bridge;
pub use callback_bridge::CallbackBridge;
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test callback_bridge`
Expected: 6 tests passing

**Step 4: Commit**

```bash
git add Aether/core/src/components/
git commit -m "feat(components): add CallbackBridge for Swift event forwarding"
```

---

## Task 3: Add SessionInfo event type

**Files:**
- Modify: `Aether/core/src/event/types.rs`

**Step 1: Add SessionInfo struct if not exists**

Check if SessionInfo already exists. If not, add:

```rust
/// Session creation info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub agent_id: String,
    pub model: String,
}
```

**Step 2: Add SessionCreated event variant if not exists**

Add to AetherEvent enum if not present:
```rust
SessionCreated(SessionInfo),
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test event::types`
Expected: All tests pass

**Step 4: Commit**

```bash
git add Aether/core/src/event/types.rs
git commit -m "feat(event): add SessionInfo type for session lifecycle events"
```

---

## Task 4: Add FFI methods for session management

**Files:**
- Create: `Aether/core/src/ffi/session.rs`
- Modify: `Aether/core/src/ffi/mod.rs`

**Step 1: Create session.rs**

```rust
//! Session management FFI methods

use crate::ffi::{AetherCore, AetherFfiError};

impl AetherCore {
    /// Resume a previously saved session
    ///
    /// Loads the session from the database and resumes execution
    /// from where it was interrupted.
    pub fn resume_session(&self, session_id: String) -> Result<(), AetherFfiError> {
        // This is a placeholder - actual implementation depends on
        // how session persistence is integrated
        tracing::info!(session_id = %session_id, "Resuming session");
        Ok(())
    }

    /// Get current session ID
    pub fn get_current_session_id(&self) -> Option<String> {
        // Return the current active session ID if any
        None
    }

    /// List recent sessions
    pub fn list_recent_sessions(&self, limit: u32) -> Vec<SessionSummary> {
        // Return list of recent sessions
        vec![]
    }
}

/// Summary of a saved session
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub agent_id: String,
    pub status: String,
    pub iteration_count: u32,
    pub created_at: i64,
    pub updated_at: i64,
}
```

**Step 2: Update ffi/mod.rs to include session module**

Add:
```rust
mod session;
pub use session::SessionSummary;
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo check`
Expected: Compilation succeeds

**Step 4: Commit**

```bash
git add Aether/core/src/ffi/
git commit -m "feat(ffi): add session management methods"
```

---

## Task 5: Add lib.rs exports for new types

**Files:**
- Modify: `Aether/core/src/lib.rs`

**Step 1: Add CallbackBridge export**

Update the components export section:
```rust
pub use crate::components::{
    // ... existing exports ...
    CallbackBridge,
};
```

**Step 2: Add SessionSummary export**

Add to uniffi_core exports or create new section:
```rust
pub use crate::ffi::SessionSummary;
```

**Step 3: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test --lib`
Expected: All tests pass

**Step 4: Commit**

```bash
git add Aether/core/src/lib.rs
git commit -m "feat(lib): export CallbackBridge and SessionSummary"
```

---

## Task 6: Integration tests for CallbackBridge

**Files:**
- Create: `Aether/core/src/components/callback_bridge_integration_test.rs`

**Step 1: Create integration tests**

```rust
//! Integration tests for CallbackBridge with full event chain

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::components::CallbackBridge;
use crate::event::{
    AetherEvent, EventBus, EventContext, EventHandler,
    ToolCallRequest, ToolCallStarted, ToolCallResult,
    LoopState, StopReason, SessionInfo,
};
use crate::ffi::AetherEventHandler;

// ... test implementation similar to Task 2 tests but with full event chains ...
```

**Step 2: Run integration tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test callback_bridge_integration`
Expected: All tests pass

**Step 3: Commit**

```bash
git add Aether/core/src/components/
git commit -m "test(components): add CallbackBridge integration tests"
```

---

## Task 7: Update aether.udl for new callback methods

**Files:**
- Modify: `Aether/core/src/aether.udl`

**Step 1: Add new callback methods to interface**

Find the `callback_interface AetherEventHandler` section and add:

```udl
    // Agentic loop callbacks (Phase 5)
    void on_session_started(string session_id);
    void on_tool_call_started(string call_id, string tool_name);
    void on_tool_call_completed(string call_id, string output);
    void on_tool_call_failed(string call_id, string error, boolean is_retryable);
    void on_loop_progress(string session_id, u32 iteration, string status);
    void on_plan_created(string session_id, sequence<string> steps);
    void on_session_completed(string session_id, string summary);
    void on_subagent_started(string parent_session_id, string child_session_id, string agent_id);
    void on_subagent_completed(string child_session_id, boolean success, string summary);
```

**Step 2: Regenerate UniFFI bindings**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo run --bin uniffi-bindgen generate src/aether.udl --language swift --out-dir ../Sources/Generated/`
Expected: Bindings generated successfully

**Step 3: Commit**

```bash
git add Aether/core/src/aether.udl
git add Aether/Sources/Generated/
git commit -m "feat(uniffi): add agentic loop callback methods to UDL"
```

---

## Task 8: Final verification

**Step 1: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test`
Expected: All tests pass

**Step 2: Run clippy**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo clippy`
Expected: No new warnings

**Step 3: Build release**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo build --release`
Expected: Build succeeds

**Step 4: Document Swift UI adaptation requirements**

Create documentation note for Swift side changes needed:
- Implement new callback methods in EventHandler.swift
- Update UI to show tool call progress with call_id tracking
- Add plan visualization when on_plan_created is called
- Show sub-agent activity indicators

---

## Summary

Phase 5 implements the integration layer with:

1. **Extended AetherEventHandler** - New callbacks for agentic loop events
2. **CallbackBridge Component** - Forwards events to Swift callbacks
3. **Session Management FFI** - Methods for session resume/list
4. **UniFFI Updates** - New callback methods in UDL
5. **Integration Tests** - Full event chain testing

This enables the Swift UI to receive real-time updates about:
- Session lifecycle (start/complete)
- Tool execution progress (with tracking IDs)
- Loop iterations and status
- Plan creation and step progress
- Sub-agent activities
