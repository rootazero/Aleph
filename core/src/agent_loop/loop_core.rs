//! AgentLoop — the core think → act two-step loop.
//!
//! This is the heart of the agent architecture. Each iteration:
//! 1. **Think**: Call the AI provider with the conversation history
//! 2. **Act**: Execute any tool calls the provider requested
//!
//! The loop terminates when:
//! - The provider returns text with `EndTurn` (task complete)
//! - `max_iterations` is reached
//! - Token budget is exhausted

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::context_budget::diagnostics::ContextDiagnostics;
use super::context_budget::pipeline::{
    CompactionPipeline, ImageStripper, MicroCompact, RoundDrop, ToolCompactStage,
};
use super::context_budget::pressure::PressureSensor;
use super::prompt_builder::{PromptBuilder, ToolInfo};
use super::safety::{SafetyError, SafetyGuard};
use super::tool::{LoopToolRegistry, ToolDefinition, ToolResult};
use crate::providers::adapter::StopReason;
use crate::providers::delta::{DeltaCollector, DeltaSink, NoopSink, ProviderDelta};
use crate::providers::message::UnifiedMessage;
use futures::stream::BoxStream;

// =============================================================================
// Context limit enforcement
// =============================================================================

const CRITICAL_CONTEXT_NOTICE: &str =
    "[SYSTEM] Context window is critically full. You MUST respond directly to the user now. \
     Do NOT call any tools. Summarize your progress and provide the best answer you can \
     with the information you have.";

const DIMINISHING_RETURNS_NOTICE: &str =
    "[SYSTEM] Your recent iterations have produced minimal progress. Summarize: \
     (1) what you accomplished, (2) what you tried that didn't work, \
     (3) what the user should do next. Then stop.";

const TRUNCATION_NOTICE: &str =
    "[SYSTEM] Earlier conversation history and memory context were truncated \
     to fit the model's context window. Continue based on the remaining context.";

// =============================================================================
// 413 Prompt-Too-Long (PTL) recovery constants
// =============================================================================

/// Maximum number of retry attempts after receiving a 413 error.
const MAX_PTL_RETRIES: usize = 3;
/// Safety margin multiplier applied to the token gap when calculating how many
/// groups to drop (e.g. 1.2 = drop 20% more than strictly needed).
const PTL_SAFETY_MARGIN: f64 = 1.2;
/// When the token gap is unknown, drop this fraction of droppable groups.
const PTL_FALLBACK_DROP_RATIO: f64 = 0.20;
/// Marker inserted at the beginning of the conversation after truncation.
const PTL_TRUNCATION_MARKER: &str = "[earlier conversation truncated for recovery]";

// =============================================================================
// 413 emergency truncation helpers
// =============================================================================

/// Group messages by API round: each group is (user → assistant [→ tool_results]*).
/// Returns Vec of (start_index, end_index_exclusive) pairs.
fn group_by_round(messages: &[UnifiedMessage]) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut start = 0;
    for (i, msg) in messages.iter().enumerate() {
        if i > 0 && msg.is_user() && !msg.is_tool_result() {
            groups.push((start, i));
            start = i;
        }
    }
    if start < messages.len() {
        groups.push((start, messages.len()));
    }
    groups
}

/// Emergency truncation for 413 recovery.
/// Drops oldest message groups to free tokens. Protects the last
/// `fresh_tail_count` groups.
fn emergency_truncate(
    messages: &mut Vec<UnifiedMessage>,
    token_gap: Option<usize>,
    fresh_tail_count: usize,
) {
    use super::context_budget::pressure::estimate_tokens_smart;

    let groups = group_by_round(messages);
    if groups.len() <= fresh_tail_count + 1 {
        return; // Not enough groups to drop
    }

    let droppable_count = groups.len().saturating_sub(fresh_tail_count);
    if droppable_count == 0 {
        return;
    }

    let groups_to_drop = if let Some(gap) = token_gap {
        let target = (gap as f64 * PTL_SAFETY_MARGIN) as usize;
        let mut freed = 0usize;
        let mut count = 0usize;
        for &(start, end) in &groups[..droppable_count] {
            if freed >= target {
                break;
            }
            for msg in &messages[start..end] {
                freed += estimate_tokens_smart(&msg.text_content());
            }
            count += 1;
        }
        count.max(1)
    } else {
        ((droppable_count as f64 * PTL_FALLBACK_DROP_RATIO).ceil() as usize).max(1)
    };

    let groups_to_drop = groups_to_drop.min(droppable_count);
    let drop_end = groups[groups_to_drop - 1].1;

    messages.drain(..drop_end);
    messages.insert(0, UnifiedMessage::user(PTL_TRUNCATION_MARKER));
}

/// Find a safe cut point that doesn't split ToolCall/ToolResult pairs.
fn find_safe_cut_point(messages: &[UnifiedMessage], initial_cut: usize) -> usize {
    let mut cut = initial_cut;
    while cut > 0 {
        if messages[cut].is_tool_result() {
            cut -= 1;
        } else {
            break;
        }
    }
    cut
}

/// Remove the oldest complete conversation round after the truncation notice.
fn remove_oldest_complete_round(messages: &mut Vec<UnifiedMessage>) {
    if messages.len() <= 2 {
        return;
    }

    if messages[1].is_assistant() && messages[1].has_tool_calls() {
        let mut end = 2;
        while end < messages.len() && messages[end].is_tool_result() {
            end += 1;
        }
        messages.drain(1..end);
    } else {
        messages.remove(1);
    }
}

/// Hard safety net: truncate message history if total estimated tokens exceed budget.
///
/// This runs after the soft compactor and is the last line of defense before
/// the LLM call. If context is still over budget, it aggressively drops old
/// messages (session summaries, old turns) while keeping the fresh tail.
///
/// **Philosophy**: keep the agent running > preserve history.
/// **Invariant**: never orphans ToolCall/ToolResult pairs.
fn enforce_context_limit(
    messages: &mut Vec<UnifiedMessage>,
    system_prompt: &str,
    tool_defs: &[ToolDefinition],
    token_budget: usize,
    fresh_tail_count: usize,
    ratio: f64,
) {
    use crate::memory::session_compactor::context_window::{
        estimate_tokens, estimate_total_tokens,
    };

    let prompt_tokens = estimate_tokens(system_prompt, ratio);
    let tool_tokens: usize = tool_defs
        .iter()
        .map(|td| {
            estimate_tokens(&td.name, ratio)
                + estimate_tokens(&td.description, ratio)
                + estimate_tokens(&td.parameters.to_string(), ratio)
        })
        .sum();
    let overhead = prompt_tokens + tool_tokens;
    let msg_budget = token_budget.saturating_sub(overhead);
    let msg_tokens = estimate_total_tokens(messages, ratio);

    if msg_tokens <= msg_budget {
        return;
    }

    tracing::warn!(
        target: "agent_loop",
        msg_tokens, msg_budget, overhead,
        total = msg_tokens + overhead,
        budget = token_budget,
        "Context exceeds budget after compaction — enforcing hard limit"
    );

    // Phase 1: Find safe cut point at round boundary
    let tail_start = messages.len().saturating_sub(fresh_tail_count);
    let cut = find_safe_cut_point(messages, tail_start);

    if cut > 0 {
        messages.drain(0..cut);
        messages.insert(0, UnifiedMessage::user(TRUNCATION_NOTICE));
    }

    // Phase 2: If still over budget, remove oldest complete rounds one by one
    while messages.len() > 2 && estimate_total_tokens(messages, ratio) > msg_budget {
        remove_oldest_complete_round(messages);
    }

    let final_tokens = estimate_total_tokens(messages, ratio);
    tracing::warn!(
        target: "agent_loop",
        remaining_messages = messages.len(),
        final_tokens, msg_budget,
        "Context limit enforced (pair-aware)"
    );
}

// =============================================================================
// LoopProvider trait
// =============================================================================

/// Abstraction over AI provider for testability.
///
/// Implementations translate `UnifiedMessage` history into provider-specific
/// API calls and return a delta stream. Callers accumulate the stream via
/// `DeltaCollector` to reconstruct a `ProviderResponse`.
#[async_trait]
pub trait LoopProvider: Send + Sync {
    async fn stream(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>>;

    /// Maximum output tokens this provider supports.
    fn max_output_tokens(&self) -> u32 {
        16_384
    }
}

// =============================================================================
// LoopConfig
// =============================================================================

/// Loop configuration — guards against runaway loops.
pub struct LoopConfig {
    pub max_iterations: usize,
    pub token_budget: usize,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 200,
            token_budget: 100_000,
        }
    }
}

