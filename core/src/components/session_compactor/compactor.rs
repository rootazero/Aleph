//! SessionCompactor implementation - core compaction logic

use crate::components::types::{
    CompactionMarker, ExecutionSession, SessionPart, SessionStatus, SummaryPart,
};
use crate::event::CompactionInfo;

use super::config::{CompactionConfig, LlmCallback, PruneInfo, COMPACTION_PROMPT};
use super::model_limits::TokenTracker;

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
    pub(crate) keep_recent_tools: usize,
    /// Compaction configuration
    config: CompactionConfig,
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
            config: CompactionConfig::default(),
        }
    }

    /// Create a SessionCompactor with custom settings for recent tools to keep
    pub fn with_keep_recent(keep_recent_tools: usize) -> Self {
        Self {
            token_tracker: TokenTracker::new(),
            keep_recent_tools,
            config: CompactionConfig::default(),
        }
    }

    /// Create a SessionCompactor with custom configuration
    pub fn with_config(config: CompactionConfig) -> Self {
        Self {
            token_tracker: TokenTracker::new(),
            keep_recent_tools: 10,
            config,
        }
    }

    /// Get the compaction configuration
    pub fn config(&self) -> &CompactionConfig {
        &self.config
    }

    /// Get mutable compaction configuration
    pub fn config_mut(&mut self) -> &mut CompactionConfig {
        &mut self.config
    }

    /// Get token tracker reference
    pub fn token_tracker(&self) -> &TokenTracker {
        &self.token_tracker
    }

    /// Get mutable token tracker reference
    pub fn token_tracker_mut(&mut self) -> &mut TokenTracker {
        &mut self.token_tracker
    }

    /// Check if the given token usage exceeds the model's compaction threshold
    ///
    /// This method provides a convenient way to check for overflow without
    /// needing a full ExecutionSession, useful when handling LoopContinue events.
    ///
    /// # Arguments
    /// * `total_tokens` - The current total token count
    /// * `model` - The model identifier to look up context limits
    ///
    /// # Returns
    /// `true` if total_tokens >= model's compaction threshold
    pub fn is_overflow_for_model(&self, total_tokens: u64, model: &str) -> bool {
        let limit = self.token_tracker.get_model_limit(model);
        total_tokens >= limit.compaction_threshold()
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
                    // Skip protected tools
                    if self.config.protected_tools.contains(&tool_call.tool_name) {
                        continue;
                    }
                    if tool_call.output.is_some() {
                        tool_call.output = Some("[Output pruned to save context]".to_string());
                    }
                }
            }
        }
    }

    /// Check if a tool is protected from pruning
    pub(crate) fn is_protected_tool(&self, tool_name: &str) -> bool {
        self.config.protected_tools.iter().any(|t| t == tool_name)
    }

    /// Smart pruning with thresholds (OpenCode-style algorithm)
    ///
    /// Algorithm:
    /// 1. Iterate backward through parts from most recent
    /// 2. Skip until reaching 2+ user turns (to preserve recent context)
    /// 3. Find completed tool calls (excluding protected tools)
    /// 4. Accumulate tool outputs until exceeding prune_protect threshold
    /// 5. If total accumulated tokens > prune_minimum, mark outputs as pruned
    ///
    /// Returns PruneInfo with statistics about the pruning operation
    pub fn prune_with_thresholds(&self, session: &mut ExecutionSession) -> PruneInfo {
        if !self.config.prune_enabled {
            return PruneInfo::default();
        }

        let mut prune_info = PruneInfo::default();

        // Collect tool call information: (index, tool_name, tokens, is_protected)
        let mut tool_info: Vec<(usize, String, u64, bool)> = Vec::new();

        for (idx, part) in session.parts.iter().enumerate() {
            if let SessionPart::ToolCall(tc) = part {
                if tc.output.is_some() && !tc.output.as_ref().unwrap().contains("[Output pruned") {
                    let output_tokens = tc.output.as_ref()
                        .map(|o| TokenTracker::estimate_tokens(o))
                        .unwrap_or(0);
                    let is_protected = self.is_protected_tool(&tc.tool_name);
                    tool_info.push((idx, tc.tool_name.clone(), output_tokens, is_protected));
                }
            }
        }

        if tool_info.is_empty() {
            return prune_info;
        }

        // Count user turns to determine safe pruning boundary
        let user_turns: Vec<usize> = session.parts.iter().enumerate()
            .filter_map(|(i, p)| {
                if matches!(p, SessionPart::UserInput(_)) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        // Determine the safe boundary - skip until we've passed 2 user turns from the end
        let safe_boundary = if user_turns.len() >= 2 {
            user_turns[user_turns.len() - 2]
        } else {
            0
        };

        // Iterate backward through tool calls
        let mut protected_tokens: u64 = 0;
        let mut tokens_to_prune: u64 = 0;
        let mut indices_to_prune: Vec<usize> = Vec::new();

        // Process tool info from most recent to oldest
        for (idx, _tool_name, tokens, is_protected) in tool_info.iter().rev() {
            // Skip recent tools (after safe boundary) to protect them
            if *idx >= safe_boundary {
                protected_tokens += tokens;
                if *is_protected {
                    prune_info.parts_protected += 1;
                }
                continue;
            }

            if *is_protected {
                // Protected tools should never be pruned
                prune_info.parts_protected += 1;
                protected_tokens += tokens;
                continue;
            }

            // Check if we're still in the protected window
            if protected_tokens < self.config.prune_protect {
                protected_tokens += tokens;
                continue;
            }

            // Beyond protected window - candidate for pruning
            tokens_to_prune += tokens;
            indices_to_prune.push(*idx);
        }

        // Only prune if we exceed the minimum threshold
        if tokens_to_prune >= self.config.prune_minimum {
            for idx in indices_to_prune {
                if let SessionPart::ToolCall(ref mut tc) = session.parts[idx] {
                    tc.output = Some(format!(
                        "[Output pruned to save context - compacted at {}]",
                        chrono::Utc::now().timestamp()
                    ));
                    prune_info.parts_pruned += 1;
                }
            }
            prune_info.tokens_pruned = tokens_to_prune;
        }

        prune_info
    }

    /// Build context string from session for summarization
    ///
    /// This method extracts all relevant session parts and formats them
    /// into a single context string suitable for LLM summarization.
    /// Tool outputs longer than 200 characters are truncated.
    ///
    /// # Arguments
    /// * `session` - The execution session to build context from
    ///
    /// # Returns
    /// A formatted string containing all session parts
    pub fn build_summary_context(&self, session: &ExecutionSession) -> String {
        let mut context_parts = Vec::new();

        for part in &session.parts {
            match part {
                SessionPart::UserInput(input) => {
                    context_parts.push(format!("User: {}", input.text));
                }
                SessionPart::AiResponse(response) => {
                    context_parts.push(format!("Assistant: {}", response.content));
                }
                SessionPart::ToolCall(tc) => {
                    let status = if tc.error.is_some() { "failed" } else { "completed" };
                    let output_preview = tc.output.as_ref().map(|o| {
                        if o.len() > 200 {
                            let end = (0..=200).rev().find(|&i| o.is_char_boundary(i)).unwrap_or(0);
                            format!("{}...", &o[..end])
                        } else {
                            o.clone()
                        }
                    }).unwrap_or_default();
                    context_parts.push(format!("Tool {}: {} ({})", tc.tool_name, status, output_preview));
                }
                SessionPart::Summary(s) => {
                    context_parts.push(format!("[Previous Summary]: {}", s.content));
                }
                SessionPart::Reasoning(r) => {
                    context_parts.push(format!("[Reasoning]: {}", r.content));
                }
                SessionPart::PlanCreated(p) => {
                    let steps = p.steps.iter().map(|s| s.description.as_str()).collect::<Vec<_>>().join(", ");
                    context_parts.push(format!("[Plan Created]: {} - Steps: {}", p.plan_id, steps));
                }
                SessionPart::SubAgentCall(sa) => {
                    let result = sa.result.as_ref().map(|r| {
                        if r.len() > 200 {
                            let end = (0..=200).rev().find(|&i| r.is_char_boundary(i)).unwrap_or(0);
                            format!("{}...", &r[..end])
                        } else {
                            r.clone()
                        }
                    }).unwrap_or_else(|| "[pending]".to_string());
                    context_parts.push(format!("[SubAgent {}]: {} -> {}", sa.agent_id, sa.prompt, result));
                }
                SessionPart::CompactionMarker(m) => {
                    let trigger = if m.auto { "auto" } else { "manual" };
                    context_parts.push(format!("[Compaction Marker]: {} ({})", m.timestamp, trigger));
                }
                SessionPart::SystemReminder(r) => {
                    context_parts.push(format!("[System Reminder]: {}", r.content));
                }
                // Step boundaries provide execution tracking context
                SessionPart::StepStart(s) => {
                    context_parts.push(format!("[Step Start]: step {} at {}", s.step_id, s.timestamp));
                }
                SessionPart::StepFinish(s) => {
                    context_parts.push(format!("[Step Finish]: step {} - {:?} ({}ms)",
                        s.step_id, s.reason, s.duration_ms));
                }
                // Snapshots and patches are metadata for session revert
                SessionPart::Snapshot(s) => {
                    context_parts.push(format!("[Snapshot]: {} ({} files)",
                        s.snapshot_id, s.files.len()));
                }
                SessionPart::Patch(p) => {
                    context_parts.push(format!("[Patch]: {} based on {} ({} changes)",
                        p.patch_id, p.base_snapshot_id, p.changes.len()));
                }
                // Streaming text is for UI, include final content if complete
                SessionPart::StreamingText(t) => {
                    if t.is_complete && !t.content.is_empty() {
                        let preview = if t.content.len() > 200 {
                            let end = (0..=200).rev().find(|&i| t.content.is_char_boundary(i)).unwrap_or(0);
                            format!("{}...", &t.content[..end])
                        } else {
                            t.content.clone()
                        };
                        context_parts.push(format!("[Streaming]: {}", preview));
                    }
                }
            }
        }

        context_parts.join("\n\n")
    }

    /// Generate LLM-driven summary of the session
    ///
    /// This method uses an LLM to generate a comprehensive summary of the
    /// session context. If no LLM callback is provided, it falls back to
    /// the template-based `generate_summary()` method.
    ///
    /// # Arguments
    /// * `session` - The execution session to summarize
    /// * `llm_callback` - Optional callback function to invoke the LLM
    ///
    /// # Returns
    /// A summary string generated by the LLM or template
    pub async fn generate_llm_summary(
        &self,
        session: &ExecutionSession,
        llm_callback: Option<&LlmCallback>,
    ) -> String {
        // Build context from session
        let context = self.build_summary_context(session);

        // If no LLM callback provided, fall back to template-based summary
        let Some(callback) = llm_callback else {
            return self.generate_summary(session);
        };

        // Call LLM with compaction prompt
        let system_prompt = COMPACTION_PROMPT.to_string();
        let user_content = format!(
            "Here is the conversation to summarize:\n\n{}\n\nPlease provide a summary for continuing this conversation.",
            context
        );

        match callback(system_prompt, user_content).await {
            Ok(summary) => summary,
            Err(e) => {
                // Log error and fall back to template-based summary
                tracing::warn!("LLM summary generation failed: {}, falling back to template", e);
                self.generate_summary(session)
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
    /// Note: This is a template-based implementation. For LLM-driven
    /// summaries, use `generate_llm_summary()` instead.
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
                        tokens += TokenTracker::estimate_tokens(&step.description);
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
                SessionPart::CompactionMarker(_) => 0, // Markers don't consume tokens
                SessionPart::SystemReminder(reminder) => TokenTracker::estimate_tokens(&reminder.content),
                // Step boundaries are minimal metadata, estimate fixed overhead
                SessionPart::StepStart(_) => 10,
                SessionPart::StepFinish(_) => 15,
                // Snapshots list file paths and hashes
                SessionPart::Snapshot(s) => {
                    let mut tokens = TokenTracker::estimate_tokens(&s.snapshot_id);
                    for file in &s.files {
                        tokens += TokenTracker::estimate_tokens(&file.path);
                        tokens += TokenTracker::estimate_tokens(&file.hash);
                    }
                    tokens
                }
                // Patches list changes
                SessionPart::Patch(p) => {
                    let mut tokens = TokenTracker::estimate_tokens(&p.patch_id);
                    tokens += TokenTracker::estimate_tokens(&p.base_snapshot_id);
                    for change in &p.changes {
                        tokens += TokenTracker::estimate_tokens(&change.path);
                        if let Some(ref hash) = change.content_hash {
                            tokens += TokenTracker::estimate_tokens(hash);
                        }
                    }
                    tokens
                }
                // Streaming text contains the full content
                SessionPart::StreamingText(t) => TokenTracker::estimate_tokens(&t.content),
            };
        }

        session.total_tokens = total;
    }

    // ========================================================================
    // Filter Compacted Methods (Context Windowing)
    // ========================================================================

    /// Filter session parts to only include those after the last compaction boundary
    ///
    /// This matches OpenCode's filterCompacted() function:
    /// - Iterates backward through messages
    /// - Finds completed summary messages (compacted_at > 0)
    /// - Breaks at compaction markers
    /// - Returns only parts after the boundary
    ///
    /// This creates natural breakpoints at summary points, discarding old context
    /// that has already been summarized into the Summary part.
    ///
    /// # Arguments
    /// * `session` - The execution session to filter
    ///
    /// # Returns
    /// A Vec of SessionParts containing only parts after the last compaction boundary
    pub fn filter_compacted(&self, session: &ExecutionSession) -> Vec<SessionPart> {
        let mut result: Vec<SessionPart> = Vec::new();
        let mut found_completed_summary = false;

        // Iterate backward to find the boundary
        for part in session.parts.iter().rev() {
            match part {
                SessionPart::Summary(s) if s.compacted_at > 0 => {
                    // Found a completed summary - this is after a compaction
                    found_completed_summary = true;
                    result.push(part.clone());
                }
                SessionPart::CompactionMarker(_) if found_completed_summary => {
                    // Found the compaction marker after seeing its summary
                    // This is the boundary - stop collecting
                    break;
                }
                _ => {
                    result.push(part.clone());
                }
            }
        }

        // Reverse to get chronological order
        result.reverse();
        result
    }

    /// Get messages for model, respecting compaction boundaries
    ///
    /// Creates a filtered copy of the session containing only parts after
    /// the last compaction boundary. This is useful for sending to the LLM
    /// to avoid exceeding context limits while preserving relevant context.
    ///
    /// # Arguments
    /// * `session` - The execution session to filter
    ///
    /// # Returns
    /// A new ExecutionSession with filtered parts
    pub fn get_filtered_session(&self, session: &ExecutionSession) -> ExecutionSession {
        let mut filtered = session.clone();
        filtered.parts = self.filter_compacted(session);
        filtered
    }

    /// Insert a compaction marker before performing compaction
    ///
    /// This should be called before replace_with_summary to create
    /// a boundary that filter_compacted can detect.
    ///
    /// # Arguments
    /// * `session` - The execution session to mark
    /// * `auto` - Whether this is automatic (true) or user-triggered (false)
    pub fn insert_compaction_marker(&self, session: &mut ExecutionSession, auto: bool) {
        let marker = CompactionMarker::new(auto);
        session.parts.push(SessionPart::CompactionMarker(marker));
    }

    // ========================================================================
    // Core Compaction Methods
    // ========================================================================

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
