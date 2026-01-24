//! MessageBuilder implementation

use std::sync::Arc;

use crate::agent_loop::overflow::OverflowDetector;
use crate::components::{
    ExecutionSession, SessionCompactor, SessionPart, ToolCallStatus,
};

use super::config::MessageBuilderConfig;
use super::types::{Message, ToolCall};

/// Main message builder that converts SessionParts to LLM messages
pub struct MessageBuilder {
    /// Configuration
    config: MessageBuilderConfig,
    /// Optional session compactor for filter_compacted functionality
    compactor: Option<Arc<SessionCompactor>>,
    /// Optional overflow detector for token limit warnings
    overflow_detector: Option<Arc<OverflowDetector>>,
}

impl MessageBuilder {
    /// Create a new MessageBuilder with the given config
    pub fn new(config: MessageBuilderConfig) -> Self {
        Self {
            config,
            compactor: None,
            overflow_detector: None,
        }
    }

    /// Create a new MessageBuilder with a compactor for filter_compacted support
    ///
    /// When a compactor is provided, `build_from_session` will use
    /// `filter_compacted` to exclude parts before the compaction boundary.
    pub fn with_compactor(config: MessageBuilderConfig, compactor: Arc<SessionCompactor>) -> Self {
        Self {
            config,
            compactor: Some(compactor),
            overflow_detector: None,
        }
    }

    /// Create a new MessageBuilder with an overflow detector for token limit warnings
    ///
    /// When an overflow detector is provided, `inject_reminders` will inject
    /// a warning when token usage exceeds 80%.
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

    /// Create a new MessageBuilder with both compactor and overflow detector
    ///
    /// This provides full functionality: compaction filtering and token limit warnings.
    pub fn with_all(
        config: MessageBuilderConfig,
        compactor: Option<Arc<SessionCompactor>>,
        detector: Option<Arc<OverflowDetector>>,
    ) -> Self {
        Self {
            config,
            compactor,
            overflow_detector: detector,
        }
    }

    /// Check if compactor is set (for testing)
    #[cfg(test)]
    pub(crate) fn has_compactor(&self) -> bool {
        self.compactor.is_some()
    }

    /// Check if overflow detector is set (for testing)
    #[cfg(test)]
    pub(crate) fn has_overflow_detector(&self) -> bool {
        self.overflow_detector.is_some()
    }

    /// Build messages from session, applying filter_compacted
    ///
    /// This method provides a convenient way to build messages from a session
    /// while respecting compaction boundaries. If a compactor is configured,
    /// it will use `filter_compacted` to exclude old parts that have been
    /// compacted. Otherwise, it uses all session parts directly.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let compactor = Arc::new(SessionCompactor::new());
    /// let builder = MessageBuilder::with_compactor(config, compactor);
    /// let messages = builder.build_from_session(&session);
    /// ```
    pub fn build_from_session(&self, session: &ExecutionSession) -> Vec<Message> {
        let filtered_parts = if let Some(ref compactor) = self.compactor {
            compactor.filter_compacted(session)
        } else {
            session.parts.clone()
        };
        self.build_messages(session, &filtered_parts)
    }

