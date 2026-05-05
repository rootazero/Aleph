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
use crate::context::budget::ContextBudget;
use crate::context::compact::compactor::ContextCompactor;
use crate::harness::agent::AgentHarness;
use crate::harness::callback::HarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent, HarnessRunner};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{BrainRef, FlowHistoryTurn, FlowInput, FlowSpec};
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::sandbox::Sandbox;
use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnTrigger};
use crate::session::service::{SessionId, SessionService};
use crate::skill::prefetch::SkillPrefetcher;
use crate::tools::service::ToolService;
use crate::verification::VerifierChain;

/// Concrete [`HarnessRunner`] that dispatches to the Phase 4 `AgentHarness`.
pub struct AgentHarnessRunner {
    pub agent_registry: Arc<AgentRegistry>,
    pub session_service: Arc<dyn SessionService>,
    pub tool_service: Arc<dyn ToolService>,
    pub default_provider: Arc<dyn AiProvider>,
    /// Named providers keyed by `ProviderId`. Wired from `AuthProfileRegistry`
    /// by Task 9; empty in early boot.
    pub named_providers: HashMap<String, Arc<dyn AiProvider>>,

    // -- Task 10 (6b) optional collaborators ---------------------------------
    //
    // Injected at orchestrator boot; forwarded into `HarnessDeps` on every
    // `run()` so each `AgentHarness` instance sees the same pressure sensor
    // / compactor / hook set.
    pub verifier_chain: Option<Arc<VerifierChain>>,
    pub context_budget: Option<Arc<Mutex<ContextBudget>>>,
    pub context_compactor: Option<Arc<ContextCompactor>>,
    pub skill_prefetcher: Option<Arc<SkillPrefetcher>>,

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
        let llm = pick_llm(&spec.brain, &self.default_provider, &self.named_providers)?;
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
        seed_session(self.session_service.as_ref(), &session_id, input).await?;

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
        let deps = HarnessDeps {
            session: self.session_service.clone(),
            tools,
            sandbox,
            llm,
            verifier_chain: self.verifier_chain.clone(),
            context_budget: self.context_budget.clone(),
            context_compactor: self.context_compactor.clone(),
            skill_prefetcher: self.skill_prefetcher.clone(),
            trace_sink: trace_sink.clone(),
            system_prompt,
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: None,
            fallback_llm: None,
            max_iterations: None,
            power,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
        let harness = AgentHarness::new(deps);
        // Fans HarnessCallback events onto the FlowStreamEvent broadcast
        // channel so downstream Gateway sinks see delta / tool_call cadence
        // equivalent to the retiring AgentLoop StreamingSink.
        let mut cb = BroadcastCallback::new(events.clone());
        let run_result = harness.run(&session_id, &mut cb, &cancel).await;
        // Flush the trace sink regardless of success or error (no-op when None).
        if let Some(sink) = trace_sink.as_ref() {
            sink.flush();
        }
        run_result.map_err(|e| match e {
            crate::harness::trait_def::HarnessError::Cancelled => FlowError::Cancelled,
            other => classify_harness_error(other, &provider_name),
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
                    final_text = content.text.clone();
                    iterations = iterations.saturating_add(1);
                }
                SessionEvent::ToolCallRequested { .. } => {
                    tool_calls_made = tool_calls_made.saturating_add(1);
                }
                _ => {}
            }
        }

