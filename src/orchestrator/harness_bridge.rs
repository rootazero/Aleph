//! Bridge between the Phase 5 Orchestrator and the Phase 4 AgentHarness.
//!
//! `AgentHarnessRunner` implements [`HarnessRunner`] by:
//!   1. Verifying `spec.agent` is registered in the [`AgentRegistry`].
//!   2. Picking an `Arc<dyn AiProvider>` from [`BrainRef`].
//!   3. Seeding the session with the [`FlowInput`] as a `UserMessage` event.
//!   4. Running the inner `AgentHarness` loop to completion.
//!   5. Extracting the last `AssistantMessage.text` as `final_text`.
//!
//! # Phase 6 follow-ups
//! * Thread `AgentDef` + `FlowOverrides` (max_iterations, extra_system_prompt,
//!   context_mode) into `HarnessDeps`. Requires widening the Phase 4 API.
//! * Honour [`BrainRef::Strict`] model selection — `AiProvider` does not
//!   expose `select_model` at this layer yet.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

use crate::agents::AgentRegistry;
use crate::context::budget::{ContextBudget, ContextBudgetConfig};
use crate::context::compact::compactor::{CompactorConfig, ContextCompactor};
use crate::harness::agent::AgentHarness;

use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent, HarnessRunner};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{FlowInput, FlowSpec};
use crate::providers::{AiProvider, DefaultProviderHandle};
use crate::routing::session_key::SessionKey;
use crate::sandbox::Sandbox;
use crate::session::events::SessionEvent;
use crate::session::service::{SessionId, SessionService};
use crate::tools::service::ToolService;
use crate::verification::VerifierChain;

mod callback;
mod error;
mod llm;
mod session_seed;

/// Stage 7 (#12): emit one `TraceSink::on_init_seam` event per Stage 1-6
/// seam. Extracted from `AgentHarnessRunner::run` so tests can assert the
/// nine-event contract without a full runner fixture. `configured = false`
/// distinguishes a deliberate `None` path (Phase-6 stub) from a missing
/// wiring; `PromptBuilder` and `ChainContext` are always configured because
/// the gateway path constructs them unconditionally.
pub(crate) fn emit_init_seams(
    sink: &dyn crate::harness::TraceSink,
    guardrails_configured: bool,
    fallback_llm_configured: bool,
    verifier_chain_configured: bool,
    stall_config_configured: bool,
    consecutive_failure_cap_configured: bool,
    turn_timeout_configured: bool,
    skill_prefetcher_configured: bool,
) {
    sink.on_init_seam("stage3-prompt", "PromptBuilder", true);
    sink.on_init_seam("stage4-chain", "ChainContext", true);
    sink.on_init_seam(
        "stage5a-guardrails",
        "GuardrailRegistry",
        guardrails_configured,
    );
    sink.on_init_seam("stage5b-fallback", "FallbackLLM", fallback_llm_configured);
    sink.on_init_seam(
        "stage6a-verifier",
        "VerifierChain",
        verifier_chain_configured,
    );
    sink.on_init_seam("p0-rescue-stall", "StallConfig", stall_config_configured);
    sink.on_init_seam(
        "p0-rescue-cap",
        "ConsecutiveFailureCap",
        consecutive_failure_cap_configured,
    );
    sink.on_init_seam("p0-rescue-timeout", "TurnTimeout", turn_timeout_configured);
    sink.on_init_seam(
        "skill-prefetcher",
        "SkillPrefetcher",
        skill_prefetcher_configured,
    );
}

/// Concrete [`HarnessRunner`] that dispatches to the Phase 4 `AgentHarness`.
pub struct AgentHarnessRunner {
    pub agent_registry: Arc<AgentRegistry>,
    pub session_service: Arc<dyn SessionService>,
    pub tool_service: Arc<dyn ToolService>,
    /// Live default-provider resolver. Each `pick_llm` call asks the handle
    /// for the current default so UI-driven `set_default` takes effect on the
    /// next turn (Step 5 hot-reload). Replaces the boot-time `Arc<dyn AiProvider>`
    /// snapshot that previously required a restart.
    pub default_provider: Arc<dyn DefaultProviderHandle>,
    /// Named providers keyed by `ProviderId`. Wired from `AuthProfileRegistry`
    /// by Task 9; empty in early boot.
    pub named_providers: HashMap<String, Arc<dyn AiProvider>>,

