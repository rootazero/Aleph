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

use crate::sync_primitives::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::agent_runtime::SharedSnapshot;
use super::context_budget::diagnostics::ContextDiagnostics;
use super::context_budget::pipeline::{
    CompactionPipeline, ImageStripper, ResultClearing, RoundDrop, ToolCompactStage,
};
use super::context_budget::pressure::PressureSensor;
use super::stop_hooks::{self, StopHookContext, StopHookHandler};
use super::tool::{LoopToolRegistry, ToolDefinition, ToolResult};
use super::tool_info::ToolInfo;
use super::tool_pipeline::PipelineOutcome;
use super::tool_pipeline::ToolPipeline;
use super::trace::{
    LoopTraceEvent, LoopTraceSessionOutcome, LoopTraceState, LoopTraceTextKind,
    LoopTraceTurnMetrics, LoopTraceTurnOutcome, ToolCallEndEvent, ToolCallStartEvent,
};
use crate::extension::hooks::{HookContext, HookExecutor};
use crate::extension::HookEvent;
use crate::providers::adapter::{ProviderResponse, StopReason};
use crate::providers::delta::{DeltaCollector, DeltaSink, NoopSink, ProviderDelta};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;
use crate::secrets::injection::AsyncSecretResolver;
use crate::security::{GuardResult, RuntimeSecurityGuard, SecurityContext};
use crate::session::ingress_safety::{SafetyError, SafetyGuard};
use crate::thinker::prompt_builder::PromptBuilder;
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

const MAX_CONSECUTIVE_ERRORS: usize = 10;
const MAX_COMPLETION_NUDGES: usize = 3;
const MAX_STOP_HOOK_BLOCKS: usize = 3;

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
    if groups.len() <= fresh_tail_count {
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
    /// Chain ID shared across all depths in a subagent call chain.
    pub chain_id: String,
    /// Nesting depth (0 = root agent).
    pub depth: u32,
}

type SummaryHandle = JoinHandle<Option<String>>;
type SkillPrefetchHandle = JoinHandle<Option<Vec<super::skill_prefetch::SkillInfo>>>;

#[derive(Clone, Copy)]
enum ActiveProvider {
    Primary,
    Fallback,
}

#[derive(Clone, Copy)]
struct TurnBudgetState {
    directive: super::context_budget::LoopDirective,
    fresh_tail_count: usize,
}

struct LoopProgress {
    final_text: Option<String>,
    intermediate_texts: Vec<String>,
    iterations: usize,
    tool_calls_made: usize,
    total_tokens: usize,
    consecutive_errors: usize,
    completion_nudge_count: usize,
    current_max_tokens: Option<u32>,
    stop_hook_blocks: usize,
    pending_summary: Option<SummaryHandle>,
    active_provider: ActiveProvider,
}

struct LoopRuntime {
    messages: Vec<UnifiedMessage>,
    system_prompt: String,
    tool_defs: Vec<ToolDefinition>,
}

impl Default for LoopProgress {
    fn default() -> Self {
        Self {
            final_text: None,
            intermediate_texts: Vec::new(),
            iterations: 0,
            tool_calls_made: 0,
            total_tokens: 0,
            consecutive_errors: 0,
            completion_nudge_count: 0,
            current_max_tokens: None,
            stop_hook_blocks: 0,
            pending_summary: None,
            active_provider: ActiveProvider::Primary,
        }
    }
}

impl LoopProgress {
    fn cancelled_result(&self, chain: &super::chain_context::ChainContext) -> LoopRunResult {
        LoopRunResult {
            final_text: None,
            iterations: self.iterations,
            tool_calls_made: self.tool_calls_made,
            total_tokens: self.total_tokens,
            hit_limit: false,
            cancelled: true,
            chain_id: chain.chain_id.clone(),
            depth: chain.depth,
        }
    }

    fn finish(self, chain: &super::chain_context::ChainContext, hit_limit: bool) -> LoopRunResult {
        LoopRunResult {
            final_text: self.final_text,
            iterations: self.iterations,
            tool_calls_made: self.tool_calls_made,
            total_tokens: self.total_tokens,
            hit_limit,
            cancelled: false,
            chain_id: chain.chain_id.clone(),
            depth: chain.depth,
        }
    }

    fn apply_final_text_update(&mut self, update: FinalTextUpdate) {
        match update {
            FinalTextUpdate::None => {}
            FinalTextUpdate::SetIfEmpty(text) => {
                if self.final_text.is_none() {
                    self.final_text = Some(text);
                }
            }
            FinalTextUpdate::Replace(text) => {
                self.final_text = Some(text);
            }
        }
    }
}

struct TurnThinkingState {
    budget: TurnBudgetState,
    response: ProviderResponse,
    prefetch_handle: Option<SkillPrefetchHandle>,
    executor_handle: JoinHandle<Vec<PipelineOutcome>>,
}

struct TurnActState {
    thinking: TurnThinkingState,
    skip_tools: bool,
}

#[derive(Default)]
enum FinalTextUpdate {
    #[default]
    None,
    SetIfEmpty(String),
    Replace(String),
}

#[derive(Clone, Copy, Default)]
enum TurnExitRequest {
    #[default]
    None,
    Stop,
    HitLimit,
}

struct ToolTurnArtifacts {
    requested_calls: usize,
    executed_calls: usize,
    skip_tools: bool,
    next_error_streak: usize,
    last_tool_name: Option<String>,
    summary_handle: Option<SummaryHandle>,
}

impl ToolTurnArtifacts {
    fn idle(current_error_streak: usize) -> Self {
        Self {
            requested_calls: 0,
            executed_calls: 0,
            skip_tools: false,
            next_error_streak: current_error_streak,
            last_tool_name: None,
            summary_handle: None,
        }
    }

    fn requested(requested_calls: usize, current_error_streak: usize, skip_tools: bool) -> Self {
        Self {
            requested_calls,
            executed_calls: 0,
            skip_tools,
            next_error_streak: current_error_streak,
            last_tool_name: None,
            summary_handle: None,
        }
    }

    fn productive(&self) -> bool {
        self.requested_calls > 0
            && self.executed_calls > 0
            && !self.skip_tools
            && self.next_error_streak == 0
    }
}

struct TurnArtifacts {
    response: ProviderResponse,
    prefetch_handle: Option<SkillPrefetchHandle>,
    tools: ToolTurnArtifacts,
    exit_request: TurnExitRequest,
    final_text_update: FinalTextUpdate,
}

enum ThinkTurnResult {
    Ready(TurnThinkingState),
    Cancelled,
}

enum TurnResolve {
    Restart,
    Act(TurnActState),
    Finalize(TurnArtifacts),
}

enum TurnLoopDecision {
    Continue,
    Exit(LoopExitDecision),
}

#[derive(Default)]
struct LoopExitDecision {
    hit_limit: bool,
    final_text_update: FinalTextUpdate,
}

enum TurnState {
    Prepare,
    Think(TurnBudgetState),
    Resolve(TurnThinkingState),
    Act(TurnActState),
    Finalize(TurnArtifacts),
}

enum TurnAdvance {
    Next(TurnState),
    ContinueLoop,
    ExitLoop(LoopExitDecision),
    Cancelled,
}

enum TurnExecution {
    Prepared(TurnBudgetState),
    Thought(ThinkTurnResult),
    Resolved(TurnResolve),
    Acted(TurnArtifacts),
    Finalized(TurnLoopDecision),
}

enum TurnRunOutcome {
    Continue,
    Exit(LoopExitDecision),
    Cancelled,
}

#[derive(Default)]
struct TurnTraceFrame {
    requested_tool_calls: usize,
    executed_tool_calls: usize,
    productive: bool,
}

impl TurnTraceFrame {
    fn observe_artifacts(&mut self, turn: &TurnArtifacts) {
        self.requested_tool_calls = turn.tools.requested_calls;
        self.executed_tool_calls = turn.tools.executed_calls;
        self.productive = turn.tools.productive();
    }

    fn snapshot(&self, progress: &LoopProgress) -> LoopTraceTurnMetrics {
        LoopTraceTurnMetrics {
            requested_tool_calls: self.requested_tool_calls,
            executed_tool_calls: self.executed_tool_calls,
            productive: self.productive,
            consecutive_errors: progress.consecutive_errors,
            total_tokens: progress.total_tokens,
        }
    }
}

