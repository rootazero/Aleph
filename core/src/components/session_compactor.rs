//! Session compactor component - manages token limits via compaction.
//!
//! Subscribes to: LoopContinue, ToolCallCompleted
//! Publishes: SessionCompacted
//!
//! This component monitors session token usage and compacts the session
//! when approaching model context limits. Compaction involves:
//! 1. Pruning old tool outputs (keeping recent ones)
//! 2. Generating a summary of earlier session parts
//! 3. Replacing old parts with the summary

use std::collections::HashMap;

use async_trait::async_trait;

use crate::components::types::{ExecutionSession, SessionPart, SessionStatus, SummaryPart};
use crate::event::{
    AetherEvent, CompactionInfo, EventContext, EventHandler, EventType, HandlerError,
};

// ============================================================================
// Model Limits
// ============================================================================

/// Model context limits configuration
#[derive(Debug, Clone)]
pub struct ModelLimit {
    /// Maximum context window size in tokens
    pub context_limit: u64,
    /// Maximum output tokens the model can generate
    pub max_output_tokens: u64,
    /// Reserve ratio (0.0-1.0) - fraction of context to keep free
    pub reserve_ratio: f32,
}

impl Default for ModelLimit {
    fn default() -> Self {
        Self {
            context_limit: 128000,
            max_output_tokens: 4096,
            reserve_ratio: 0.2,
        }
    }
}

impl ModelLimit {
    /// Create a new ModelLimit with custom values
    pub fn new(context_limit: u64, max_output_tokens: u64, reserve_ratio: f32) -> Self {
        Self {
            context_limit,
            max_output_tokens,
            reserve_ratio: reserve_ratio.clamp(0.0, 1.0),
        }
    }

    /// Calculate the effective threshold for compaction trigger
    ///
    /// Returns the token count at which compaction should be triggered
    pub fn compaction_threshold(&self) -> u64 {
        let usable = self.context_limit as f64 * (1.0 - self.reserve_ratio as f64);
        usable as u64
    }
}

// ============================================================================
// Token Tracker
// ============================================================================

/// Token usage tracker with model-specific limits
#[derive(Debug, Clone)]
pub struct TokenTracker {
    /// Model-specific limits
    model_limits: HashMap<String, ModelLimit>,
}

impl Default for TokenTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenTracker {
    /// Create a new TokenTracker with preset model limits
    pub fn new() -> Self {
        let mut model_limits = HashMap::new();

        // Claude models (200K context)
        model_limits.insert(
            "claude-3-opus".to_string(),
            ModelLimit::new(200000, 4096, 0.2),
        );
        model_limits.insert(
            "claude-3-sonnet".to_string(),
            ModelLimit::new(200000, 4096, 0.2),
        );
        model_limits.insert(
            "claude-3-haiku".to_string(),
            ModelLimit::new(200000, 4096, 0.2),
        );
        model_limits.insert(
            "claude-3.5-sonnet".to_string(),
            ModelLimit::new(200000, 8192, 0.2),
        );

        // GPT-4 models (128K context)
        model_limits.insert(
            "gpt-4-turbo".to_string(),
            ModelLimit::new(128000, 4096, 0.2),
        );
        model_limits.insert(
            "gpt-4-turbo-preview".to_string(),
            ModelLimit::new(128000, 4096, 0.2),
        );
        model_limits.insert("gpt-4o".to_string(), ModelLimit::new(128000, 4096, 0.2));

        // Gemini models (32K context for Pro)
        model_limits.insert("gemini-pro".to_string(), ModelLimit::new(32000, 8192, 0.2));
        model_limits.insert(
            "gemini-1.5-pro".to_string(),
            ModelLimit::new(1000000, 8192, 0.2),
        );

        Self { model_limits }
    }

    /// Add or update a model's limits
    pub fn set_model_limit(&mut self, model: &str, limit: ModelLimit) {
        self.model_limits.insert(model.to_string(), limit);
    }

    /// Get the limit for a specific model, or default if not found
    pub fn get_model_limit(&self, model: &str) -> ModelLimit {
        // Try exact match first
        if let Some(limit) = self.model_limits.get(model) {
            return limit.clone();
        }

        // Try prefix match (e.g., "claude-3-opus-20240229" matches "claude-3-opus")
        for (key, limit) in &self.model_limits {
            if model.starts_with(key) {
                return limit.clone();
            }
        }

        // Return default
        ModelLimit::default()
    }