        // `total_tokens` still defaults to 0 — provider-side usage surfacing
        // is outside Task-10 scope. `hit_limit` is now populated from the
        // budget sensor via `AgentHarness::hit_limit()`.
        let outcome = FlowOutcome {
            final_text,
            iterations,
            tool_calls_made,
            hit_limit: harness.hit_limit(),
            ..Default::default()
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

        let mcp = self.memory_context_provider.as_ref()?;

        let session_key_str = session_id.to_key_string();

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

        let agent_def = self.agent_registry.get(agent_id);

        // Skip prompt assembly entirely when there is nothing memory-side to
        // inject AND no AgentDef to seed the role layer; the harness's
        // pre-Phase-6 behaviour was already `system_prompt: None` in that case.
        if curated_text.is_none() && memory_text.is_none() && agent_def.is_none() {
            return None;
        }

        let mut builder = PromptBuilder::new(PromptConfig::default());
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

/// Adapter that fans `HarnessCallback` lifecycle events onto the
/// orchestrator's `FlowStreamEvent` broadcast channel.
///
/// * `on_delta(text)` → `FlowStreamEvent::Delta(text)`
/// * `on_reasoning(text)` → `FlowStreamEvent::Reasoning(text)`
/// * `on_tool_call(name)` → `FlowStreamEvent::ToolCallStart { id: "legacy", name, args: null }`
/// * `on_tool_call_start(id, name, args)` → `FlowStreamEvent::ToolCallStart { id, name, args }`
/// * `on_tool_call_done(id, result, error)` → `FlowStreamEvent::ToolCallDone { id, result, error }`
/// * `on_tool_summary(id, text)` → `FlowStreamEvent::ToolSummary { id, text }`
/// * `on_safety_block(reason)` → `FlowStreamEvent::SafetyBlock { reason }`
/// * `on_stop_hook_block(reason)` → `FlowStreamEvent::StopHookBlock { reason }`
/// * `on_model_fallback(reason, fallback_model)` → `FlowStreamEvent::ModelFallback { reason, fallback_model }`
/// * `on_complete()` → no-op (`Complete(outcome)` is emitted by `AgentHarnessRunner::run`)
///
/// `broadcast::Sender::send` returns an error only when there are zero
/// receivers; we deliberately ignore that since a dropped receiver must not
/// abort the harness loop. The inner harness still produces session events
/// as the canonical log.
struct BroadcastCallback {
    tx: broadcast::Sender<FlowStreamEvent>,
}

impl BroadcastCallback {
    fn new(tx: broadcast::Sender<FlowStreamEvent>) -> Self {
        Self { tx }
    }
}

impl HarnessCallback for BroadcastCallback {
    fn on_delta(&mut self, text: &str) {
        let _ = self.tx.send(FlowStreamEvent::Delta(text.to_string()));
    }

    fn on_reasoning(&mut self, text: &str) {
        let _ = self.tx.send(FlowStreamEvent::Reasoning(text.to_string()));
    }

    /// Legacy compatibility shim — fires `ToolCallStart` with a synthetic id.
    /// Prefer `on_tool_call_start` for structured tool events.
    fn on_tool_call(&mut self, name: &str) {
        let _ = self.tx.send(FlowStreamEvent::ToolCallStart {
            id: "legacy".to_string(),
            name: name.to_string(),
            args: serde_json::Value::Null,
        });
    }

    fn on_tool_call_start(&mut self, id: &str, name: &str, args: &serde_json::Value) {
        let _ = self.tx.send(FlowStreamEvent::ToolCallStart {
            id: id.to_string(),
            name: name.to_string(),
            args: args.clone(),
        });
    }

    fn on_tool_call_done(
        &mut self,
        id: &str,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) {
        let _ = self.tx.send(FlowStreamEvent::ToolCallDone {
            id: id.to_string(),
            result: result.cloned(),
            error: error.map(|s| s.to_string()),
        });
    }

    fn on_tool_summary(&mut self, id: &str, text: &str) {
        let _ = self.tx.send(FlowStreamEvent::ToolSummary {
            id: id.to_string(),
            text: text.to_string(),
        });
    }

    fn on_safety_block(&mut self, reason: &str) {
        let _ = self.tx.send(FlowStreamEvent::SafetyBlock {
            reason: reason.to_string(),
        });
    }

    fn on_stop_hook_block(&mut self, reason: &str) {
        let _ = self.tx.send(FlowStreamEvent::StopHookBlock {
            reason: reason.to_string(),
        });
    }

    fn on_model_fallback(&mut self, reason: &str, fallback_model: &str) {
        let _ = self.tx.send(FlowStreamEvent::ModelFallback {
            reason: reason.to_string(),
            fallback_model: fallback_model.to_string(),
        });
    }

    // `on_complete` is intentionally a no-op here.
    // `AgentHarnessRunner::run` emits `Complete(outcome)` after synthesising
    // the full `FlowOutcome`, ensuring it is always the last event on the
    // broadcast channel (see Task 1 plan §Step 3).
    fn on_complete(&mut self) {}
}

/// Pick the `AiProvider` for a given [`BrainRef`]. `Strict` returns
/// `ProviderUnavailable` when the named provider is not registered; model
/// matching is deferred to Phase 6.
fn pick_llm(
    brain: &BrainRef,
    default_provider: &Arc<dyn AiProvider>,
    named: &HashMap<String, Arc<dyn AiProvider>>,
) -> Result<Arc<dyn AiProvider>, FlowError> {
    match brain {
        BrainRef::Default => Ok(default_provider.clone()),
        BrainRef::Preferred { provider } => {
            if let Some(llm) = named.get(provider) {
                Ok(llm.clone())
            } else {
                // Silent fallback is intentional — Preferred means "use this if
                // available, otherwise default." A debug log signals the mismatch
                // so operators can spot misconfigured preferred providers.
                tracing::debug!(
                    provider = %provider,
                    "preferred provider not registered, falling back to default"
                );
                Ok(default_provider.clone())
            }
        }
        BrainRef::Strict { provider, .. } => named
            .get(provider)
            .cloned()
            .ok_or_else(|| FlowError::ProviderUnavailable(provider.clone())),
    }
}

/// Seed the session log with the events required for the given [`FlowInput`].
///
/// * `Prompt` — one `UserMessage` event.
/// * `Messages` — one `UserMessage` event per entry.
/// * `History` — each turn replayed in order as the role-appropriate event,
///   then the `prompt` as a trailing `UserMessage`.
/// * `Multimodal` — one `UserMessage` event per entry (each may carry
///   non-text `blocks` that the LLM layer interprets).
///
/// Every emitted event shares a fresh `turn_id` except the trailing
/// `UserMessage` of `History`, which also emits a `TurnStarted` event so the
/// harness loop identifies the new user turn correctly.
async fn seed_session(
    service: &dyn SessionService,
    session_id: &SessionId,
    input: FlowInput,
) -> Result<(), FlowError> {
    match input {
        FlowInput::Prompt(text) => {
            emit_message(
                service,
                session_id,
                MessageContent {
                    text,
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                true,
            )
            .await?;
        }
        FlowInput::Messages(msgs) => {
            for content in msgs {
                emit_message(service, session_id, content, true).await?;
            }
        }
        FlowInput::History { turns, prompt } => {
            for turn in turns {
                match turn {
                    FlowHistoryTurn::User(content) => {
                        emit_message(service, session_id, content, true).await?;
                    }
                    FlowHistoryTurn::Assistant(content) => {
                        emit_message(service, session_id, content, false).await?;
                    }
                }
            }
            // Announce a new user turn so the harness scans from the right
            // tail boundary (see `tail_start_index` in agent.rs).
            let turn_id = uuid::Uuid::new_v4();
            service
                .emit_event(
                    session_id,
                    SessionEvent::TurnStarted {
                        turn_id,
                        trigger: TurnTrigger::UserMessage,
                        at: now_ms(),
                    },
                )
                .await
                .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
            service
                .emit_event(
                    session_id,
                    SessionEvent::UserMessage {
                        turn_id,
                        content: MessageContent {
                            text: prompt,
                            blocks: Vec::new(),
                            thinking: None,
                            thinking_signature: None,
                        },
                        at: now_ms(),
                    },
                )
                .await
                .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
        }
        FlowInput::Multimodal(msgs) => {
            for content in msgs {
                emit_message(service, session_id, content, true).await?;
            }
        }
    }
    Ok(())
}

async fn emit_message(
    service: &dyn SessionService,
    session_id: &SessionId,
    content: MessageContent,
    is_user: bool,
) -> Result<(), FlowError> {
    let event = if is_user {
        SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content,
            at: now_ms(),
        }
    } else {
        SessionEvent::AssistantMessage {
            turn_id: uuid::Uuid::new_v4(),
            content,
            at: now_ms(),
        }
    };
    service
        .emit_event(session_id, event)
        .await
        .map(|_| ())
        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))
}

