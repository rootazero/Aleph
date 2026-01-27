//! Agent Loop FFI Adapter
//!
//! This module provides the bridge between the new AgentLoop architecture
//! and the existing FFI event handler interface.
//!
//! # Architecture
//!
//! ```text
//! Swift UI ← AetherEventHandler ← FfiLoopCallback ← AgentLoop
//! ```
//!
//! The `FfiLoopCallback` translates AgentLoop events into FFI callbacks
//! that the Swift UI layer can understand and display.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::agent_loop::{
    callback::LoopCallback, guards::GuardViolation, Action, ActionResult, LoopState, Thinking,
};
use crate::components::{PartUpdateData, SessionPart, ToolCallPart, ToolCallStatus};
use crate::ffi::{AetherEventHandler, PartUpdateEventFFI};

/// Safely truncate a string at character boundaries (UTF-8 safe)
///
/// Unlike byte-based truncation `&s[..n]` which panics on multi-byte chars,
/// this function truncates at character count, ensuring valid UTF-8 output.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}

/// Format tool action into a human-readable description
///
/// Returns a tuple of (action_description, action_verb) for display.
/// - action_description: "正在创建目录: output"
/// - action_verb: "创建目录" (used for completion message)
fn format_tool_description(tool_name: &str, arguments: &Value) -> (String, String) {
    let obj = arguments.as_object();

    match tool_name {
        "file_ops" => {
            let operation = obj
                .and_then(|o| o.get("operation"))
                .and_then(|v| v.as_str())
                .unwrap_or("操作");
            let path = obj
                .and_then(|o| o.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path_display = truncate_path(path, 50);

            match operation {
                "mkdir" => (
                    format!("正在创建目录: {}", path_display),
                    "创建目录".to_string(),
                ),
                "write" => (
                    format!("正在写入文件: {}", path_display),
                    "写入文件".to_string(),
                ),
                "read" => (
                    format!("正在读取文件: {}", path_display),
                    "读取文件".to_string(),
                ),
                "delete" => (
                    format!("正在删除: {}", path_display),
                    "删除".to_string(),
                ),
                "move" => {
                    let dest = obj
                        .and_then(|o| o.get("destination"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    (
                        format!("正在移动: {} → {}", path_display, truncate_path(dest, 30)),
                        "移动文件".to_string(),
                    )
                }
                "copy" => {
                    let dest = obj
                        .and_then(|o| o.get("destination"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    (
                        format!("正在复制: {} → {}", path_display, truncate_path(dest, 30)),
                        "复制文件".to_string(),
                    )
                }
                "list" => (
                    format!("正在列出目录: {}", path_display),
                    "列出目录".to_string(),
                ),
                "search" => {
                    let pattern = obj
                        .and_then(|o| o.get("pattern"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("*");
                    (
                        format!("正在搜索文件: {} ({})", pattern, path_display),
                        "搜索文件".to_string(),
                    )
                }
                "organize" => (
                    format!("正在整理目录: {}", path_display),
                    "整理目录".to_string(),
                ),
                "batch_move" => (
                    format!("正在批量移动文件: {}", path_display),
                    "批量移动".to_string(),
                ),
                _ => (
                    format!("正在执行文件操作: {}", path_display),
                    "文件操作".to_string(),
                ),
            }
        }
        "search" => {
            let query = obj
                .and_then(|o| o.get("query"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let query_display = truncate_str(query, 25);
            (
                format!("正在搜索: {}", query_display),
                "搜索".to_string(),
            )
        }
        "web_fetch" => {
            let url = obj
                .and_then(|o| o.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let url_display = truncate_str(url, 50);
            (
                format!("正在获取网页: {}", url_display),
                "获取网页".to_string(),
            )
        }
        "youtube" => {
            let url = obj
                .and_then(|o| o.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            (
                format!("正在获取视频信息: {}", truncate_path(url, 50)),
                "获取视频".to_string(),
            )
        }
        "generate_image" => {
            let prompt = obj
                .and_then(|o| o.get("prompt"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let prompt_display = truncate_str(prompt, 20);
            (
                format!("正在生成图像: {}", prompt_display),
                "生成图像".to_string(),
            )
        }
        "generate_video" => {
            let prompt = obj
                .and_then(|o| o.get("prompt"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let prompt_display = truncate_str(prompt, 20);
            (
                format!("正在生成视频: {}", prompt_display),
                "生成视频".to_string(),
            )
        }
        "generate_audio" => {
            let prompt = obj
                .and_then(|o| o.get("prompt"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let prompt_display = truncate_str(prompt, 20);
            (
                format!("正在生成音频: {}", prompt_display),
                "生成音频".to_string(),
            )
        }
        "pdf_generate" => (
            "正在生成 PDF 文档".to_string(),
            "生成PDF".to_string(),
        ),
        _ => {
            // Generic fallback for unknown tools
            (
                format!("正在执行: {}", tool_name),
                tool_name.to_string(),
            )
        }
    }
}

/// Truncate a path for display, preserving the filename (UTF-8 safe)
fn truncate_path(path: &str, max_chars: usize) -> String {
    let char_count = path.chars().count();
    if char_count <= max_chars {
        return path.to_string();
    }

    // Try to preserve the filename
    if let Some(pos) = path.rfind('/') {
        let filename = &path[pos + 1..];
        let filename_chars = filename.chars().count();
        if filename_chars < max_chars.saturating_sub(3) {
            let available = max_chars.saturating_sub(filename_chars).saturating_sub(4);
            if available > 0 {
                // Find byte position for character-safe slicing
                let path_chars: Vec<(usize, char)> = path.char_indices().collect();
                if let Some(start_idx) = path_chars.iter().position(|(i, _)| *i == pos) {
                    if start_idx >= available {
                        let start_byte = path_chars[start_idx - available].0;
                        return format!("...{}", &path[start_byte..]);
                    }
                }
            }
        }
    }

    // Simple truncation at character boundary
    truncate_str(path, max_chars.saturating_sub(3))
}

/// FFI-compatible callback adapter for AgentLoop
///
/// This adapter translates AgentLoop callback events into
/// AetherEventHandler calls that the UI layer understands.
///
/// # Streaming Display Strategy
///
/// The adapter separates streaming content into two parts:
/// - **Status**: Temporary progress info (tool calls, thinking) - gets replaced each step
/// - **Response**: Actual content (completion summary) - accumulates
///
/// This ensures the UI shows current activity without cluttering with historical steps.
///
/// # Part Events
///
/// The adapter also publishes Part events for message flow rendering:
/// - Tool calls are published as ToolCallPart (Added → Updated on completion)
/// - AI responses stream as PartUpdated with delta content
pub struct FfiLoopCallback {
    /// The underlying FFI event handler
    handler: Arc<dyn AetherEventHandler>,
    /// Accumulated response text (actual content, persists)
    response_buffer: RwLock<String>,
    /// Current status text (temporary, replaced each step)
    status_buffer: RwLock<String>,
    /// Whether streaming has started
    streaming_started: RwLock<bool>,
    /// Whether to skip calling on_complete during finalize_response
    /// Set to true when the caller will manually call on_complete with additional data
    /// (e.g., to append [GENERATED_FILES] block)
    skip_on_complete_on_finalize: bool,
    /// Current session ID for Part events
    session_id: RwLock<String>,
    /// Active tool call parts (part_id -> ToolCallPart)
    active_tool_calls: RwLock<std::collections::HashMap<String, ToolCallPart>>,
}

impl FfiLoopCallback {
    /// Create a new FFI callback adapter
    pub fn new(handler: Arc<dyn AetherEventHandler>) -> Self {
        Self {
            handler,
            response_buffer: RwLock::new(String::new()),
            status_buffer: RwLock::new(String::new()),
            streaming_started: RwLock::new(false),
            skip_on_complete_on_finalize: false,
            session_id: RwLock::new(String::new()),
            active_tool_calls: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Create a new FFI callback adapter with manual completion control
    ///
    /// When `skip_on_complete_on_finalize` is true, the adapter will NOT call
    /// `on_complete` during `finalize_response()`. This is useful when the caller
    /// needs to append additional data (e.g., [GENERATED_FILES] block) before
    /// signaling completion.
    pub fn new_with_manual_completion(handler: Arc<dyn AetherEventHandler>) -> Self {
        Self {
            handler,
            response_buffer: RwLock::new(String::new()),
            status_buffer: RwLock::new(String::new()),
            streaming_started: RwLock::new(false),
            skip_on_complete_on_finalize: true,
            session_id: RwLock::new(String::new()),
            active_tool_calls: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Publish a Part event to the FFI handler
    async fn publish_part_event(&self, data: PartUpdateData) {
        use tracing::warn;

        // Log Part event publication with session_id check
        if data.session_id.is_empty() {
            warn!(
                part_type = %data.part_type,
                event_type = ?data.event_type,
                "Publishing Part event with EMPTY session_id - this may indicate session not started"
            );
        } else {
            debug!(
                session_id = %data.session_id,
                part_id = %data.part_id,
                part_type = %data.part_type,
                event_type = ?data.event_type,
                "Publishing Part event to FFI handler"
            );
        }

        let ffi_event = PartUpdateEventFFI::from(&data);
        self.handler.on_part_update(ffi_event);
    }

    /// Get the accumulated response
    pub async fn get_response(&self) -> String {
        self.response_buffer.read().await.clone()
    }

    /// Set status text (replaces previous status, temporary display)
    ///
    /// Status messages are transient progress indicators (tool calls, thinking).
    /// They replace each other and are NOT accumulated with previous responses.
    /// This prevents historical step clutter in the streaming display.
    async fn set_status(&self, text: &str) {
        let mut started = self.streaming_started.write().await;
        if !*started {
            *started = true;
        }
        drop(started);

        let mut status = self.status_buffer.write().await;
        *status = text.to_string();

        // Stream only the current status (replaces previous)
        // Do NOT combine with response_buffer to avoid accumulation
        self.handler.on_stream_chunk(status.clone());
    }

    /// Append text to response buffer (actual content, persists)
    async fn append_response(&self, text: &str) {
        let mut started = self.streaming_started.write().await;
        if !*started {
            *started = true;
        }
        drop(started);

        // Clear status when appending real content
        {
            let mut status = self.status_buffer.write().await;
            status.clear();
        }

        let mut buffer = self.response_buffer.write().await;
        buffer.push_str(text);

        // Stream the response content
        self.handler.on_stream_chunk(buffer.clone());
    }

    /// Finalize the response
    async fn finalize_response(&self) {
        let buffer = self.response_buffer.read().await;
        let started = self.streaming_started.read().await;

        if *started && !self.skip_on_complete_on_finalize {
            // Use on_complete to signal completion
            // When skip_on_complete_on_finalize is true, caller will handle completion manually
            self.handler.on_complete(buffer.clone());
        }
    }
}

#[async_trait]
impl LoopCallback for FfiLoopCallback {
    async fn on_loop_start(&self, state: &LoopState) {
        debug!(
            session_id = %state.session_id,
            request = %state.original_request,
            "AgentLoop started"
        );

        // Store session ID for Part events
        {
            let mut session_id = self.session_id.write().await;
            *session_id = state.session_id.clone();
        }

        self.handler.on_thinking();
    }

    async fn on_step_start(&self, step: usize) {
        info!(step = step, "AgentLoop step started");
        // Don't show step headers - each step's status will replace the previous one
        // The UI will show current activity without historical step clutter
    }

    async fn on_thinking_start(&self, step: usize) {
        debug!(step = step, "Thinking started");
        // UI shows thinking indicator (already set by on_loop_start)
    }

    async fn on_thinking_done(&self, thinking: &Thinking) {
        debug!(
            decision_type = thinking.decision.decision_type(),
            "Thinking completed"
        );

        // If there's reasoning, we could optionally stream it
        if let Some(ref reasoning) = thinking.reasoning {
            // For debugging, log reasoning
            debug!(reasoning = %reasoning, "LLM reasoning");
        }
    }

    async fn on_thinking_stream(&self, content: &str) {
        // Stream thinking content to UI as status (replaces previous status)
        if !content.is_empty() {
            let formatted = format!("💭 {}", content);
            self.set_status(&formatted).await;
            debug!(content_len = content.len(), "Thinking stream chunk sent to UI");

            // Publish streaming text delta as Part event
            let session_id = self.session_id.read().await.clone();
            if !session_id.is_empty() {
                let data = PartUpdateData::text_delta(
                    &session_id,
                    "thinking_stream",
                    "reasoning",
                    content,
                );
                self.publish_part_event(data).await;
            }
        }
    }

    async fn on_action_start(&self, action: &Action) {
        info!(action_type = %action.action_type(), "Action started");

        match action {
            Action::ToolCall { tool_name, arguments } => {
                info!(tool = %tool_name, "Executing tool");

                // Format tool call with human-readable description
                let (description, _verb) = format_tool_description(tool_name, arguments);

                // Show as status (replaces previous status, not accumulated)
                let message = format!("⚡ {}", description);
                self.set_status(&message).await;
                self.handler.on_tool_start(tool_name.clone());

                // Create and publish ToolCallPart (Added event)
                let part_id = uuid::Uuid::new_v4().to_string();
                let tool_call_part = ToolCallPart {
                    id: part_id.clone(),
                    tool_name: tool_name.clone(),
                    input: arguments.clone(),
                    status: ToolCallStatus::Running,
                    output: None,
                    error: None,
                    started_at: chrono::Utc::now().timestamp_millis(),
                    completed_at: None,
                };

                // Store for later update
                {
                    let mut active = self.active_tool_calls.write().await;
                    active.insert(tool_name.clone(), tool_call_part.clone());
                }

                // Publish PartAdded event
                let session_id = self.session_id.read().await.clone();
                let part = SessionPart::ToolCall(tool_call_part);
                let data = PartUpdateData::added(&session_id, &part);
                self.publish_part_event(data).await;
            }
            Action::Completion { summary } => {
                // Append the completion summary to response (actual content, persists)
                self.append_response(summary).await;
            }
            Action::UserInteraction { question, .. } => {
                // This will be handled by on_user_input_required
                debug!(question = %question, "User interaction requested");
            }
            Action::Failure { reason } => {
                // Append failure reason to response (persists)
                self.append_response(&format!("❌ 错误: {}\n", reason)).await;
            }
        }
    }

    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        info!(
            action_type = %action.action_type(),
            success = result.is_success(),
            "Action completed"
        );

        // Notify UI about tool execution results
        if let Action::ToolCall { tool_name, arguments } = action {
            // Get the action verb for completion message
            let (_description, verb) = format_tool_description(tool_name, arguments);

            // Get and update the active tool call part
            let maybe_updated_part = {
                let mut active = self.active_tool_calls.write().await;
                if let Some(mut part) = active.remove(tool_name) {
                    let now = chrono::Utc::now().timestamp_millis();
                    part.completed_at = Some(now);

                    match result {
                        ActionResult::ToolSuccess { output, .. } => {
                            part.status = ToolCallStatus::Completed;
                            part.output = Some(output.to_string());
                        }
                        ActionResult::ToolError { error, .. } => {
                            part.status = ToolCallStatus::Failed;
                            part.error = Some(error.clone());
                        }
                        _ => {
                            part.status = ToolCallStatus::Completed;
                        }
                    }
                    Some(part)
                } else {
                    None
                }
            };

            // Publish PartUpdated event
            if let Some(updated_part) = maybe_updated_part {
                let session_id = self.session_id.read().await.clone();
                let part = SessionPart::ToolCall(updated_part);
                let data = PartUpdateData::updated(&session_id, &part, None);
                self.publish_part_event(data).await;
            }

            match result {
                ActionResult::ToolSuccess { output, duration_ms } => {
                    info!(
                        tool = %tool_name,
                        duration_ms = duration_ms,
                        output_size = output.to_string().len(),
                        "Tool execution successful"
                    );
                    // Send tool result to UI (truncate for display)
                    let output_str = output.to_string();
                    let display_output = truncate_str(&output_str, 100);

                    // Show success as status (replaces tool call status)
                    let message = format!("✓ {}完成 ({}ms)", verb, duration_ms);
                    self.set_status(&message).await;
                    self.handler.on_tool_result(tool_name.clone(), display_output);
                }
                ActionResult::ToolError { error, .. } => {
                    warn!(
                        tool = %tool_name,
                        error = %error,
                        "Tool execution failed"
                    );
                    // Show error as status (replaces tool call status)
                    let message = format!("✗ {}失败: {}", verb, error);
                    self.set_status(&message).await;
                    self.handler.on_tool_result(tool_name.clone(), format!("Error: {}", error));
                }
                _ => {}
            }
        }
    }

    async fn on_confirmation_required(&self, tool_name: &str, _arguments: &Value) -> bool {
        info!(
            tool = %tool_name,
            "Confirmation required for tool execution"
        );

        // For now, auto-confirm. In the future, this should prompt the user
        // through the FFI layer using a dedicated confirmation callback
        warn!("Auto-confirming tool execution (confirmation UI not implemented)");
        true
    }

    async fn on_user_input_required(
        &self,
        question: &str,
        options: Option<&[String]>,
    ) -> String {
        info!(
            question = %question,
            has_options = options.is_some(),
            "User input required"
        );

        // Build question display with options
        let mut question_display = format!("❓ {}", question);
        if let Some(opts) = options {
            for (i, opt) in opts.iter().enumerate() {
                question_display.push_str(&format!("\n  {}. {}", i + 1, opt));
            }
        }
        // Show question as status (temporary)
        self.set_status(&question_display).await;

        // Create pending input request and notify Swift UI
        let options_vec = options.map(|opts| opts.to_vec()).unwrap_or_default();
        let (request_id, receiver) = crate::ffi::user_input::store_pending_input(
            question.to_string(),
            if options_vec.is_empty() { None } else { Some(options_vec.clone()) },
        );

        info!(request_id = %request_id, "Waiting for user input via FFI callback");

        // Notify Swift UI to show input dialog
        self.handler.on_user_input_request(
            request_id.clone(),
            question.to_string(),
            options_vec,
        );

        // Wait for user response via oneshot channel
        match receiver.await {
            Ok(response) => {
                info!(
                    request_id = %request_id,
                    response_len = response.len(),
                    "Received user input response"
                );

                // Append user's response to response buffer (visible in final output)
                if !response.is_empty() {
                    let response_text = format!("📝 用户回复: {}\n\n", response);
                    self.append_response(&response_text).await;
                }

                response
            }
            Err(_) => {
                warn!(request_id = %request_id, "User input channel closed, returning empty response");
                String::new()
            }
        }
    }

    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        warn!(
            violation = ?violation,
            "Guard triggered"
        );

        // Append guard violation to response (persists)
        let message = format!("Limit reached: {}", violation.description());
        self.append_response(&message).await;
    }

    async fn on_complete(&self, summary: &str) {
        info!(summary_len = summary.len(), "AgentLoop completed");

        // Ensure the summary is in the response
        let buffer = self.response_buffer.read().await;
        if !buffer.contains(summary) {
            drop(buffer);
            self.append_response(summary).await;
        }

        // Finalize the response
        self.finalize_response().await;
    }

    async fn on_failed(&self, reason: &str) {
        warn!(reason = %reason, "AgentLoop failed");

        // Append error to response (persists)
        self.append_response(&format!("\n\nError: {}", reason)).await;

        // Call error handler
        self.handler.on_error(reason.to_string());
    }

    async fn on_aborted(&self) {
        info!("AgentLoop aborted");
        self.handler.on_error("Operation cancelled".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mock event handler for testing
    struct MockEventHandler {
        events: Mutex<Vec<String>>,
    }

    impl MockEventHandler {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl AetherEventHandler for MockEventHandler {
        fn on_thinking(&self) {
            self.events.lock().unwrap().push("thinking".to_string());
        }

        fn on_tool_start(&self, tool_name: String) {
            self.events.lock().unwrap().push(format!("tool_start:{}", tool_name));
        }

        fn on_tool_result(&self, _tool_name: String, _result: String) {}

        fn on_stream_chunk(&self, chunk: String) {
            self.events.lock().unwrap().push(format!("chunk:{}", chunk));
        }

        fn on_complete(&self, response: String) {
            self.events.lock().unwrap().push(format!("complete:{}", response.len()));
        }

        fn on_error(&self, error: String) {
            self.events.lock().unwrap().push(format!("error:{}", error));
        }

        fn on_memory_stored(&self) {}

        fn on_agent_mode_detected(&self, _task: crate::intent::ExecutableTaskFFI) {}

        fn on_tools_changed(&self, _tool_count: u32) {}

        fn on_mcp_startup_complete(&self, _report: crate::event_handler::McpStartupReportFFI) {}

        fn on_runtime_updates_available(&self, _updates: Vec<crate::ffi::RuntimeUpdateInfo>) {}

        fn on_session_started(&self, _session_id: String) {}

        fn on_tool_call_started(&self, _call_id: String, _tool_name: String) {}

        fn on_tool_call_completed(&self, _call_id: String, _output: String) {}

        fn on_tool_call_failed(&self, _call_id: String, _error: String, _is_retryable: bool) {}

        fn on_loop_progress(&self, _session_id: String, _iteration: u32, _status: String) {}

        fn on_plan_created(&self, _session_id: String, _steps: Vec<String>) {}

        fn on_session_completed(&self, _session_id: String, _summary: String) {}

        fn on_subagent_started(&self, _parent_session_id: String, _child_session_id: String, _agent_id: String) {}

        fn on_subagent_completed(&self, _child_session_id: String, _success: bool, _summary: String) {}

        fn on_plan_confirmation_required(&self, _plan_id: String, _plan: crate::dispatcher::DagTaskPlan) {}

        fn on_user_input_request(&self, request_id: String, question: String, _options: Vec<String>) {
            self.events.lock().unwrap().push(format!("user_input_request:{}:{}", request_id, question));
        }

        fn on_part_update(&self, event: crate::ffi::PartUpdateEventFFI) {
            self.events.lock().unwrap().push(format!("part_update:{}:{}", event.part_id, event.part_type));
        }
    }

    #[tokio::test]
    async fn test_callback_adapter_streaming() {
        let handler = Arc::new(MockEventHandler::new());
        let callback = FfiLoopCallback::new(handler.clone());

        // Simulate streaming with status (replaces) and response (accumulates)
        callback.set_status("Loading...").await;
        callback.set_status("Processing...").await; // Replaces previous status
        callback.append_response("Hello, ").await; // Clears status, adds to response
        callback.append_response("world!").await; // Accumulates
        callback.finalize_response().await;

        let events = handler.events();
        // Status updates replace each other
        assert!(events.contains(&"chunk:Loading...".to_string()));
        assert!(events.contains(&"chunk:Processing...".to_string()));
        // Response accumulates
        assert!(events.contains(&"chunk:Hello, ".to_string()));
        assert!(events.contains(&"chunk:Hello, world!".to_string()));

        let response = callback.get_response().await;
        assert_eq!(response, "Hello, world!");
    }

    #[tokio::test]
    async fn test_callback_adapter_completion() {
        let handler = Arc::new(MockEventHandler::new());
        let callback = FfiLoopCallback::new(handler.clone());

        // Simulate completion
        let action = Action::Completion {
            summary: "Task completed successfully".to_string(),
        };
        callback.on_action_start(&action).await;
        callback.on_complete("Task completed successfully").await;

        let events = handler.events();
        assert!(events.iter().any(|e| e.starts_with("complete:")));
    }

    #[test]
    fn test_format_tool_description() {
        use serde_json::json;

        // Test file_ops with different operations
        let args = json!({"operation": "mkdir", "path": "/tmp/test"});
        let (desc, verb) = format_tool_description("file_ops", &args);
        assert!(desc.contains("正在创建目录"));
        assert!(desc.contains("/tmp/test"));
        assert_eq!(verb, "创建目录");

        let args = json!({"operation": "write", "path": "/tmp/test.txt"});
        let (desc, verb) = format_tool_description("file_ops", &args);
        assert!(desc.contains("正在写入文件"));
        assert_eq!(verb, "写入文件");

        let args = json!({"operation": "read", "path": "/tmp/test.txt"});
        let (desc, verb) = format_tool_description("file_ops", &args);
        assert!(desc.contains("正在读取文件"));
        assert_eq!(verb, "读取文件");

        // Test search tool
        let args = json!({"query": "rust async programming"});
        let (desc, verb) = format_tool_description("search", &args);
        assert!(desc.contains("正在搜索"));
        assert!(desc.contains("rust async programming"));
        assert_eq!(verb, "搜索");

        // Test web_fetch tool
        let args = json!({"url": "https://example.com/page"});
        let (desc, verb) = format_tool_description("web_fetch", &args);
        assert!(desc.contains("正在获取网页"));
        assert!(desc.contains("example.com"));
        assert_eq!(verb, "获取网页");

        // Test generate_image tool
        let args = json!({"prompt": "a beautiful sunset", "provider": "dalle"});
        let (desc, verb) = format_tool_description("generate_image", &args);
        assert!(desc.contains("正在生成图像"));
        assert!(desc.contains("beautiful sunset"));
        assert_eq!(verb, "生成图像");

        // Test unknown tool
        let args = json!({"param": "value"});
        let (desc, verb) = format_tool_description("custom_tool", &args);
        assert!(desc.contains("正在执行"));
        assert!(desc.contains("custom_tool"));
        assert_eq!(verb, "custom_tool");
    }

    #[test]
    fn test_truncate_path() {
        // Short path should not be truncated
        assert_eq!(truncate_path("/tmp/test.txt", 50), "/tmp/test.txt");

        // Long path should be truncated
        let long_path = "/Users/user/.aether/output/E75F2A21-50DE-4FB2-8B6B-13E59CFBD90B/chapter-1/triples.json";
        let truncated = truncate_path(long_path, 40);
        assert!(truncated.len() <= 40);
        assert!(truncated.contains("..."));
    }
}