    /// Convert session parts to LLM messages
    ///
    /// This method converts each SessionPart to the appropriate Message format:
    /// - UserInput → User message
    /// - AiResponse → Assistant message
    /// - ToolCall → Assistant with tool_call + Tool result message
    /// - Summary → Q&A pair for context continuity
    /// - SystemReminder → Skipped (handled in inject_reminders)
    pub fn parts_to_messages(&self, parts: &[SessionPart]) -> Vec<Message> {
        let mut messages = Vec::new();

        for part in parts {
            match part {
                SessionPart::UserInput(input) => {
                    let mut content = input.text.clone();
                    if let Some(ref ctx) = input.context {
                        if !ctx.is_empty() {
                            content = format!("{}\n\nContext: {}", content, ctx);
                        }
                    }
                    messages.push(Message::user(content));
                }

                SessionPart::AiResponse(response) => {
                    let mut content = response.content.clone();
                    if let Some(ref reasoning) = response.reasoning {
                        if !reasoning.is_empty() && content.is_empty() {
                            // If only reasoning, use it as content
                            content = reasoning.clone();
                        }
                    }
                    if !content.is_empty() {
                        messages.push(Message::assistant(content));
                    }
                }

                SessionPart::ToolCall(tool_call) => {
                    // Create tool call from part
                    let tc = ToolCall::from_part(tool_call);

                    // Add assistant message with tool call
                    messages.push(Message::assistant_with_tool_call(tc));

                    // Add tool result message based on status
                    let result_content = self.tool_call_to_result_content(tool_call);
                    messages.push(Message::tool_result(&tool_call.id, result_content));
                }

                SessionPart::Summary(summary) => {
                    // Convert summary to Q&A pair for context continuity
                    // This follows the pattern of injecting historical context
                    messages.push(Message::user("What did we do so far?"));
                    messages.push(Message::assistant(&summary.content));
                }

                SessionPart::Reasoning(reasoning) => {
                    // Include reasoning as assistant message for transparency
                    if !reasoning.content.is_empty() {
                        messages.push(Message::assistant(&reasoning.content));
                    }
                }

                SessionPart::PlanCreated(plan) => {
                    // Include plan as assistant response
                    let plan_text = format!(
                        "I've created a plan with the following steps:\n{}",
                        plan.steps
                            .iter()
                            .enumerate()
                            .map(|(i, s)| format!("{}. {}", i + 1, s))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    messages.push(Message::assistant(plan_text));
                }

                SessionPart::SubAgentCall(sub_agent) => {
                    // Include sub-agent call as assistant message
                    let content = if let Some(ref result) = sub_agent.result {
                        format!(
                            "Delegated to sub-agent '{}' with prompt: {}\n\nResult: {}",
                            sub_agent.agent_id, sub_agent.prompt, result
                        )
                    } else {
                        format!(
                            "Delegating to sub-agent '{}' with prompt: {}",
                            sub_agent.agent_id, sub_agent.prompt
                        )
                    };
                    messages.push(Message::assistant(content));
                }

                // Skip markers, reminders, and metadata parts - they are handled separately
                SessionPart::CompactionMarker(_) => {}
                SessionPart::SystemReminder(_) => {}
                // Step boundaries are metadata for execution tracking, not converted to messages
                SessionPart::StepStart(_) => {}
                SessionPart::StepFinish(_) => {}
                // Snapshots and patches are for session revert, not message content
                SessionPart::Snapshot(_) => {}
                SessionPart::Patch(_) => {}
                // StreamingText is for UI incremental updates, not final messages
                SessionPart::StreamingText(_) => {}
            }
        }

        // Apply max_messages limit
        if messages.len() > self.config.max_messages {
            // Keep the most recent messages, but always include the first user message
            let excess = messages.len() - self.config.max_messages;
            messages.drain(1..=excess);
        }

        messages
    }

    /// Build messages from a session with reminder injection
    ///
    /// This is the main entry point that:
    /// 1. Converts filtered parts to messages
    /// 2. Injects system reminders if needed
    pub fn build_messages(
        &self,
        session: &ExecutionSession,
        filtered_parts: &[SessionPart],
    ) -> Vec<Message> {
        let mut messages = self.parts_to_messages(filtered_parts);

        // Inject reminders if enabled and threshold met
        if self.config.inject_reminders {
            self.inject_reminders(&mut messages, session);
        }

        messages
    }

    /// Inject system reminders by wrapping the last user message
    ///
    /// Following OpenCode's pattern, we wrap the last user message with
    /// `<system-reminder>` tags to provide context for multi-step tasks.
    pub fn inject_reminders(&self, messages: &mut Vec<Message>, session: &ExecutionSession) {
        // Only inject when iteration count exceeds threshold
        if session.iteration_count <= self.config.reminder_threshold {
            return;
        }

        // Find the last user message
        let last_user_idx = messages.iter().rposition(|m| m.role == "user");

        if let Some(idx) = last_user_idx {
            let original_content = messages[idx].content.clone();

            // Wrap with system-reminder tags
            let wrapped_content = format!(
                "<system-reminder>\nThe user sent the following message:\n{}\nPlease address this message and continue with your tasks.\n</system-reminder>",
                original_content
            );

            messages[idx].content = wrapped_content;
        }

        // Inject token limit warnings if overflow detector is configured
        self.inject_limit_warnings(messages, session);

        // Inject max steps warning if on the last iteration
        self.inject_max_steps_warning(messages, session);
    }

    /// Inject token limit warning when usage is high (>=80%)
    ///
    /// When the overflow detector is configured and token usage exceeds 80%,
    /// a system reminder is inserted after the last user message to warn
    /// that the session may need to be compacted soon.
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

    /// Inject warning when on the last iteration
    ///
    /// When the current iteration count equals max_iterations - 1,
    /// a system reminder is inserted to warn the agent that this is their
    /// last step and they must complete or ask for guidance.
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

    /// Convert tool call status to result content
    fn tool_call_to_result_content(&self, tool_call: &crate::components::ToolCallPart) -> String {
        match tool_call.status {
            ToolCallStatus::Completed => {
                tool_call
                    .output
                    .clone()
                    .unwrap_or_else(|| "Tool completed successfully".to_string())
            }
            ToolCallStatus::Failed => {
                format!(
                    "Error: {}",
                    tool_call.error.as_ref().unwrap_or(&"Unknown error".to_string())
                )
            }
            ToolCallStatus::Pending | ToolCallStatus::Running => {
                "[Tool execution was interrupted]".to_string()
            }
            ToolCallStatus::Aborted => {
                "[Tool execution was aborted]".to_string()
            }
        }
    }
}