impl TurnState {
    fn trace_state(&self) -> LoopTraceState {
        match self {
            Self::Prepare => LoopTraceState::Prepare,
            Self::Think(_) => LoopTraceState::Think,
            Self::Resolve(_) => LoopTraceState::Resolve,
            Self::Act(_) => LoopTraceState::Act,
            Self::Finalize(_) => LoopTraceState::Finalize,
        }
    }
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
    fn on_trace(&mut self, event: &LoopTraceEvent) {
        match event {
            LoopTraceEvent::TextEmitted { stream, text, .. } => match stream {
                LoopTraceTextKind::Final => self.on_text(text),
                LoopTraceTextKind::Intermediate => self.on_intermediate_text(text),
            },
            LoopTraceEvent::ToolCallStarted { call, .. } => self.on_tool_call_start(call),
            LoopTraceEvent::ToolCallCompleted { call, result, .. } => {
                self.on_tool_call_done(call, result)
            }
            LoopTraceEvent::ToolSummary { summary, .. } => self.on_tool_summary(summary),
            LoopTraceEvent::TurnStarted { .. }
            | LoopTraceEvent::TurnStateEntered { .. }
            | LoopTraceEvent::TurnCompleted { .. }
            | LoopTraceEvent::SessionCompleted { .. } => {}
        }
    }
    fn on_text(&mut self, _text: &str) {}
    /// Called when the LLM produces text alongside tool calls (intermediate progress).
    /// This text should be delivered to the user immediately, not buffered.
    fn on_intermediate_text(&mut self, _text: &str) {}
    fn on_tool_start(&mut self, _name: &str, _input: &Value) {}
    fn on_tool_done(&mut self, _name: &str, _result: &ToolResult) {}
    fn on_tool_call_start(&mut self, event: &ToolCallStartEvent) {
        self.on_tool_start(&event.tool_name, &event.input);
    }
    fn on_tool_call_done(&mut self, event: &ToolCallEndEvent, result: &ToolResult) {
        self.on_tool_done(&event.tool_name, result);
    }
    fn on_safety_block(&mut self, _error: &SafetyError) {}
    fn on_model_fallback(&mut self, _reason: &str, _fallback_model: &str) {}
    fn on_stop_hook_block(&mut self, _reason: &str) {}
    fn on_stop_hook_error(&mut self, _hook_name: &str, _error: &str) {}
    fn on_tool_summary(&mut self, _summary: &str) {}