    // -- Task 10 (6b) optional collaborators ---------------------------------
    //
    // Injected at orchestrator boot; forwarded into `HarnessDeps` on every
    // `run()` so each `AgentHarness` instance sees the same pressure sensor
    // / compactor / hook set.
    pub verifier_chain: Option<Arc<VerifierChain>>,
    /// Opt-in mid-run context management (`[context_budget]`). Held as the
    /// *config*, not a live `ContextBudget`: `run()` constructs a fresh
    /// `ContextBudget` per call because its circuit-breaker /
    /// diminishing-returns state must never be shared across concurrent
    /// sessions. `None` disables mid-run compaction entirely.
    pub context_budget_config: Option<ContextBudgetConfig>,
    /// Shared v2 SkillSystem. When `Some`, `build_system_prompt` injects the
    /// eligible-skill `<available_skills>` block into the system prompt.
    pub skill_system: Option<crate::skill::SkillSystem>,

    // -- Stage 7 (init audit) — production wiring for Stage 5a/5b + P0 rescue
    //    seams. Each field defaults to None on the gateway path; PHASE-6
    //    will load values from `aleph.toml` and wire them here so HarnessDeps
    //    receives the configured impls instead of hardcoded None.
    pub guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
    pub fallback_llm: Option<Arc<dyn AiProvider>>,
    pub stall_config: Option<crate::harness::deps::StallConfig>,
    pub consecutive_failure_cap: Option<usize>,
    pub turn_timeout: Option<std::time::Duration>,

    /// Boot-time default for the harness Think→Act iteration cap, sourced from
    /// `[execution] max_iterations`. A per-flow `FlowOverrides.max_iterations`
    /// overrides it on a run-by-run basis. The harness loop is never left
    /// uncapped — see [`resolve_max_iterations`].
    pub default_max_iterations: usize,

    /// Platform-specific power-management capability. Injected at boot so the
    /// core never directly imports platform crates (R1: Brain–Limb separation).
    pub power: Option<Arc<dyn aleph_desktop::traits::PowerCapability>>,

    /// Phase 6 follow-up — closes the BUG-2/BUG-3 gap where the gateway path
    /// was constructing `HarnessDeps { system_prompt: None }` and bypassing
    /// curated/hybrid memory entirely. When `Some`, `run()` invokes
    /// `build_curated_message` + `build_memory_user_message` and threads the
    /// rendered envelopes through `PromptBuilder` so the system prompt carries
    /// per-agent curated memory plus retrieval hits. `None` preserves the old
    /// behaviour (boot path for tests / env without a memory backend).
    pub memory_context_provider:
        Option<Arc<crate::thinker::memory_context_provider::MemoryContextProvider>>,
}

