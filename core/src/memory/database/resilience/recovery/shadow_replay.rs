//! Shadow Replay Engine
//!
//! Implements deterministic task recovery through trace replay.
//! The engine reconstructs conversation history from stored traces
//! without consuming LLM tokens.

use crate::agent_loop::message_builder::{Message, ToolCall};
use crate::error::AlephError;
use crate::memory::database::resilience::{TaskTrace, TraceRole};
use crate::memory::database::VectorDatabase;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Result of Shadow Replay operation
#[derive(Debug, Clone)]
pub struct ReplayResult {
    /// Reconstructed messages from traces
    pub messages: Vec<Message>,

    /// The step index where replay ended
    pub last_step: u32,

    /// Whether the task was fully replayed (no more traces)
    pub complete: bool,

    /// Last tool call ID for checkpoint tracking
    pub last_tool_call_id: Option<String>,
}

/// Divergence detection result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DivergenceStatus {
    /// LLM produced the same next step as recorded
    Aligned,

    /// LLM produced a different tool call than recorded
    Diverged {
        expected_tool: String,
        actual_tool: String,
    },

    /// No more recorded steps, ready for live execution
    ReachedEnd,
}

/// Content of a trace entry (for deserialization)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TraceContent {
    /// Text content (for assistant messages without tool calls)
    #[serde(default)]
    content: String,

    /// Tool calls (for assistant messages with tool use)
    #[serde(default)]
    tool_calls: Option<Vec<SerializedToolCall>>,

    /// Tool call ID (for tool result messages)
    #[serde(default)]
    tool_call_id: Option<String>,

    /// Tool result (for tool messages)
    #[serde(default)]
    result: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializedToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Shadow Replay Engine for deterministic task recovery
pub struct ShadowReplayEngine {
    db: Arc<VectorDatabase>,
}

impl std::fmt::Debug for ShadowReplayEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShadowReplayEngine")
            .field("db", &"<VectorDatabase>")
            .finish()
    }
}

impl ShadowReplayEngine {
    /// Create a new Shadow Replay Engine
    pub fn new(db: Arc<VectorDatabase>) -> Self {
        Self { db }
    }

    /// Load all traces for a task and reconstruct conversation history
    ///
    /// This is the core of Shadow Replay - we reconstruct the LLM's
    /// conversation state without making any API calls.
    pub async fn replay_task(&self, task_id: &str) -> Result<ReplayResult, AlephError> {
        let traces = self.db.get_traces_by_task(task_id).await?;

        if traces.is_empty() {
            return Ok(ReplayResult {
                messages: Vec::new(),
                last_step: 0,
                complete: true,
                last_tool_call_id: None,
            });
        }

        let mut messages = Vec::with_capacity(traces.len());
        let mut last_tool_call_id = None;

        for trace in &traces {
            let message = self.trace_to_message(trace)?;

            // Track last tool call ID for checkpoint
            if let Some(ref calls) = message.tool_calls {
                let tool_calls: &Vec<ToolCall> = calls;
                if let Some(last_call) = tool_calls.last() {
                    last_tool_call_id = Some(last_call.id.clone());
                }
            }

            messages.push(message);
        }

        let last_step = traces.last().map(|t| t.step_index).unwrap_or(0);

        Ok(ReplayResult {
            messages,
            last_step,
            complete: true,
            last_tool_call_id,
        })
    }

    /// Replay task up to a specific step index
    pub async fn replay_until_step(
        &self,
        task_id: &str,
        until_step: u32,
    ) -> Result<ReplayResult, AlephError> {
        let traces = self.db.get_traces_by_task(task_id).await?;

        let filtered: Vec<_> = traces
            .into_iter()
            .filter(|t| t.step_index <= until_step)
            .collect();

        if filtered.is_empty() {
            return Ok(ReplayResult {
                messages: Vec::new(),
                last_step: 0,
                complete: false,
                last_tool_call_id: None,
            });
        }

        let mut messages = Vec::with_capacity(filtered.len());
        let mut last_tool_call_id = None;

        for trace in &filtered {
            let message = self.trace_to_message(trace)?;

            if let Some(ref calls) = message.tool_calls {
                let tool_calls: &Vec<ToolCall> = calls;
                if let Some(last_call) = tool_calls.last() {
                    last_tool_call_id = Some(last_call.id.clone());
                }
            }

            messages.push(message);
        }

        let last_step = filtered.last().map(|t| t.step_index).unwrap_or(0);

        Ok(ReplayResult {
            messages,
            last_step,
            complete: false,
            last_tool_call_id,
        })
    }

