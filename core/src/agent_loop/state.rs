//! Agent Loop state management
//!
//! This module defines the core state structures for the Agent Loop,
//! including LoopState (session state) and LoopStep (individual step record).

use serde::{Deserialize, Serialize};
use std::time::Instant;

use super::decision::{Action, ActionResult, Decision};
use crate::core::MediaAttachment;

/// Complete state of an Agent Loop session
#[derive(Debug, Clone)]
pub struct LoopState {
    /// Unique session ID
    pub session_id: String,
    /// User's original request
    pub original_request: String,
    /// Request context (attachments, selected files, clipboard, etc.)
    pub context: RequestContext,
    /// Execution step history
    pub steps: Vec<LoopStep>,
    /// Current step count
    pub step_count: usize,
    /// Cumulative token usage
    pub total_tokens: usize,
    /// Session start time
    pub started_at: Instant,
    /// Compressed history summary (updated by ContextCompressor)
    pub history_summary: String,
    /// Step index up to which history has been compressed
    pub compressed_until_step: usize,
}

impl LoopState {
    /// Create a new LoopState
    pub fn new(session_id: String, request: String, context: RequestContext) -> Self {
        Self {
            session_id,
            original_request: request,
            context,
            steps: Vec::new(),
            step_count: 0,
            total_tokens: 0,
            started_at: Instant::now(),
            history_summary: String::new(),
            compressed_until_step: 0,
        }
    }

    /// Record a completed step
    pub fn record_step(&mut self, step: LoopStep) {
        self.total_tokens += step.tokens_used;
        self.step_count += 1;
        self.steps.push(step);
    }

    /// Get the last step's result (if any)
    pub fn last_result(&self) -> Option<&ActionResult> {
        self.steps.last().map(|s| &s.result)
    }

    /// Check if compression is needed
    pub fn needs_compression(&self, threshold: usize) -> bool {
        let uncompressed_steps = self.steps.len() - self.compressed_until_step;
        uncompressed_steps > threshold
    }

    /// Apply compression result
    pub fn apply_compression(&mut self, summary: String, compressed_until: usize) {
        self.history_summary = summary;
        self.compressed_until_step = compressed_until;
    }

    /// Get recent steps (within sliding window)
    pub fn recent_steps(&self, window_size: usize) -> &[LoopStep] {
        if self.steps.len() <= window_size {
            &self.steps
        } else {
            &self.steps[self.steps.len() - window_size..]
        }
    }

    /// Get elapsed time since session start
    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }
}

/// Single loop step record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopStep {
    /// Step ID (0-indexed)
    pub step_id: usize,
    /// Observation at this step (serialized for storage)
    pub observation_summary: String,
    /// LLM's thinking process
    pub thinking: Thinking,
    /// Action taken
    pub action: Action,
    /// Action result
    pub result: ActionResult,
    /// Token consumption for this step
    pub tokens_used: usize,
    /// Step duration in milliseconds
    pub duration_ms: u64,
}

/// LLM's thinking output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thinking {
    /// LLM's reasoning process (optional, for debugging/display)
    pub reasoning: Option<String>,
    /// Decided next action
    pub decision: Decision,
}

/// Request context containing attachments and environment info
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    /// Media attachments (images, files, etc.)
    pub attachments: Vec<MediaAttachment>,
    /// Selected text (if any)
    pub selected_text: Option<String>,
    /// Clipboard content (if relevant)
    pub clipboard_content: Option<String>,
    /// Current application name
    pub current_app: Option<String>,
    /// Current window title
    pub window_title: Option<String>,
    /// Working directory
    pub working_directory: Option<String>,
    /// Additional metadata
    pub metadata: std::collections::HashMap<String, String>,
}

impl RequestContext {
    /// Create empty context
    pub fn empty() -> Self {
        Self::default()
    }

    /// Check if context has any attachments
    pub fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
    }

    /// Check if context has image attachments
    pub fn has_images(&self) -> bool {
        self.attachments.iter().any(|a| {
            a.media_type == "image" || a.mime_type.starts_with("image/")
        })
    }
}

/// Observation data passed to Thinker
#[derive(Debug, Clone)]
pub struct Observation {
    /// Compressed history summary
    pub history_summary: String,
    /// Recent steps with full details (sliding window)
    pub recent_steps: Vec<StepSummary>,
    /// Available tools for this step
    pub available_tools: Vec<ToolInfo>,
    /// Context attachments
    pub attachments: Vec<MediaAttachment>,
    /// Current step number
    pub current_step: usize,
    /// Total tokens used so far
    pub total_tokens: usize,
}

/// Summary of a step for observation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepSummary {
    /// Step ID
    pub step_id: usize,
    /// Brief reasoning
    pub reasoning: String,
    /// Action type and name
    pub action_type: String,
    /// Action arguments (serialized)
    pub action_args: String,
    /// Result summary
    pub result_summary: String,
    /// Whether action succeeded
    pub success: bool,
}

impl From<&LoopStep> for StepSummary {
    fn from(step: &LoopStep) -> Self {
        Self {
            step_id: step.step_id,
            reasoning: step
                .thinking
                .reasoning
                .clone()
                .unwrap_or_else(|| "No reasoning".to_string()),
            action_type: step.action.action_type(),
            action_args: step.action.args_summary(),
            result_summary: step.result.summary(),
            success: step.result.is_success(),
        }
    }
}

/// Tool information for observation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Parameter schema (JSON)
    pub parameters_schema: String,
    /// Tool category
    pub category: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loop_state_creation() {
        let state = LoopState::new(
            "test-session".to_string(),
            "Test request".to_string(),
            RequestContext::empty(),
        );

        assert_eq!(state.session_id, "test-session");
        assert_eq!(state.original_request, "Test request");
        assert_eq!(state.step_count, 0);
        assert_eq!(state.total_tokens, 0);
        assert!(state.steps.is_empty());
    }

    #[test]
    fn test_needs_compression() {
        let mut state = LoopState::new(
            "test".to_string(),
            "request".to_string(),
            RequestContext::empty(),
        );

        // Add 6 dummy steps
        for i in 0..6 {
            state.steps.push(LoopStep {
                step_id: i,
                observation_summary: String::new(),
                thinking: Thinking {
                    reasoning: None,
                    decision: Decision::Complete {
                        summary: "done".to_string(),
                    },
                },
                action: Action::Completion {
                    summary: "done".to_string(),
                },
                result: ActionResult::Completed,
                tokens_used: 100,
                duration_ms: 1000,
            });
        }

        assert!(state.needs_compression(5));
        assert!(!state.needs_compression(6));
    }

    #[test]
    fn test_recent_steps() {
        let mut state = LoopState::new(
            "test".to_string(),
            "request".to_string(),
            RequestContext::empty(),
        );

        for i in 0..10 {
            state.steps.push(LoopStep {
                step_id: i,
                observation_summary: String::new(),
                thinking: Thinking {
                    reasoning: None,
                    decision: Decision::Complete {
                        summary: "done".to_string(),
                    },
                },
                action: Action::Completion {
                    summary: "done".to_string(),
                },
                result: ActionResult::Completed,
                tokens_used: 100,
                duration_ms: 1000,
            });
        }

        let recent = state.recent_steps(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].step_id, 7);
        assert_eq!(recent[2].step_id, 9);
    }
}
