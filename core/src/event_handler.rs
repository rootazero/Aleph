//! Event handler trait for callbacks from Rust to client code.
//!
//! This trait defines the callback interface that Swift/Kotlin clients
//! must implement to receive events from the Aether core. UniFFI will
//! generate a protocol/interface for each target language.

use crate::clarification::{ClarificationRequest, ClarificationResult};
use crate::dispatcher::PendingConfirmationInfo;

/// Processing states for the Aether system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingState {
    /// System is idle, not processing
    Idle,
    /// Listening for hotkey events
    Listening,
    /// Retrieving memories from database (Phase 7 - Task 7.5)
    RetrievingMemory,
    /// AI provider is processing the request (Phase 7 - Task 7.5)
    ProcessingWithAI,
    /// Processing AI request (kept for backward compatibility)
    Processing,
    /// Typewriter animation in progress (Phase 7.2)
    Typewriting,
    /// Operation completed successfully
    Success,
    /// Operation failed with error
    Error,
}

/// Error types for typed error handling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    /// Network connectivity error
    Network,
    /// Permission denied error
    Permission,
    /// API quota exceeded
    Quota,
    /// Request timeout
    Timeout,
    /// Unknown error type
    Unknown,
}

// ========================================================================
// MCP Startup Report Types (Phase 3.3 - Swift callback)
// ========================================================================

/// MCP server error information for FFI
///
/// Contains details about a server that failed to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerError {
    /// Name of the server that failed
    pub server_name: String,
    /// Human-readable error message
    pub error_message: String,
}

/// MCP startup report for FFI
///
/// Contains information about MCP server startup results,
/// sent to Swift via `on_mcp_startup_complete` callback.
#[derive(Debug, Clone, Default)]
pub struct McpStartupReport {
    /// Names of servers that started successfully
    pub succeeded_servers: Vec<String>,
    /// Servers that failed to start with error details
    pub failed_servers: Vec<McpServerError>,
}

impl McpStartupReport {
    /// Create from internal McpStartupReport
    pub fn from_internal(report: &crate::mcp::McpStartupReport) -> Self {
        Self {
            succeeded_servers: report.succeeded.clone(),
            failed_servers: report
                .failed
                .iter()
                .map(|(name, error): &(String, String)| McpServerError {
                    server_name: name.clone(),
                    error_message: error.clone(),
                })
                .collect(),
        }
    }

    /// Check if all servers started successfully
    pub fn all_succeeded(&self) -> bool {
        self.failed_servers.is_empty()
    }

    /// Get total number of servers (succeeded + failed)
    pub fn total_count(&self) -> usize {
        self.succeeded_servers.len() + self.failed_servers.len()
    }
}

/// Trait for receiving events from Aether core
///
/// Clients (Swift, Kotlin, etc.) implement this trait to receive
/// callbacks when hotkeys are detected, states change, or errors occur.
pub trait InternalEventHandler: Send + Sync {
    /// Called when the processing state changes
    fn on_state_changed(&self, state: ProcessingState);

    /// Called when an error occurs
    fn on_error(&self, message: String, suggestion: Option<String>);

    /// Called when AI response chunk arrives (for streaming display)
    fn on_response_chunk(&self, text: String);

    /// Called when typed error occurs with error type
    fn on_error_typed(&self, error_type: ErrorType, message: String);

    /// Called when progress update is available
    fn on_progress(&self, percent: f32);

    /// Called when AI processing starts with provider information (Phase 7 - Task 7.4)
    fn on_ai_processing_started(&self, provider_name: String, provider_color: String);

    /// Called when AI response is received with response preview (Phase 7 - Task 7.4)
    fn on_ai_response_received(&self, response_preview: String);

    /// Called when primary provider fails and fallback is attempted (Phase 10 - Task 10.2)
    fn on_provider_fallback(&self, from_provider: String, to_provider: String);

    /// Called when config file changes externally (Phase 6 - Task 6.1)
    fn on_config_changed(&self);

    /// Called during typewriter animation with progress percentage (0.0-1.0) (Phase 7.2)
    fn on_typewriter_progress(&self, percent: f32);

    /// Called when typewriter animation is cancelled (Escape key or new hotkey) (Phase 7.2)
    fn on_typewriter_cancelled(&self);