    /// Check if the session has exceeded the compaction threshold
    ///
    /// Returns true if the session's total tokens exceed the model's
    /// compaction threshold (context_limit * (1 - reserve_ratio))
    pub fn is_overflow(&self, session: &ExecutionSession) -> bool {
        let limit = self.get_model_limit(&session.model);
        session.total_tokens >= limit.compaction_threshold()
    }

    /// Estimate token count from text
    ///
    /// Uses a simple heuristic: ~0.4 tokens per character
    /// This is a rough approximation that works reasonably well for English text.
    pub fn estimate_tokens(text: &str) -> u64 {
        let chars = text.chars().count();
        // 0.4 tokens per character on average
        (chars as f64 * 0.4).ceil() as u64
    }
}

// ============================================================================
// Session Compactor
// ============================================================================

/// Session Compactor - summarizes old parts when token limit approached
///
/// This component:
/// - Subscribes to LoopContinue, ToolCallCompleted events
/// - Monitors session token usage
/// - Compacts sessions by:
///   1. Pruning old tool outputs
///   2. Generating summaries
///   3. Replacing old parts with summary
/// - Publishes SessionCompacted event when compaction occurs
pub struct SessionCompactor {
    /// Token tracker for managing limits
    token_tracker: TokenTracker,
    /// Number of recent tool calls to keep with full output
    keep_recent_tools: usize,
}

impl Default for SessionCompactor {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionCompactor {
    /// Create a new SessionCompactor with default settings
    pub fn new() -> Self {
        Self {
            token_tracker: TokenTracker::new(),
            keep_recent_tools: 10,
        }
    }

    /// Create a SessionCompactor with custom settings for recent tools to keep
    pub fn with_keep_recent(keep_recent_tools: usize) -> Self {
        Self {
            token_tracker: TokenTracker::new(),
            keep_recent_tools,
        }
    }

    /// Get token tracker reference
    pub fn token_tracker(&self) -> &TokenTracker {
        &self.token_tracker
    }

    /// Get mutable token tracker reference
    pub fn token_tracker_mut(&mut self) -> &mut TokenTracker {
        &mut self.token_tracker
    }

    // ========================================================================
    // Compaction Methods
    // ========================================================================