    /// Check if LLM's next action aligns with recorded trace
    ///
    /// This is the "Handover Inference" step from the architecture design.
    /// After replay, we compare the LLM's first new action with the recorded next step.
    pub async fn check_divergence(
        &self,
        task_id: &str,
        current_step: u32,
        next_tool_call: Option<&ToolCall>,
    ) -> Result<DivergenceStatus, AlephError> {
        // Get the next recorded trace
        let traces = self.db.get_traces_from_step(task_id, current_step + 1).await?;

        let next_trace = match traces.first() {
            Some(t) => t,
            None => return Ok(DivergenceStatus::ReachedEnd),
        };

        // Only check divergence for assistant messages with tool calls
        if next_trace.role != TraceRole::Assistant {
            return Ok(DivergenceStatus::ReachedEnd);
        }

        // Parse the recorded trace content
        let content: TraceContent = serde_json::from_str(&next_trace.content_json)
            .map_err(|e| AlephError::config(format!("Failed to parse trace content: {}", e)))?;

        // Extract expected tool call
        let expected_tool = match content.tool_calls {
            Some(calls) if !calls.is_empty() => calls[0].name.clone(),
            _ => return Ok(DivergenceStatus::ReachedEnd),
        };

        // Compare with actual tool call
        match next_tool_call {
            Some(call) => {
                if call.name == expected_tool {
                    Ok(DivergenceStatus::Aligned)
                } else {
                    Ok(DivergenceStatus::Diverged {
                        expected_tool,
                        actual_tool: call.name.clone(),
                    })
                }
            }
            None => Ok(DivergenceStatus::Diverged {
                expected_tool,
                actual_tool: "(no tool call)".to_string(),
            }),
        }
    }

    /// Convert a TaskTrace to a Message
    fn trace_to_message(&self, trace: &TaskTrace) -> Result<Message, AlephError> {
        let content: TraceContent = serde_json::from_str(&trace.content_json)
            .map_err(|e| AlephError::config(format!("Failed to parse trace content: {}", e)))?;

        match trace.role {
            TraceRole::Assistant => {
                if let Some(tool_calls) = content.tool_calls {
                    let calls: Vec<ToolCall> = tool_calls
                        .into_iter()
                        .map(|tc| ToolCall::new(tc.id, tc.name, tc.arguments))
                        .collect();
                    Ok(Message::assistant_with_tool_calls(calls))
                } else {
                    Ok(Message::assistant(content.content))
                }
            }
            TraceRole::Tool => {
                let tool_call_id = content.tool_call_id.unwrap_or_default();
                let result = content.result.unwrap_or_else(|| content.content);
                Ok(Message::tool_result(tool_call_id, result))
            }
        }
    }

    /// Create a trace from a Message (for recording)
    pub fn message_to_trace(
        task_id: &str,
        step_index: u32,
        message: &Message,
    ) -> Result<TaskTrace, AlephError> {
        let (role, content) = if message.role == "tool" {
            let content = TraceContent {
                content: String::new(),
                tool_calls: None,
                tool_call_id: message.tool_call_id.clone(),
                result: Some(message.content.clone()),
            };
            (TraceRole::Tool, content)
        } else {
            let tool_calls: Option<Vec<SerializedToolCall>> = message.tool_calls.as_ref().map(|calls: &Vec<ToolCall>| {
                calls
                    .iter()
                    .map(|tc| SerializedToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    })
                    .collect()
            });

            let content = TraceContent {
                content: message.content.clone(),
                tool_calls,
                tool_call_id: None,
                result: None,
            };
            (TraceRole::Assistant, content)
        };

        let content_json = serde_json::to_string(&content)
            .map_err(|e| AlephError::config(format!("Failed to serialize trace: {}", e)))?;

        Ok(TaskTrace::new(task_id, step_index, role, content_json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_content_serialization() {
        let content = TraceContent {
            content: "Hello".to_string(),
            tool_calls: Some(vec![SerializedToolCall {
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"query":"test"}"#.to_string(),
            }]),
            tool_call_id: None,
            result: None,
        };

        let json = serde_json::to_string(&content).unwrap();
        let parsed: TraceContent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.content, "Hello");
        assert!(parsed.tool_calls.is_some());
        assert_eq!(parsed.tool_calls.unwrap()[0].name, "search");
    }

    #[test]
    fn test_divergence_status_eq() {
        assert_eq!(DivergenceStatus::Aligned, DivergenceStatus::Aligned);
        assert_eq!(DivergenceStatus::ReachedEnd, DivergenceStatus::ReachedEnd);
        assert_ne!(DivergenceStatus::Aligned, DivergenceStatus::ReachedEnd);
    }
}