    /// Called when clarification is needed from the user (Phantom Flow interaction)
    ///
    /// This is a BLOCKING callback - the Rust core will wait for the Swift UI
    /// to display the clarification request and return the user's response.
    /// The callback returns when the user selects an option, enters text,
    /// cancels, or the request times out.
    fn on_clarification_needed(&self, request: ClarificationRequest) -> ClarificationResult;

    // ========================================================================
    // Multi-Turn Conversation Callbacks (add-multi-turn-conversation)
    // ========================================================================

    /// Called when a new conversation session starts.
    ///
    /// # Arguments
    /// * `session_id` - Unique identifier for the conversation session
    fn on_conversation_started(&self, session_id: String);

    /// Called when a conversation turn is completed.
    ///
    /// # Arguments
    /// * `turn` - The completed turn with user input and AI response
    fn on_conversation_turn_completed(&self, turn: crate::conversation::ConversationTurn);

    /// Called when the AI response is ready and continuation input can be shown.
    ///
    /// The UI should display the Halo input box for user to continue the conversation.
    fn on_conversation_continuation_ready(&self);

    /// Called when a conversation session ends.
    ///
    /// # Arguments
    /// * `session_id` - The ID of the ended session
    /// * `total_turns` - Total number of turns in the conversation
    fn on_conversation_ended(&self, session_id: String, total_turns: u32);

    // ========================================================================
    // Async Confirmation Callbacks (async-confirmation-flow)
    // ========================================================================

    /// Called when tool execution confirmation is needed from user.
    ///
    /// This is a NON-BLOCKING callback - Swift should show UI and call
    /// `confirm_action()` when user makes a decision. This replaces the
    /// blocking `on_clarification_needed` pattern for tool confirmations.
    ///
    /// # Arguments
    /// * `confirmation` - Pending confirmation info with tool details
    fn on_confirmation_needed(&self, confirmation: PendingConfirmationInfo);

    /// Called when a pending confirmation expires (timeout).
    ///
    /// # Arguments
    /// * `confirmation_id` - ID of the expired confirmation
    fn on_confirmation_expired(&self, confirmation_id: String);

    // ========================================================================
    // Tool Registry Callbacks (unify-tool-registry)
    // ========================================================================

    /// Called when the unified tool registry is refreshed.
    ///
    /// This fires after tools are aggregated from all sources:
    /// - Native capabilities (Search, Video)
    /// - System tools (MCP builtin services)
    /// - External MCP servers
    /// - Installed skills
    /// - Custom commands from config rules
    ///
    /// # Arguments
    /// * `tool_count` - Total number of active tools in the registry
    fn on_tools_changed(&self, tool_count: u32);

    /// Called when the tool registry needs to be refreshed.
    ///
    /// This is triggered by config hot-reload when tools configuration changes.
    /// Swift layer should call `refresh_skills()` or similar method to trigger
    /// a full tool registry refresh.
    ///
    /// Note: This is separate from `on_tools_changed` because the config watcher
    /// callback doesn't have access to AetherCore to call refresh_tool_registry()
    /// directly.
    fn on_tools_refresh_needed(&self);

    // ========================================================================
    // MCP Startup Callbacks (Phase 3.3 - mcp-startup-feedback)
    // ========================================================================

    /// Called when MCP servers finish starting.
    ///
    /// This callback provides detailed information about which MCP servers
    /// started successfully and which failed. The UI can use this to:
    /// - Show success notifications for started servers
    /// - Display error messages for failed servers
    /// - Update server status indicators
    ///
    /// This is called after each tool registry refresh that involves MCP servers,
    /// including initial startup and after adding/updating MCP server configurations.
    ///
    /// # Arguments
    /// * `report` - Startup report with succeeded/failed server information
    fn on_mcp_startup_complete(&self, report: McpStartupReport);

    // ========================================================================
    // Agent Loop Callbacks (enhance-intent-routing-pipeline)
    // ========================================================================

    /// Called when agent loop starts executing a multi-step plan.
    ///
    /// # Arguments
    /// * `plan_id` - Unique identifier for the plan
    /// * `total_steps` - Total number of steps in the plan
    /// * `description` - Human-readable description of the plan
    fn on_agent_started(&self, plan_id: String, total_steps: u32, description: String);