#[async_trait]
impl HarnessRunner for AgentHarnessRunner {
    async fn run(
        &self,
        session_key: String,
        spec: Arc<FlowSpec>,
        input: FlowInput,
        sandbox: Arc<dyn Sandbox>,
        events: broadcast::Sender<FlowStreamEvent>,
        cancel: CancellationToken,
        tool_service_override: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
        trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
    ) -> Result<FlowOutcome, FlowError> {
        // Step 1: honour pre-dispatch cancellation fast-path (short-circuit
        // before provider lookup / LLM construction). The same token is also
        // threaded into `harness.run` below so the inner Think→Act loop
        // aborts between turns when cancel fires mid-run.
        if cancel.is_cancelled() {
            return Err(FlowError::Cancelled);
        }

        // Step 2: verify the agent exists. AgentDef itself is not threaded
        // into HarnessDeps at this phase.
        // PHASE-6 FOLLOW-UP: thread AgentDef + FlowOverrides into HarnessDeps.
        if self.agent_registry.get(&spec.agent).is_none() {
            return Err(FlowError::UnknownAgent(spec.agent.clone()));
        }

        // Step 3: brain pick.
        let llm = llm::pick_llm(&spec.brain, &self.default_provider, &self.named_providers)?;
        // Stage J-pre: wrap the root provider with MeteringProvider so every
        // LLM call emits a LoopTraceEvent::ProviderUsage event labelled "root".
        // The trace_sink is available here (per-run, passed in from the gateway)
        // and flows into the same sink as all other harness trace events.
        let llm: Arc<dyn crate::providers::AiProvider> = Arc::new(
            crate::providers::MeteringProvider::new(llm, trace_sink.clone(), "root"),
        );
        // Remember the provider name so transient error classification below
        // can attach it to FlowError::Transient (Gateway's outer retry loop
        // reads this to call `report_outcome(&provider_name, ...)`).
        let provider_name = llm.name().to_string();

        // Step 4: convert String → SessionId. Serialized SessionKeys parse
        // directly; otherwise treat the incoming string as an ephemeral id
        // under `spec.agent` so orchestrator ↔ harness session identity stays
        // deterministic (no fresh-uuid divergence).
        let session_id: SessionId =
            SessionKey::from_key_string(&session_key).unwrap_or_else(|| SessionKey::Ephemeral {
                agent_id: spec.agent.clone(),
                ephemeral_id: session_key.clone(),
            });

        // Step 5: seed the session with the input as the appropriate event(s)
        // so the inner harness Think loop can read it. Preserve per-message
        // structure — do not flatten via string join.
        // Capture the user's last query before moving `input` so step 5b can
        // ask MemoryContextProvider for retrieval-relevant facts.
        let user_query = last_user_query(&input);
        session_seed::seed_session(self.session_service.as_ref(), &session_id, input).await?;

        // Step 5b (BUG-2/BUG-3 fix, Phase 6 follow-up): assemble the system
        // prompt from per-agent curated memory + hybrid retrieval before the
        // harness loop starts. Failures are warned and degraded to `None` so
        // memory issues never block a turn.
        let system_prompt = self
            .build_system_prompt(&spec.agent, &session_id, &user_query)
            .await;

        // Step 6: assemble HarnessDeps and run the inner Think→Act loop.
        // Apply per-request tool_service override; fall back to the runner's
        // default when the caller supplies None.
        let tools = tool_service_override.unwrap_or_else(|| self.tool_service.clone());
        // Wire the platform-specific power capability so the harness can
        // inhibit idle sleep for the duration of each Think→Act turn.
        let power = self.power.clone();
        // H2: build a per-run context budget + compactor when `[context_budget]`
        // is enabled. The budget is fresh per run — its circuit-breaker and
        // diminishing-returns counters must not leak across concurrent
        // sessions. The compactor reuses this run's provider for side-channel
        // summarization (deterministic-truncation fallback on provider error).
        let (context_budget, context_compactor) = match self.context_budget_config.as_ref() {
            Some(cfg) => {
                let budget = Arc::new(Mutex::new(ContextBudget::new(cfg)));
                let compactor = Arc::new(ContextCompactor::new(
                    llm.clone(),
                    CompactorConfig {
                        fresh_tail: cfg.fresh_tail_count,
                        ..CompactorConfig::default()
                    },
                ));
                (Some(budget), Some(compactor))
            }
            None => (None, None),
        };
        let deps = HarnessDeps {
            session: self.session_service.clone(),
            tools,
            sandbox,
            llm,
            verifier_chain: self.verifier_chain.clone(),
            context_budget,
            context_compactor,
            skill_prefetcher: None,
            trace_sink: trace_sink.clone(),
            system_prompt,
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: self.guardrails.clone(),
            fallback_llm: self.fallback_llm.clone(),
            // H1: the Think→Act loop is always capped. Per-flow override wins;
            // otherwise the boot-time `[execution] max_iterations` default.
            max_iterations: Some(resolve_max_iterations(
                spec.overrides.max_iterations,
                self.default_max_iterations,
            )),
            power,
            stall_config: self.stall_config.clone(),
            consecutive_failure_cap: self.consecutive_failure_cap,
            turn_timeout: self.turn_timeout,
        };
        // Stage 7 (#12): emit init-seam visibility before the harness
        // starts its Think→Act loop. Order mirrors HarnessDeps field
        // declaration so trace consumers can correlate event index ↔
        // deps.rs line number. Extracted helper lets the orchestrator
        // tests assert the contract without a full AgentHarnessRunner
        // fixture.
        if let Some(sink) = trace_sink.as_ref() {
            emit_init_seams(
                sink.as_ref(),
                deps.guardrails.is_some(),
                deps.fallback_llm.is_some(),
                deps.verifier_chain.is_some(),
                deps.stall_config.is_some(),
                deps.consecutive_failure_cap.is_some(),
                deps.turn_timeout.is_some(),
                deps.skill_prefetcher.is_some(),
            );
        }
        // Production telemetry path — operators read these via the
        // existing tracing subscriber regardless of TraceSink wiring.
        tracing::info!(
            guardrails = deps.guardrails.is_some(),
            fallback_llm = deps.fallback_llm.is_some(),
            verifier_chain = deps.verifier_chain.is_some(),
            stall_config = deps.stall_config.is_some(),
            consecutive_failure_cap = deps.consecutive_failure_cap.is_some(),
            turn_timeout = deps.turn_timeout.is_some(),
            skill_prefetcher = deps.skill_prefetcher.is_some(),
            "harness deps assembled"
        );
        let harness = AgentHarness::new(deps);
        // Fans HarnessCallback events onto the FlowStreamEvent broadcast
        // channel so downstream Gateway sinks see delta / tool_call cadence
        // equivalent to the retiring AgentLoop StreamingSink.
        let mut cb = callback::BroadcastCallback::new(events.clone());
        let run_result = harness.run(&session_id, &mut cb, &cancel).await;
        // Flush the trace sink regardless of success or error (no-op when None).
        if let Some(sink) = trace_sink.as_ref() {
            sink.flush();
        }
        run_result.map_err(|e| match e {
            crate::harness::trait_def::HarnessError::Cancelled => FlowError::Cancelled,
            other => error::classify_harness_error(other, &provider_name),
        })?;

        // Step 7: read final AssistantMessage text + count assistant turns.
        let records = self
            .session_service
            .get_events(&session_id, None, None)
            .await
            .map_err(|e| FlowError::Internal(format!("session read: {e}")))?;

        let mut final_text = String::new();
        let mut iterations: u32 = 0;
        let mut tool_calls_made: u32 = 0;
        for r in &records {
            match &r.event {
                SessionEvent::AssistantMessage { content, .. } => {
                    // Use thinking content as fallback when text is empty
                    // (extended-thinking models may put output in thinking field)
                    let mut text = if content.text.is_empty() {
                        content.thinking.clone().unwrap_or_default()
                    } else {
                        content.text.clone()
                    };

                    let trimmed = text.trim();
                    let payload = if trimmed.starts_with("```") {
                        trimmed
                            .lines()
                            .skip(1)
                            .take_while(|l| !l.trim_start().starts_with("```"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        trimmed.to_string()
                    };

                    let json_trimmed = payload.trim_start();
                    if json_trimmed.starts_with('{') || json_trimmed.starts_with('[') {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&payload) {
                            // Try to extract a human-readable field (prefer longer content)
                            let candidates = [
                                json.get("action")
                                    .and_then(|a| a.get("summary"))
                                    .and_then(|v| v.as_str()),
                                json.get("action")
                                    .and_then(|a| a.get("content"))
                                    .and_then(|v| v.as_str()),
                                json.get("action")
                                    .and_then(|a| a.get("text"))
                                    .and_then(|v| v.as_str()),
                                json.get("summary").and_then(|v| v.as_str()),
                                json.get("content").and_then(|v| v.as_str()),
                                json.get("message").and_then(|v| v.as_str()),
                                json.get("text").and_then(|v| v.as_str()),
                                json.get("reasoning").and_then(|v| v.as_str()),
                            ];
                            // Pick the longest candidate to avoid short reasoning over full report
                            if let Some(best) =
                                candidates.iter().filter_map(|&c| c).max_by_key(|c| c.len())
                            {
                                text = best.to_string();
                            }
                            // If no known field found, keep original JSON
                        }
                    }

                    final_text = text;
                    iterations = iterations.saturating_add(1);
                }
                SessionEvent::ToolCallRequested { .. } => {
                    tool_calls_made = tool_calls_made.saturating_add(1);
                }
                _ => {}
            }
        }

        // `total_tokens` and `hit_limit` are read straight off the harness
        // after the run: the harness retains the cumulative token counter
        // and the budget-sensor flag. `total_tokens` saturates into the
        // `u32` field (`as u32` would truncate; a run is realistically far
        // below `u32::MAX` tokens).
        let outcome = FlowOutcome {
            final_text,
            iterations,
            tool_calls_made,
            total_tokens: u32::try_from(harness.total_tokens()).unwrap_or(u32::MAX),
            hit_limit: harness.hit_limit(),
        };

        // Emit `Complete(outcome)` as the terminal broadcast event.
        // `BroadcastCallback::on_complete` is a no-op; this is the only place
        // that fires the `Complete` variant so it is always last on the channel.
        let _ = events.send(FlowStreamEvent::Complete(outcome.clone()));

        Ok(outcome)
    }
}

impl AgentHarnessRunner {
    /// Assemble the per-turn system prompt with curated memory + hybrid
    /// retrieval. Returns `None` when no `MemoryContextProvider` is wired
    /// (test envs without a memory backend) or when both memory builders
    /// returned empty envelopes.
    ///
    /// Errors from individual builders are downgraded to a warn log: the
    /// remaining sections (curated/memory/agent role) still render so a
    /// transient memory failure never blocks a turn. This matches the
    /// `Ok(None)` semantics already exposed by `MemoryContextProvider`'s
    /// builders and keeps the harness path resilient.
    async fn build_system_prompt(
        &self,
        agent_id: &str,
        session_id: &SessionId,
        user_query: &str,
    ) -> Option<String> {
        use crate::providers::message::UnifiedMessage;
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        // Phase 1 — fetch the eligible-skill snapshot once; reused below.
        let skill_snapshot = match self.skill_system.as_ref() {
            Some(sys) => Some(sys.current_snapshot().await),
            None => None,
        };

        let session_key_str = session_id.to_key_string();

        let (curated_text, memory_text) =
            if let Some(mcp) = self.memory_context_provider.as_ref() {
                let curated_text: Option<String> =
                    match mcp.build_curated_message(agent_id, &session_key_str).await {
                        Ok(opt) => opt.as_ref().map(UnifiedMessage::text_content),
                        Err(e) => {
                            tracing::warn!(
                                agent_id,
                                session = %session_key_str,
                                error = %e,
                                "build_curated_message failed; degrading curated envelope to None"
                            );
                            None
                        }
                    };

                let memory_text: Option<String> = if user_query.is_empty() {
                    None
                } else {
                    match mcp.build_memory_user_message(agent_id, user_query).await {
                        Ok(opt) => opt.as_ref().map(UnifiedMessage::text_content),
                        Err(e) => {
                            tracing::warn!(
                                agent_id,
                                error = %e,
                                "build_memory_user_message failed; degrading memory envelope to None"
                            );
                            None
                        }
                    }
                };
                (curated_text, memory_text)
            } else {
                (None, None)
            };

        let agent_def = self.agent_registry.get(agent_id);

        let has_skills = skill_snapshot
            .as_ref()
            .map(|s| !s.eligible_manifests.is_empty())
            .unwrap_or(false);
        // Skip prompt assembly entirely when there is nothing to inject:
        // no memory, no AgentDef, and no eligible skills.
        if curated_text.is_none() && memory_text.is_none() && agent_def.is_none() && !has_skills {
            return None;
        }

        let eligible_skills = skill_snapshot
            .map(|s| s.eligible_manifests)
            .filter(|m| !m.is_empty());
        // The harness path delivers tool schemas via native tool_use
        // (`with_tools(tools_ref)` in agent.rs). When `native_tools_enabled` is
        // false (the default), `ToolsLayer` injects the literal string
        // "No tools available" and `ResponseFormatLayer` mandates the legacy
        // `{reasoning, action}` JSON envelope — both of which contradict the
        // native-tool-use API the harness actually drives. Force the flag on
        // here so the assembled prompt matches the runtime contract.
        let mut builder = PromptBuilder::new(PromptConfig {
            native_tools_enabled: true,
            eligible_skills,
            ..PromptConfig::default()
        });
        let role_present = agent_def.is_some();
        if let Some(def) = agent_def {
            builder = builder.with_agent(def);
        }
        let curated_chars = curated_text.as_ref().map(String::len).unwrap_or(0);
        builder = builder.with_curated_envelope(curated_text);
        let memory_chars = memory_text.as_ref().map(String::len).unwrap_or(0);
        if let Some(text) = memory_text {
            builder = builder.with_memory_user_message(text);
        }
        let prompt = builder.build_system_prompt(&[]);
        // Phase 6 observability — confirm BUG-2/BUG-3 wiring at runtime.
        // Logs character counts (not contents) so prompts are observable
        // without leaking memory content to disk-side telemetry.
        tracing::info!(
            target: "alephcore::orchestrator::prompt",
            agent_id,
            session = %session_key_str,
            curated_chars,
            memory_chars,
            role_present,
            prompt_chars = prompt.len(),
            "system prompt assembled"
        );
        Some(prompt)
    }
}

/// Hard fallback iteration cap — used only when both the per-flow override
/// and the boot-configured default are absent or zero. The harness Think→Act
/// loop must never run uncapped: a model that keeps emitting tool calls would
/// otherwise loop (and bill) forever.
///
/// Kept numerically equal to `config::types::execution::default_max_iterations()`
/// (the `[execution] max_iterations` default) — both express "the default
/// per-run cap"; update them together.
pub(crate) const FALLBACK_MAX_ITERATIONS: usize = 200;

/// Resolve the hard per-run iteration cap for the harness loop.
///
/// A positive `FlowOverrides.max_iterations` (per-flow preset override) wins;
/// otherwise the boot-time `default` (from `[execution] max_iterations`)
/// applies. A zero on either input is treated as "unset" so a misconfigured
/// `0` can never leave the loop uncapped — it falls through to
/// [`FALLBACK_MAX_ITERATIONS`].
fn resolve_max_iterations(flow_override: Option<u32>, default: usize) -> usize {
    flow_override
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .or(Some(default).filter(|&n| n > 0))
        .unwrap_or(FALLBACK_MAX_ITERATIONS)
}

/// Extract the user's most recent prompt text from a `FlowInput` for use as
/// the retrieval query against `MemoryContextProvider::build_memory_user_message`.
/// Returns an empty string when no user-side text is available; callers treat
/// the empty case as "skip retrieval".
fn last_user_query(input: &FlowInput) -> String {
    fn text_of(content: &crate::session::events::MessageContent) -> &str {
        content.text.as_str()
    }
    match input {
        FlowInput::Prompt(s) => s.clone(),
        FlowInput::Messages(msgs) | FlowInput::Multimodal(msgs) => msgs
            .iter()
            .rev()
            .map(text_of)
            .find(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_default(),
        FlowInput::History { prompt, .. } => prompt.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::callback::HarnessCallback;
    use crate::orchestrator::flow_spec::FlowHistoryTurn;
    use crate::session::events::MessageContent;
    use crate::session::service::SessionService;
    use session_seed::seed_session;

    #[test]
    fn broadcast_callback_fans_lifecycle_events() {
        let (tx, mut rx) = broadcast::channel::<FlowStreamEvent>(16);
        let mut cb = super::callback::BroadcastCallback::new(tx);

        cb.on_delta("hello ");
        cb.on_delta("world");
        // Use legacy on_tool_call — fires ToolCallStart with id="legacy"
        cb.on_tool_call("read_file");
        // on_complete is now a no-op; Complete(outcome) is emitted by AgentHarnessRunner
        cb.on_complete();

        let mut received = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            received.push(ev);
        }

        // 3 events: two Deltas + one ToolCallStart (on_complete is no-op)
        assert_eq!(received.len(), 3);
        match &received[0] {
            FlowStreamEvent::Delta(s) => assert_eq!(s, "hello "),
            other => panic!("expected Delta(\"hello \"), got {other:?}"),
        }
        match &received[1] {
            FlowStreamEvent::Delta(s) => assert_eq!(s, "world"),
            other => panic!("expected Delta(\"world\"), got {other:?}"),
        }
        match &received[2] {
            FlowStreamEvent::ToolCallStart { name, .. } => assert_eq!(name, "read_file"),
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
    }

    #[test]
    fn classify_harness_error_network_is_transient() {
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::network(
            "connection reset mid-stream",
        ));
        let out = super::error::classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Transient { .. }));
    }