// =============================================================================
// LoopRunResult
// =============================================================================

/// Result of a loop run.
#[derive(Debug)]
pub struct LoopRunResult {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub total_tokens: usize,
    pub hit_limit: bool,
    pub cancelled: bool,
}

// =============================================================================
// Helpers
// =============================================================================

/// Strip intermediate text that the LLM repeated at the start of its final response.
///
/// When the LLM produces intermediate messages (text + tool_calls), it sees them
/// in its conversation history. It often repeats these messages verbatim at the
/// start of its final response. This function strips those known prefixes so
/// channel deliveries (Telegram, etc.) don't duplicate content.
fn strip_repeated_intermediate(text: &str, intermediates: &[String]) -> String {
    if intermediates.is_empty() {
        return text.to_string();
    }
    let mut remaining = text.trim_start();
    let mut stripped_any = false;
    for intermediate in intermediates {
        let trimmed = intermediate.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = remaining.strip_prefix(trimmed) {
            remaining = rest.trim_start();
            stripped_any = true;
        } else {
            break;
        }
    }
    if stripped_any {
        remaining.to_string()
    } else {
        text.to_string()
    }
}

// =============================================================================
// LoopCallback
// =============================================================================

/// Callback for streaming events during the loop.
pub trait LoopCallback: Send {
    fn on_text(&mut self, _text: &str) {}
    /// Called when the LLM produces text alongside tool calls (intermediate progress).
    /// This text should be delivered to the user immediately, not buffered.
    fn on_intermediate_text(&mut self, _text: &str) {}
    fn on_tool_start(&mut self, _name: &str, _input: &Value) {}
    fn on_tool_done(&mut self, _name: &str, _result: &ToolResult) {}
    fn on_safety_block(&mut self, _error: &SafetyError) {}
    fn on_model_fallback(&mut self, _reason: &str, _fallback_model: &str) {}
}

/// No-op callback for when you don't need events.
pub(crate) struct NoopCallback;
impl LoopCallback for NoopCallback {}

// =============================================================================
// AgentLoop
// =============================================================================

/// The core agent loop: think → act, repeated until done.
pub struct AgentLoop<P: LoopProvider> {
    provider: P,
    fallback_provider: Option<Box<dyn LoopProvider>>,
    fallback_label: Option<String>,
    tool_registry: Arc<LoopToolRegistry>,
    prompt_builder: PromptBuilder,
    safety_guard: Arc<SafetyGuard>,
    config: LoopConfig,
    /// Optional context budget for pressure sensing and budget tracking.
    /// Wrapped in `Mutex` for interior mutability — `run_with_history_messages`
    /// takes `&self` but the budget needs mutable state across turns.
    context_budget: Mutex<Option<super::context_budget::ContextBudget>>,
    /// Pressure sensor anchored to API-reported token usage.
    pressure_sensor: Mutex<PressureSensor>,
    /// Pipeline of compaction stages (image strip → micro compact → tool compact → round drop).
    compaction_pipeline: CompactionPipeline,
    /// Diagnostics collector for pipeline run history.
    diagnostics: Mutex<ContextDiagnostics>,
    /// Truncation recovery state machine — handles `MaxTokens` stop reason by
    /// escalating token limits, generating continuation prompts, and assembling
    /// fragmented outputs. Wrapped in `Mutex` for interior mutability.
    truncation_recovery: Mutex<super::truncation_recovery::TruncationRecovery>,
    /// Optional LLM-based context compactor for elevated pressure.
    context_compactor: Option<super::context_compactor::ContextCompactor>,
    /// Sink for streaming deltas during the Think step. Defaults to NoopSink.
    delta_sink: Box<dyn DeltaSink>,
    /// Token for cooperative cancellation of streaming and tool execution.
    cancel_token: CancellationToken,
}

impl<P: LoopProvider> AgentLoop<P> {
    /// Create a new agent loop with all dependencies injected.
    ///
    /// `delta_sink` defaults to `NoopSink` — call `with_delta_sink()` to attach a real sink.
    pub fn new(
        provider: P,
        tool_registry: LoopToolRegistry,
        prompt_builder: PromptBuilder,
        safety_guard: SafetyGuard,
        config: LoopConfig,
        cancel_token: CancellationToken,
    ) -> Self {
        let pipeline = CompactionPipeline::new(vec![
            Box::new(ImageStripper),
            Box::new(MicroCompact),
            Box::new(ToolCompactStage {
                token_budget: config.token_budget as u64,
                threshold: 0.70,
                ratio: 3.5,
            }),
            Box::new(RoundDrop {
                token_budget: config.token_budget as u64,
                ratio: 3.5,
            }),
        ]);
        let provider_max = provider.max_output_tokens();
        Self {
            provider,
            fallback_provider: None,
            fallback_label: None,
            tool_registry: Arc::new(tool_registry),
            prompt_builder,
            safety_guard: Arc::new(safety_guard),
            config,
            context_budget: Mutex::new(None),
            pressure_sensor: Mutex::new(PressureSensor::new(3.5)),
            compaction_pipeline: pipeline,
            diagnostics: Mutex::new(ContextDiagnostics::new()),
            context_compactor: None,
            truncation_recovery: Mutex::new(
                super::truncation_recovery::TruncationRecovery::new(provider_max),
            ),
            delta_sink: Box::new(NoopSink),
            cancel_token,
        }
    }

    /// Attach a [`ContextBudget`](super::context_budget::ContextBudget) for pressure sensing and budget tracking.
    pub fn with_context_budget(self, budget: Option<super::context_budget::ContextBudget>) -> Self {
        *self
            .context_budget
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = budget;
        self
    }

    /// Attach a [`ContextCompactor`](super::context_compactor::ContextCompactor) for LLM-based compaction
    /// at elevated context pressure.
    pub fn with_context_compactor(mut self, compactor: super::context_compactor::ContextCompactor) -> Self {
        self.context_compactor = Some(compactor);
        self
    }

    /// Attach a `DeltaSink` to observe streaming deltas during each Think step.
    ///
    /// This replaces the default `NoopSink`. Used to forward real-time text tokens
    /// to WebSocket clients or other reactive consumers.
    pub fn with_delta_sink(mut self, sink: Box<dyn DeltaSink>) -> Self {
        self.delta_sink = sink;
        self
    }

    /// Attach a fallback provider for automatic model switching.
    ///
    /// When the primary model is unavailable (overloaded, auth failure,
    /// not found), the loop automatically switches to this fallback.
    pub fn with_fallback(
        mut self,
        provider: Box<dyn LoopProvider>,
        label: impl Into<String>,
    ) -> Self {
        self.fallback_provider = Some(provider);
        self.fallback_label = Some(label.into());
        self
    }