    /// Called when agent starts executing a tool.
    ///
    /// # Arguments
    /// * `plan_id` - Plan identifier
    /// * `step_index` - Current step index (0-based)
    /// * `tool_name` - Name of the tool being executed
    /// * `tool_description` - Human-readable description of the tool
    fn on_agent_tool_started(
        &self,
        plan_id: String,
        step_index: u32,
        tool_name: String,
        tool_description: String,
    );

    /// Called when agent tool execution completes (success or failure).
    ///
    /// # Arguments
    /// * `plan_id` - Plan identifier
    /// * `step_index` - Step index that completed
    /// * `tool_name` - Name of the tool
    /// * `success` - Whether execution succeeded
    /// * `result_preview` - Preview of the result (truncated if long)
    fn on_agent_tool_completed(
        &self,
        plan_id: String,
        step_index: u32,
        tool_name: String,
        success: bool,
        result_preview: String,
    );

    /// Called when agent loop completes (success or failure).
    ///
    /// # Arguments
    /// * `plan_id` - Plan identifier
    /// * `success` - Whether the overall plan succeeded
    /// * `total_duration_ms` - Total execution time in milliseconds
    /// * `final_response` - Final response text (may be empty on failure)
    fn on_agent_completed(
        &self,
        plan_id: String,
        success: bool,
        total_duration_ms: u64,
        final_response: String,
    );
}

/// Mock event handler for testing
///
/// Records all callback invocations for assertion in tests
#[cfg(test)]
pub struct MockEventHandler {
    pub state_changes: std::sync::Arc<std::sync::Mutex<Vec<ProcessingState>>>,
    pub errors: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub response_chunks: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub typed_errors: std::sync::Arc<std::sync::Mutex<Vec<(ErrorType, String)>>>,
    pub progress_updates: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
    pub ai_processing_started: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    pub ai_responses: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub provider_fallbacks: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    pub config_changes: std::sync::Arc<std::sync::Mutex<u32>>, // Count of config change events
    pub typewriter_progress: std::sync::Arc<std::sync::Mutex<Vec<f32>>>, // Typewriter progress updates
    pub typewriter_cancelled: std::sync::Arc<std::sync::Mutex<u32>>,     // Count of cancellations
    pub clarification_requests: std::sync::Arc<std::sync::Mutex<Vec<ClarificationRequest>>>, // Phantom Flow requests
    pub clarification_response: std::sync::Arc<std::sync::Mutex<Option<ClarificationResult>>>, // Mock response to return
    // Multi-turn conversation tracking
    pub conversation_started: std::sync::Arc<std::sync::Mutex<Vec<String>>>, // Session IDs
    pub conversation_turns:
        std::sync::Arc<std::sync::Mutex<Vec<crate::conversation::ConversationTurn>>>,
    pub conversation_continuation_ready_count: std::sync::Arc<std::sync::Mutex<u32>>,
    pub conversation_ended: std::sync::Arc<std::sync::Mutex<Vec<(String, u32)>>>, // (session_id, total_turns)
    // Async confirmation tracking
    pub confirmations_needed: std::sync::Arc<std::sync::Mutex<Vec<PendingConfirmationInfo>>>,
    pub confirmations_expired: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    // Tool registry tracking
    pub tools_changed: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
    pub tools_refresh_needed_count: std::sync::Arc<std::sync::Mutex<u32>>,
    // MCP startup tracking
    pub mcp_startup_reports: std::sync::Arc<std::sync::Mutex<Vec<McpStartupReport>>>,
    // Agent loop tracking
    pub agent_started: std::sync::Arc<std::sync::Mutex<Vec<(String, u32, String)>>>, // (plan_id, total_steps, description)
    pub agent_tool_started: std::sync::Arc<std::sync::Mutex<Vec<(String, u32, String, String)>>>, // (plan_id, step_index, tool_name, description)
    pub agent_tool_completed:
        std::sync::Arc<std::sync::Mutex<Vec<(String, u32, String, bool, String)>>>, // (plan_id, step_index, tool_name, success, result_preview)
    pub agent_completed: std::sync::Arc<std::sync::Mutex<Vec<(String, bool, u64, String)>>>, // (plan_id, success, duration_ms, response)
}