    #[test]
    fn classify_harness_error_http_500_is_transient() {
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::network(
            "upstream returned 500",
        ));
        let out = super::error::classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Transient { .. }));
    }

    #[test]
    fn classify_harness_error_generic_is_internal() {
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::Other {
            message: "opaque failure".into(),
            suggestion: None,
        });
        let out = super::error::classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Internal(_)));
    }

    #[test]
    fn classify_harness_error_4500_is_not_server_transient() {
        // Word-boundary check: "4500" contains "500" substring but is not status 500.
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::Other {
            message: "processed 4500 items then gave up".into(),
            suggestion: None,
        });
        let out = super::error::classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Internal(_)));
    }

    #[test]
    fn broadcast_callback_is_silent_when_no_receivers() {
        // No active receiver — `send` returns Err(SendError) but
        // BroadcastCallback swallows it so the harness loop is unaffected.
        let (tx, _rx) = broadcast::channel::<FlowStreamEvent>(1);
        drop(_rx);
        let mut cb = super::callback::BroadcastCallback::new(tx);
        cb.on_delta("nobody is listening");
        cb.on_tool_call("read_file");
        cb.on_complete();
        // No panic = pass.
    }

    // -- seed_session tests --------------------------------------------------

    use crate::routing::session_key::SessionKey;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

    fn fresh_service() -> std::sync::Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: std::sync::Arc<dyn SessionEventStore> =
            std::sync::Arc::new(SqliteEventStore::new(conn));
        std::sync::Arc::new(InProcessActorSessionService::new(store))
    }

    #[tokio::test]
    async fn seed_session_prompt_emits_one_user_message() {
        let service = fresh_service();
        let sid = SessionKey::ephemeral("seed-prompt");
        super::session_seed::seed_session(
            service.as_ref(),
            &sid,
            FlowInput::Prompt("hello".into()),
        )
        .await
        .expect("seed Prompt");

        let events = service.get_events(&sid, None, None).await.unwrap();
        let user_count = events
            .iter()
            .filter(|r| matches!(r.event, SessionEvent::UserMessage { .. }))
            .count();
        assert_eq!(user_count, 1);
    }

    #[tokio::test]
    async fn seed_session_history_replays_turns_and_adds_prompt() {
        let service = fresh_service();
        let sid = SessionKey::ephemeral("seed-history");
        let turns = vec![
            FlowHistoryTurn::User(MessageContent {
                text: "q1".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            }),
            FlowHistoryTurn::Assistant(MessageContent {
                text: "a1".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            }),
            FlowHistoryTurn::User(MessageContent {
                text: "q2".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            }),
            FlowHistoryTurn::Assistant(MessageContent {
                text: "a2".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            }),
        ];
        seed_session(
            service.as_ref(),
            &sid,
            FlowInput::History {
                turns,
                prompt: "q3".into(),
            },
        )
        .await
        .expect("seed History");

        let events = service.get_events(&sid, None, None).await.unwrap();
        let users: Vec<String> = events
            .iter()
            .filter_map(|r| match &r.event {
                SessionEvent::UserMessage { content, .. } => Some(content.text.clone()),
                _ => None,
            })
            .collect();
        let assistants: Vec<String> = events
            .iter()
            .filter_map(|r| match &r.event {
                SessionEvent::AssistantMessage { content, .. } => Some(content.text.clone()),
                _ => None,
            })
            .collect();
        let turn_started_count = events
            .iter()
            .filter(|r| matches!(r.event, SessionEvent::TurnStarted { .. }))
            .count();

        assert_eq!(users, vec!["q1", "q2", "q3"]);
        assert_eq!(assistants, vec!["a1", "a2"]);
        assert_eq!(
            turn_started_count, 1,
            "exactly one TurnStarted for the trailing prompt"
        );
    }

    #[tokio::test]
    async fn seed_session_multimodal_emits_one_user_per_entry() {
        let service = fresh_service();
        let sid = SessionKey::ephemeral("seed-multimodal");
        let msgs = vec![
            MessageContent {
                text: "m1".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            MessageContent {
                text: "m2".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
        ];
        super::session_seed::seed_session(service.as_ref(), &sid, FlowInput::Multimodal(msgs))
            .await
            .expect("seed Multimodal");

        let events = service.get_events(&sid, None, None).await.unwrap();
        let users: Vec<String> = events
            .iter()
            .filter_map(|r| match &r.event {
                SessionEvent::UserMessage { content, .. } => Some(content.text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(users, vec!["m1", "m2"]);
    }

    // BUG-2/BUG-3 regression coverage — `last_user_query` must round-trip the
    // most recent user-side text out of every `FlowInput` variant so the
    // gateway path can hand it to `MemoryContextProvider::build_memory_user_message`.
    // Empty strings degrade cleanly to "" so callers can short-circuit
    // retrieval without a panic.

    fn msg(text: &str) -> crate::session::events::MessageContent {
        crate::session::events::MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        }
    }

    #[test]
    fn last_user_query_extracts_prompt() {
        let q = super::last_user_query(&FlowInput::Prompt("hello world".into()));
        assert_eq!(q, "hello world");
    }

    #[test]
    fn last_user_query_extracts_history_prompt() {
        let input = FlowInput::History {
            turns: vec![],
            prompt: "next turn please".into(),
        };
        assert_eq!(super::last_user_query(&input), "next turn please");
    }

    #[test]
    fn last_user_query_extracts_last_non_empty_message() {
        let input = FlowInput::Messages(vec![msg("first"), msg("second")]);
        assert_eq!(super::last_user_query(&input), "second");
    }

    #[test]
    fn last_user_query_skips_trailing_empty_messages() {
        let input = FlowInput::Messages(vec![msg("real query"), msg(""), msg("")]);
        assert_eq!(super::last_user_query(&input), "real query");
    }

    #[test]
    fn last_user_query_handles_multimodal() {
        let input = FlowInput::Multimodal(vec![msg("first"), msg("multimodal-tail")]);
        assert_eq!(super::last_user_query(&input), "multimodal-tail");
    }

    #[test]
    fn last_user_query_returns_empty_for_empty_messages() {
        let input = FlowInput::Messages(vec![]);
        assert_eq!(super::last_user_query(&input), "");
    }

    // Note on build_system_prompt coverage: the prompt assembly path itself
    // requires a wired `MemoryContextProvider` (LLM-backed reranker, embedder,
    // hybrid assembler, FactSourceFilter pipeline). That is exercised in the
    // P0 联合 e2e validation step against a live aleph-server, where curated
    // markers and retrieval markers can be planted in a known-state fixture
    // and the resulting RequestPayload.system_prompt asserted via TraceSink.
    // Adding a unit test here would require a heavy fixture stack (provider,
    // session_service, tool_service, agent registry) that the surrounding
    // file already builds via `fresh_service` for `seed_session` only.

    /// Regression: harness_bridge must build the system prompt with
    /// `native_tools_enabled = true`. Otherwise `ToolsLayer` injects
    /// "No tools available" (the harness still passes tools via native
    /// tool_use, so the prompt would be lying to the LLM) and
    /// `ResponseFormatLayer` mandates the legacy `{reasoning, action}` JSON
    /// envelope (which then leaks raw to clients because the harness no
    /// longer expects it).
    ///
    /// This test exercises the exact `PromptConfig` shape used at
    /// `harness_bridge.rs::build_system_prompt`, decoupled from the
    /// `MemoryContextProvider` fixture, so a future refactor that drops the
    /// flag fails fast.
    #[test]
    fn harness_bridge_prompt_config_skips_tools_and_response_format_layers() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        let prompt = PromptBuilder::new(PromptConfig {
            native_tools_enabled: true,
            ..PromptConfig::default()
        })
        .build_system_prompt(&[]);

        assert!(
            !prompt.contains("No tools available"),
            "ToolsLayer leaked the empty-tools sentinel into a native-tool-use prompt:\n{prompt}"
        );
        assert!(
            !prompt.contains("## Response Format"),
            "ResponseFormatLayer leaked the JSON-envelope mandate into a native-tool-use prompt:\n{prompt}"
        );
        assert!(
            !prompt.contains("\"reasoning\""),
            "Prompt still references the {{reasoning, action}} envelope schema:\n{prompt}"
        );
    }

    /// Companion check: with `native_tools_enabled = false` the ToolsLayer
    /// must still announce empty tools when called with `&[]`. The harness
    /// path opts out via `native_tools_enabled = true` (above); other paths
    /// that still rely on prompt-injected tool listings (e.g. providers
    /// without native tool_use) must keep getting that section.
    ///
    /// ResponseFormatLayer is intentionally not asserted here — it was
    /// unregistered from the default pipeline on 2026-05-10 and is no longer
    /// expected on any path.
    // -- resolve_max_iterations tests (H1: cap the Think→Act loop) ----------
    //
    // The harness loop must always be capped. Before this wiring the
    // orchestrator passed `max_iterations: None`, so a model that kept
    // emitting tool calls looped forever. These tests pin the resolution
    // rules: per-flow override wins, zero means "unset", and a misconfigured
    // default still yields a non-zero cap.

    #[test]
    fn resolve_max_iterations_uses_default_when_no_override() {
        assert_eq!(super::resolve_max_iterations(None, 200), 200);
    }

    #[test]
    fn resolve_max_iterations_flow_override_wins() {
        assert_eq!(super::resolve_max_iterations(Some(50), 200), 50);
    }

    #[test]
    fn resolve_max_iterations_treats_zero_override_as_unset() {
        assert_eq!(super::resolve_max_iterations(Some(0), 200), 200);
    }

    #[test]
    fn resolve_max_iterations_falls_back_when_default_is_zero() {
        // Misconfigured `[execution] max_iterations = 0` must still cap.
        assert_eq!(
            super::resolve_max_iterations(None, 0),
            super::FALLBACK_MAX_ITERATIONS
        );
    }

    #[test]
    fn resolve_max_iterations_never_returns_zero() {
        assert_eq!(
            super::resolve_max_iterations(Some(0), 0),
            super::FALLBACK_MAX_ITERATIONS
        );
    }

    #[test]
    fn legacy_prompt_config_still_emits_tools_layer() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        let prompt = PromptBuilder::new(PromptConfig::default()).build_system_prompt(&[]);

        assert!(
            prompt.contains("No tools available"),
            "ToolsLayer should still announce empty tools on the legacy path"
        );
        assert!(
            !prompt.contains("## Response Format"),
            "ResponseFormatLayer is unregistered; no path should still emit it"
        );
    }
}