    /// Request user confirmation for a high-risk tool call.
    ///
    /// Called when SafetyGuard or a Hook classifies a tool as needing
    /// user confirmation before execution. Default returns `false` (reject),
    /// preserving backward compatibility with all Channel implementations.
    ///
    /// Channels override this to implement their own confirmation UX:
    /// CLI → stdin prompt, Telegram → inline keyboard, API → webhook.
    fn on_confirmation_needed(
        &mut self,
        _tool_name: &str,
        _tool_input: &Value,
        _reason: &str,
    ) -> bool {
        false
    }
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
    tool_registry: crate::sync_primitives::RwLock<Arc<LoopToolRegistry>>,
    /// Optional source for runtime tool hot-refresh.
    tool_refresh: Option<Arc<dyn super::tool_refresh::ToolRefreshSource>>,
    prompt_builder: PromptBuilder,
    safety_guard: Arc<SafetyGuard>,
    /// Hook-integrated tool execution pipeline (wraps safety_guard).
    tool_pipeline: Arc<ToolPipeline>,
    config: LoopConfig,
    /// Optional context budget for pressure sensing and budget tracking.
    /// Wrapped in `Mutex` for interior mutability — `run_with_history_messages`
    /// takes `&self` but the budget needs mutable state across turns.
    context_budget: Mutex<Option<super::context_budget::ContextBudget>>,
    /// Pressure sensor anchored to API-reported token usage.
    pressure_sensor: Mutex<PressureSensor>,
    /// Pipeline of compaction stages (image strip → micro compact → tool compact → round drop).
    compaction_pipeline: CompactionPipeline,
    /// Pre-flight context preparation pipeline (microcompact → collapse → autocompact).
    preflight_pipeline: super::context_budget::preflight::PreflightPipeline,
    /// Diagnostics collector for pipeline run history.
    diagnostics: Mutex<ContextDiagnostics>,
    /// Truncation recovery state machine — handles `MaxTokens` stop reason by
    /// escalating token limits, generating continuation prompts, and assembling
    /// fragmented outputs. Wrapped in `Mutex` for interior mutability.
    truncation_recovery: Mutex<super::truncation_recovery::TruncationRecovery>,
    /// Orchestrator that dispatches MicroCompactor → LLM summary in priority
    /// order when context pressure reaches the warning threshold.
    compaction_orchestrator: Option<Arc<super::compaction::CompactionOrchestrator>>,
    /// Sink for streaming deltas during the Think step. Defaults to NoopSink.
    delta_sink: Box<dyn DeltaSink>,
    /// Token for cooperative cancellation of streaming and tool execution.
    cancel_token: CancellationToken,
    /// Stop hooks to run before the loop exits at task-completion break points.
    stop_hooks: Vec<Box<dyn StopHookHandler>>,
    /// Optional lightweight provider for async tool use summaries.
    summary_provider: Option<Arc<dyn AiProvider>>,
    /// Chain context tracking subagent call chain nesting.
    chain: super::chain_context::ChainContext,
    /// Optional skill prefetcher for async discovery during inference.
    skill_prefetcher: Option<super::skill_prefetch::SkillPrefetcher>,
    /// Optional session context for environment info injection.
    session_context: Option<super::sections::SessionContext>,
    /// Optional approval gate for two-phase exec approval.
    approval_gate: Option<crate::agent_loop::exec_approval::ApprovalGate>,
    /// Shared prompt snapshot for fork path. Written once after first prompt assembly.
    shared_snapshot: Option<SharedSnapshot>,
    /// Runtime security guard for outbound/inbound content filtering.
    security_guard: RuntimeSecurityGuard,
    /// Optional secret resolver for placeholder injection.
    secret_resolver: Option<Arc<dyn AsyncSecretResolver>>,
    /// Platform name for security context (e.g., "telegram", "discord").
    platform_name: Option<String>,
    /// Provider name for security context (e.g., "anthropic", "openai").
    provider_name: Option<String>,
    /// Session identifier for security context.
    session_id: Option<String>,
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
            Box::new(ResultClearing),
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
        let safety_guard = Arc::new(safety_guard);
        let tool_pipeline = Arc::new(ToolPipeline::new(
            Arc::new(HookExecutor::empty()),
            Arc::clone(&safety_guard),
            "",
        ));
        Self {
            provider,
            fallback_provider: None,
            fallback_label: None,
            tool_registry: crate::sync_primitives::RwLock::new(Arc::new(tool_registry)),
            tool_refresh: None,
            prompt_builder,
            safety_guard,
            tool_pipeline,
            config,
            context_budget: Mutex::new(None),
            pressure_sensor: Mutex::new(PressureSensor::new(3.5)),
            compaction_pipeline: pipeline,
            preflight_pipeline: super::context_budget::preflight::PreflightPipeline::new(vec![
                Box::new(super::context_budget::microcompact::MicrocompactStage::new()),
                Box::new(super::context_budget::context_collapse::ContextCollapseStage::new()),
                Box::new(super::context_budget::autocompact::AutocompactStage::noop()),
            ]),
            diagnostics: Mutex::new(ContextDiagnostics::new()),
            compaction_orchestrator: None,
            truncation_recovery: Mutex::new(super::truncation_recovery::TruncationRecovery::new(
                provider_max,
            )),
            delta_sink: Box::new(NoopSink),
            cancel_token,
            stop_hooks: Vec::new(),
            summary_provider: None,
            chain: super::chain_context::ChainContext::new(),
            skill_prefetcher: None,
            session_context: None,
            approval_gate: None,
            shared_snapshot: None,
            security_guard: RuntimeSecurityGuard::default_guard(),
            secret_resolver: None,
            platform_name: None,
            provider_name: None,
            session_id: None,
        }
    }

    /// Attach a platform name to the agent loop for security context.
    pub fn with_platform_name(mut self, platform_name: Option<String>) -> Self {
        self.platform_name = platform_name;
        self
    }

    /// Attach a provider name to the agent loop for security context.
    pub fn with_provider_name(mut self, provider_name: impl Into<String>) -> Self {
        self.provider_name = Some(provider_name.into());
        self
    }

    /// Attach a session identifier to the agent loop for security context.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
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
    ///
    /// This also builds a [`CompactionOrchestrator`](super::compaction::CompactionOrchestrator)
    /// that chains `MicroCompactor` (zero-LLM-cost) before the LLM-based compactor so that
    /// cheap pruning is always attempted first.
    pub fn with_context_compactor(
        mut self,
        compactor: super::context_compactor::ContextCompactor,
    ) -> Self {
        let micro = Arc::new(super::compaction::MicroCompactor::new(
            super::compaction::MicroCompactorConfig::default(),
        ));
        let llm = Arc::new(compactor) as Arc<dyn super::compaction::CompactionStrategy>;
        let orchestrator = Arc::new(
            super::compaction::CompactionOrchestrator::builder()
                .strategy(micro)
                .strategy(llm)
                .build(),
        );
        self.compaction_orchestrator = Some(orchestrator);
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

    pub fn with_approval_gate(
        mut self,
        gate: crate::agent_loop::exec_approval::ApprovalGate,
    ) -> Self {
        self.approval_gate = Some(gate);
        self
    }

    /// Attach a secret resolver for runtime secret placeholder injection.
    pub fn with_secret_resolver(mut self, resolver: Arc<dyn AsyncSecretResolver>) -> Self {
        self.secret_resolver = Some(resolver);
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

    /// Register stop hooks to run before the loop exits.
    pub fn with_stop_hooks(mut self, hooks: Vec<Box<dyn StopHookHandler>>) -> Self {
        self.stop_hooks = hooks;
        self
    }

    /// Attach a lightweight provider for async tool use summaries.
    pub fn with_summary_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.summary_provider = Some(provider);
        self
    }

    /// Attach a [`ChainContext`](super::chain_context::ChainContext) for subagent call chain tracking.
    pub fn with_chain(mut self, chain: super::chain_context::ChainContext) -> Self {
        self.tool_pipeline.set_session_id(chain.chain_id.clone());
        self.chain = chain;
        self
    }

    /// Attach a [`ToolRefreshSource`](super::tool_refresh::ToolRefreshSource) for runtime tool hot-refresh.
    pub fn with_tool_refresh(
        mut self,
        source: Arc<dyn super::tool_refresh::ToolRefreshSource>,
    ) -> Self {
        self.tool_refresh = Some(source);
        self
    }

    /// Attach a [`SkillPrefetcher`](super::skill_prefetch::SkillPrefetcher) for async skill discovery.
    pub fn with_skill_prefetcher(
        mut self,
        prefetcher: super::skill_prefetch::SkillPrefetcher,
    ) -> Self {
        self.skill_prefetcher = Some(prefetcher);
        self
    }

    /// Attach a [`SessionContext`] for environment info injection into the prompt.
    pub fn with_session_context(mut self, ctx: super::sections::SessionContext) -> Self {
        self.session_context = Some(ctx);
        self
    }

    /// Replace the default (empty) hook executor with one loaded from the extension system.
    ///
    /// This rebuilds the internal `ToolPipeline` so that `has_hooks()` returns `true`
    /// and all pre/post/failure/session hooks actually fire at runtime.
    pub fn with_hook_executor(mut self, hooks: Arc<HookExecutor>) -> Self {
        self.tool_pipeline = Arc::new(ToolPipeline::new(
            hooks,
            Arc::clone(&self.safety_guard),
            self.chain.chain_id.clone(),
        ));
        self
    }

    /// Attach a shared prompt snapshot for the fork path.
    ///
    /// The snapshot is written once after the first prompt assembly and reused
    /// by sub-agents spawned via [`SubagentTool`] to avoid redundant prompt
    /// rebuilds.
    pub fn with_shared_snapshot(mut self, snapshot: SharedSnapshot) -> Self {
        self.shared_snapshot = Some(snapshot);
        self
    }

    /// Get tool definitions from the registry (for inspection/testing).
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tool_registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .tool_definitions()
    }

    async fn prepare_turn(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
        progress: &mut LoopProgress,
        callback: &mut dyn LoopCallback,
    ) -> TurnBudgetState {
        if let Some(handle) = progress.pending_summary.as_ref() {
            if handle.is_finished() {
                let handle = progress.pending_summary.take().unwrap();
                if let Ok(Some(summary)) = handle.await {
                    let iteration = progress.iterations.saturating_sub(1).max(1);
                    Self::emit_tool_summary_trace(iteration, summary, callback);
                }
            }
        }

        let mut budget_directive = super::context_budget::LoopDirective::Continue;
        let mut has_budget = false;

        // Phase 1: Acquire lock, run before_turn, extract preflight values, release lock.
        let (preflight_pressure, preflight_fresh_tail) = {
            let mut ctx_budget_ref = self
                .context_budget
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut ctx_budget) = *ctx_budget_ref {
                has_budget = true;
                budget_directive = ctx_budget.before_turn(messages, system_prompt, tool_defs);
                let pressure = ctx_budget.last_pressure().cloned();
                let fresh_tail = ctx_budget.fresh_tail_count();
                (pressure, fresh_tail)
            } else {
                (None, 6_usize)
            }
        };

        // Phase 2: Pre-flight context preparation (async — lock released).
        if matches!(
            budget_directive,
            super::context_budget::LoopDirective::Continue
                | super::context_budget::LoopDirective::CompactAndContinue
        ) {
            if let Some(ref pressure) = preflight_pressure {
                let preflight_freed = self
                    .preflight_pipeline
                    .run(messages, pressure, preflight_fresh_tail)
                    .await;
                if preflight_freed > 0 {
                    tracing::info!(
                        target: "agent_loop",
                        tokens_freed = preflight_freed,
                        "Pre-flight context preparation freed tokens"
                    );
                }
            }
        }

        // Phase 3: Re-acquire lock for compaction pipeline.
        let (budget_fresh_tail, budget_ratio) = {
            let mut ctx_budget_ref = self
                .context_budget
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if has_budget {
                if let Some(ref mut ctx_budget) = *ctx_budget_ref {
                    match budget_directive {
                        super::context_budget::LoopDirective::CompactAndContinue => {
                            let result = {
                                let sensor = self
                                    .pressure_sensor
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                self.compaction_pipeline.run(
                                    messages,
                                    &sensor,
                                    system_prompt,
                                    tool_defs,
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
                        super::context_budget::LoopDirective::FinalReply => {
                            let result = {
                                let sensor = self
                                    .pressure_sensor
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                self.compaction_pipeline.run(
                                    messages,
                                    &sensor,
                                    system_prompt,
                                    tool_defs,
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
                    (
                        ctx_budget.fresh_tail_count(),
                        ctx_budget.token_estimate_ratio(),
                    )
                } else {
                    // Budget was removed between phases — fall back to defaults.
                    (6_usize, 3.5_f64)
                }
            } else {
                let sensor = self
                    .pressure_sensor
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let pressure = sensor.measure(
                    messages,
                    system_prompt,
                    tool_defs,
                    self.config.token_budget as u64,
                );
                if pressure.ratio >= 0.85 {
                    self.compaction_pipeline.run(
                        messages,
                        &sensor,
                        system_prompt,
                        tool_defs,
                        self.config.token_budget as u64,
                        0.70,
                        6,
                    );
                }
                (6_usize, 3.5_f64)
            }
        };

        if let Some(ref orchestrator) = self.compaction_orchestrator {
            let (pressure, pressure_level) = {
                let budget = self
                    .context_budget
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let pressure = budget.as_ref().and_then(|b| b.last_pressure()).cloned();
                let level = pressure
                    .as_ref()
                    .map(|p| super::compaction::PressureLevel::from_ratio(p.ratio))
                    .unwrap_or(super::compaction::PressureLevel::Calm);
                (pressure, level)
            };
            if pressure_level >= super::compaction::PressureLevel::Warning {
                if let Some(pressure) = pressure {
                    let mut ctx = super::compaction::CompactionContext {
                        messages: std::mem::take(messages),
                        pressure,
                        pressure_level,
                        token_estimate_ratio: budget_ratio,
                        fresh_tail_count: budget_fresh_tail,
                    };
                    let execute_result = orchestrator.execute(&mut ctx).await;
                    // ALWAYS restore messages before inspecting the result so
                    // that a panic or early-return can never leave `messages` empty.
                    *messages = ctx.messages;
                    match execute_result {
                        Ok(result) => {
                            if result.pressure_reduced() {
                                let mut budget = self
                                    .context_budget
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                if let Some(ref mut b) = *budget {
                                    b.notify_compaction_success();
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("compaction orchestrator failed: {e}");
                        }
                    }
                }
            }
        }

        enforce_context_limit(
            messages,
            system_prompt,
            tool_defs,
            self.config.token_budget,
            budget_fresh_tail,
            budget_ratio,
        );

        TurnBudgetState {
            directive: budget_directive,
            fresh_tail_count: budget_fresh_tail,
        }
    }

    async fn think_turn(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
        budget: TurnBudgetState,
        progress: &mut LoopProgress,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<ThinkTurnResult> {
        let mut ptl_attempts: usize = 0;
        // Apply runtime security guard to outbound messages
        for msg in messages.iter_mut() {
            let content_blocks = match msg {
                UnifiedMessage::User { content } => Some(content),
                UnifiedMessage::Assistant { content } => Some(content),
                UnifiedMessage::ToolResult { content, .. } => Some(content),
            };
            if let Some(blocks) = content_blocks {
                for block in blocks.iter_mut() {
                    if let ContentBlock::Text { text, .. } = block {
                        let context = SecurityContext {
                            provider_name: self.provider_name.clone(),
                            platform_name: self.platform_name.clone(),
                            session_id: self.session_id.clone(),
                            ..Default::default()
                        };
                        match self
                            .security_guard
                            .process_outbound(text, self.secret_resolver.as_deref(), context)
                            .await
                        {
                            Ok(GuardResult::Clean { text: t })
                            | Ok(GuardResult::Redacted { text: t, .. })
                            | Ok(GuardResult::Warned { text: t, .. }) => {
                                *text = t;
                            }
                            Ok(GuardResult::Blocked { reason, .. }) => {
                                return Err(anyhow::anyhow!(
                                    "Security blocked outbound content: {}",
                                    reason
                                ));
                            }
                            Err(e) => {
                                return Err(anyhow::anyhow!(
                                    "Security guard outbound error: {}",
                                    e
                                ));
                            }
                        }
                    }
                }
            }
        }

        let delta_stream = loop {
            let active_provider = progress.active_provider;
            match crate::providers::llm_retry::retry_async(
                || {
                    let p: &dyn LoopProvider = match active_provider {
                        ActiveProvider::Primary => &self.provider,
                        ActiveProvider::Fallback => {
                            self.fallback_provider.as_ref().unwrap().as_ref()
                        }
                    };
                    p.stream(messages.as_slice(), system_prompt, tool_defs)
                },
                &self.cancel_token,
                3,
            )
            .await
            {
                Ok(stream) => break stream,
                Err(e) => {
                    let verdict = crate::providers::llm_retry::classify_exhausted_error(&e);
                    match verdict {
                        crate::providers::llm_retry::RetryVerdict::CompactAndRetry {
                            token_gap,
                        } => {
                            ptl_attempts += 1;
                            if ptl_attempts > MAX_PTL_RETRIES {
                                return Err(e);
                            }
                            tracing::warn!(
                                attempt = ptl_attempts,
                                ?token_gap,
                                "413 recovery: multi-tier cascade"
                            );

                            // Tier 1: Pre-flight pipeline (microcompact → collapse → autocompact)
                            {
                                let pressure = {
                                    let sensor = self
                                        .pressure_sensor
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    sensor.measure(
                                        messages,
                                        system_prompt,
                                        tool_defs,
                                        self.config.token_budget as u64,
                                    )
                                };
                                let preflight_freed = self
                                    .preflight_pipeline
                                    .run(messages, &pressure, budget.fresh_tail_count)
                                    .await;
                                if preflight_freed > 0 {
                                    tracing::info!(
                                        target: "agent_loop",
                                        freed = preflight_freed,
                                        "413 Tier 1: pre-flight freed tokens"
                                    );
                                }
                            }

                            // Tier 2: Emergency truncation (existing logic)
                            emergency_truncate(messages, token_gap, budget.fresh_tail_count);

                            // Tier 3: Aggressive round drop with halved fresh_tail
                            // Only needed if Tier 2 didn't free enough (checked on next
                            // retry attempt via another 413). Apply preemptively on
                            // attempt >= 2 to avoid wasting a round-trip.
                            if ptl_attempts >= 2 {
                                use super::context_budget::pipeline::{CompactionStage, RoundDrop};
                                let aggressive_tail = (budget.fresh_tail_count / 2).max(2);
                                let stage = RoundDrop {
                                    token_budget: self.config.token_budget as u64,
                                    ratio: 0.85,
                                };
                                let tier3_freed = stage.compact(messages, aggressive_tail);
                                if tier3_freed > 0 {
                                    tracing::info!(
                                        target: "agent_loop",
                                        freed = tier3_freed,
                                        aggressive_tail,
                                        "413 Tier 3: aggressive round drop freed tokens"
                                    );
                                }
                            }

                            continue;
                        }
                        crate::providers::llm_retry::RetryVerdict::Fallback { reason }
                            if self.fallback_provider.is_some()
                                && matches!(progress.active_provider, ActiveProvider::Primary) =>
                        {
                            progress.active_provider = ActiveProvider::Fallback;
                            let label = self.fallback_label.as_deref().unwrap_or("fallback");
                            tracing::warn!(
                                %reason,
                                fallback = label,
                                "Switching to fallback model"
                            );
                            callback.on_model_fallback(&reason, label);
                            continue;
                        }
                        _ => return Err(e),
                    }
                }
            }
        };

        let prefetch_handle = self.skill_prefetcher.as_ref().and_then(|p| p.start_scan());
        let (mut bridge, executor) = crate::session::streaming::StreamingToolBridge::new(
            Arc::clone(&self.tool_registry.read().unwrap_or_else(|e| e.into_inner())),
            Arc::clone(&self.tool_pipeline),
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
                        Some(Err(e)) => {
                            executor_handle.abort();
                            return Err(e);
                        }
                        None => break,
                    }
                }
                _ = self.cancel_token.cancelled() => {
                    executor_handle.abort();
                    return Ok(ThinkTurnResult::Cancelled);
                }
            }
        }
        bridge.finish();

        let mut response = collector.finish();

        // Apply runtime security guard to inbound response
        if let Some(text) = response.text.as_ref() {
            match self.security_guard.process_inbound(text) {
                Ok(GuardResult::Clean { text: t })
                | Ok(GuardResult::Redacted { text: t, .. })
                | Ok(GuardResult::Warned { text: t, .. }) => {
                    response.text = Some(t);
                }
                Ok(GuardResult::Blocked { reason, .. }) => {
                    return Err(anyhow::anyhow!(
                        "Security blocked inbound content: {}",
                        reason
                    ));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Security guard inbound error: {}", e));
                }
            }
        }

        self.security_guard.clear_injected_secrets();

        Ok(ThinkTurnResult::Ready(TurnThinkingState {
            budget,
            response,
            prefetch_handle,
            executor_handle,
        }))
    }

    fn record_response_usage(
        &self,
        response: &ProviderResponse,
        messages_len: usize,
        progress: &mut LoopProgress,
    ) {
        if let Some(usage) = &response.usage {
            progress.total_tokens += (usage.input_tokens + usage.output_tokens) as usize;
            self.pressure_sensor
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .update_anchor(usage.input_tokens as usize, messages_len);
        }
    }

    fn emit_text_trace(
        iteration: usize,
        stream: LoopTraceTextKind,
        text: &str,
        callback: &mut dyn LoopCallback,
    ) {
        callback.on_trace(&LoopTraceEvent::TextEmitted {
            iteration,
            stream,
            text: text.to_string(),
        });
    }

    fn emit_tool_summary_trace(iteration: usize, summary: String, callback: &mut dyn LoopCallback) {
        callback.on_trace(&LoopTraceEvent::ToolSummary { iteration, summary });
    }

    fn process_response_text(
        &self,
        response: &ProviderResponse,
        progress: &mut LoopProgress,
        callback: &mut dyn LoopCallback,
    ) {
        if let Some(text) = &response.text {
            if response.has_tool_calls() {
                Self::emit_text_trace(
                    progress.iterations,
                    LoopTraceTextKind::Intermediate,
                    text,
                    callback,
                );
                progress.intermediate_texts.push(text.clone());
            } else {
                let cleaned = strip_repeated_intermediate(text, &progress.intermediate_texts);
                Self::emit_text_trace(
                    progress.iterations,
                    LoopTraceTextKind::Final,
                    &cleaned,
                    callback,
                );
                let is_recovering = {
                    let recovery = self
                        .truncation_recovery
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    *recovery.phase() != super::truncation_recovery::RecoveryPhase::Idle
                };
                if let Some(ref mut existing) = progress.final_text {
                    if is_recovering {
                        existing.push_str(&cleaned);
                    } else {
                        *existing = cleaned;
                    }
                } else {
                    progress.final_text = Some(cleaned);
                }
            }
        }
    }

    async fn maybe_blocked_by_stop_hooks(
        &self,
        stop_reason: &str,
        progress: &mut LoopProgress,
        messages: &mut Vec<UnifiedMessage>,
        callback: &mut dyn LoopCallback,
    ) -> bool {
        if self.stop_hooks.is_empty() || progress.stop_hook_blocks >= MAX_STOP_HOOK_BLOCKS {
            return false;
        }

        let ctx = StopHookContext {
            final_text: progress.final_text.clone(),
            iterations: progress.iterations,
            tool_calls_made: progress.tool_calls_made,
            stop_reason: stop_reason.to_string(),
        };
        let hook_result =
            stop_hooks::execute_stop_hooks(&self.stop_hooks, &ctx, &self.cancel_token).await;
        for (name, msg) in hook_result.errors() {
            callback.on_stop_hook_error(name, msg);
        }
        if let Some(reason) = hook_result.blocking_reason() {
            progress.stop_hook_blocks += 1;
            callback.on_stop_hook_block(reason);
            messages.push(UnifiedMessage::user(format!(
                "[SYSTEM] Stop hook blocked exit: {reason}. Address the issue and try again."
            )));
            return true;
        }

        false
    }

    async fn resolve_turn_response(
        &self,
        thinking: TurnThinkingState,
        messages: &mut Vec<UnifiedMessage>,
        progress: &mut LoopProgress,
        callback: &mut dyn LoopCallback,
    ) -> TurnResolve {
        let TurnThinkingState {
            budget,
            response,
            prefetch_handle,
            executor_handle,
        } = thinking;

        let has_tool_calls = response.has_tool_calls();
        if !has_tool_calls {
            executor_handle.abort();
        }

        self.record_response_usage(&response, messages.len(), progress);
        self.process_response_text(&response, progress, callback);
        messages.push(UnifiedMessage::from_provider_response(&response));

        if response.stop_reason != StopReason::MaxTokens {
            let mut recovery = self
                .truncation_recovery
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(original) = recovery.reset() {
                progress.current_max_tokens = Some(original);
            } else {
                progress.current_max_tokens = None;
            }
        }

        if !has_tool_calls && response.stop_reason == StopReason::EndTurn {
            if progress.tool_calls_made == 0 {
                return TurnResolve::Finalize(TurnArtifacts {
                    response,
                    prefetch_handle,
                    tools: ToolTurnArtifacts::idle(progress.consecutive_errors),
                    exit_request: TurnExitRequest::Stop,
                    final_text_update: FinalTextUpdate::None,
                });
            }

            let has_completion_tag = response
                .text
                .as_ref()
                .is_some_and(|t| t.contains("<task-complete/>"));

            if has_completion_tag {
                if self
                    .maybe_blocked_by_stop_hooks("end_turn", progress, messages, callback)
                    .await
                {
                    return TurnResolve::Restart;
                }
                return TurnResolve::Finalize(TurnArtifacts {
                    response,
                    prefetch_handle,
                    tools: ToolTurnArtifacts::idle(progress.consecutive_errors),
                    exit_request: TurnExitRequest::Stop,
                    final_text_update: FinalTextUpdate::None,
                });
            }

            if progress.completion_nudge_count < MAX_COMPLETION_NUDGES {
                progress.completion_nudge_count += 1;
                tracing::info!(
                    iteration = progress.iterations,
                    nudge = progress.completion_nudge_count,
                    "Completion protocol: LLM stopped without <task-complete/>, injecting nudge"
                );

                let nudge_msg = if progress.completion_nudge_count < MAX_COMPLETION_NUDGES {
                    "[SYSTEM] You stopped but have not confirmed task completion. \
                     Do NOT apologize or explain. Review your work against the original request: \
                     is every requirement met? If not, try a different approach. \
                     When fully done, output a <completion-check> block and <task-complete/>."
                } else {
                    "[SYSTEM] Final attempt. Summarize: (1) what approaches you tried, \
                     (2) what succeeded and what failed, (3) what the user should do next. \
                     Then output <task-complete/>."
                };

                messages.push(UnifiedMessage::user(nudge_msg));
                return TurnResolve::Restart;
            }

            if self
                .maybe_blocked_by_stop_hooks("nudge_exhausted", progress, messages, callback)
                .await
            {
                return TurnResolve::Restart;
            }

            return TurnResolve::Finalize(TurnArtifacts {
                response,
                prefetch_handle,
                tools: ToolTurnArtifacts::idle(progress.consecutive_errors),
                exit_request: TurnExitRequest::Stop,
                final_text_update: FinalTextUpdate::None,
            });
        }

        if !has_tool_calls && response.stop_reason == StopReason::MaxTokens {
            let action = {
                let mut recovery = self
                    .truncation_recovery
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                recovery.on_truncation(
                    progress.current_max_tokens,
                    response.text.as_deref().unwrap_or(""),
                )
            };
            if action.should_continue {
                if let Some(override_val) = action.max_tokens_override {
                    progress.current_max_tokens = Some(override_val);
                }
                messages.push(UnifiedMessage::user(&action.continuation_prompt));
                tracing::info!(
                    iteration = progress.iterations,
                    escalated = action.max_tokens_override.is_some(),
                    "truncation recovery: continuing"
                );
                return TurnResolve::Restart;
            }

            let recovery = self
                .truncation_recovery
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let assembled = recovery.assemble_output();
            let notice = "\n\n---\n⚠️ 输出因 token 限制被截断。请回复「继续」获取剩余内容。";
            Self::emit_text_trace(
                progress.iterations,
                LoopTraceTextKind::Final,
                notice,
                callback,
            );
            tracing::warn!(
                iterations = progress.iterations,
                "truncation recovery exhausted, assembling fragments"
            );
            return TurnResolve::Finalize(TurnArtifacts {
                response,
                prefetch_handle,
                tools: ToolTurnArtifacts::idle(progress.consecutive_errors),
                exit_request: TurnExitRequest::HitLimit,
                final_text_update: FinalTextUpdate::Replace(format!("{assembled}{notice}")),
            });
        }

        if !has_tool_calls {
            return TurnResolve::Finalize(TurnArtifacts {
                response,
                prefetch_handle,
                tools: ToolTurnArtifacts::idle(progress.consecutive_errors),
                exit_request: TurnExitRequest::Stop,
                final_text_update: FinalTextUpdate::None,
            });
        }

        TurnResolve::Act(TurnActState {
            thinking: TurnThinkingState {
                budget,
                response,
                prefetch_handle,
                executor_handle,
            },
            skip_tools: matches!(
                budget.directive,
                super::context_budget::LoopDirective::FinalReply
                    | super::context_budget::LoopDirective::StopDiminishing
            ),
        })
    }

    async fn execute_turn_tools(
        &self,
        turn: TurnActState,
        messages: &mut Vec<UnifiedMessage>,
        iteration: usize,
        current_error_streak: usize,
        last_intermediate_text: Option<String>,
        callback: &mut dyn LoopCallback,
    ) -> TurnArtifacts {
        let TurnActState {
            thinking,
            skip_tools,
        } = turn;
        let TurnThinkingState {
            budget: _,
            response,
            prefetch_handle,
            executor_handle,
        } = thinking;

        let mut tools = ToolTurnArtifacts::requested(
            response.tool_calls.len(),
            current_error_streak,
            skip_tools,
        );
        let mut exit_request = TurnExitRequest::None;
        let mut final_text_update = FinalTextUpdate::None;

        if skip_tools {
            executor_handle.abort();
            for tc in &response.tool_calls {
                messages.push(UnifiedMessage::tool_result(
                    tc.id.clone(),
                    tc.name.clone(),
                    "[SYSTEM] Tool execution skipped — context budget exhausted. Provide your best response now.",
                    true,
                ));
            }
        } else {
            let outcomes = match executor_handle.await {
                Ok(results) => results,
                Err(e) => {
                    tracing::warn!("streaming tool executor panicked: {e}");
                    vec![]
                }
            };

            if let Some(ref gate) = self.approval_gate {
                let tool_names: Vec<&str> = response
                    .tool_calls
                    .iter()
                    .map(|tc| tc.name.as_str())
                    .collect();
                let decision = gate.parse_and_decide(&response, &tool_names);
                tracing::info!(
                    "Exec approval decision: {:?}, reason: {}",
                    decision.action,
                    decision.reason
                );
            }

            let tool_args_by_id: std::collections::HashMap<&str, &Value> = response
                .tool_calls
                .iter()
                .map(|tc| (tc.id.as_str(), &tc.arguments))
                .collect();

            for outcome in &outcomes {
                let o = &outcome.outcome;
                tools.last_tool_name = Some(o.tool_name.clone());

                // Confirmation flow: if pipeline flagged confirmation needed,
                // ask the channel before treating as error.
                if outcome.needs_user_confirmation {
                    let reason = outcome
                        .confirmation_reason
                        .as_deref()
                        .unwrap_or("Tool requires confirmation");
                    let tool_input = tool_args_by_id
                        .get(o.tool_id.as_str())
                        .map(|v| (*v).clone())
                        .unwrap_or(serde_json::json!({}));
                    let confirmed =
                        callback.on_confirmation_needed(&o.tool_name, &tool_input, reason);
                    if !confirmed {
                        // User rejected — override output to denial message
                        let denial_msg = format!(
                            "[DENIED] Tool '{}' requires user confirmation. Confirmation was rejected.",
                            o.tool_name
                        );
                        callback.on_safety_block(&SafetyError::NeedsConfirmation {
                            tool: o.tool_name.clone(),
                        });
                        messages.push(UnifiedMessage::tool_result(
                            o.tool_id.clone(),
                            o.tool_name.clone(),
                            denial_msg,
                            true,
                        ));
                        continue; // skip normal outcome processing for this tool
                    }
                    // If confirmed, fall through to normal processing — tool already executed
                }

                let is_safety_denial = o.is_error
                    && (o.output_text.starts_with("[BLOCKED]")
                        || o.output_text.starts_with("[DENIED]"));

                if !is_safety_denial {
                    let args = tool_args_by_id
                        .get(o.tool_id.as_str())
                        .copied()
                        .cloned()
                        .unwrap_or(Value::Null);
                    callback.on_trace(&LoopTraceEvent::ToolCallStarted {
                        iteration,
                        call: ToolCallStartEvent {
                            tool_id: o.tool_id.clone(),
                            tool_name: o.tool_name.clone(),
                            input: args,
                        },
                    });
                }

                let tool_result = if o.is_error {
                    ToolResult::Error {
                        error: o.output_text.clone(),
                        retryable: o.retryable,
                    }
                } else if o.should_stop {
                    ToolResult::SuccessAndStopLoop {
                        output: Value::String(o.output_text.clone()),
                    }
                } else {
                    ToolResult::Success {
                        output: Value::String(o.output_text.clone()),
                    }
                };

                if !is_safety_denial {
                    let args = tool_args_by_id
                        .get(o.tool_id.as_str())
                        .copied()
                        .cloned()
                        .unwrap_or(Value::Null);
                    callback.on_trace(&LoopTraceEvent::ToolCallCompleted {
                        iteration,
                        call: ToolCallEndEvent {
                            tool_id: o.tool_id.clone(),
                            tool_name: o.tool_name.clone(),
                            input: args,
                            duration_ms: o.duration_ms,
                        },
                        result: tool_result.clone(),
                    });
                }

                if o.is_error {
                    let is_non_counting =
                        is_safety_denial || o.output_text.starts_with("[CANCELLED]");
                    if is_non_counting {
                        if o.output_text.starts_with("[BLOCKED]") {
                            callback.on_safety_block(&SafetyError::Blocked {
                                tool: o.tool_name.clone(),
                                pattern: String::new(),
                            });
                        } else if o.output_text.starts_with("[DENIED]") {
                            callback.on_safety_block(&SafetyError::PolicyDenied {
                                tool: o.tool_name.clone(),
                            });
                        }
                    } else if !o.retryable {
                        tools.next_error_streak += 1;
                    }
                } else {
                    tools.next_error_streak = 0;
                }

                messages.push(UnifiedMessage::tool_result(
                    o.tool_id.clone(),
                    o.tool_name.clone(),
                    o.output_text.clone(),
                    o.is_error,
                ));

                if o.should_stop {
                    if matches!(final_text_update, FinalTextUpdate::None) {
                        final_text_update = FinalTextUpdate::SetIfEmpty(o.output_text.clone());
                    }
                    exit_request = TurnExitRequest::Stop;
                }

                if outcome.prevent_continuation {
                    exit_request = TurnExitRequest::Stop;
                }
            }

            let mut hook_parts: Vec<String> = Vec::new();
            for pipeline_outcome in &outcomes {
                for ctx in &pipeline_outcome.additional_contexts {
                    hook_parts.push(format!("<system-reminder>\n{}\n</system-reminder>", ctx));
                }
                for msg in &pipeline_outcome.hook_messages {
                    hook_parts.push(format!("<system-reminder>\n{}\n</system-reminder>", msg));
                }
            }
            if !hook_parts.is_empty() {
                messages.push(UnifiedMessage::user(hook_parts.join("\n")));
            }

            tools.executed_calls = outcomes.len();

            if let Some(ref sp) = self.summary_provider {
                let inputs: Vec<super::tool_summary::ToolSummaryInput> = outcomes
                    .iter()
                    .map(|pipeline_outcome| {
                        let o = &pipeline_outcome.outcome;
                        let tool_input = tool_args_by_id
                            .get(o.tool_id.as_str())
                            .map(|v| serde_json::to_string(v).unwrap_or_default())
                            .unwrap_or_default();
                        super::tool_summary::ToolSummaryInput {
                            tool_name: o.tool_name.clone(),
                            tool_input,
                            tool_output: o.output_text.clone(),
                        }
                    })
                    .collect();
                let sp = sp.clone();
                tools.summary_handle = Some(tokio::spawn(async move {
                    super::tool_summary::generate_tool_summary(
                        &*sp,
                        &inputs,
                        last_intermediate_text.as_deref(),
                    )
                    .await
                }));
            }
        }

        TurnArtifacts {
            response,
            prefetch_handle,
            tools,
            exit_request,
            final_text_update,
        }
    }

    async fn finalize_turn(
        &mut self,
        turn: TurnArtifacts,
        messages: &mut Vec<UnifiedMessage>,
        system_prompt: &mut String,
        tool_defs: &mut Vec<ToolDefinition>,
        progress: &mut LoopProgress,
    ) -> TurnLoopDecision {
        let TurnArtifacts {
            response,
            prefetch_handle,
            tools,
            exit_request,
            final_text_update,
        } = turn;
        let turn_productive = tools.productive();
        let last_tool_name = tools.last_tool_name.clone();

        if let Some(ref refresh_source) = self.tool_refresh {
            if refresh_source.poll_changes() {
                let new_tools = refresh_source.fetch_tools();
                let new_registry = super::tool_refresh::build_refreshed_registry(new_tools);
                *self
                    .tool_registry
                    .write()
                    .unwrap_or_else(|e| e.into_inner()) = Arc::new(new_registry);
                *tool_defs = self
                    .tool_registry
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .tool_definitions();
                tracing::info!(
                    chain_id = %self.chain.chain_id,
                    tools = tool_defs.len(),
                    "tool registry refreshed"
                );
            }
        }

        if let Some(handle) = prefetch_handle {
            match handle.await {
                Ok(Some(new_skills)) => {
                    if !new_skills.is_empty() {
                        let lines: Vec<String> = new_skills
                            .iter()
                            .map(|s| format!("- **{}**: {}", s.name, s.description))
                            .collect();
                        let content = format!("## Discovered Skills\n{}", lines.join("\n"));
                        system_prompt.push_str("\n\n---\n\n");
                        system_prompt.push_str(&content);
                    }
                    if let Some(ref prefetcher) = self.skill_prefetcher {
                        prefetcher.commit(new_skills);
                    }
                    tracing::info!(
                        chain_id = %self.chain.chain_id,
                        "skill prefetch: new skills discovered"
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("skill prefetch task failed: {e}");
                }
            }
        }

        progress.consecutive_errors = tools.next_error_streak;
        progress.tool_calls_made += tools.executed_calls;
        if let Some(handle) = tools.summary_handle {
            if let Some(previous) = progress.pending_summary.take() {
                previous.abort();
            }
            progress.pending_summary = Some(handle);
        }

        {
            let mut ctx_budget_ref = self
                .context_budget
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut ctx_budget) = *ctx_budget_ref {
                let output_tokens = response
                    .usage
                    .as_ref()
                    .map(|u| u.output_tokens as usize)
                    .unwrap_or(0);
                let post_directive = ctx_budget.after_turn(super::context_budget::TurnMetrics {
                    output_tokens,
                    tool_calls: response.tool_calls.len(),
                    productive: turn_productive,
                });
                if post_directive == super::context_budget::LoopDirective::StopDiminishing {
                    messages.push(UnifiedMessage::user(DIMINISHING_RETURNS_NOTICE));
                }
            }
        }

        if progress.consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
            return TurnLoopDecision::Exit(LoopExitDecision {
                hit_limit: true,
                final_text_update: FinalTextUpdate::Replace(format!(
                    "Tool execution failed repeatedly ({} consecutive errors). The last error was for tool '{}'. Please try rephrasing your request.",
                    progress.consecutive_errors,
                    last_tool_name.as_deref().unwrap_or("unknown")
                )),
            });
        }

        match exit_request {
            TurnExitRequest::Stop => {
                return TurnLoopDecision::Exit(LoopExitDecision {
                    hit_limit: false,
                    final_text_update,
                });
            }
            TurnExitRequest::HitLimit => {
                return TurnLoopDecision::Exit(LoopExitDecision {
                    hit_limit: true,
                    final_text_update,
                });
            }
            TurnExitRequest::None => {}
        }

        if progress.total_tokens >= self.config.token_budget {
            return TurnLoopDecision::Exit(LoopExitDecision {
                hit_limit: true,
                final_text_update,
            });
        }

        TurnLoopDecision::Continue
    }

    async fn execute_turn_state(
        &mut self,
        state: TurnState,
        runtime: &mut LoopRuntime,
        progress: &mut LoopProgress,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<TurnExecution> {
        Ok(match state {
            TurnState::Prepare => TurnExecution::Prepared(
                self.prepare_turn(
                    &mut runtime.messages,
                    &runtime.system_prompt,
                    &runtime.tool_defs,
                    progress,
                    callback,
                )
                .await,
            ),
            TurnState::Think(budget) => TurnExecution::Thought(
                self.think_turn(
                    &mut runtime.messages,
                    &runtime.system_prompt,
                    &runtime.tool_defs,
                    budget,
                    progress,
                    callback,
                )
                .await?,
            ),
            TurnState::Resolve(thinking) => TurnExecution::Resolved(
                self.resolve_turn_response(thinking, &mut runtime.messages, progress, callback)
                    .await,
            ),
            TurnState::Act(turn) => TurnExecution::Acted(
                self.execute_turn_tools(
                    turn,
                    &mut runtime.messages,
                    progress.iterations,
                    progress.consecutive_errors,
                    progress.intermediate_texts.last().cloned(),
                    callback,
                )
                .await,
            ),
            TurnState::Finalize(turn) => TurnExecution::Finalized(
                self.finalize_turn(
                    turn,
                    &mut runtime.messages,
                    &mut runtime.system_prompt,
                    &mut runtime.tool_defs,
                    progress,
                )
                .await,
            ),
        })
    }

    fn reduce_turn_execution(execution: TurnExecution) -> TurnAdvance {
        match execution {
            TurnExecution::Prepared(budget) => TurnAdvance::Next(TurnState::Think(budget)),
            TurnExecution::Thought(ThinkTurnResult::Ready(thinking)) => {
                TurnAdvance::Next(TurnState::Resolve(thinking))
            }
            TurnExecution::Thought(ThinkTurnResult::Cancelled) => TurnAdvance::Cancelled,
            TurnExecution::Resolved(TurnResolve::Restart) => TurnAdvance::ContinueLoop,
            TurnExecution::Resolved(TurnResolve::Act(turn)) => {
                TurnAdvance::Next(TurnState::Act(turn))
            }
            TurnExecution::Resolved(TurnResolve::Finalize(turn)) => {
                TurnAdvance::Next(TurnState::Finalize(turn))
            }
            TurnExecution::Acted(turn) => TurnAdvance::Next(TurnState::Finalize(turn)),
            TurnExecution::Finalized(TurnLoopDecision::Continue) => TurnAdvance::ContinueLoop,
            TurnExecution::Finalized(TurnLoopDecision::Exit(decision)) => {
                TurnAdvance::ExitLoop(decision)
            }
        }
    }

    async fn run_turn(
        &mut self,
        runtime: &mut LoopRuntime,
        progress: &mut LoopProgress,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<TurnRunOutcome> {
        let iteration = progress.iterations;
        let mut trace = TurnTraceFrame::default();
        callback.on_trace(&LoopTraceEvent::TurnStarted { iteration });
        let mut state = TurnState::Prepare;
        loop {
            callback.on_trace(&LoopTraceEvent::TurnStateEntered {
                iteration,
                state: state.trace_state(),
            });
            let execution = self
                .execute_turn_state(state, runtime, progress, callback)
                .await?;
            match &execution {
                TurnExecution::Resolved(TurnResolve::Finalize(turn))
                | TurnExecution::Acted(turn) => trace.observe_artifacts(turn),
                TurnExecution::Prepared(_)
                | TurnExecution::Thought(_)
                | TurnExecution::Resolved(TurnResolve::Restart)
                | TurnExecution::Resolved(TurnResolve::Act(_))
                | TurnExecution::Finalized(_) => {}
            }
            match Self::reduce_turn_execution(execution) {
                TurnAdvance::Next(next_state) => {
                    state = next_state;
                }
                TurnAdvance::ContinueLoop => {
                    callback.on_trace(&LoopTraceEvent::TurnCompleted {
                        iteration,
                        outcome: LoopTraceTurnOutcome::Continue,
                        metrics: trace.snapshot(progress),
                    });
                    return Ok(TurnRunOutcome::Continue);
                }
                TurnAdvance::ExitLoop(decision) => {
                    callback.on_trace(&LoopTraceEvent::TurnCompleted {
                        iteration,
                        outcome: if decision.hit_limit {
                            LoopTraceTurnOutcome::HitLimit
                        } else {
                            LoopTraceTurnOutcome::Stop
                        },
                        metrics: trace.snapshot(progress),
                    });
                    return Ok(TurnRunOutcome::Exit(decision));
                }
                TurnAdvance::Cancelled => {
                    callback.on_trace(&LoopTraceEvent::TurnCompleted {
                        iteration,
                        outcome: LoopTraceTurnOutcome::Cancelled,
                        metrics: trace.snapshot(progress),
                    });
                    return Ok(TurnRunOutcome::Cancelled);
                }
            }
        }
    }

    /// Run the agent loop with the given user input.
    pub async fn run(
        &mut self,
        input: &str,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<LoopRunResult> {
        self.run_with_history(input, Vec::new(), callback).await
    }

    /// Run the agent loop with conversation history prepended.
    pub async fn run_with_history(
        &mut self,
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
        &mut self,
        messages: Vec<UnifiedMessage>,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<LoopRunResult> {
        tracing::info!(
            chain_id = %self.chain.chain_id,
            depth = self.chain.depth,
            "agent_loop: starting"
        );

        // Session-level hook: SessionStart (observers only)
        if self.tool_pipeline.has_hooks() {
            let ctx = HookContext::new(&self.chain.chain_id);
            self.tool_pipeline
                .hooks()
                .execute_observers(HookEvent::SessionStart, &ctx)
                .await;
        }

        // Build system prompt with tool info
        let tool_infos: Vec<ToolInfo> = self
            .tool_registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .tool_definitions()
            .iter()
            .map(|td| ToolInfo {
                name: td.name.clone(),
                description: td.description.clone(),
                parameters_schema: Some(td.parameters.clone()),
                usage_hint: None,
            })
            .collect();
        // Build base prompt via the pipeline, then append dynamic sections
        let mut system_prompt = self.prompt_builder.build_system_prompt(&tool_infos);

        // Append session guidance (tool-aware behavioral hints)
        let tool_names: Vec<&str> = tool_infos.iter().map(|t| t.name.as_str()).collect();
        {
            let has = |prefix: &str| tool_names.iter().any(|t| t.starts_with(prefix));
            let mut rules: Vec<String> = Vec::new();
            if has("skill_") {
                rules.push(
                    "- /<skill-name> is shorthand for users to invoke a skill. \
                     When a user message starts with /, use the skill tool to execute it."
                        .into(),
                );
            }
            if has("subagent") || has("agent_") {
                rules.push(
                    "- Use the agent/sub-agent tool for complex, multi-step sub-tasks. \
                     Launch multiple agents concurrently for independent tasks."
                        .into(),
                );
            }
            if tool_names.contains(&"session_search") {
                rules.push(
                    "- Use session_search to find information from past conversations \
                     before asking the user to repeat themselves."
                        .into(),
                );
            }
            if has("memory_") {
                rules.push(
                    "- Save user corrections and preferences to memory immediately. \
                     This prevents repeating mistakes in future sessions."
                        .into(),
                );
            }
            if !rules.is_empty() {
                let guidance_content = format!("# Session Guidance\n\n{}", rules.join("\n"));
                system_prompt.push_str("\n\n---\n\n");
                system_prompt.push_str(&guidance_content);
            }
        }

        // Append environment info
        if let Some(ref ctx) = self.session_context {
            let env = crate::context::EnvironmentInfo {
                cwd: ctx.cwd.clone(),
                is_git: ctx.git_branch.is_some(),
                git_branch: ctx.git_branch.clone(),
                os: ctx.os.clone(),
                os_version: String::new(),
                shell: ctx.shell.clone(),
                date: chrono::Local::now().format("%Y-%m-%d").to_string(),
                model_name: None,
                knowledge_cutoff: None,
            };
            {
                let mut lines = Vec::new();
                lines.push(format!("- Working directory: {}", env.cwd));
                lines.push(format!("- Is git repository: {}", env.is_git));
                if let Some(branch) = &env.git_branch {
                    lines.push(format!("- Git branch: {}", branch));
                }
                lines.push(format!("- Platform: {}", env.os));
                lines.push(format!("- OS Version: {}", env.os_version));
                lines.push(format!("- Shell: {}", env.shell));
                lines.push(format!("- Date: {}", env.date));
                if let Some(model) = &env.model_name {
                    lines.push(format!("- Model: {}", model));
                }
                if let Some(cutoff) = &env.knowledge_cutoff {
                    lines.push(format!("- Knowledge cutoff: {}", cutoff));
                }
                let env_content = format!("# Environment\n\n{}", lines.join("\n"));
                system_prompt.push_str("\n\n---\n\n");
                system_prompt.push_str(&env_content);
            }
        }

        let mut runtime = LoopRuntime {
            messages,
            system_prompt,
            tool_defs: self
                .tool_registry
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .tool_definitions(),
        };

        // Capture prompt snapshot for fork path (once)
        if let Some(ref shared) = self.shared_snapshot {
            let mut guard = shared.write().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                *guard = Some(self.prompt_builder.capture_snapshot(&tool_infos));
            }
        }

        let mut progress = LoopProgress::default();
        let mut exit_decision = LoopExitDecision::default();

        while progress.iterations < self.config.max_iterations {
            progress.iterations += 1;

            match self.run_turn(&mut runtime, &mut progress, callback).await? {
                TurnRunOutcome::Continue => {}
                TurnRunOutcome::Exit(decision) => {
                    exit_decision = decision;
                    break;
                }
                TurnRunOutcome::Cancelled => {
                    callback.on_trace(&LoopTraceEvent::SessionCompleted {
                        outcome: LoopTraceSessionOutcome::Cancelled,
                        iterations: progress.iterations,
                        tool_calls_made: progress.tool_calls_made,
                        total_tokens: progress.total_tokens,
                        hit_limit: false,
                        final_text: progress.final_text.clone(),
                    });
                    return Ok(progress.cancelled_result(&self.chain));
                }
            }
        }

        if progress.iterations >= self.config.max_iterations {
            exit_decision.hit_limit = true;
        }

        if let Some(handle) = progress.pending_summary.take() {
            if handle.is_finished() {
                if let Ok(Some(summary)) = handle.await {
                    Self::emit_tool_summary_trace(progress.iterations.max(1), summary, callback);
                }
            } else {
                handle.abort();
            }
        }

        progress.apply_final_text_update(exit_decision.final_text_update);
        callback.on_trace(&LoopTraceEvent::SessionCompleted {
            outcome: if exit_decision.hit_limit {
                LoopTraceSessionOutcome::HitLimit
            } else {
                LoopTraceSessionOutcome::Completed
            },
            iterations: progress.iterations,
            tool_calls_made: progress.tool_calls_made,
            total_tokens: progress.total_tokens,
            hit_limit: exit_decision.hit_limit,
            final_text: progress.final_text.clone(),
        });

        // Session-level hook: SessionEnd (observers only)
        if self.tool_pipeline.has_hooks() {
            let ctx = HookContext::new(&self.chain.chain_id);
            self.tool_pipeline
                .hooks()
                .execute_observers(HookEvent::SessionEnd, &ctx)
                .await;
        }

        // Write session snapshot for root agents (depth 0) before returning.
        self.maybe_write_session_snapshot(&progress, &runtime.messages);

        Ok(progress.finish(&self.chain, exit_decision.hit_limit))
    }

    /// Write a session snapshot if this is a root-level agent (depth 0).
    ///
    /// Synchronous write is acceptable here — we're at loop exit, no more LLM
    /// calls or tool execution pending. The write is < 1ms for a small JSON file.
    fn maybe_write_session_snapshot(&self, progress: &LoopProgress, messages: &[UnifiedMessage]) {
        // Only write snapshots for root agents, not subagents
        if self.chain.depth > 0 {
            return;
        }

        let writer = match crate::memory::session_resume::SnapshotWriter::default_path() {
            Some(w) => w,
            None => return,
        };

        // Extract summary from recent assistant messages
        let summary: String = messages
            .iter()
            .rev()
            .filter(|m| m.is_assistant())
            .take(3)
            .filter_map(|m| {
                let text = m.text_content();
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" ");

        let summary_truncated: String = summary.chars().take(500).collect();

        let key_decisions =
            crate::memory::session_resume::SessionSnapshot::extract_decisions(&summary_truncated);

        let snapshot = crate::memory::session_resume::SessionSnapshot {
            session_id: self.chain.chain_id.clone(),
            created_at: chrono::Utc::now(),
            summary: summary_truncated,
            key_decisions,
            active_files: vec![],
            tool_state: None,
            pending_tasks: vec![],
        };

        if let Err(e) = writer.write(&snapshot) {
            tracing::debug!(error = %e, "Failed to write session snapshot");
        }

        let _ = progress;
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
    use crate::thinker::prompt_builder::PromptConfig;
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
        fn on_confirmation_needed(
            &mut self,
            tool_name: &str,
            _tool_input: &Value,
            reason: &str,
        ) -> bool {
            self.safety_blocks
                .push(format!("confirmation_needed: {} ({})", tool_name, reason));
            false
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
            PromptBuilder::new(PromptConfig::default()),
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

        let mut agent = make_loop(provider);
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

        let mut agent = make_loop(provider);
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
        let mut agent = AgentLoop::new(
            provider,
            {
                let mut r = LoopToolRegistry::new();
                r.register(Box::new(EchoTool));
                r
            },
            PromptBuilder::new(PromptConfig::default()),
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

        let mut agent = AgentLoop::new(
            provider,
            LoopToolRegistry::new(),
            PromptBuilder::new(PromptConfig::default()),
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
        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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

        let mut agent = make_loop(provider);
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

        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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

        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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
        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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

        let mut agent = make_loop(provider);
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

        let mut agent = make_loop(provider);
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

        let mut agent = make_loop(provider);
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
        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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

        let mut agent = AgentLoop::new(
            provider,
            {
                let mut r = LoopToolRegistry::new();
                r.register(Box::new(EchoTool));
                r
            },
            PromptBuilder::new(PromptConfig::default()),
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
        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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
                    matches!(&content[1], ContentBlock::Text { text, .. } if text == "I'll search for that.")
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
        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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
                    if let ContentBlock::Text { text, .. } = b {
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
        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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

        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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
        let mut agent = AgentLoop::new(
            provider,
            registry,
            PromptBuilder::new(PromptConfig::default()),
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
                        if let ContentBlock::Text { text, .. } = b {
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

        let mut agent = make_loop(provider);
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

        let mut agent = make_loop(provider);
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

        let mut agent = make_loop(provider);
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

        let mut agent = make_loop(provider);
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

        let mut agent = make_loop(provider);
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
        assert_eq!(messages.last().unwrap().text_content(), "current question");
    }

    #[test]
    fn test_emergency_truncate_with_known_gap() {
        let mut messages = vec![];
        for i in 0..10 {
            messages.push(UnifiedMessage::user(&format!(
                "question {i} {}",
                "x".repeat(80)
            )));
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

    // =========================================================================
    // 413 recovery + fallback integration tests
    // =========================================================================

    #[tokio::test]
    async fn test_413_recovery_retries_after_truncation() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // 413 errors are classified as CompactAndRetry (not Retry), so
        // retry_async returns them immediately on the first call.
        // The recovery loop in the Think step truncates messages and retries.
        // This provider fails with 413 on the first call, then succeeds.
        struct Ptl413ThenOk {
            call_count: AtomicUsize,
        }

        #[async_trait]
        impl LoopProvider for Ptl413ThenOk {
            async fn stream(
                &self,
                _messages: &[UnifiedMessage],
                _system_prompt: &str,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
                let count = self.call_count.fetch_add(1, Ordering::SeqCst);
                if count < 1 {
                    Err(anyhow::anyhow!(
                        "prompt is too long: 137500 tokens > 135000 maximum"
                    ))
                } else {
                    let resp = ProviderResponse::text_only("recovered".to_string());
                    Ok(crate::providers::delta::response_to_delta_stream(resp))
                }
            }
        }

        let provider = Ptl413ThenOk {
            call_count: AtomicUsize::new(0),
        };
        let mut agent = AgentLoop::new(
            provider,
            LoopToolRegistry::new(),
            PromptBuilder::new(PromptConfig::default()),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 5,
                token_budget: 200_000,
            },
            CancellationToken::new(),
        );

        let mut history = Vec::new();
        for i in 0..10 {
            history.push(UnifiedMessage::user(&format!("question {i}")));
            history.push(UnifiedMessage::assistant(&format!("answer {i}")));
        }
        history.push(UnifiedMessage::user("final question"));

        let mut cb = TrackingCallback::default();
        let result = agent
            .run_with_history_messages(history, &mut cb)
            .await
            .unwrap();

        assert_eq!(result.final_text.as_deref(), Some("recovered"));
        assert!(!result.cancelled);
    }

    #[tokio::test]
    async fn test_fallback_model_switches_on_overload() {
        struct OverloadedProvider;

        #[async_trait]
        impl LoopProvider for OverloadedProvider {
            async fn stream(
                &self,
                _messages: &[UnifiedMessage],
                _system_prompt: &str,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
                Err(anyhow::anyhow!("HTTP 529 overloaded"))
            }
        }

        struct FallbackProvider;

        #[async_trait]
        impl LoopProvider for FallbackProvider {
            async fn stream(
                &self,
                _messages: &[UnifiedMessage],
                _system_prompt: &str,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
                let resp = ProviderResponse::text_only("fallback response".to_string());
                Ok(crate::providers::delta::response_to_delta_stream(resp))
            }
        }

        let mut agent = AgentLoop::new(
            OverloadedProvider,
            LoopToolRegistry::new(),
            PromptBuilder::new(PromptConfig::default()),
            SafetyGuard::default_guard(),
            LoopConfig::default(),
            CancellationToken::new(),
        )
        .with_fallback(Box::new(FallbackProvider), "test-fallback");

        let mut cb = TrackingCallback::default();
        let result = agent.run("hello", &mut cb).await.unwrap();

        assert_eq!(result.final_text.as_deref(), Some("fallback response"));
        assert_eq!(cb.fallback_events.len(), 1);
        assert_eq!(cb.fallback_events[0].1, "test-fallback");
    }

    #[tokio::test]
    async fn test_fallback_not_available_propagates_error() {
        struct OverloadedProvider;

        #[async_trait]
        impl LoopProvider for OverloadedProvider {
            async fn stream(
                &self,
                _messages: &[UnifiedMessage],
                _system_prompt: &str,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
                Err(anyhow::anyhow!("HTTP 529 overloaded"))
            }
        }

        let mut agent = AgentLoop::new(
            OverloadedProvider,
            LoopToolRegistry::new(),
            PromptBuilder::new(PromptConfig::default()),
            SafetyGuard::default_guard(),
            LoopConfig::default(),
            CancellationToken::new(),
        );

        let mut cb = NoopCallback;
        let result = agent.run("hello", &mut cb).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tier2_all_callbacks_fire() {
        #[derive(Default)]
        struct Tier2Callback {
            fallback_fired: bool,
            stop_hook_block_fired: bool,
            stop_hook_error_fired: bool,
            summary_fired: bool,
        }

        impl LoopCallback for Tier2Callback {
            fn on_model_fallback(&mut self, reason: &str, model: &str) {
                assert!(!reason.is_empty());
                assert!(!model.is_empty());
                self.fallback_fired = true;
            }
            fn on_stop_hook_block(&mut self, reason: &str) {
                assert!(!reason.is_empty());
                self.stop_hook_block_fired = true;
            }
            fn on_stop_hook_error(&mut self, name: &str, error: &str) {
                assert!(!name.is_empty());
                assert!(!error.is_empty());
                self.stop_hook_error_fired = true;
            }
            fn on_tool_summary(&mut self, summary: &str) {
                assert!(!summary.is_empty());
                self.summary_fired = true;
            }
        }

        let mut cb = Tier2Callback::default();
        cb.on_model_fallback("test reason", "test-model");
        cb.on_stop_hook_block("test block");
        cb.on_stop_hook_error("test-hook", "test error");
        cb.on_tool_summary("Searched for bugs");

        assert!(cb.fallback_fired);
        assert!(cb.stop_hook_block_fired);
        assert!(cb.stop_hook_error_fired);
        assert!(cb.summary_fired);
    }

    #[test]
    fn test_trace_callback_defaults_bridge_legacy_callbacks() {
        #[derive(Default)]
        struct TraceCompatCallback {
            texts: Vec<String>,
            intermediates: Vec<String>,
            tool_starts: Vec<String>,
            tool_dones: Vec<String>,
            summaries: Vec<String>,
        }

        impl LoopCallback for TraceCompatCallback {
            fn on_text(&mut self, text: &str) {
                self.texts.push(text.to_string());
            }

            fn on_intermediate_text(&mut self, text: &str) {
                self.intermediates.push(text.to_string());
            }

            fn on_tool_start(&mut self, name: &str, _input: &Value) {
                self.tool_starts.push(name.to_string());
            }

            fn on_tool_done(&mut self, name: &str, _result: &ToolResult) {
                self.tool_dones.push(name.to_string());
            }

            fn on_tool_summary(&mut self, summary: &str) {
                self.summaries.push(summary.to_string());
            }
        }

        let mut callback = TraceCompatCallback::default();
        callback.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Intermediate,
            text: "thinking".to_string(),
        });
        callback.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "done".to_string(),
        });
        callback.on_trace(&LoopTraceEvent::ToolCallStarted {
            iteration: 1,
            call: ToolCallStartEvent {
                tool_id: "call-1".to_string(),
                tool_name: "read_file".to_string(),
                input: json!({"path": "README.md"}),
            },
        });
        callback.on_trace(&LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: ToolCallEndEvent {
                tool_id: "call-1".to_string(),
                tool_name: "read_file".to_string(),
                input: json!({"path": "README.md"}),
                duration_ms: 12,
            },
            result: ToolResult::Success {
                output: json!({"ok": true}),
            },
        });
        callback.on_trace(&LoopTraceEvent::ToolSummary {
            iteration: 1,
            summary: "Looked up the file".to_string(),
        });

        assert_eq!(callback.intermediates, vec!["thinking"]);
        assert_eq!(callback.texts, vec!["done"]);
        assert_eq!(callback.tool_starts, vec!["read_file"]);
        assert_eq!(callback.tool_dones, vec!["read_file"]);
        assert_eq!(callback.summaries, vec!["Looked up the file"]);
    }

    #[test]
    fn test_confirmation_needed_calls_callback() {
        let mut callback = TrackingCallback::default();
        let result = callback.on_confirmation_needed(
            "shell",
            &serde_json::json!({"command": "rm -rf temp"}),
            "high-risk tool",
        );
        assert!(!result);
        assert!(callback
            .safety_blocks
            .iter()
            .any(|s| s.contains("confirmation_needed")));
    }
}