#[cfg(test)]
impl MockEventHandler {
    pub fn new() -> Self {
        Self {
            state_changes: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            errors: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            response_chunks: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            typed_errors: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            progress_updates: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            ai_processing_started: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            ai_responses: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            provider_fallbacks: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            config_changes: std::sync::Arc::new(std::sync::Mutex::new(0)),
            typewriter_progress: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            typewriter_cancelled: std::sync::Arc::new(std::sync::Mutex::new(0)),
            clarification_requests: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            clarification_response: std::sync::Arc::new(std::sync::Mutex::new(None)),
            conversation_started: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            conversation_turns: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            conversation_continuation_ready_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            conversation_ended: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            confirmations_needed: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            confirmations_expired: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            tools_changed: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            tools_refresh_needed_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            mcp_startup_reports: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            agent_started: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            agent_tool_started: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            agent_tool_completed: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            agent_completed: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn get_agent_started(&self) -> Vec<(String, u32, String)> {
        self.agent_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_agent_tool_started(&self) -> Vec<(String, u32, String, String)> {
        self.agent_tool_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_agent_tool_completed(&self) -> Vec<(String, u32, String, bool, String)> {
        self.agent_tool_completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_agent_completed(&self) -> Vec<(String, bool, u64, String)> {
        self.agent_completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_tools_changed(&self) -> Vec<u32> {
        self.tools_changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_mcp_startup_reports(&self) -> Vec<McpStartupReport> {
        self.mcp_startup_reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_state_changes(&self) -> Vec<ProcessingState> {
        self.state_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_response_chunks(&self) -> Vec<String> {
        self.response_chunks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_typed_errors(&self) -> Vec<(ErrorType, String)> {
        self.typed_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_progress_updates(&self) -> Vec<f32> {
        self.progress_updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_ai_processing_started(&self) -> Vec<(String, String)> {
        self.ai_processing_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_ai_responses(&self) -> Vec<String> {
        self.ai_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_provider_fallbacks(&self) -> Vec<(String, String)> {
        self.provider_fallbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_config_change_count(&self) -> u32 {
        *self
            .config_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_typewriter_progress(&self) -> Vec<f32> {
        self.typewriter_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_typewriter_cancelled_count(&self) -> u32 {
        *self
            .typewriter_cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_clarification_requests(&self) -> Vec<ClarificationRequest> {
        self.clarification_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Set the mock response to return when on_clarification_needed is called
    pub fn set_clarification_response(&self, response: ClarificationResult) {
        *self
            .clarification_response
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(response);
    }

    pub fn get_conversation_started(&self) -> Vec<String> {
        self.conversation_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_conversation_turns(&self) -> Vec<crate::conversation::ConversationTurn> {
        self.conversation_turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_conversation_continuation_ready_count(&self) -> u32 {
        *self
            .conversation_continuation_ready_count
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_conversation_ended(&self) -> Vec<(String, u32)> {
        self.conversation_ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(test)]
impl InternalEventHandler for MockEventHandler {
    fn on_state_changed(&self, state: ProcessingState) {
        self.state_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(state);
    }

    fn on_error(&self, message: String, _suggestion: Option<String>) {
        self.errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(message);
    }

    fn on_response_chunk(&self, text: String) {
        self.response_chunks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(text);
    }

    fn on_error_typed(&self, error_type: ErrorType, message: String) {
        self.typed_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((error_type, message));
    }

    fn on_progress(&self, percent: f32) {
        self.progress_updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(percent);
    }

    fn on_ai_processing_started(&self, provider_name: String, provider_color: String) {
        self.ai_processing_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((provider_name, provider_color));
    }

    fn on_ai_response_received(&self, response_preview: String) {
        self.ai_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(response_preview);
    }

    fn on_provider_fallback(&self, from_provider: String, to_provider: String) {
        self.provider_fallbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((from_provider, to_provider));
    }

    fn on_config_changed(&self) {
        let mut count = self
            .config_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count += 1;
    }

    fn on_typewriter_progress(&self, percent: f32) {
        self.typewriter_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(percent);
    }

    fn on_typewriter_cancelled(&self) {
        let mut count = self
            .typewriter_cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count += 1;
    }

    fn on_clarification_needed(&self, request: ClarificationRequest) -> ClarificationResult {
        // Record the request
        self.clarification_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(request);

        // Return the pre-configured mock response, or default to cancelled
        self.clarification_response
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(ClarificationResult::cancelled)
    }

    fn on_conversation_started(&self, session_id: String) {
        self.conversation_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(session_id);
    }

    fn on_conversation_turn_completed(&self, turn: crate::conversation::ConversationTurn) {
        self.conversation_turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(turn);
    }

    fn on_conversation_continuation_ready(&self) {
        let mut count = self
            .conversation_continuation_ready_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count += 1;
    }

    fn on_conversation_ended(&self, session_id: String, total_turns: u32) {
        self.conversation_ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((session_id, total_turns));
    }

    fn on_confirmation_needed(&self, confirmation: PendingConfirmationInfo) {
        self.confirmations_needed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(confirmation);
    }

    fn on_confirmation_expired(&self, confirmation_id: String) {
        self.confirmations_expired
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(confirmation_id);
    }

    fn on_tools_changed(&self, tool_count: u32) {
        self.tools_changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tool_count);
    }

    fn on_tools_refresh_needed(&self) {
        let mut count = self
            .tools_refresh_needed_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count += 1;
    }

    fn on_mcp_startup_complete(&self, report: McpStartupReport) {
        self.mcp_startup_reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(report);
    }

    fn on_agent_started(&self, plan_id: String, total_steps: u32, description: String) {
        self.agent_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((plan_id, total_steps, description));
    }

    fn on_agent_tool_started(
        &self,
        plan_id: String,
        step_index: u32,
        tool_name: String,
        tool_description: String,
    ) {
        self.agent_tool_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((plan_id, step_index, tool_name, tool_description));
    }

    fn on_agent_tool_completed(
        &self,
        plan_id: String,
        step_index: u32,
        tool_name: String,
        success: bool,
        result_preview: String,
    ) {
        self.agent_tool_completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((plan_id, step_index, tool_name, success, result_preview));
    }

    fn on_agent_completed(
        &self,
        plan_id: String,
        success: bool,
        total_duration_ms: u64,
        final_response: String,
    ) {
        self.agent_completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((plan_id, success, total_duration_ms, final_response));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_handler_state_changes() {
        let handler = MockEventHandler::new();
        handler.on_state_changed(ProcessingState::Idle);
        handler.on_state_changed(ProcessingState::Listening);

        let states = handler.get_state_changes();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0], ProcessingState::Idle);
        assert_eq!(states[1], ProcessingState::Listening);
    }

    #[test]
    fn test_mock_handler_errors() {
        let handler = MockEventHandler::new();
        handler.on_error("error 1".to_string(), None);
        handler.on_error("error 2".to_string(), Some("Try again".to_string()));

        let errors = handler.get_errors();
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0], "error 1");
        assert_eq!(errors[1], "error 2");
    }

    #[test]
    fn test_processing_state_equality() {
        assert_eq!(ProcessingState::Idle, ProcessingState::Idle);
        assert_ne!(ProcessingState::Idle, ProcessingState::Listening);
    }

    #[test]
    fn test_mock_handler_clarification() {
        use crate::clarification::{
            ClarificationOption, ClarificationRequest, ClarificationResult,
        };

        let handler = MockEventHandler::new();

        // Set up a mock response
        let mock_response = ClarificationResult::selected(1, "casual".to_string());
        handler.set_clarification_response(mock_response);

        // Create a clarification request
        let request = ClarificationRequest::select(
            "style-choice",
            "What style would you like?",
            vec![
                ClarificationOption::new("professional", "Professional"),
                ClarificationOption::new("casual", "Casual"),
            ],
        );

        // Call the handler
        let result = handler.on_clarification_needed(request);

        // Verify the request was recorded
        let requests = handler.get_clarification_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].id, "style-choice");

        // Verify the response
        assert!(result.is_success());
        assert_eq!(result.selected_index, Some(1));
        assert_eq!(result.value, Some("casual".to_string()));
    }

    #[test]
    fn test_mock_handler_clarification_default_cancelled() {
        use crate::clarification::{ClarificationRequest, ClarificationResultType};

        let handler = MockEventHandler::new();
        // Don't set a response - should default to cancelled

        let request = ClarificationRequest::text("test", "Enter name:", None);
        let result = handler.on_clarification_needed(request);

        assert_eq!(result.result_type, ClarificationResultType::Cancelled);
        assert!(!result.is_success());
    }
}