    /// Prune old tool outputs to save context space
    ///
    /// Keeps the most recent `keep_recent_tools` tool outputs intact,
    /// replaces older outputs with "[Output pruned to save context]"
    pub fn prune_old_tool_outputs(&self, session: &mut ExecutionSession) {
        // Count tool call parts
        let tool_call_indices: Vec<usize> = session
            .parts
            .iter()
            .enumerate()
            .filter_map(|(i, part)| {
                if matches!(part, SessionPart::ToolCall(_)) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        // If we have more than keep_recent_tools, prune the older ones
        if tool_call_indices.len() > self.keep_recent_tools {
            let prune_count = tool_call_indices.len() - self.keep_recent_tools;

            for &idx in tool_call_indices.iter().take(prune_count) {
                if let SessionPart::ToolCall(ref mut tool_call) = session.parts[idx] {
                    if tool_call.output.is_some() {
                        tool_call.output = Some("[Output pruned to save context]".to_string());
                    }
                }
            }
        }
    }

    /// Generate a summary of the session so far
    ///
    /// Creates a text summary including:
    /// - Original user request
    /// - List of completed steps
    /// - Iteration count
    ///
    /// Note: This is a stub implementation. In production, this would use
    /// an LLM to generate a more intelligent summary.
    pub fn generate_summary(&self, session: &ExecutionSession) -> String {
        let mut summary_parts = Vec::new();

        // Extract original request
        let original_request = session
            .parts
            .iter()
            .find_map(|part| {
                if let SessionPart::UserInput(input) = part {
                    Some(input.text.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "[No original request found]".to_string());

        summary_parts.push(format!("Original Request: {}", original_request));

        // Extract completed steps (tool calls)
        let completed_tools: Vec<String> = session
            .parts
            .iter()
            .filter_map(|part| {
                if let SessionPart::ToolCall(tool_call) = part {
                    if tool_call.output.is_some() && tool_call.error.is_none() {
                        Some(format!("- {}: completed", tool_call.tool_name))
                    } else if tool_call.error.is_some() {
                        Some(format!("- {}: failed", tool_call.tool_name))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        if !completed_tools.is_empty() {
            summary_parts.push("Completed Steps:".to_string());
            summary_parts.extend(completed_tools);
        }

        // Add iteration count
        summary_parts.push(format!("Iterations: {}", session.iteration_count));

        summary_parts.join("\n")
    }

    /// Replace old session parts with a summary
    ///
    /// Keeps the most recent parts (based on keep_recent_tools) and replaces
    /// older parts with a SummaryPart containing the generated summary.
    pub fn replace_with_summary(&self, session: &mut ExecutionSession, summary: String) {
        // Count how many parts we have
        let total_parts = session.parts.len();

        // Keep at least the last keep_recent_tools parts
        let keep_count = self.keep_recent_tools.min(total_parts);
        let compact_count = total_parts.saturating_sub(keep_count);

        if compact_count == 0 {
            return; // Nothing to compact
        }

        // Create summary part
        let summary_part = SessionPart::Summary(SummaryPart {
            content: summary,
            original_count: compact_count as u32,
            compacted_at: chrono::Utc::now().timestamp(),
        });

        // Remove old parts and insert summary
        let kept_parts: Vec<SessionPart> = session.parts.drain(compact_count..).collect();
        session.parts.clear();
        session.parts.push(summary_part);
        session.parts.extend(kept_parts);
    }

    /// Recalculate total token count for the session
    ///
    /// Estimates tokens for all session parts and updates the session's total_tokens
    pub fn recalculate_tokens(&self, session: &mut ExecutionSession) {
        let mut total: u64 = 0;

        for part in &session.parts {
            total += match part {
                SessionPart::UserInput(input) => {
                    let mut tokens = TokenTracker::estimate_tokens(&input.text);
                    if let Some(ref ctx) = input.context {
                        tokens += TokenTracker::estimate_tokens(ctx);
                    }
                    tokens
                }
                SessionPart::AiResponse(response) => {
                    let mut tokens = TokenTracker::estimate_tokens(&response.content);
                    if let Some(ref reasoning) = response.reasoning {
                        tokens += TokenTracker::estimate_tokens(reasoning);
                    }
                    tokens
                }
                SessionPart::ToolCall(tool_call) => {
                    let mut tokens = TokenTracker::estimate_tokens(&tool_call.tool_name);
                    tokens += TokenTracker::estimate_tokens(&tool_call.input.to_string());
                    if let Some(ref output) = tool_call.output {
                        tokens += TokenTracker::estimate_tokens(output);
                    }
                    if let Some(ref error) = tool_call.error {
                        tokens += TokenTracker::estimate_tokens(error);
                    }
                    tokens
                }
                SessionPart::Reasoning(reasoning) => {
                    TokenTracker::estimate_tokens(&reasoning.content)
                }
                SessionPart::PlanCreated(plan) => {
                    let mut tokens = TokenTracker::estimate_tokens(&plan.plan_id);
                    for step in &plan.steps {
                        tokens += TokenTracker::estimate_tokens(step);
                    }
                    tokens
                }
                SessionPart::SubAgentCall(sub_agent) => {
                    let mut tokens = TokenTracker::estimate_tokens(&sub_agent.agent_id);
                    tokens += TokenTracker::estimate_tokens(&sub_agent.prompt);
                    if let Some(ref result) = sub_agent.result {
                        tokens += TokenTracker::estimate_tokens(result);
                    }
                    tokens
                }
                SessionPart::Summary(summary) => TokenTracker::estimate_tokens(&summary.content),
            };
        }

        session.total_tokens = total;
    }

    /// Compact the session to reduce token usage
    ///
    /// Performs compaction in two stages:
    /// 1. Prune old tool outputs
    /// 2. If still overflowing, generate summary and replace old parts
    ///
    /// Returns true if compaction was performed
    pub fn compact(&self, session: &mut ExecutionSession) -> bool {
        let tokens_before = session.total_tokens;

        // Stage 1: Prune old tool outputs
        self.prune_old_tool_outputs(session);
        self.recalculate_tokens(session);

        // Check if we're still overflowing
        if self.token_tracker.is_overflow(session) {
            // Stage 2: Generate summary and replace old parts
            let summary = self.generate_summary(session);
            self.replace_with_summary(session, summary);
            self.recalculate_tokens(session);
        }

        // Return true if we actually reduced tokens
        session.total_tokens < tokens_before
    }

    /// Check if compaction is needed and perform it
    ///
    /// Returns Some(CompactionInfo) if compaction was performed, None otherwise
    pub async fn check_and_compact(
        &self,
        session: &mut ExecutionSession,
    ) -> Option<CompactionInfo> {
        // Check if we're approaching the limit
        if !self.token_tracker.is_overflow(session) {
            return None;
        }

        let tokens_before = session.total_tokens;
        let session_id = session.id.clone();

        // Mark session as compacting
        let original_status = session.status.clone();
        session.status = SessionStatus::Compacting;

        // Perform compaction
        let compacted = self.compact(session);

        // Restore status
        session.status = original_status;

        if compacted {
            Some(CompactionInfo {
                session_id,
                tokens_before,
                tokens_after: session.total_tokens,
                timestamp: chrono::Utc::now().timestamp(),
            })
        } else {
            None
        }
    }
}

// ============================================================================
// EventHandler Implementation
// ============================================================================

#[async_trait]
impl EventHandler for SessionCompactor {
    fn name(&self) -> &'static str {
        "SessionCompactor"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::ToolCallCompleted, EventType::LoopContinue]
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        // Only handle relevant events
        match event {
            AetherEvent::ToolCallCompleted(_) | AetherEvent::LoopContinue(_) => {
                // In a full implementation, we would get the session from ComponentContext
                // For now, this is a stub that demonstrates the event handling pattern
                //
                // The actual compaction logic would:
                // 1. Get session from shared state
                // 2. Check if overflow
                // 3. If overflow, compact and publish SessionCompacted
                //
                // Example (pseudo-code):
                // let session = ctx.get_session().await;
                // if let Some(compaction_info) = self.check_and_compact(&mut session).await {
                //     return Ok(vec![AetherEvent::SessionCompacted(compaction_info)]);
                // }

                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::types::{AiResponsePart, ToolCallPart, ToolCallStatus, UserInputPart};
    use serde_json::json;

    // ========================================================================
    // ModelLimit Tests
    // ========================================================================

    #[test]
    fn test_model_limit_default() {
        let limit = ModelLimit::default();

        assert_eq!(limit.context_limit, 128000);
        assert_eq!(limit.max_output_tokens, 4096);
        assert!((limit.reserve_ratio - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn test_model_limit_custom() {
        let limit = ModelLimit::new(200000, 8192, 0.3);

        assert_eq!(limit.context_limit, 200000);
        assert_eq!(limit.max_output_tokens, 8192);
        assert!((limit.reserve_ratio - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_model_limit_reserve_ratio_clamped() {
        let limit1 = ModelLimit::new(100000, 4096, 1.5);
        assert!((limit1.reserve_ratio - 1.0).abs() < f32::EPSILON);

        let limit2 = ModelLimit::new(100000, 4096, -0.5);
        assert!((limit2.reserve_ratio - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compaction_threshold() {
        let limit = ModelLimit::new(100000, 4096, 0.2);
        // 100000 * (1 - 0.2) = 80000 (allow for floating point precision)
        let threshold1 = limit.compaction_threshold();
        assert!(
            threshold1 >= 79990 && threshold1 <= 80010,
            "Expected ~80000, got {}",
            threshold1
        );

        let limit2 = ModelLimit::new(200000, 4096, 0.1);
        // 200000 * (1 - 0.1) = 180000 (allow for floating point precision)
        let threshold2 = limit2.compaction_threshold();
        assert!(
            threshold2 >= 179990 && threshold2 <= 180010,
            "Expected ~180000, got {}",
            threshold2
        );
    }

    // ========================================================================
    // TokenTracker Tests
    // ========================================================================

    #[test]
    fn test_token_tracker_default() {
        let tracker = TokenTracker::new();

        // Check preset models
        let claude_opus = tracker.get_model_limit("claude-3-opus");
        assert_eq!(claude_opus.context_limit, 200000);

        let gpt4_turbo = tracker.get_model_limit("gpt-4-turbo");
        assert_eq!(gpt4_turbo.context_limit, 128000);

        let gemini_pro = tracker.get_model_limit("gemini-pro");
        assert_eq!(gemini_pro.context_limit, 32000);
    }

    #[test]
    fn test_token_tracker_unknown_model() {
        let tracker = TokenTracker::new();

        // Unknown model should return default
        let unknown = tracker.get_model_limit("unknown-model");
        assert_eq!(unknown.context_limit, 128000); // Default
    }

    #[test]
    fn test_token_tracker_prefix_match() {
        let tracker = TokenTracker::new();

        // Should match by prefix
        let claude_versioned = tracker.get_model_limit("claude-3-opus-20240229");
        assert_eq!(claude_versioned.context_limit, 200000);
    }

    #[test]
    fn test_token_estimation() {
        // Test basic estimation
        // "Hello" = 5 chars * 0.4 = 2 tokens (ceil)
        assert_eq!(TokenTracker::estimate_tokens("Hello"), 2);

        // Empty string = 0 tokens
        assert_eq!(TokenTracker::estimate_tokens(""), 0);

        // 100 chars * 0.4 = 40 tokens
        let text = "a".repeat(100);
        assert_eq!(TokenTracker::estimate_tokens(&text), 40);

        // 250 chars * 0.4 = 100 tokens
        let longer_text = "x".repeat(250);
        assert_eq!(TokenTracker::estimate_tokens(&longer_text), 100);
    }

    #[test]
    fn test_is_overflow() {
        let tracker = TokenTracker::new();

        // Create session with tokens below threshold
        let mut session = ExecutionSession::new().with_model("gemini-pro");
        session.total_tokens = 25000; // Below 32000 * 0.8 = 25600

        assert!(!tracker.is_overflow(&session));

        // Set tokens above threshold
        session.total_tokens = 26000; // Above 25600

        assert!(tracker.is_overflow(&session));
    }

    // ========================================================================
    // SessionCompactor Tests
    // ========================================================================

    fn create_test_session() -> ExecutionSession {
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Please help me analyze this code".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add multiple tool calls
        for i in 0..15 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("tool-{}", i),
                tool_name: format!("tool_{}", i),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                output: Some(format!("Output for tool {} with some content here", i)),
                error: None,
                started_at: 1000 + i * 100,
                completed_at: Some(1050 + i * 100),
            }));
        }

        // Add AI response
        session.parts.push(SessionPart::AiResponse(AiResponsePart {
            content: "Analysis complete. The code looks good.".to_string(),
            reasoning: Some("Reviewed all components".to_string()),
            timestamp: 3000,
        }));

        session
    }

    #[test]
    fn test_session_compactor_new() {
        let compactor = SessionCompactor::new();
        assert_eq!(compactor.keep_recent_tools, 10);
    }

    #[test]
    fn test_session_compactor_with_keep_recent() {
        let compactor = SessionCompactor::with_keep_recent(5);
        assert_eq!(compactor.keep_recent_tools, 5);
    }

    #[test]
    fn test_prune_old_tool_outputs() {
        let compactor = SessionCompactor::with_keep_recent(5);
        let mut session = create_test_session();

        // We have 15 tool calls, should prune 10
        compactor.prune_old_tool_outputs(&mut session);

        // Count pruned vs non-pruned
        let (pruned, kept): (Vec<_>, Vec<_>) = session
            .parts
            .iter()
            .filter_map(|part| {
                if let SessionPart::ToolCall(tc) = part {
                    Some(tc.output.as_ref().unwrap().as_str())
                } else {
                    None
                }
            })
            .partition(|output| *output == "[Output pruned to save context]");

        assert_eq!(pruned.len(), 10);
        assert_eq!(kept.len(), 5);
    }

    #[test]
    fn test_prune_old_tool_outputs_no_pruning_needed() {
        let compactor = SessionCompactor::with_keep_recent(20);
        let mut session = create_test_session();

        // We have 15 tool calls, keep_recent is 20, so no pruning
        compactor.prune_old_tool_outputs(&mut session);

        // All outputs should be preserved
        let pruned_count = session
            .parts
            .iter()
            .filter(|part| {
                if let SessionPart::ToolCall(tc) = part {
                    tc.output.as_ref().map_or(false, |o| o.contains("pruned"))
                } else {
                    false
                }
            })
            .count();

        assert_eq!(pruned_count, 0);
    }

    #[test]
    fn test_generate_summary() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        let summary = compactor.generate_summary(&session);

        // Summary should contain original request
        assert!(summary.contains("Please help me analyze this code"));

        // Summary should mention completed steps
        assert!(summary.contains("Completed Steps"));

        // Summary should contain iteration count
        assert!(summary.contains("Iterations"));
    }

    #[test]
    fn test_generate_summary_empty_session() {
        let compactor = SessionCompactor::new();
        let session = ExecutionSession::new();

        let summary = compactor.generate_summary(&session);

        // Should handle empty session gracefully
        assert!(summary.contains("[No original request found]"));
    }

    #[test]
    fn test_replace_with_summary() {
        let compactor = SessionCompactor::with_keep_recent(5);
        let mut session = create_test_session();

        let original_count = session.parts.len();
        let summary = "Test summary content".to_string();

        compactor.replace_with_summary(&mut session, summary.clone());

        // Should have 1 summary + 5 kept parts = 6 total
        assert_eq!(session.parts.len(), 6);

        // First part should be summary
        if let SessionPart::Summary(s) = &session.parts[0] {
            assert_eq!(s.content, "Test summary content");
            assert_eq!(s.original_count as usize, original_count - 5);
        } else {
            panic!("First part should be Summary");
        }
    }

    #[test]
    fn test_recalculate_tokens() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        // Add some parts
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Hello world".to_string(), // 11 chars * 0.4 = 5 tokens (ceil)
            context: None,
            timestamp: 0,
        }));

        session.parts.push(SessionPart::AiResponse(AiResponsePart {
            content: "Hi there!".to_string(), // 9 chars * 0.4 = 4 tokens (ceil)
            reasoning: None,
            timestamp: 0,
        }));

        compactor.recalculate_tokens(&mut session);

        // Total should be approximately 5 + 4 = 9 tokens
        assert!(session.total_tokens > 0);
        assert!(session.total_tokens < 20); // Reasonable bounds
    }

    #[test]
    fn test_compact_reduces_tokens() {
        let compactor = SessionCompactor::with_keep_recent(3);
        let mut session = create_test_session();

        // First calculate current tokens
        compactor.recalculate_tokens(&mut session);
        let before = session.total_tokens;

        // Perform compaction
        let compacted = compactor.compact(&mut session);

        assert!(compacted);
        assert!(session.total_tokens < before);
    }

    // ========================================================================
    // EventHandler Tests
    // ========================================================================

    #[test]
    fn test_handler_name() {
        let compactor = SessionCompactor::new();
        assert_eq!(compactor.name(), "SessionCompactor");
    }

    #[test]
    fn test_handler_subscriptions() {
        let compactor = SessionCompactor::new();
        let subs = compactor.subscriptions();

        assert_eq!(subs.len(), 2);
        assert!(subs.contains(&EventType::ToolCallCompleted));
        assert!(subs.contains(&EventType::LoopContinue));
    }

    #[tokio::test]
    async fn test_handler_ignores_other_events() {
        use crate::event::{EventBus, InputEvent};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // InputReceived event should be ignored
        let event = AetherEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        });

        let result = compactor.handle(&event, &ctx).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_handler_handles_tool_call_completed() {
        use crate::event::{EventBus, TokenUsage, ToolCallResult};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let result_event = ToolCallResult {
            call_id: "test-call".to_string(),
            tool: "search".to_string(),
            input: json!({}),
            output: "results".to_string(),
            started_at: 1000,
            completed_at: 2000,
            token_usage: TokenUsage::default(),
        };

        let event = AetherEvent::ToolCallCompleted(result_event);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // In the stub implementation, this returns empty
        // In full implementation, it would check overflow and potentially compact
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_handler_handles_loop_continue() {
        use crate::event::{EventBus, LoopState};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let loop_state = LoopState {
            session_id: "test-session".to_string(),
            iteration: 5,
            total_tokens: 10000,
            last_tool: Some("search".to_string()),
        };

        let event = AetherEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // In the stub implementation, this returns empty
        assert!(result.is_empty());
    }

    // ========================================================================
    // Integration Tests
    // ========================================================================

    #[tokio::test]
    async fn test_check_and_compact_no_overflow() {
        let compactor = SessionCompactor::new();
        let mut session = create_test_session();
        session.total_tokens = 1000; // Well below threshold

        let result = compactor.check_and_compact(&mut session).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_check_and_compact_overflow() {
        let compactor = SessionCompactor::with_keep_recent(3);
        let mut session = create_test_session();

        // Set tokens above threshold for gpt-4-turbo (128000 * 0.8 = 102400)
        session.total_tokens = 110000;

        // First calculate actual tokens
        compactor.recalculate_tokens(&mut session);

        // Manually set high token count to trigger compaction
        session.total_tokens = 110000;

        let result = compactor.check_and_compact(&mut session).await;

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.tokens_before, 110000);
        assert!(info.tokens_after < info.tokens_before);
    }
}