/// Classify a non-cancelled `HarnessError` as either a provider-transient
/// failure (retryable by Gateway's outer fallback loop) or an internal error.
///
/// Transient indicators (per Gateway's existing classification in the
/// retiring `run_loop.rs::run_agent_loop`): HTTP 5xx (500/502/503), network
/// failures, connection drops, timeouts, and 401/403 auth errors that the
/// fallback loop used to treat as "try another provider".
///
/// Intentionally message-based — `HarnessError` wraps `AlephError` but the
/// specific AlephError variant isn't propagated structurally through
/// `HarnessError::Llm(AlephError)` in a way that survives the async trait
/// boundary without widening the public API. Message matching here mirrors
/// the exact classification the retiring run_loop did (see §5 behaviour
/// parity in the resolution design).
///
/// TODO(phase6c): replace with structural matching once HarnessError
/// surfaces a `Transient(AlephError)` variant directly.
fn classify_harness_error(
    err: crate::harness::trait_def::HarnessError,
    provider: &str,
) -> FlowError {
    let msg = err.to_string();
    if is_transient_harness_message(&msg) {
        FlowError::Transient {
            provider: provider.to_string(),
            message: msg,
        }
    } else {
        FlowError::Internal(format!("harness: {msg}"))
    }
}

fn is_transient_harness_message(msg: &str) -> bool {
    // Network / connection.
    let is_network = msg.contains("Network error")
        || msg.contains("error sending request")
        || msg.contains("connection")
        || msg.contains("dns")
        || msg.contains("timed out");
    // Auth — Gateway treats 401/403 as retryable (switch provider).
    let is_auth = msg.contains("401") || msg.contains("403") || msg.contains("Unauthorized");
    // Server — match 500/502/503 with word boundaries to avoid matching "4500".
    let is_server = contains_http_status(msg, 500)
        || contains_http_status(msg, 502)
        || contains_http_status(msg, 503);
    // Rate-limited responses are NOT treated as retryable here (mirrors the
    // retiring run_loop.rs which explicitly skips retry for rate limits).
    is_network || is_auth || is_server
}

