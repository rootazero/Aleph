//! Message types and message building for LLM conversation
//!
//! Contains `Message`, `MessageRole`, `build_messages()`, `build_observation()`,
//! and helper functions for formatting.

use crate::agent_loop::{LoopState, Observation, StepSummary, ToolInfo};
use crate::core::MediaAttachment;

use super::PromptBuilder;

/// Message type for LLM conversation
#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    /// Native tool call ID (for protocol-aware tool result formatting in Phase 2)
    pub tool_call_id: Option<String>,
}

/// Message role
#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
}

impl Message {
    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            tool_call_id: None,
        }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_call_id: None,
        }
    }

    /// Create a tool result message
    pub fn tool_result(tool_name: &str, result: &str) -> Self {
        Self {
            role: MessageRole::Tool,
            content: format!("[{}]\n{}", tool_name, result),
            tool_call_id: None,
        }
    }

    /// Create a native tool result message with call ID
    pub fn native_tool_result(tool_call_id: &str, tool_name: &str, result: &str) -> Self {
        Self {
            role: MessageRole::Tool,
            content: format!("[{}]\n{}", tool_name, result),
            tool_call_id: Some(tool_call_id.to_string()),
        }
    }
}

/// Safely truncate a string at character boundaries (UTF-8 safe)
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

/// Format attachment for display
fn format_attachment(attachment: &MediaAttachment) -> String {
    let preview = truncate_str(&attachment.data, 50);

    match attachment.media_type.as_str() {
        "image" => {
            format!(
                "Image ({}, {} bytes)",
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        "document" => {
            format!(
                "Document: {} ({}, {} bytes)",
                attachment.filename.as_deref().unwrap_or("unnamed"),
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        "file" => {
            format!(
                "File: {} ({}, {} bytes)",
                attachment.filename.as_deref().unwrap_or("unnamed"),
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        _ => {
            format!(
                "{}: {} ({} bytes)",
                attachment.media_type,
                attachment.filename.as_deref().unwrap_or(&preview),
                attachment.size_bytes
            )
        }
    }
}

impl PromptBuilder {
    /// Build messages for the thinking step
    pub fn build_messages(
        &self,
        original_request: &str,
        observation: &Observation,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // 1. User's original request with context
        let mut user_msg = format!("Task: {}\n", original_request);

        // Add attachments info
        if !observation.attachments.is_empty() {
            user_msg.push_str("\nAttachments:\n");
            for (i, attachment) in observation.attachments.iter().enumerate() {
                user_msg.push_str(&format!("{}. {}\n", i + 1, format_attachment(attachment)));
            }
        }

        messages.push(Message::user(user_msg));

        // 2. Compressed history summary (if any)
        if !observation.history_summary.is_empty() {
            messages.push(Message::assistant(format!(
                "[Previous steps summary]\n{}",
                observation.history_summary
            )));
        }

        // 3. Recent steps with full details
        for step in &observation.recent_steps {
            // Assistant's thinking and action
            messages.push(Message::assistant(format!(
                "Reasoning: {}\nAction: {} {}",
                step.reasoning, step.action_type, step.action_args
            )));

            // CRITICAL FIX: User responses must use User role, not Tool role
            // This ensures the LLM understands the user has answered the question
            // and doesn't ask the same question again
            if step.action_type == "ask_user" {
                // User's response to a question - use User role
                messages.push(Message::user(step.result_output.clone()));
            } else if let Some(ref call_id) = step.tool_call_id {
                // Native tool result with ID reference for LLM context
                messages.push(Message::native_tool_result(
                    call_id,
                    &step.action_type,
                    &step.result_output,
                ));
            } else {
                // Tool result - use full output to ensure LLM sees complete data
                // (e.g., full file paths, complete JSON output)
                messages.push(Message::tool_result(&step.action_type, &step.result_output));
            }
        }

        // 4. Current context and request for next action
        // IMPORTANT: Use clear system-level language to avoid confusing agent
        // with user instructions (e.g., "Current step: X" was misinterpreted
        // as user requesting to restart at step X, causing infinite loops)
        let context_msg = format!(
            "[System] Loop iteration: {} | Tokens: {} | Continue with your next action.",
            observation.current_step, observation.total_tokens
        );
        messages.push(Message::user(context_msg));

        messages
    }

    /// Build observation from state
    pub fn build_observation(
        &self,
        state: &LoopState,
        tools: &[ToolInfo],
        window_size: usize,
    ) -> Observation {
        let recent_steps: Vec<StepSummary> = state
            .recent_steps(window_size)
            .iter()
            .map(StepSummary::from)
            .collect();

        Observation {
            history_summary: state.history_summary.clone(),
            recent_steps,
            available_tools: tools.to_vec(),
            attachments: state.context.attachments.clone(),
            current_step: state.step_count,
            total_tokens: state.total_tokens,
        }
    }
}
