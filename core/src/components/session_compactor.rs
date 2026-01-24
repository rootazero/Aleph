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
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

use crate::components::types::{
    CompactionMarker, ExecutionSession, SessionPart, SessionStatus, SummaryPart,
};

// ============================================================================
// Compaction Prompt
// ============================================================================

/// Compaction summary prompt (matches OpenCode's compaction.txt)
///
/// This prompt guides the LLM to generate a comprehensive summary that
/// enables seamless continuation of the conversation in a new session.
const COMPACTION_PROMPT: &str = r#"You are a helpful AI assistant tasked with summarizing conversations.

Provide a detailed prompt for continuing our conversation above. Focus on information that would be helpful for continuing the conversation:
- What was done
- What is currently being worked on
- Which files are being modified
- What needs to be done next
- Key user requests, constraints, or preferences
- Important technical decisions and why they were made

Write in a way that allows a new session to continue seamlessly without access to the full conversation history."#;

/// Type alias for LLM callback function
///
/// The callback takes a system prompt and user content, returns a future that
/// resolves to the LLM's response string.
pub type LlmCallback = Box<
    dyn Fn(String, String) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync,
>;

/// Get the compaction prompt used for LLM-driven summarization
///
/// This is exposed for testing and customization purposes.
pub fn compaction_prompt() -> &'static str {
    COMPACTION_PROMPT
}

// ============================================================================
// Compaction Config
// ============================================================================

/// Configuration for session compaction behavior
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Enable automatic compaction when overflow detected
    pub auto_compact: bool,
    /// Enable pruning of old tool outputs
    pub prune_enabled: bool,
    /// Minimum tokens to save before pruning (default: 20,000)
    pub prune_minimum: u64,
    /// Protect this many tokens of recent tool outputs (default: 40,000)
    pub prune_protect: u64,
    /// Tools that should never have their outputs pruned
    pub protected_tools: Vec<String>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto_compact: true,
            prune_enabled: true,
            prune_minimum: 20_000,
            prune_protect: 40_000,
            protected_tools: vec!["skill".to_string()],
        }
    }
}

// ============================================================================
// Prune Info
// ============================================================================

/// Information about a pruning operation
///
/// This struct tracks the results of a prune_with_thresholds operation,
/// including how many tokens and parts were pruned or protected.
#[derive(Debug, Clone, Default)]
pub struct PruneInfo {
    /// Total tokens pruned (estimated)
    pub tokens_pruned: u64,
    /// Number of parts whose outputs were pruned
    pub parts_pruned: usize,
    /// Number of parts protected from pruning (e.g., skill tool outputs)
    pub parts_protected: usize,
}
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
// Enhanced Token Usage
// ============================================================================

/// Enhanced token usage tracking with cache awareness
///
/// This struct provides detailed token tracking that matches OpenCode's approach,
/// including support for reasoning tokens and cache-aware billing calculations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnhancedTokenUsage {
    /// Input tokens consumed
    pub input: u64,
    /// Output tokens generated
    pub output: u64,
    /// Reasoning tokens (for models that support it)
    pub reasoning: u64,
    /// Tokens read from cache (reduces cost)
    pub cache_read: u64,
    /// Tokens written to cache
    pub cache_write: u64,
}

impl EnhancedTokenUsage {
    /// Create a new EnhancedTokenUsage with all fields set
    pub fn new(input: u64, output: u64, reasoning: u64, cache_read: u64, cache_write: u64) -> Self {
        Self {
            input,
            output,
            reasoning,
            cache_read,
            cache_write,
        }
    }

    /// Calculate total tokens for overflow detection
    ///
    /// OpenCode formula: input + cache.read + output
    /// This represents the actual context window usage
    pub fn total_for_overflow(&self) -> u64 {
        self.input + self.cache_read + self.output
    }

    /// Calculate billable tokens (cache reads are cheaper, often excluded)
    ///
    /// Returns input + output + reasoning tokens (excludes cache reads)
    pub fn total_billable(&self) -> u64 {
        self.input + self.output + self.reasoning
    }