    /// Get tool definitions from the registry (for inspection/testing).
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tool_registry.tool_definitions()
    }

    /// Run the agent loop with the given user input.
    pub async fn run(
        &self,
        input: &str,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<LoopRunResult> {
        self.run_with_history(input, Vec::new(), callback).await
    }

    /// Run the agent loop with conversation history prepended.
    pub async fn run_with_history(
        &self,
        input: &str,
        history: Vec<UnifiedMessage>,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<LoopRunResult> {
        let mut messages = history;
        messages.push(UnifiedMessage::user(input));
        self.run_with_history_messages(messages, callback).await
    }

    /// Run with pre-built messages (multimodal support).
    ///
    /// Unlike `run_with_history`, the caller is responsible for constructing
    /// the final user message (e.g. with `UnifiedMessage::user_with_content`
    /// for multimodal content blocks). This method does not append any
    /// additional user message.
    pub async fn run_with_history_messages(
        &self,
        messages: Vec<UnifiedMessage>,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<LoopRunResult> {
        // Build system prompt with tool info
        let tool_infos: Vec<ToolInfo> = self
            .tool_registry
            .tool_definitions()
            .iter()
            .map(|td| ToolInfo {
                name: td.name.clone(),
                description: td.description.clone(),
                parameters_schema: Some(td.parameters.clone()),
            })
            .collect();
        let system_prompt = self.prompt_builder.build(&tool_infos, None);

        // Get tool definitions for the provider
        let tool_defs = self.tool_registry.tool_definitions();

        let mut messages = messages;

        let mut final_text: Option<String> = None;
        let mut intermediate_texts: Vec<String> = Vec::new();
        let mut iterations: usize = 0;
        let mut tool_calls_made: usize = 0;
        let mut total_tokens: usize = 0;
        let mut hit_limit = false;
        let mut stop_requested = false;
        let mut consecutive_errors: usize = 0;
        let mut completion_nudge_count: usize = 0;
        let mut current_max_tokens: Option<u32> = None;
        const MAX_CONSECUTIVE_ERRORS: usize = 10;
        const MAX_COMPLETION_NUDGES: usize = 3;

        // === THE LOOP ===
        while iterations < self.config.max_iterations {
            iterations += 1;

            // --- Context budget evaluation (single lock scope) ---
            let mut budget_directive = super::context_budget::LoopDirective::Continue;
            let (budget_fresh_tail, budget_ratio) = {
                let mut ctx_budget_ref = self
                    .context_budget
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(ref mut ctx_budget) = *ctx_budget_ref {
                    budget_directive =
                        ctx_budget.before_turn(&messages, &system_prompt, &tool_defs);

                    match budget_directive {
                        super::context_budget::LoopDirective::CompactAndContinue => {
                            let result = {
                                let sensor = self
                                    .pressure_sensor
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                self.compaction_pipeline.run(
                                    &mut messages,
                                    &sensor,
                                    &system_prompt,
                                    &tool_defs,
                                    ctx_budget.token_budget(),
                                    ctx_budget.warning_threshold(),
                                    ctx_budget.fresh_tail_count(),
                                )
                            };
                            if result.pressure_after.ratio < ctx_budget.warning_threshold()
                                || result.tokens_freed > 500
                            {
                                ctx_budget.notify_compaction_success();
                            }
                            self.diagnostics
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .record_pipeline(result);
                        }
                        // NOTE: LLM-based compaction is handled below, outside this lock scope.
                        super::context_budget::LoopDirective::FinalReply => {
                            let result = {
                                let sensor = self
                                    .pressure_sensor
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                self.compaction_pipeline.run(
                                    &mut messages,
                                    &sensor,
                                    &system_prompt,
                                    &tool_defs,
                                    ctx_budget.token_budget(),
                                    0.5,
                                    ctx_budget.fresh_tail_count(),
                                )
                            };
                            self.diagnostics
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .record_pipeline(result);
                            messages.push(UnifiedMessage::user(CRITICAL_CONTEXT_NOTICE));
                        }
                        super::context_budget::LoopDirective::StopDiminishing => {
                            messages.push(UnifiedMessage::user(DIMINISHING_RETURNS_NOTICE));
                        }
                        super::context_budget::LoopDirective::Continue => {}
                    }
                    (ctx_budget.fresh_tail_count(), ctx_budget.token_estimate_ratio())
                } else {
                    // No context budget configured — still enforce a hard limit
                    // using the pipeline as a safety net against provider-side
                    // context-length errors.
                    let sensor = self
                        .pressure_sensor
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let pressure = sensor.measure(
                        &messages,
                        &system_prompt,
                        &tool_defs,
                        self.config.token_budget as u64,
                    );
                    if pressure.ratio >= 0.85 {
                        self.compaction_pipeline.run(
                            &mut messages,
                            &sensor,
                            &system_prompt,
                            &tool_defs,
                            self.config.token_budget as u64,
                            0.70,
                            6, // default fresh tail
                        );
                    }
                    (6_usize, 3.5_f64)
                }
            };

            // LLM-based compaction at elevated pressure (ratio >= 0.78)
            if let Some(ref compactor) = self.context_compactor {
                let should_llm_compact = {
                    let budget = self.context_budget.lock()
                        .unwrap_or_else(|e| e.into_inner());
                    budget.as_ref()
                        .and_then(|b| b.last_pressure())
                        .map(|p| p.ratio >= 0.78)
                        .unwrap_or(false)
                };
                if should_llm_compact {
                    match compactor.compact(&mut messages, budget_fresh_tail).await {
                        Ok(result) => {
                            tracing::info!(
                                strategy = ?result.strategy_used,
                                tokens_before = result.tokens_before,
                                tokens_after = result.tokens_after,
                                "context compaction complete"
                            );
                        }
                        Err(e) => {
                            tracing::warn!("context compaction failed: {e}");
                        }
                    }
                }
            }

            // Hard safety net: enforce context limit
            enforce_context_limit(
                &mut messages,
                &system_prompt,
                &tool_defs,
                self.config.token_budget,
                budget_fresh_tail,
                budget_ratio,
            );

            // Think: stream deltas with retry and cancellation
            let delta_stream = super::retry::retry_async(
                || self.provider.stream(&messages, &system_prompt, &tool_defs),
                &self.cancel_token,
                3,
            )
            .await?;

            // The bridge+executor are created every iteration (even for pure text turns)
            // because tools must start executing AS THEY STREAM — we cannot know whether
            // tool calls will arrive until the stream completes. The overhead for non-tool
            // turns is minimal: 1 mpsc channel + 1 tokio::spawn + 1 abort.
            let (mut bridge, executor) = super::streaming_bridge::StreamingToolBridge::new(
                Arc::clone(&self.tool_registry),
                Arc::clone(&self.safety_guard),
                self.cancel_token.clone(),
            );
            let executor_handle = tokio::spawn(executor.run());

            let mut collector = DeltaCollector::new();
            futures::pin_mut!(delta_stream);
            loop {
                tokio::select! {
                    maybe_delta = delta_stream.next() => {
                        match maybe_delta {
                            Some(Ok(delta)) => {
                                self.delta_sink.on_delta(&delta).await;
                                bridge.feed(&delta);
                                collector.push(delta);
                            }
                            Some(Err(e)) => return Err(e),
                            None => break,
                        }
                    }
                    _ = self.cancel_token.cancelled() => {
                        return Ok(LoopRunResult {
                            final_text: None,
                            iterations,
                            tool_calls_made,
                            total_tokens,
                            hit_limit: false,
                            cancelled: true,
                        });
                    }
                }
            }
            bridge.finish(); // close the channel so executor can drain
            let response = collector.finish();

            // Removed debug logging

            // Track tokens
            if let Some(usage) = &response.usage {
                total_tokens += (usage.input_tokens + usage.output_tokens) as usize;
                // Anchor pressure sensor to API-reported usage
                self.pressure_sensor
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .update_anchor(usage.input_tokens as usize, messages.len());
            }

            // Process text output
            if let Some(text) = &response.text {
                if response.has_tool_calls() {
                    // Intermediate: LLM said something AND requested tools → still working
                    callback.on_intermediate_text(text);
                    intermediate_texts.push(text.clone());
                    // Don't accumulate into final_text — intermediate messages
                    // are already delivered separately to channels
                } else {
                    // Final: LLM said something with no tool calls → done
                    // Strip any intermediate text the LLM repeated at the start
                    let cleaned = strip_repeated_intermediate(text, &intermediate_texts);
                    callback.on_text(&cleaned);
                    // Append text if recovering from truncation, otherwise replace
                    let is_recovering = {
                        let recovery = self.truncation_recovery.lock()
                            .unwrap_or_else(|e| e.into_inner());
                        *recovery.phase() != super::truncation_recovery::RecoveryPhase::Idle
                    };
                    if let Some(ref mut existing) = final_text {
                        if is_recovering {
                            existing.push_str(&cleaned);
                        } else {
                            *existing = cleaned;
                        }
                    } else {
                        final_text = Some(cleaned);
                    }
                }
            }

            // Push complete assistant message from response
            messages.push(UnifiedMessage::from_provider_response(&response));

            // Reset truncation recovery on normal completion (not MaxTokens)
            if response.stop_reason != StopReason::MaxTokens {
                let mut recovery = self.truncation_recovery.lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(original) = recovery.reset() {
                    current_max_tokens = Some(original);
                } else {
                    current_max_tokens = None;
                }
            }

            // If no tool calls and EndTurn → check completion protocol before stopping
            if !response.has_tool_calls() && response.stop_reason == StopReason::EndTurn {
                // Simple Q&A (no tools used) → stop naturally, no completion protocol
                if tool_calls_made == 0 {
                    break;
                }

                // Complex task: check for completion tag in CURRENT response
                let has_completion_tag = response
                    .text
                    .as_ref()
                    .is_some_and(|t| t.contains("<task-complete/>"));

                if has_completion_tag {
                    break;
                }

                // No completion tag — nudge based on stage
                if completion_nudge_count < MAX_COMPLETION_NUDGES {
                    completion_nudge_count += 1;
                    tracing::info!(
                        iteration = iterations,
                        nudge = completion_nudge_count,
                        "Completion protocol: LLM stopped without <task-complete/>, injecting nudge"
                    );

                    let nudge_msg = if completion_nudge_count < MAX_COMPLETION_NUDGES {
                        // Stage 1: challenge (first 2 nudges)
                        "[SYSTEM] You stopped but have not confirmed task completion. \
                         Do NOT apologize or explain. Review your work against the original request: \
                         is every requirement met? If not, try a different approach. \
                         When fully done, output a <completion-check> block and <task-complete/>."
                    } else {
                        // Stage 2: graceful exit (3rd nudge)
                        "[SYSTEM] Final attempt. Summarize: (1) what approaches you tried, \
                         (2) what succeeded and what failed, (3) what the user should do next. \
                         Then output <task-complete/>."
                    };

                    messages.push(UnifiedMessage::user(nudge_msg));
                    continue;
                }

                break; // Exhausted all nudges
            }

            // If no tool calls and MaxTokens → use TruncationRecovery state machine
            if !response.has_tool_calls() && response.stop_reason == StopReason::MaxTokens {
                let action = {
                    let mut recovery = self.truncation_recovery.lock()
                        .unwrap_or_else(|e| e.into_inner());
                    recovery.on_truncation(
                        current_max_tokens,
                        response.text.as_deref().unwrap_or(""),
                    )
                };
                if action.should_continue {
                    if let Some(override_val) = action.max_tokens_override {
                        current_max_tokens = Some(override_val);
                    }
                    messages.push(UnifiedMessage::user(&action.continuation_prompt));
                    tracing::info!(
                        iteration = iterations,
                        escalated = action.max_tokens_override.is_some(),
                        "truncation recovery: continuing"
                    );
                    continue;
                } else {
                    // Recovery exhausted — assemble all fragments
                    let recovery = self.truncation_recovery.lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let assembled = recovery.assemble_output();
                    let notice = "\n\n---\n⚠️ 输出因 token 限制被截断。请回复「继续」获取剩余内容。";
                    final_text = Some(format!("{assembled}{notice}"));
                    callback.on_text(notice);
                    tracing::warn!(iterations, "truncation recovery exhausted, assembling fragments");
                    hit_limit = true;
                    break;
                }
            }

            // If no tool calls and not EndTurn/MaxTokens → done
            if !response.has_tool_calls() {
                break;
            }

            // Skip tool execution if context budget says to stop
            let skip_tools = matches!(
                budget_directive,
                super::context_budget::LoopDirective::FinalReply
                    | super::context_budget::LoopDirective::StopDiminishing
            );

            if skip_tools && response.has_tool_calls() {
                // Abort the streaming executor — tools are being skipped.
                executor_handle.abort();

                // Inject a tool result telling the LLM tools were skipped
                for tc in &response.tool_calls {
                    messages.push(UnifiedMessage::tool_result(
                        tc.id.clone(),
                        tc.name.clone(),
                        "[SYSTEM] Tool execution skipped — context budget exhausted. Provide your best response now.",
                        true,
                    ));
                }
            } else if response.has_tool_calls() {
                // Act: collect tool results from the streaming executor.
                // Tools started executing during delta streaming — now await completion.
                let outcomes = match executor_handle.await {
                    Ok(results) => results,
                    Err(e) => {
                        tracing::warn!("streaming tool executor panicked: {e}");
                        vec![]
                    }
                };

                // Fire main-loop callbacks and process outcomes.
                // The bridge used a no-op callback, so we fire events here
                // to preserve the callback contract for external consumers.
                // Build a lookup from tool_id → arguments for on_tool_start.
                let tool_args_by_id: std::collections::HashMap<&str, &Value> = response
                    .tool_calls
                    .iter()
                    .map(|tc| (tc.id.as_str(), &tc.arguments))
                    .collect();

                for outcome in &outcomes {
                    // Determine if this outcome was a safety denial (no execution happened).
                    let is_safety_denial = outcome.is_error
                        && (outcome.output_text.starts_with("[BLOCKED]")
                            || outcome.output_text.starts_with("[NEEDS_CONFIRMATION]")
                            || outcome.output_text.starts_with("[DENIED]"));

                    // Fire on_tool_start only for tools that actually executed.
                    if !is_safety_denial {
                        let args = tool_args_by_id
                            .get(outcome.tool_id.as_str())
                            .copied()
                            .unwrap_or(&Value::Null);
                        callback.on_tool_start(&outcome.tool_name, args);
                    }

                    // Reconstruct a ToolResult for the callback.
                    let tool_result = if outcome.is_error {
                        ToolResult::Error {
                            error: outcome.output_text.clone(),
                            retryable: outcome.retryable,
                        }
                    } else if outcome.should_stop {
                        ToolResult::SuccessAndStopLoop {
                            output: Value::String(outcome.output_text.clone()),
                        }
                    } else {
                        ToolResult::Success {
                            output: Value::String(outcome.output_text.clone()),
                        }
                    };
                    if !is_safety_denial {
                        callback.on_tool_done(&outcome.tool_name, &tool_result);
                    }

                    if outcome.is_error {
                        // Safety denials, cancellations, and retryable errors
                        // don't count toward consecutive limit.
                        let is_non_counting = is_safety_denial
                            || outcome.output_text.starts_with("[CANCELLED]");
                        if is_non_counting {
                            // Fire safety block callback for denied tools
                            if outcome.output_text.starts_with("[BLOCKED]") {
                                callback.on_safety_block(&SafetyError::Blocked {
                                    tool: outcome.tool_name.clone(),
                                    pattern: String::new(),
                                });
                            } else if outcome.output_text.starts_with("[NEEDS_CONFIRMATION]") {
                                callback.on_safety_block(&SafetyError::NeedsConfirmation {
                                    tool: outcome.tool_name.clone(),
                                });
                            } else if outcome.output_text.starts_with("[DENIED]") {
                                callback.on_safety_block(&SafetyError::PolicyDenied {
                                    tool: outcome.tool_name.clone(),
                                });
                            }
                        } else if !outcome.retryable {
                            consecutive_errors += 1;
                        }
                    } else {
                        consecutive_errors = 0;
                    }

                    messages.push(UnifiedMessage::tool_result(
                        outcome.tool_id.clone(),
                        outcome.tool_name.clone(),
                        outcome.output_text.clone(),
                        outcome.is_error,
                    ));

                    if outcome.should_stop {
                        if final_text.is_none() {
                            final_text = Some(outcome.output_text.clone());
                        }
                        stop_requested = true;
                    }
                }

                tool_calls_made += outcomes.len();
            } else {
                // No tool calls — abort the idle executor.
                executor_handle.abort();
            }

            // --- After-turn: record metrics for diminishing returns detection ---
            {
                let mut ctx_budget_ref = self
                    .context_budget
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(ref mut ctx_budget) = *ctx_budget_ref {
                    let turn_productive = response.has_tool_calls()
                        && !skip_tools
                        && tool_calls_made > 0
                        && consecutive_errors == 0;
                    let output_tokens = response
                        .usage
                        .as_ref()
                        .map(|u| u.output_tokens as usize)
                        .unwrap_or(0);
                    let post_directive =
                        ctx_budget.after_turn(super::context_budget::TurnMetrics {
                            output_tokens,
                            tool_calls: response.tool_calls.len(),
                            productive: turn_productive,
                        });
                    if post_directive == super::context_budget::LoopDirective::StopDiminishing {
                        messages.push(UnifiedMessage::user(DIMINISHING_RETURNS_NOTICE));
                    }
                }
            }

            if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                hit_limit = true;
                final_text = Some(format!(
                    "Tool execution failed repeatedly ({} consecutive errors). The last error was for tool '{}'. Please try rephrasing your request.",
                    consecutive_errors,
                    response.tool_calls.last().map(|tc| tc.name.as_str()).unwrap_or("unknown")
                ));
                break;
            }

            if stop_requested {
                break;
            }

            // Check token budget
            if total_tokens >= self.config.token_budget {
                hit_limit = true;
                break;
            }
        }

        // Check if we hit max iterations
        if iterations >= self.config.max_iterations {
            hit_limit = true;
        }

        Ok(LoopRunResult {
            final_text,
            iterations,
            tool_calls_made,
            total_tokens,
            hit_limit,
            cancelled: false,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{NativeToolCall, ProviderResponse, TokenUsage};
    use crate::providers::message::ContentBlock;
    use crate::sync_primitives::{Arc, Mutex};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    struct MockProvider {
        responses: Mutex<Vec<ProviderResponse>>,
    }

    impl MockProvider {
        fn new(responses: Vec<ProviderResponse>) -> Self {
            let mut responses = responses;
            responses.reverse();
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl LoopProvider for MockProvider {
        async fn stream(
            &self,
            _messages: &[UnifiedMessage],
            _system_prompt: &str,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
            let mut responses = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            let resp = if let Some(resp) = responses.pop() {
                resp
            } else {
                ProviderResponse::text_only("(no more mock responses)".to_string())
            };
            Ok(crate::providers::delta::response_to_delta_stream(resp))
        }
    }

    /// MockProvider that captures messages it receives on each call.
    struct CapturingMockProvider {
        responses: Mutex<Vec<ProviderResponse>>,
        captured_messages: Arc<Mutex<Vec<Vec<UnifiedMessage>>>>,
    }

    impl CapturingMockProvider {
        fn new(
            responses: Vec<ProviderResponse>,
            captured: Arc<Mutex<Vec<Vec<UnifiedMessage>>>>,
        ) -> Self {
            let mut responses = responses;
            responses.reverse();
            Self {
                responses: Mutex::new(responses),
                captured_messages: captured,
            }
        }
    }

    #[async_trait]
    impl LoopProvider for CapturingMockProvider {
        async fn stream(
            &self,
            messages: &[UnifiedMessage],
            _system_prompt: &str,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
            self.captured_messages
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(messages.to_vec());
            let mut responses = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            let resp = if let Some(resp) = responses.pop() {
                resp
            } else {
                ProviderResponse::text_only("(no more mock responses)".to_string())
            };
            Ok(crate::providers::delta::response_to_delta_stream(resp))
        }
    }

    /// A tool that always returns a non-retryable error.
    struct FailTool;

    #[async_trait]
    impl super::super::tool::LoopTool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Error {
                error: "intentional failure".into(),
                retryable: false,
            }
        }
    }

    /// A tool that always returns a retryable error.
    struct RetryableFailTool;

    #[async_trait]
    impl super::super::tool::LoopTool for RetryableFailTool {
        fn name(&self) -> &str {
            "fail_retryable"
        }
        fn description(&self) -> &str {
            "Always fails but retryable"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Error {
                error: "transient failure".into(),
                retryable: true,
            }
        }
    }

    /// A tool that returns SuccessAndStopLoop.
    struct StopTool;

    #[async_trait]
    impl super::super::tool::LoopTool for StopTool {
        fn name(&self) -> &str {
            "stop"
        }
        fn description(&self) -> &str {
            "Stops the loop"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::SuccessAndStopLoop {
                output: json!({ "stopped": true }),
            }
        }
    }

    #[derive(Default)]
    struct TrackingCallback {
        texts: Vec<String>,
        intermediate_texts: Vec<String>,
        tool_starts: Vec<String>,
        tool_dones: Vec<String>,
        safety_blocks: Vec<String>,
        fallback_events: Vec<(String, String)>,
    }

    impl LoopCallback for TrackingCallback {
        fn on_text(&mut self, text: &str) {
            self.texts.push(text.to_string());
        }
        fn on_intermediate_text(&mut self, text: &str) {
            self.intermediate_texts.push(text.to_string());
        }
        fn on_tool_start(&mut self, name: &str, _input: &Value) {
            self.tool_starts.push(name.to_string());
        }
        fn on_tool_done(&mut self, name: &str, _result: &ToolResult) {
            self.tool_dones.push(name.to_string());
        }
        fn on_safety_block(&mut self, error: &SafetyError) {
            self.safety_blocks.push(error.to_string());
        }
        fn on_model_fallback(&mut self, reason: &str, fallback_model: &str) {
            self.fallback_events
                .push((reason.to_string(), fallback_model.to_string()));
        }
    }

    struct EchoTool;

    #[async_trait]
    impl super::super::tool::LoopTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes the input back"
        }
        fn schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }
        async fn execute(&self, input: Value) -> ToolResult {
            ToolResult::Success { output: input }
        }
    }

    fn make_loop(provider: MockProvider) -> AgentLoop<MockProvider> {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        )
    }

    #[tokio::test]
    async fn test_simple_text_response() {
        let provider = MockProvider::new(vec![ProviderResponse {
            text: Some("Hello, world!".to_string()),
            tool_calls: vec![],
            thinking: None,
            stop_reason: StopReason::EndTurn,
            usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: None,
                thinking_tokens: None,
            }),
        }]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("Hi", &mut cb).await.unwrap();

        assert_eq!(result.final_text.as_deref(), Some("Hello, world!"));
        assert_eq!(result.iterations, 1);
        assert_eq!(result.tool_calls_made, 0);
        assert_eq!(result.total_tokens, 15);
        assert!(!result.hit_limit);
        assert_eq!(cb.texts, vec!["Hello, world!"]);
    }

    #[tokio::test]
    async fn test_tool_call_then_response() {
        let provider = MockProvider::new(vec![
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "test" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: Some(TokenUsage {
                    input_tokens: 20,
                    output_tokens: 10,
                    cache_read_tokens: None,
                    thinking_tokens: None,
                }),
            },
            ProviderResponse {
                text: Some("Done echoing. <task-complete/>".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: Some(TokenUsage {
                    input_tokens: 30,
                    output_tokens: 5,
                    cache_read_tokens: None,
                    thinking_tokens: None,
                }),
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("Echo something", &mut cb).await.unwrap();

        assert_eq!(
            result.final_text.as_deref(),
            Some("Done echoing. <task-complete/>")
        );
        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 1);
        assert_eq!(result.total_tokens, 65);
        assert!(!result.hit_limit);
        assert_eq!(cb.tool_starts, vec!["echo"]);
        assert_eq!(cb.tool_dones, vec!["echo"]);
    }

    #[tokio::test]
    async fn test_max_iterations_guard() {
        let responses: Vec<ProviderResponse> = (0..15)
            .map(|i| ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: format!("call_{}", i),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "loop" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: Some(TokenUsage {
                    input_tokens: 5,
                    output_tokens: 5,
                    cache_read_tokens: None,
                    thinking_tokens: None,
                }),
            })
            .collect();

        let provider = MockProvider::new(responses);
        let agent = AgentLoop::new(
            provider,
            {
                let mut r = LoopToolRegistry::new();
                r.register(Box::new(EchoTool));
                r
            },
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 5,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("keep going", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 5);
        assert!(result.hit_limit);
        assert_eq!(result.tool_calls_made, 5);
    }

    #[tokio::test]
    async fn test_safety_guard_blocks_tool() {
        let provider = MockProvider::new(vec![
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_bad".to_string(),
                    name: "shell".to_string(),
                    arguments: json!({ "command": "rm -rf /" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            ProviderResponse {
                text: Some("I cannot do that. <task-complete/>".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = AgentLoop::new(
            provider,
            LoopToolRegistry::new(),
            PromptBuilder::new(),
            SafetyGuard::new(
                vec![r"rm\s+-rf\s+/".to_string()],
                std::collections::HashMap::new(),
                crate::extension::PermissionAction::Allow,
            ),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("delete everything", &mut cb).await.unwrap();

        assert_eq!(
            result.final_text.as_deref(),
            Some("I cannot do that. <task-complete/>")
        );
        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 1);
        assert!(!result.hit_limit);
        assert_eq!(cb.safety_blocks.len(), 1);
        assert!(cb.safety_blocks[0].contains("blocked"));
        assert!(cb.tool_starts.is_empty());
    }

    // =========================================================================
    // L1: Multi-turn tool chain
    // =========================================================================

    #[tokio::test]
    async fn test_multi_turn_tool_chain() {
        let captured = Arc::new(Mutex::new(Vec::<Vec<UnifiedMessage>>::new()));
        let provider = CapturingMockProvider::new(
            vec![
                // Turn 1: call tool A (echo)
                ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: "call_a".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "step1" }),
                    }],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: Some(TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
                // Turn 2: call tool B (echo again with different input)
                ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: "call_b".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "step2" }),
                    }],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: Some(TokenUsage {
                        input_tokens: 15,
                        output_tokens: 5,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
                // Turn 3: final text
                ProviderResponse {
                    text: Some("All done. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: Some(TokenUsage {
                        input_tokens: 20,
                        output_tokens: 5,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
            ],
            captured.clone(),
        );

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("chain test", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 3);
        assert_eq!(result.tool_calls_made, 2);
        assert_eq!(
            result.final_text.as_deref(),
            Some("All done. <task-complete/>")
        );
        assert!(!result.hit_limit);

        // Verify history accumulates: each call should have more messages
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 3);
        // Call 1: [user]
        assert_eq!(caps[0].len(), 1);
        // Call 2: [user, assistant(tool_call_a), tool_result_a]
        assert_eq!(caps[1].len(), 3);
        // Call 3: [user, assistant(tool_call_a), tool_result_a, assistant(tool_call_b), tool_result_b]
        assert_eq!(caps[2].len(), 5);
    }

    // =========================================================================
    // L2: Single turn multiple tools
    // =========================================================================

    #[tokio::test]
    async fn test_single_turn_multiple_tools() {
        let provider = MockProvider::new(vec![
            // Turn 1: two tool calls in one response
            ProviderResponse {
                text: None,
                tool_calls: vec![
                    NativeToolCall {
                        id: "call_x".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "first" }),
                    },
                    NativeToolCall {
                        id: "call_y".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "second" }),
                    },
                ],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 2: final text
            ProviderResponse {
                text: Some("Both done. <task-complete/>".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("parallel tools", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 2);
        assert_eq!(
            result.final_text.as_deref(),
            Some("Both done. <task-complete/>")
        );
        assert!(!result.hit_limit);
        assert_eq!(cb.tool_starts, vec!["echo", "echo"]);
    }

    // =========================================================================
    // L3: Consecutive errors threshold
    // =========================================================================

    #[tokio::test]
    async fn test_consecutive_errors_threshold() {
        // Need 10+ tool calls that all fail. Each response has one fail call.
        let responses: Vec<ProviderResponse> = (0..12)
            .map(|i| ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: format!("fail_{}", i),
                    name: "fail".to_string(),
                    arguments: json!({}),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            })
            .collect();

        let provider = MockProvider::new(responses);
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(FailTool));

        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 25,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("keep failing", &mut cb).await.unwrap();

        assert!(result.hit_limit);
        assert_eq!(result.tool_calls_made, 10); // stops at MAX_CONSECUTIVE_ERRORS
        let text = result.final_text.unwrap();
        assert!(text.contains("failed repeatedly"));
    }

    // =========================================================================
    // L4: Success resets error counter
    // =========================================================================

    #[tokio::test]
    async fn test_success_resets_error_counter() {
        // Pattern: 5 fails, 1 success (echo), 5 fails, then text.
        // Total errors never reach 10 consecutive because success resets counter.
        let mut responses: Vec<ProviderResponse> = Vec::new();

        // 5 fail calls
        for i in 0..5 {
            responses.push(ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: format!("fail_{}", i),
                    name: "fail".to_string(),
                    arguments: json!({}),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            });
        }
        // 1 success (echo)
        responses.push(ProviderResponse {
            text: None,
            tool_calls: vec![NativeToolCall {
                id: "success_1".to_string(),
                name: "echo".to_string(),
                arguments: json!({ "message": "reset" }),
            }],
            thinking: None,
            stop_reason: StopReason::ToolUse,
            usage: None,
        });
        // 5 more fail calls
        for i in 5..10 {
            responses.push(ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: format!("fail_{}", i),
                    name: "fail".to_string(),
                    arguments: json!({}),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            });
        }
        // Final text (LLM tries to stop — but persistence nudge will fire
        // because consecutive_errors > 0 from the second batch of fails)
        responses.push(ProviderResponse {
            text: Some("Trying to stop.".to_string()),
            tool_calls: vec![],
            thinking: None,
            stop_reason: StopReason::EndTurn,
            usage: None,
        });
        // After completion nudge: LLM acknowledges and finishes with tag
        responses.push(ProviderResponse {
            text: Some("Survived. <task-complete/>".to_string()),
            tool_calls: vec![],
            thinking: None,
            stop_reason: StopReason::EndTurn,
            usage: None,
        });

        let provider = MockProvider::new(responses);
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(FailTool));

        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 25,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("alternate errors", &mut cb).await.unwrap();

        assert!(!result.hit_limit);
        assert_eq!(
            result.final_text.as_deref(),
            Some("Survived. <task-complete/>")
        );
        // 5 fails + 1 echo + 5 fails = 11 tool calls, +1 nudge iteration
        assert_eq!(result.tool_calls_made, 11);
        // 11 tool iterations + 1 EndTurn (nudge fires) + 1 post-nudge EndTurn = 13
        assert_eq!(result.iterations, 13);
    }

    // =========================================================================
    // L5: SuccessAndStopLoop
    // =========================================================================

    #[tokio::test]
    async fn test_success_and_stop_loop() {
        let provider = MockProvider::new(vec![
            // Provider calls the stop tool
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_stop".to_string(),
                    name: "stop".to_string(),
                    arguments: json!({}),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // This response should never be reached
            ProviderResponse {
                text: Some("Should not reach here.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(StopTool));
        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("stop early", &mut cb).await.unwrap();

        // Loop should stop after 1 iteration (the stop tool)
        assert_eq!(result.iterations, 1);
        assert_eq!(result.tool_calls_made, 1);
        assert!(!result.hit_limit);
        // final_text should come from the stop tool's output
        assert!(result.final_text.is_some());
        let text = result.final_text.unwrap();
        assert!(text.contains("stopped"));
    }

    // =========================================================================
    // L6: MaxTokens stop reason
    // =========================================================================

    #[tokio::test]
    async fn test_max_tokens_stop_reason() {
        // First response is truncated (MaxTokens), loop auto-continues.
        // Second response completes normally (EndTurn) — continuation text is appended.
        let provider = MockProvider::new(vec![
            ProviderResponse {
                text: Some("Truncated response...".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::MaxTokens,
                usage: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 4096,
                    cache_read_tokens: None,
                    thinking_tokens: None,
                }),
            },
            ProviderResponse {
                text: Some(" and here is the rest.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("long question", &mut cb).await.unwrap();

        assert!(!result.hit_limit);
        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 0);
        assert_eq!(
            result.final_text.as_deref(),
            Some("Truncated response... and here is the rest.")
        );
    }

    #[tokio::test]
    async fn test_max_tokens_double_auto_continue() {
        // Truncated twice, second auto-continue succeeds with EndTurn.
        let provider = MockProvider::new(vec![
            ProviderResponse {
                text: Some("Part 1...".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::MaxTokens,
                usage: None,
            },
            ProviderResponse {
                text: Some("Part 2...".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::MaxTokens,
                usage: None,
            },
            ProviderResponse {
                text: Some("Part 3 done.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("long question", &mut cb).await.unwrap();

        assert!(!result.hit_limit);
        assert_eq!(result.iterations, 3);
        let text = result.final_text.unwrap();
        assert_eq!(text, "Part 1...Part 2...Part 3 done.");
    }

    #[tokio::test]
    async fn test_max_tokens_triple_truncation() {
        // All 3 responses truncated — after 2 auto-continues, hit_limit is set
        // and a truncation notice is appended.
        let provider = MockProvider::new(vec![
            ProviderResponse {
                text: Some("Part 1...".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::MaxTokens,
                usage: None,
            },
            ProviderResponse {
                text: Some("Part 2...".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::MaxTokens,
                usage: None,
            },
            ProviderResponse {
                text: Some("Part 3...".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::MaxTokens,
                usage: None,
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("long question", &mut cb).await.unwrap();

        assert!(result.hit_limit);
        assert_eq!(result.iterations, 3);
        let text = result.final_text.unwrap();
        assert!(text.starts_with("Part 1...Part 2...Part 3..."));
        assert!(text.contains("⚠️"));
    }

    // =========================================================================
    // L7: History injection via run_with_history
    // =========================================================================

    #[tokio::test]
    async fn test_history_injection() {
        let captured = Arc::new(Mutex::new(Vec::<Vec<UnifiedMessage>>::new()));
        let provider = CapturingMockProvider::new(
            vec![ProviderResponse {
                text: Some("Got your history.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            }],
            captured.clone(),
        );

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let history = vec![
            UnifiedMessage::user("Previous question"),
            UnifiedMessage::assistant("Previous answer"),
        ];

        let mut cb = TrackingCallback::default();
        let result = agent
            .run_with_history("New question", history, &mut cb)
            .await
            .unwrap();

        assert_eq!(result.iterations, 1);
        assert!(!result.hit_limit);

        // Verify provider received history + new user message
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 1);
        let messages = &caps[0];
        // [history_user, history_assistant, new_user]
        assert_eq!(messages.len(), 3);
        // First message should be the history user message
        match &messages[0] {
            UnifiedMessage::User { content } => {
                assert_eq!(content[0].as_text(), Some("Previous question"));
            }
            _ => panic!("expected User message"),
        }
        // Second should be the history assistant message
        match &messages[1] {
            UnifiedMessage::Assistant { content } => {
                assert_eq!(content[0].as_text(), Some("Previous answer"));
            }
            _ => panic!("expected Assistant message"),
        }
        // Third should be the new user message
        match &messages[2] {
            UnifiedMessage::User { content } => {
                assert_eq!(content[0].as_text(), Some("New question"));
            }
            _ => panic!("expected User message"),
        }
    }

    // =========================================================================
    // L8: Token budget exhaustion
    // =========================================================================

    #[tokio::test]
    async fn test_token_budget_exhaustion() {
        let provider = MockProvider::new(vec![
            // Turn 1: tool call consuming 30 tokens
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "hi" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: Some(TokenUsage {
                    input_tokens: 20,
                    output_tokens: 10,
                    cache_read_tokens: None,
                    thinking_tokens: None,
                }),
            },
            // Turn 2: another tool call consuming 30 more (total: 60, over budget of 50)
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_2".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "bye" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: Some(TokenUsage {
                    input_tokens: 20,
                    output_tokens: 10,
                    cache_read_tokens: None,
                    thinking_tokens: None,
                }),
            },
            // Turn 3: should not be reached
            ProviderResponse {
                text: Some("Unreachable.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = AgentLoop::new(
            provider,
            {
                let mut r = LoopToolRegistry::new();
                r.register(Box::new(EchoTool));
                r
            },
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 50,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("use tokens", &mut cb).await.unwrap();

        assert!(result.hit_limit);
        assert_eq!(result.total_tokens, 60);
        assert_eq!(result.iterations, 2);
    }

    // =========================================================================
    // L9: Assistant message completeness (thinking + text + tool_call)
    // =========================================================================

    #[tokio::test]
    async fn test_assistant_message_completeness() {
        let captured = Arc::new(Mutex::new(Vec::<Vec<UnifiedMessage>>::new()));
        let provider = CapturingMockProvider::new(
            vec![
                // Turn 1: response with thinking + text + tool_call
                ProviderResponse {
                    text: Some("I'll search for that.".to_string()),
                    tool_calls: vec![NativeToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "search" }),
                    }],
                    thinking: Some("Let me think about this...".to_string()),
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                },
                // Turn 2: final response
                ProviderResponse {
                    text: Some("Here are the results. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
            captured.clone(),
        );

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("complete message test", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 2);

        // Inspect the second call's messages to verify the first assistant message
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 2);
        let second_call_msgs = &caps[1];
        // second_call_msgs: [user, assistant(thinking+text+tool_call), tool_result]
        assert_eq!(second_call_msgs.len(), 3);

        // The assistant message should have all 3 ContentBlock types
        match &second_call_msgs[1] {
            UnifiedMessage::Assistant { content } => {
                assert_eq!(content.len(), 3);
                assert!(
                    matches!(&content[0], ContentBlock::Thinking { thinking } if thinking == "Let me think about this...")
                );
                assert!(
                    matches!(&content[1], ContentBlock::Text { text } if text == "I'll search for that.")
                );
                assert!(
                    matches!(&content[2], ContentBlock::ToolCall { id, name, .. } if id == "call_1" && name == "echo")
                );
            }
            _ => panic!("expected Assistant message with full content"),
        }
    }

    // =========================================================================
    // L10: Completion protocol nudge fires when EndTurn lacks <task-complete/>
    // =========================================================================

    #[tokio::test]
    async fn test_completion_nudge_on_missing_tag() {
        let captured = Arc::new(Mutex::new(Vec::<Vec<UnifiedMessage>>::new()));
        let provider = CapturingMockProvider::new(
            vec![
                // Turn 1: call a tool
                ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "work" }),
                    }],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                },
                // Turn 2: LLM stops without completion tag
                ProviderResponse {
                    text: Some("I think I'm done.".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
                // Turn 3: After nudge, LLM completes properly with tag
                ProviderResponse {
                    text: Some("Verified. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
            captured.clone(),
        );

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("do something", &mut cb).await.unwrap();

        // The loop continued past the first EndTurn thanks to the nudge
        assert_eq!(result.iterations, 3);
        assert_eq!(
            result.final_text.as_deref(),
            Some("Verified. <task-complete/>")
        );
        assert!(!result.hit_limit);

        // Verify the nudge message was injected
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        let third_call_msgs = &caps[2];
        let has_nudge = third_call_msgs.iter().any(|m| {
            if let UnifiedMessage::User { content } = m {
                content.iter().any(|b| {
                    if let ContentBlock::Text { text } = b {
                        text.contains("have not confirmed task completion")
                    } else {
                        false
                    }
                })
            } else {
                false
            }
        });
        assert!(has_nudge, "Expected a completion nudge message");
    }

    // =========================================================================
    // L11: Completion nudge escalates through 3 stages then stops
    // =========================================================================

    #[tokio::test]
    async fn test_completion_nudge_3_stages() {
        let provider = MockProvider::new(vec![
            // Turn 1: tool call
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "work" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 2: EndTurn without tag → nudge 1 (challenge)
            ProviderResponse {
                text: Some("I'm done.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
            // Turn 3: Still no tag → nudge 2 (challenge)
            ProviderResponse {
                text: Some("Really done.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
            // Turn 4: Still no tag → nudge 3 (graceful exit)
            ProviderResponse {
                text: Some("Still no tag.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
            // Turn 5: After 3 nudges, still no tag → loop stops unconditionally
            ProviderResponse {
                text: Some("Giving up.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("stubborn task", &mut cb).await.unwrap();

        // 1 tool + 3 EndTurns (each nudged) + 1 final EndTurn (stops) = 5 iterations
        assert_eq!(result.iterations, 5);
        assert_eq!(result.final_text.as_deref(), Some("Giving up."));
        assert!(!result.hit_limit);
    }

    // =========================================================================
    // L12: Retryable errors don't count toward consecutive limit
    // =========================================================================

    #[tokio::test]
    async fn test_retryable_errors_dont_count_toward_limit() {
        // 12 retryable errors — should NOT hit the 10-consecutive-errors limit
        let mut responses: Vec<ProviderResponse> = (0..12)
            .map(|i| ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: format!("call_{}", i),
                    name: "fail_retryable".to_string(),
                    arguments: json!({}),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            })
            .collect();
        // Final text (EndTurn with consecutive_errors=0 since retryable doesn't count)
        responses.push(ProviderResponse {
            text: Some("Done after retryable errors. <task-complete/>".to_string()),
            tool_calls: vec![],
            thinking: None,
            stop_reason: StopReason::EndTurn,
            usage: None,
        });

        let provider = MockProvider::new(responses);
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(RetryableFailTool));

        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 25,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("retryable failures", &mut cb).await.unwrap();

        // Did NOT hit the consecutive error limit
        assert!(!result.hit_limit);
        assert_eq!(result.iterations, 13);
        assert_eq!(result.tool_calls_made, 12);
        assert_eq!(
            result.final_text.as_deref(),
            Some("Done after retryable errors. <task-complete/>")
        );
    }

    // =========================================================================
    // L13: No nudge when EndTurn has completion tag
    // =========================================================================

    #[tokio::test]
    async fn test_no_nudge_on_clean_completion() {
        let captured = Arc::new(Mutex::new(Vec::<Vec<UnifiedMessage>>::new()));
        let provider = CapturingMockProvider::new(
            vec![
                // Turn 1: successful tool call
                ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "ok" }),
                    }],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                },
                // Turn 2: clean EndTurn with completion tag
                ProviderResponse {
                    text: Some("All good. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
            captured.clone(),
        );

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("clean task", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 2);
        assert_eq!(
            result.final_text.as_deref(),
            Some("All good. <task-complete/>")
        );

        // No nudge should have been injected (completion tag was present)
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        for call_msgs in caps.iter() {
            let has_nudge = call_msgs.iter().any(|m| {
                if let UnifiedMessage::User { content } = m {
                    content.iter().any(|b| {
                        if let ContentBlock::Text { text } = b {
                            text.contains("have not confirmed task completion")
                        } else {
                            false
                        }
                    })
                } else {
                    false
                }
            });
            assert!(
                !has_nudge,
                "No nudge should fire when completion tag is present"
            );
        }
    }

    // =========================================================================
    // L14: Intermediate text callback for tool-accompanied responses
    // =========================================================================

    #[tokio::test]
    async fn test_intermediate_text_with_tool_calls() {
        let provider = MockProvider::new(vec![
            // Turn 1: text + tool call → should be intermediate
            ProviderResponse {
                text: Some("Let me search for that...".to_string()),
                tool_calls: vec![NativeToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "search" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 2: text only → should be final
            ProviderResponse {
                text: Some("Here are the results. <task-complete/>".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("find something", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 2);
        // Intermediate text goes to intermediate_texts, not texts
        assert_eq!(cb.intermediate_texts, vec!["Let me search for that..."]);
        // Final text goes to texts
        assert_eq!(cb.texts, vec!["Here are the results. <task-complete/>"]);
        // final_text should be the last text produced
        assert_eq!(
            result.final_text.as_deref(),
            Some("Here are the results. <task-complete/>")
        );
    }

    // =========================================================================
    // L14b: LLM repeats intermediate text in final response → stripped
    // =========================================================================

    #[tokio::test]
    async fn test_repeated_intermediate_text_stripped_from_final() {
        let provider = MockProvider::new(vec![
            // Turn 1: intermediate text + tool call
            ProviderResponse {
                text: Some("Let me set up the team.".to_string()),
                tool_calls: vec![NativeToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "team" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 2: another intermediate text + tool call
            ProviderResponse {
                text: Some("Team is ready.".to_string()),
                tool_calls: vec![NativeToolCall {
                    id: "call_2".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "run" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 3: LLM repeats intermediate texts at the start of final response
            ProviderResponse {
                text: Some(
                    "Let me set up the team. Team is ready. Here are the results. <task-complete/>"
                        .to_string(),
                ),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("analyze stocks", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 3);
        assert_eq!(
            cb.intermediate_texts,
            vec!["Let me set up the team.", "Team is ready.",]
        );
        // Repeated intermediate text should be stripped from the final
        assert_eq!(cb.texts, vec!["Here are the results. <task-complete/>"]);
        assert_eq!(
            result.final_text.as_deref(),
            Some("Here are the results. <task-complete/>")
        );
    }

    // =========================================================================
    // L15: No completion protocol for pure Q&A (no tool calls)
    // =========================================================================

    #[tokio::test]
    async fn test_no_completion_protocol_without_tools() {
        // Pure Q&A: no tools used, no completion tag needed
        let provider = MockProvider::new(vec![ProviderResponse {
            text: Some("The answer is 42.".to_string()),
            tool_calls: vec![],
            thinking: None,
            stop_reason: StopReason::EndTurn,
            usage: None,
        }]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent
            .run("What is the meaning of life?", &mut cb)
            .await
            .unwrap();

        assert_eq!(result.iterations, 1);
        assert_eq!(result.tool_calls_made, 0);
        assert_eq!(result.final_text.as_deref(), Some("The answer is 42."));
        assert!(!result.hit_limit);
    }

    // =========================================================================
    // L16: Completion tag in intermediate response is ignored
    // =========================================================================

    #[tokio::test]
    async fn test_completion_tag_in_intermediate_ignored() {
        let provider = MockProvider::new(vec![
            // Turn 1: text with tag BUT also has tool calls → tag ignored
            ProviderResponse {
                text: Some("Almost done <task-complete/>".to_string()),
                tool_calls: vec![NativeToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "more work" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 2: actual final response with tag
            ProviderResponse {
                text: Some("Now truly done. <task-complete/>".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("complex task", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 1);
        assert_eq!(
            result.final_text.as_deref(),
            Some("Now truly done. <task-complete/>")
        );
    }

    // =========================================================================
    // L17: No false positive from stale final_text after nudge
    // =========================================================================

    #[tokio::test]
    async fn test_no_stale_final_text_false_positive() {
        let provider = MockProvider::new(vec![
            // Turn 1: tool call
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "work" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 2: EndTurn without tag → nudge fires
            ProviderResponse {
                text: Some("Done without tag.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
            // Turn 3: EndTurn with NO text at all (response.text = None)
            // final_text still holds "Done without tag." from turn 2
            // but we check response.text, not final_text, so no false positive
            ProviderResponse {
                text: None,
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
            // Turn 4: After 2nd nudge, finally completes with tag
            ProviderResponse {
                text: Some("OK. <task-complete/>".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("tricky task", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 4);
        assert_eq!(result.final_text.as_deref(), Some("OK. <task-complete/>"));
    }

    // =========================================================================
    // strip_repeated_intermediate tests
    // =========================================================================

    #[test]
    fn test_strip_repeated_intermediate_no_intermediates() {
        let result = strip_repeated_intermediate("Hello world", &[]);
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_strip_repeated_intermediate_exact_match() {
        let intermediates = vec!["Setting up...".to_string()];
        let text = "Setting up... Here is the result.";
        let result = strip_repeated_intermediate(text, &intermediates);
        assert_eq!(result, "Here is the result.");
    }

    #[test]
    fn test_strip_repeated_intermediate_multiple() {
        let intermediates = vec!["Step 1 done.".to_string(), "Step 2 done.".to_string()];
        let text = "Step 1 done. Step 2 done. Final answer.";
        let result = strip_repeated_intermediate(text, &intermediates);
        assert_eq!(result, "Final answer.");
    }

    #[test]
    fn test_strip_repeated_intermediate_no_match() {
        let intermediates = vec!["Something else".to_string()];
        let text = "Completely different text";
        let result = strip_repeated_intermediate(text, &intermediates);
        assert_eq!(result, "Completely different text");
    }

    #[test]
    fn test_strip_repeated_intermediate_partial_match() {
        // Only first intermediate matches, second doesn't — stops stripping
        let intermediates = vec!["First part.".to_string(), "Nonexistent.".to_string()];
        let text = "First part. Actual content here.";
        let result = strip_repeated_intermediate(text, &intermediates);
        assert_eq!(result, "Actual content here.");
    }

    #[test]
    fn test_strip_repeated_intermediate_empty_intermediate() {
        let intermediates = vec!["".to_string(), "  ".to_string()];
        let text = "Should not be modified";
        let result = strip_repeated_intermediate(text, &intermediates);
        assert_eq!(result, "Should not be modified");
    }

    #[test]
    fn test_strip_repeated_intermediate_whitespace_handling() {
        let intermediates = vec!["  Hello  ".to_string()];
        let text = "  Hello   World";
        let result = strip_repeated_intermediate(text, &intermediates);
        assert_eq!(result, "World");
    }

    // =========================================================================
    // 413 emergency truncation tests
    // =========================================================================

    #[test]
    fn test_group_by_round_basic() {
        let messages = vec![
            UnifiedMessage::user("q1"),
            UnifiedMessage::assistant("a1"),
            UnifiedMessage::user("q2"),
            UnifiedMessage::assistant("a2"),
        ];
        let groups = group_by_round(&messages);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], (0, 2));
        assert_eq!(groups[1], (2, 4));
    }

    #[test]
    fn test_group_by_round_single() {
        let messages = vec![UnifiedMessage::user("q1")];
        let groups = group_by_round(&messages);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], (0, 1));
    }

    #[test]
    fn test_emergency_truncate_drops_oldest_groups() {
        let mut messages = vec![
            UnifiedMessage::user("round 1 question"),
            UnifiedMessage::assistant("round 1 answer"),
            UnifiedMessage::user("round 2 question"),
            UnifiedMessage::assistant("round 2 answer"),
            UnifiedMessage::user("round 3 question"),
            UnifiedMessage::assistant("round 3 answer"),
            UnifiedMessage::user("current question"),
        ];
        let original_len = messages.len();
        emergency_truncate(&mut messages, None, 2);
        assert!(messages.len() < original_len);
        // First message should be truncation marker
        assert!(messages[0].text_content().contains("truncated"));
        // Last message should be preserved
        assert_eq!(
            messages.last().unwrap().text_content(),
            "current question"
        );
    }

    #[test]
    fn test_emergency_truncate_with_known_gap() {
        let mut messages = vec![];
        for i in 0..10 {
            messages.push(UnifiedMessage::user(&format!("question {i} {}", "x".repeat(80))));
            messages.push(UnifiedMessage::assistant(&format!(
                "answer {i} {}",
                "y".repeat(80)
            )));
        }
        messages.push(UnifiedMessage::user("final"));
        let original_len = messages.len();
        emergency_truncate(&mut messages, Some(500), 3);
        assert!(messages.len() < original_len);
        assert_eq!(messages.last().unwrap().text_content(), "final");
    }

    #[test]
    fn test_emergency_truncate_too_few_messages_is_noop() {
        let mut messages = vec![
            UnifiedMessage::user("only question"),
            UnifiedMessage::assistant("only answer"),
        ];
        let original_len = messages.len();
        emergency_truncate(&mut messages, None, 2);
        assert_eq!(messages.len(), original_len);
    }
}