fn contains_http_status(msg: &str, code: u16) -> bool {
    let code_str = code.to_string();
    let mut search_from = 0;
    while let Some(pos) = msg[search_from..].find(&code_str) {
        let abs_pos = search_from + pos;
        let before_ok = abs_pos == 0 || !msg.as_bytes()[abs_pos - 1].is_ascii_digit();
        let after_pos = abs_pos + code_str.len();
        let after_ok = after_pos >= msg.len() || !msg.as_bytes()[after_pos].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        search_from = abs_pos + code_str.len();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_callback_fans_lifecycle_events() {
        let (tx, mut rx) = broadcast::channel::<FlowStreamEvent>(16);
        let mut cb = BroadcastCallback::new(tx);

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
        let out = classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Transient { .. }));
    }

    #[test]
    fn classify_harness_error_http_500_is_transient() {
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::network(
            "upstream returned 500",
        ));
        let out = classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Transient { .. }));
    }

    #[test]
    fn classify_harness_error_generic_is_internal() {
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::Other {
            message: "opaque failure".into(),
            suggestion: None,
        });
        let out = classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Internal(_)));
    }

    #[test]
    fn classify_harness_error_4500_is_not_server_transient() {
        // Word-boundary check: "4500" contains "500" substring but is not status 500.
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::Other {
            message: "processed 4500 items then gave up".into(),
            suggestion: None,
        });
        let out = classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Internal(_)));
    }

    #[test]
    fn broadcast_callback_is_silent_when_no_receivers() {
        // No active receiver — `send` returns Err(SendError) but
        // BroadcastCallback swallows it so the harness loop is unaffected.
        let (tx, _rx) = broadcast::channel::<FlowStreamEvent>(1);
        drop(_rx);
        let mut cb = BroadcastCallback::new(tx);
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
        seed_session(service.as_ref(), &sid, FlowInput::Prompt("hello".into()))
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
        seed_session(service.as_ref(), &sid, FlowInput::Multimodal(msgs))
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
}