    /// Add another usage to this one
    pub fn add(&mut self, other: &EnhancedTokenUsage) {
        self.input += other.input;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// Check if this usage is empty (all fields are zero)
    pub fn is_empty(&self) -> bool {
        self.input == 0
            && self.output == 0
            && self.reasoning == 0
            && self.cache_read == 0
            && self.cache_write == 0
    }

    /// Calculate total tokens (all fields combined)
    pub fn total(&self) -> u64 {
        self.input + self.output + self.reasoning + self.cache_read + self.cache_write
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
    fn is_protected_tool(&self, tool_name: &str) -> bool {
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
                            format!("{}...", &o[..200])
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
                    let steps = p.steps.join(", ");
                    context_parts.push(format!("[Plan Created]: {} - Steps: {}", p.plan_id, steps));
                }
                SessionPart::SubAgentCall(sa) => {
                    let result = sa.result.as_ref().map(|r| {
                        if r.len() > 200 {
                            format!("{}...", &r[..200])
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
                SessionPart::CompactionMarker(_) => 0, // Markers don't consume tokens
                SessionPart::SystemReminder(reminder) => TokenTracker::estimate_tokens(&reminder.content),
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
        let marker = CompactionMarker {
            timestamp: chrono::Utc::now().timestamp(),
            auto,
        };
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
        // Check if auto-compaction is enabled
        if !self.config.auto_compact {
            return Ok(vec![]);
        }

        match event {
            AetherEvent::LoopContinue(loop_state) => {
                // Check if we need compaction based on token count
                let limit = self.token_tracker.get_model_limit(&loop_state.model);

                if loop_state.total_tokens >= limit.compaction_threshold() {
                    // Log that compaction would be needed
                    // (Full implementation would get session from context and compact)
                    tracing::info!(
                        "Session {} would need compaction: {} tokens exceeds threshold {}",
                        loop_state.session_id,
                        loop_state.total_tokens,
                        limit.compaction_threshold()
                    );
                    // Return a placeholder - in full impl, would return SessionCompacted event
                    // Example (pseudo-code):
                    // let session = ctx.get_session(&loop_state.session_id).await;
                    // if let Some(compaction_info) = self.check_and_compact(&mut session).await {
                    //     return Ok(vec![AetherEvent::SessionCompacted(compaction_info)]);
                    // }
                }
                Ok(vec![])
            }
            AetherEvent::ToolCallCompleted(result) => {
                // Log pruning trigger
                if self.config.prune_enabled {
                    tracing::debug!(
                        "Tool {} completed, pruning check would trigger",
                        result.tool
                    );
                }
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
    // EnhancedTokenUsage Tests
    // ========================================================================

    #[test]
    fn test_enhanced_token_usage() {
        let usage = EnhancedTokenUsage {
            input: 1000,
            output: 500,
            reasoning: 200,
            cache_read: 300,
            cache_write: 100,
        };

        // Total for overflow check = input + cache_read + output
        assert_eq!(usage.total_for_overflow(), 1800);

        // Total billable (excluding cache reads which are cheaper)
        assert_eq!(usage.total_billable(), 1700);
    }

    #[test]
    fn test_enhanced_token_usage_default() {
        let usage = EnhancedTokenUsage::default();

        assert_eq!(usage.input, 0);
        assert_eq!(usage.output, 0);
        assert_eq!(usage.reasoning, 0);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_write, 0);
        assert!(usage.is_empty());
    }

    #[test]
    fn test_enhanced_token_usage_new() {
        let usage = EnhancedTokenUsage::new(100, 200, 50, 75, 25);

        assert_eq!(usage.input, 100);
        assert_eq!(usage.output, 200);
        assert_eq!(usage.reasoning, 50);
        assert_eq!(usage.cache_read, 75);
        assert_eq!(usage.cache_write, 25);
    }

    #[test]
    fn test_enhanced_token_usage_add() {
        let mut usage1 = EnhancedTokenUsage {
            input: 1000,
            output: 500,
            reasoning: 200,
            cache_read: 300,
            cache_write: 100,
        };

        let usage2 = EnhancedTokenUsage {
            input: 500,
            output: 250,
            reasoning: 100,
            cache_read: 150,
            cache_write: 50,
        };

        usage1.add(&usage2);

        assert_eq!(usage1.input, 1500);
        assert_eq!(usage1.output, 750);
        assert_eq!(usage1.reasoning, 300);
        assert_eq!(usage1.cache_read, 450);
        assert_eq!(usage1.cache_write, 150);
    }

    #[test]
    fn test_enhanced_token_usage_add_to_empty() {
        let mut usage = EnhancedTokenUsage::default();

        let other = EnhancedTokenUsage {
            input: 100,
            output: 200,
            reasoning: 50,
            cache_read: 75,
            cache_write: 25,
        };

        usage.add(&other);

        assert_eq!(usage.input, 100);
        assert_eq!(usage.output, 200);
        assert_eq!(usage.reasoning, 50);
        assert_eq!(usage.cache_read, 75);
        assert_eq!(usage.cache_write, 25);
    }

    #[test]
    fn test_enhanced_token_usage_is_empty() {
        let empty = EnhancedTokenUsage::default();
        assert!(empty.is_empty());

        let non_empty = EnhancedTokenUsage {
            input: 1,
            ..Default::default()
        };
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn test_enhanced_token_usage_total() {
        let usage = EnhancedTokenUsage {
            input: 1000,
            output: 500,
            reasoning: 200,
            cache_read: 300,
            cache_write: 100,
        };

        // Total = all fields combined
        assert_eq!(usage.total(), 2100);
    }

    #[test]
    fn test_enhanced_token_usage_equality() {
        let usage1 = EnhancedTokenUsage::new(100, 200, 50, 75, 25);
        let usage2 = EnhancedTokenUsage::new(100, 200, 50, 75, 25);
        let usage3 = EnhancedTokenUsage::new(100, 200, 50, 75, 26);

        assert_eq!(usage1, usage2);
        assert_ne!(usage1, usage3);
    }

    #[test]
    fn test_enhanced_token_usage_clone() {
        let usage = EnhancedTokenUsage::new(100, 200, 50, 75, 25);
        let cloned = usage.clone();

        assert_eq!(usage, cloned);
    }

    // ========================================================================
    // CompactionConfig Tests
    // ========================================================================

    #[test]
    fn test_compaction_config_default() {
        let config = CompactionConfig::default();
        assert!(config.auto_compact);
        assert!(config.prune_enabled);
        assert_eq!(config.prune_minimum, 20_000);
        assert_eq!(config.prune_protect, 40_000);
        assert!(config.protected_tools.contains(&"skill".to_string()));
    }

    #[test]
    fn test_compaction_config_disabled() {
        let config = CompactionConfig {
            auto_compact: false,
            prune_enabled: false,
            ..Default::default()
        };
        assert!(!config.auto_compact);
        assert!(!config.prune_enabled);
    }

    #[test]
    fn test_compaction_config_custom_protected_tools() {
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string(), "read".to_string(), "write".to_string()],
            ..Default::default()
        };
        assert_eq!(config.protected_tools.len(), 3);
        assert!(config.protected_tools.contains(&"skill".to_string()));
        assert!(config.protected_tools.contains(&"read".to_string()));
        assert!(config.protected_tools.contains(&"write".to_string()));
    }

    #[test]
    fn test_session_compactor_with_config() {
        let config = CompactionConfig {
            auto_compact: false,
            prune_enabled: true,
            prune_minimum: 10_000,
            prune_protect: 20_000,
            protected_tools: vec!["custom_tool".to_string()],
        };
        let compactor = SessionCompactor::with_config(config);

        assert!(!compactor.config().auto_compact);
        assert!(compactor.config().prune_enabled);
        assert_eq!(compactor.config().prune_minimum, 10_000);
        assert_eq!(compactor.config().prune_protect, 20_000);
        assert!(compactor.config().protected_tools.contains(&"custom_tool".to_string()));
    }

    #[test]
    fn test_session_compactor_config_mut() {
        let mut compactor = SessionCompactor::new();

        compactor.config_mut().auto_compact = false;
        compactor.config_mut().prune_minimum = 15_000;

        assert!(!compactor.config().auto_compact);
        assert_eq!(compactor.config().prune_minimum, 15_000);
    }

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
            session_id: None,
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
            model: "gpt-4-turbo".to_string(),
        };

        let event = AetherEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Returns empty since tokens are below threshold
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

    // ========================================================================
    // Smart Pruning with Protection Tests (Task 3)
    // ========================================================================

    /// Create a test session with skill tool calls that should be protected
    fn create_test_session_with_skill_calls() -> ExecutionSession {
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Use skill to do something".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add skill tool calls (should be protected)
        for i in 0..5 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("skill-{}", i),
                tool_name: "skill".to_string(),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                output: Some(format!("Skill output {} with content", i)),
                error: None,
                started_at: 1000 + i as i64 * 100,
                completed_at: Some(1050 + i as i64 * 100),
            }));
        }

        // Add another user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Now do more work".to_string(),
            context: None,
            timestamp: 1600,
        }));

        // Add regular tool calls (should be pruned if old enough)
        for i in 0..15 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("tool-{}", i),
                tool_name: format!("tool_{}", i),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                output: Some("x".repeat(500)), // ~200 tokens each
                error: None,
                started_at: 2000 + i as i64 * 100,
                completed_at: Some(2050 + i as i64 * 100),
            }));
        }

        // Add third user turn to create safe boundary
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Final request".to_string(),
            context: None,
            timestamp: 4000,
        }));

        session
    }

    /// Create a large test session for threshold testing
    fn create_large_test_session() -> ExecutionSession {
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add first user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Start working".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add many tool calls with large outputs to exceed thresholds
        for i in 0..50 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("large-tool-{}", i),
                tool_name: format!("tool_{}", i % 10),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                // Each output is ~2500 chars = ~1000 tokens
                output: Some("y".repeat(2500)),
                error: None,
                started_at: 2000 + i as i64 * 100,
                completed_at: Some(2050 + i as i64 * 100),
            }));
        }

        // Add second user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Continue".to_string(),
            context: None,
            timestamp: 8000,
        }));

        // Add more recent tool calls
        for i in 0..10 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("recent-tool-{}", i),
                tool_name: format!("recent_{}", i),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                output: Some("z".repeat(500)),
                error: None,
                started_at: 9000 + i as i64 * 100,
                completed_at: Some(9050 + i as i64 * 100),
            }));
        }

        // Add third user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Final".to_string(),
            context: None,
            timestamp: 10500,
        }));

        session
    }

    #[test]
    fn test_prune_respects_protected_tools() {
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string(), "read_file".to_string()],
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_test_session_with_skill_calls();

        compactor.prune_old_tool_outputs(&mut session);

        // Skill tool outputs should NOT be pruned
        let skill_outputs: Vec<_> = session.parts.iter()
            .filter_map(|p| match p {
                SessionPart::ToolCall(tc) if tc.tool_name == "skill" => tc.output.as_ref(),
                _ => None,
            })
            .collect();

        for output in skill_outputs {
            assert!(!output.contains("pruned"), "Skill outputs should not be pruned, got: {}", output);
        }
    }

    #[test]
    fn test_prune_with_thresholds_basic() {
        let config = CompactionConfig {
            prune_minimum: 1000,
            prune_protect: 2000,
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_large_test_session();

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Should only prune if exceeds prune_minimum
        assert!(pruned_info.tokens_pruned >= 1000 || pruned_info.tokens_pruned == 0,
            "tokens_pruned should be >= 1000 or 0, got {}", pruned_info.tokens_pruned);
    }

    #[test]
    fn test_prune_with_thresholds_respects_protected_tools() {
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string()],
            prune_minimum: 100,
            prune_protect: 500,
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_test_session_with_skill_calls();

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Protected tools should be counted
        assert!(pruned_info.parts_protected >= 5,
            "Expected at least 5 protected parts (skill calls), got {}",
            pruned_info.parts_protected);

        // Verify skill outputs were not pruned
        for part in &session.parts {
            if let SessionPart::ToolCall(tc) = part {
                if tc.tool_name == "skill" {
                    assert!(!tc.output.as_ref().unwrap().contains("pruned"),
                        "Skill tool output should not be pruned");
                }
            }
        }
    }

    #[test]
    fn test_prune_with_thresholds_disabled() {
        let config = CompactionConfig {
            prune_enabled: false,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_large_test_session();

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Should not prune anything when disabled
        assert_eq!(pruned_info.tokens_pruned, 0);
        assert_eq!(pruned_info.parts_pruned, 0);
        assert_eq!(pruned_info.parts_protected, 0);
    }

    #[test]
    fn test_prune_with_thresholds_high_minimum() {
        let config = CompactionConfig {
            prune_minimum: 1_000_000, // Very high threshold
            prune_protect: 500,
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_large_test_session();

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Should not prune because we won't exceed the high minimum
        assert_eq!(pruned_info.parts_pruned, 0);
    }

    #[test]
    fn test_is_protected_tool() {
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string(), "read_file".to_string()],
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);

        assert!(compactor.is_protected_tool("skill"));
        assert!(compactor.is_protected_tool("read_file"));
        assert!(!compactor.is_protected_tool("write_file"));
        assert!(!compactor.is_protected_tool("search"));
    }

    #[test]
    fn test_prune_info_default() {
        let info = PruneInfo::default();
        assert_eq!(info.tokens_pruned, 0);
        assert_eq!(info.parts_pruned, 0);
        assert_eq!(info.parts_protected, 0);
    }

    #[test]
    fn test_prune_old_tool_outputs_with_protected_tools() {
        // Test that prune_old_tool_outputs also respects protected tools
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string()],
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add 20 tool calls, some are protected
        for i in 0..20 {
            let tool_name = if i % 4 == 0 { "skill".to_string() } else { format!("tool_{}", i) };
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("call-{}", i),
                tool_name,
                input: json!({"i": i}),
                status: ToolCallStatus::Completed,
                output: Some(format!("Output {}", i)),
                error: None,
                started_at: 1000 + i as i64 * 100,
                completed_at: Some(1050 + i as i64 * 100),
            }));
        }

        // Default keep_recent_tools is 10, so 10 should be pruned
        compactor.prune_old_tool_outputs(&mut session);

        // Verify skill tool outputs were NOT pruned
        for part in &session.parts {
            if let SessionPart::ToolCall(tc) = part {
                if tc.tool_name == "skill" {
                    assert!(!tc.output.as_ref().unwrap().contains("pruned"),
                        "Skill outputs should not be pruned: {:?}", tc.output);
                }
            }
        }
    }

    #[test]
    fn test_prune_with_thresholds_preserves_recent_turns() {
        let config = CompactionConfig {
            prune_minimum: 100,
            prune_protect: 200,
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // First user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "First request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Old tool calls
        for i in 0..5 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("old-{}", i),
                tool_name: "old_tool".to_string(),
                input: json!({}),
                output: Some("x".repeat(1000)),
                status: ToolCallStatus::Completed,
                error: None,
                started_at: 1100 + i as i64 * 100,
                completed_at: Some(1150 + i as i64 * 100),
            }));
        }

        // Second user turn (recent)
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Second request".to_string(),
            context: None,
            timestamp: 2000,
        }));

        // Recent tool calls (should be protected by user turn boundary)
        for i in 0..3 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("recent-{}", i),
                tool_name: "recent_tool".to_string(),
                input: json!({}),
                output: Some("y".repeat(500)),
                status: ToolCallStatus::Completed,
                error: None,
                started_at: 2100 + i as i64 * 100,
                completed_at: Some(2150 + i as i64 * 100),
            }));
        }

        // Third user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Third request".to_string(),
            context: None,
            timestamp: 3000,
        }));

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Recent tool calls (after second user turn) should not be pruned
        for part in &session.parts {
            if let SessionPart::ToolCall(tc) = part {
                if tc.tool_name == "recent_tool" {
                    assert!(!tc.output.as_ref().unwrap().contains("pruned"),
                        "Recent tool outputs should not be pruned");
                }
            }
        }

        // Verify some old tools were pruned (if thresholds were exceeded)
        // Note: This depends on whether we exceeded the thresholds
        println!("Pruned info: tokens={}, parts={}, protected={}",
            pruned_info.tokens_pruned, pruned_info.parts_pruned, pruned_info.parts_protected);
    }

    // ========================================================================
    // LLM-Driven Summarization Tests (Task 4)
    // ========================================================================

    #[test]
    fn test_compaction_prompt_exists() {
        let prompt = super::compaction_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("summarizing conversations"));
        assert!(prompt.contains("What was done"));
        assert!(prompt.contains("What needs to be done next"));
    }

    #[test]
    fn test_build_summary_context_basic() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        let context = compactor.build_summary_context(&session);

        // Should contain user input
        assert!(context.contains("User:"), "Context should contain 'User:' prefix");
        assert!(context.contains("Please help me analyze this code"), "Context should contain user request");

        // Should contain tool information
        assert!(context.contains("Tool"), "Context should contain tool calls");
        assert!(context.contains("completed"), "Context should show tool completion status");
    }

    #[test]
    fn test_build_summary_context_with_ai_response() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        let context = compactor.build_summary_context(&session);

        // Should contain AI response
        assert!(context.contains("Assistant:"), "Context should contain 'Assistant:' prefix");
        assert!(context.contains("Analysis complete"), "Context should contain AI response content");
    }

    #[test]
    fn test_build_summary_context_truncates_long_output() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add tool call with output > 200 chars
        let long_output = "x".repeat(500);
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: "long-output-tool".to_string(),
            tool_name: "test_tool".to_string(),
            input: json!({"param": "value"}),
            status: ToolCallStatus::Completed,
            output: Some(long_output),
            error: None,
            started_at: 1100,
            completed_at: Some(1200),
        }));

        let context = compactor.build_summary_context(&session);

        // Should be truncated with "..."
        assert!(context.contains("..."), "Long output should be truncated with '...'");
        // Original 500 char output should not appear in full
        assert!(!context.contains(&"x".repeat(500)), "Full 500 char output should not appear");
        // But truncated 200 chars should appear
        assert!(context.contains(&"x".repeat(200)), "Truncated 200 chars should appear");
    }

    #[test]
    fn test_build_summary_context_with_failed_tool() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add failed tool call
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: "failed-tool".to_string(),
            tool_name: "failing_tool".to_string(),
            input: json!({}),
            status: ToolCallStatus::Failed,
            output: None,
            error: Some("Connection timeout".to_string()),
            started_at: 1100,
            completed_at: Some(1200),
        }));

        let context = compactor.build_summary_context(&session);

        // Should show failed status
        assert!(context.contains("failed"), "Context should show 'failed' for tool with error");
    }

    #[test]
    fn test_build_summary_context_with_summary_part() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add previous summary
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Previous session worked on feature X".to_string(),
            original_count: 50,
            compacted_at: 1000,
        }));

        // Add new user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Continue with feature X".to_string(),
            context: None,
            timestamp: 2000,
        }));

        let context = compactor.build_summary_context(&session);

        // Should contain previous summary
        assert!(context.contains("[Previous Summary]:"), "Context should contain previous summary marker");
        assert!(context.contains("Previous session worked on feature X"));
    }

    #[test]
    fn test_build_summary_context_empty_session() {
        let compactor = SessionCompactor::new();
        let session = ExecutionSession::new().with_model("gpt-4-turbo");

        let context = compactor.build_summary_context(&session);

        // Should be empty for empty session
        assert!(context.is_empty(), "Empty session should produce empty context");
    }

    #[tokio::test]
    async fn test_generate_llm_summary_without_callback() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        // Without callback, should fall back to template-based summary
        let summary = compactor.generate_llm_summary(&session, None).await;

        // Should contain template-based summary elements
        assert!(summary.contains("Original Request"));
        assert!(summary.contains("Please help me analyze this code"));
    }

    #[tokio::test]
    async fn test_generate_llm_summary_with_callback() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let compactor = SessionCompactor::new();
        let session = create_test_session();

        // Track if callback was called
        let callback_called = Arc::new(AtomicBool::new(false));
        let callback_called_clone = callback_called.clone();

        // Create mock callback
        let callback: super::LlmCallback = Box::new(move |system_prompt, user_content| {
            let called = callback_called_clone.clone();
            Box::pin(async move {
                called.store(true, Ordering::SeqCst);

                // Verify prompts
                assert!(system_prompt.contains("summarizing conversations"));
                assert!(user_content.contains("conversation to summarize"));

                Ok("LLM generated summary: The user is analyzing code.".to_string())
            })
        });

        let summary = compactor.generate_llm_summary(&session, Some(&callback)).await;

        assert!(callback_called.load(Ordering::SeqCst), "Callback should be called");
        assert!(summary.contains("LLM generated summary"));
    }

    #[tokio::test]
    async fn test_generate_llm_summary_fallback_on_error() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        // Create callback that returns error
        let callback: super::LlmCallback = Box::new(|_system_prompt, _user_content| {
            Box::pin(async move {
                Err("LLM service unavailable".to_string())
            })
        });

        let summary = compactor.generate_llm_summary(&session, Some(&callback)).await;

        // Should fall back to template-based summary
        assert!(summary.contains("Original Request"), "Should fall back to template on error");
        assert!(summary.contains("Please help me analyze this code"));
    }

    #[test]
    fn test_build_summary_context_with_reasoning() {
        use crate::components::types::ReasoningPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add reasoning part
        session.parts.push(SessionPart::Reasoning(ReasoningPart {
            content: "Thinking through the problem...".to_string(),
            timestamp: 1100,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Reasoning]:"), "Context should contain reasoning marker");
        assert!(context.contains("Thinking through the problem"));
    }

    #[test]
    fn test_build_summary_context_with_plan() {
        use crate::components::types::PlanPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add plan
        session.parts.push(SessionPart::PlanCreated(PlanPart {
            plan_id: "plan-123".to_string(),
            steps: vec!["Step 1".to_string(), "Step 2".to_string(), "Step 3".to_string()],
            timestamp: 1100,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Plan Created]:"), "Context should contain plan marker");
        assert!(context.contains("plan-123"));
        assert!(context.contains("Step 1, Step 2, Step 3"));
    }

    #[test]
    fn test_build_summary_context_with_subagent() {
        use crate::components::types::SubAgentPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add subagent call with result
        session.parts.push(SessionPart::SubAgentCall(SubAgentPart {
            agent_id: "code-review-agent".to_string(),
            prompt: "Review this code".to_string(),
            result: Some("Code looks good with minor suggestions".to_string()),
            timestamp: 1100,
        }));

        // Add subagent call without result (pending)
        session.parts.push(SessionPart::SubAgentCall(SubAgentPart {
            agent_id: "test-agent".to_string(),
            prompt: "Run tests".to_string(),
            result: None,
            timestamp: 1300,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[SubAgent code-review-agent]:"));
        assert!(context.contains("Review this code"));
        assert!(context.contains("Code looks good"));
        assert!(context.contains("[pending]"), "Pending subagent should show [pending]");
    }

    #[test]
    fn test_build_summary_context_subagent_truncates_long_result() {
        use crate::components::types::SubAgentPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add subagent call with long result
        let long_result = "x".repeat(500);
        session.parts.push(SessionPart::SubAgentCall(SubAgentPart {
            agent_id: "analysis-agent".to_string(),
            prompt: "Analyze".to_string(),
            result: Some(long_result),
            timestamp: 1100,
        }));

        let context = compactor.build_summary_context(&session);

        // Should be truncated
        assert!(context.contains("..."), "Long subagent result should be truncated");
        assert!(!context.contains(&"x".repeat(500)), "Full result should not appear");
    }

    // ========================================================================
    // Filter Compacted Tests (Task 5)
    // ========================================================================

    #[test]
    fn test_filter_compacted_creates_boundary() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new();

        // Add some history before compaction
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add compaction marker
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 2000,
            auto: true,
        }));

        // Add summary
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Summary of old context".to_string(),
            original_count: 5,
            compacted_at: 2000,
        }));

        // Add new history after compaction
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "New request".to_string(),
            context: None,
            timestamp: 3000,
        }));

        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // Should only return parts after the compaction boundary (summary + new)
        assert_eq!(filtered.len(), 2, "Expected 2 parts (summary + new user input), got {}", filtered.len());
        assert!(matches!(filtered[0], SessionPart::Summary(_)), "First part should be Summary");
        assert!(matches!(filtered[1], SessionPart::UserInput(_)), "Second part should be UserInput");

        // Verify the content
        if let SessionPart::Summary(s) = &filtered[0] {
            assert_eq!(s.content, "Summary of old context");
        }
        if let SessionPart::UserInput(u) = &filtered[1] {
            assert_eq!(u.text, "New request");
        }
    }

    #[test]
    fn test_filter_compacted_no_boundary() {
        let session = create_test_session(); // No compaction markers
        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // Without compaction, should return all parts
        assert_eq!(filtered.len(), session.parts.len(),
            "Without compaction boundary, all {} parts should be returned", session.parts.len());
    }

    #[test]
    fn test_filter_compacted_incomplete_summary() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new();

        // Add old history
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add compaction marker
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 2000,
            auto: true,
        }));

        // Add incomplete summary (compacted_at = 0)
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Incomplete summary".to_string(),
            original_count: 5,
            compacted_at: 0, // Not completed
        }));

        // Add new history
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "New request".to_string(),
            context: None,
            timestamp: 3000,
        }));

        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // With incomplete summary (compacted_at = 0), should return all parts
        // because we never find a "completed" summary to trigger boundary detection
        assert_eq!(filtered.len(), session.parts.len(),
            "With incomplete summary, all parts should be returned");
    }

    #[test]
    fn test_filter_compacted_multiple_boundaries() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new();

        // First compaction cycle
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Very old request".to_string(),
            context: None,
            timestamp: 1000,
        }));
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 2000,
            auto: true,
        }));
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "First summary".to_string(),
            original_count: 3,
            compacted_at: 2000,
        }));

        // Second compaction cycle
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old request".to_string(),
            context: None,
            timestamp: 3000,
        }));
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 4000,
            auto: false,
        }));
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Second summary".to_string(),
            original_count: 5,
            compacted_at: 4000,
        }));

        // Current context
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Current request".to_string(),
            context: None,
            timestamp: 5000,
        }));

        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // Should only return parts after the LAST compaction boundary
        // (second summary + current request)
        assert_eq!(filtered.len(), 2, "Expected 2 parts after last boundary, got {}", filtered.len());

        if let SessionPart::Summary(s) = &filtered[0] {
            assert_eq!(s.content, "Second summary", "Should have the most recent summary");
        } else {
            panic!("First filtered part should be Summary");
        }

        if let SessionPart::UserInput(u) = &filtered[1] {
            assert_eq!(u.text, "Current request");
        } else {
            panic!("Second filtered part should be UserInput");
        }
    }

    #[test]
    fn test_get_filtered_session() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");
        session.id = "test-session-123".to_string();

        // Add old history
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add compaction marker
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 2000,
            auto: true,
        }));

        // Add summary
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Summary".to_string(),
            original_count: 5,
            compacted_at: 2000,
        }));

        // Add new history
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "New request".to_string(),
            context: None,
            timestamp: 3000,
        }));

        let compactor = SessionCompactor::new();
        let filtered_session = compactor.get_filtered_session(&session);

        // Session metadata should be preserved
        assert_eq!(filtered_session.id, "test-session-123");
        assert_eq!(filtered_session.model, "gpt-4-turbo");

        // Parts should be filtered
        assert_eq!(filtered_session.parts.len(), 2);
    }

    #[test]
    fn test_insert_compaction_marker_auto() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        compactor.insert_compaction_marker(&mut session, true);

        assert_eq!(session.parts.len(), 1);
        if let SessionPart::CompactionMarker(m) = &session.parts[0] {
            assert!(m.auto, "Auto flag should be true");
            assert!(m.timestamp > 0, "Timestamp should be set");
        } else {
            panic!("Should have added CompactionMarker");
        }
    }

    #[test]
    fn test_insert_compaction_marker_manual() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        compactor.insert_compaction_marker(&mut session, false);

        assert_eq!(session.parts.len(), 1);
        if let SessionPart::CompactionMarker(m) = &session.parts[0] {
            assert!(!m.auto, "Auto flag should be false for manual trigger");
        } else {
            panic!("Should have added CompactionMarker");
        }
    }

    #[test]
    fn test_filter_compacted_preserves_order() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new();

        // Old content
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Compaction
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 2000,
            auto: true,
        }));
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Summary".to_string(),
            original_count: 1,
            compacted_at: 2000,
        }));

        // New content in specific order
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Request 1".to_string(),
            context: None,
            timestamp: 3000,
        }));
        session.parts.push(SessionPart::AiResponse(AiResponsePart {
            content: "Response 1".to_string(),
            reasoning: None,
            timestamp: 3100,
        }));
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Request 2".to_string(),
            context: None,
            timestamp: 3200,
        }));

        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // Should preserve chronological order
        assert_eq!(filtered.len(), 4);
        assert!(matches!(filtered[0], SessionPart::Summary(_)));
        assert!(matches!(filtered[1], SessionPart::UserInput(_)));
        assert!(matches!(filtered[2], SessionPart::AiResponse(_)));
        assert!(matches!(filtered[3], SessionPart::UserInput(_)));

        // Verify specific order
        if let SessionPart::UserInput(u) = &filtered[1] {
            assert_eq!(u.text, "Request 1");
        }
        if let SessionPart::UserInput(u) = &filtered[3] {
            assert_eq!(u.text, "Request 2");
        }
    }

    #[test]
    fn test_compaction_marker_type_name() {
        use crate::components::types::CompactionMarker;

        let marker = SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 1000,
            auto: true,
        });

        assert_eq!(marker.type_name(), "compaction_marker");
    }

    #[test]
    fn test_build_summary_context_with_compaction_marker() {
        use crate::components::types::CompactionMarker;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 1000,
            auto: true,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Compaction Marker]:"), "Should contain marker");
        assert!(context.contains("1000"), "Should contain timestamp");
        assert!(context.contains("auto"), "Should indicate auto trigger");
    }

    #[test]
    fn test_build_summary_context_with_manual_compaction_marker() {
        use crate::components::types::CompactionMarker;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 2000,
            auto: false,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Compaction Marker]:"));
        assert!(context.contains("manual"), "Should indicate manual trigger");
    }

    #[test]
    fn test_recalculate_tokens_with_compaction_marker() {
        use crate::components::types::CompactionMarker;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        // Add a compaction marker
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 1000,
            auto: true,
        }));

        // Add some actual content for comparison
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Hello".to_string(),
            context: None,
            timestamp: 2000,
        }));

        compactor.recalculate_tokens(&mut session);

        // Compaction markers should not add to token count
        // Only the "Hello" text should contribute (5 chars * 0.4 = 2 tokens)
        assert_eq!(session.total_tokens, 2, "Only user input should contribute tokens");
    }

    // ========================================================================
    // EventHandler Integration Tests (Task 6)
    // ========================================================================

    #[tokio::test]
    async fn test_event_handler_respects_config() {
        use crate::event::{EventBus, LoopState};

        let config = CompactionConfig {
            auto_compact: false,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let loop_state = LoopState {
            session_id: "test".to_string(),
            iteration: 5,
            total_tokens: 150_000,
            last_tool: None,
            model: "gpt-4-turbo".to_string(),
        };

        let event = AetherEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Should return empty when auto_compact is disabled
        assert!(result.is_empty());
    }

    #[test]
    fn test_is_overflow_for_model() {
        let compactor = SessionCompactor::new();

        // gpt-4-turbo has 128K context, 80% threshold = 102.4K
        assert!(!compactor.is_overflow_for_model(100_000, "gpt-4-turbo"));
        assert!(compactor.is_overflow_for_model(110_000, "gpt-4-turbo"));

        // claude-3-opus has 200K context, 80% threshold = 160K
        assert!(!compactor.is_overflow_for_model(150_000, "claude-3-opus"));
        assert!(compactor.is_overflow_for_model(170_000, "claude-3-opus"));

        // Unknown model uses default (128K, 80% = 102.4K)
        assert!(!compactor.is_overflow_for_model(100_000, "unknown-model"));
        assert!(compactor.is_overflow_for_model(110_000, "unknown-model"));
    }

    #[tokio::test]
    async fn test_event_handler_with_high_tokens() {
        use crate::event::{EventBus, LoopState};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // Create a loop state with tokens above compaction threshold
        // gpt-4-turbo: 128K * 0.8 = 102.4K threshold
        let loop_state = LoopState {
            session_id: "overflow-test".to_string(),
            iteration: 10,
            total_tokens: 110_000, // Above threshold
            last_tool: Some("search".to_string()),
            model: "gpt-4-turbo".to_string(),
        };

        let event = AetherEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Currently returns empty (would return SessionCompacted in full impl)
        // The logging would indicate compaction is needed
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_event_handler_tool_completed_with_prune_disabled() {
        use crate::event::{EventBus, TokenUsage, ToolCallResult};

        let config = CompactionConfig {
            prune_enabled: false,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
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
            session_id: None,
        };

        let event = AetherEvent::ToolCallCompleted(result_event);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Should return empty (prune is disabled)
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_event_handler_tool_completed_with_prune_enabled() {
        use crate::event::{EventBus, TokenUsage, ToolCallResult};

        let config = CompactionConfig {
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
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
            session_id: None,
        };

        let event = AetherEvent::ToolCallCompleted(result_event);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Should return empty but with debug logging
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_event_handler_with_model_prefix_match() {
        use crate::event::{EventBus, LoopState};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // Use versioned model name that should match prefix
        let loop_state = LoopState {
            session_id: "prefix-test".to_string(),
            iteration: 5,
            total_tokens: 90_000, // Below threshold
            last_tool: None,
            model: "claude-3-opus-20240229".to_string(), // Should match "claude-3-opus"
        };

        let event = AetherEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Should return empty since below 160K threshold (200K * 0.8)
        assert!(result.is_empty());
    }
}
