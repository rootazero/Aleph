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
//! * Wire `CancellationToken` into `Harness::run` once the inner loop supports
//!   cooperative abort. For now we only early-return if cancelled pre-dispatch.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::agents::AgentRegistry;
use crate::harness::agent::AgentHarness;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent, HarnessRunner};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{BrainRef, FlowInput, FlowSpec};
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::sandbox::Sandbox;
use crate::session::events::{now_ms, MessageContent, SessionEvent};
use crate::session::service::{SessionId, SessionService};
use crate::tools::service::ToolService;

/// Concrete [`HarnessRunner`] that dispatches to the Phase 4 `AgentHarness`.
pub struct AgentHarnessRunner {
    pub agent_registry: Arc<AgentRegistry>,
    pub session_service: Arc<dyn SessionService>,
    pub tool_service: Arc<dyn ToolService>,
    pub default_provider: Arc<dyn AiProvider>,
    /// Named providers keyed by `ProviderId`. Wired from `AuthProfileRegistry`
    /// by Task 9; empty in early boot.
    pub named_providers: HashMap<String, Arc<dyn AiProvider>>,
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
    ) -> Result<FlowOutcome, FlowError> {
        // Step 1: honour pre-dispatch cancellation.
        // PHASE-6: wire CancellationToken through harness.run once it supports abort.
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

        // Step 4: convert String → SessionId. Serialized SessionKeys parse
        // directly; otherwise treat the incoming string as an ephemeral id
        // under `spec.agent` so orchestrator ↔ harness session identity stays
        // deterministic (no fresh-uuid divergence).
        let session_id: SessionId = SessionKey::from_key_string(&session_key)
            .unwrap_or_else(|| SessionKey::Ephemeral {
                agent_id: spec.agent.clone(),
                ephemeral_id: session_key.clone(),
            });

        // Step 5: seed the session with the input as UserMessage event(s) so
        // the inner harness Think loop can read it. Preserve per-message
        // structure for `Messages` — do not flatten via string join.
        match input {
            FlowInput::Prompt(text) => {
                let event = SessionEvent::UserMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: MessageContent {
                        text,
                        blocks: Vec::new(),
                    },
                    at: now_ms(),
                };
                self.session_service
                    .emit_event(&session_id, event)
                    .await
                    .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
            }
            FlowInput::Messages(msgs) => {
                for content in msgs {
                    let event = SessionEvent::UserMessage {
                        turn_id: uuid::Uuid::new_v4(),
                        content,
                        at: now_ms(),
                    };
                    self.session_service
                        .emit_event(&session_id, event)
                        .await
                        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
                }
            }
        }

        // Step 6: assemble HarnessDeps and run the inner Think→Act loop.
        let deps = HarnessDeps {
            session: self.session_service.clone(),
            tools: self.tool_service.clone(),
            sandbox,
            llm,
        };
        let harness = AgentHarness::new(deps);
        // PHASE 6a Task 2 replaces NoopHarnessCallback with a
        // `BroadcastCallback` that fans `on_delta`/`on_tool_call` into
        // `FlowStreamEvent`. Task 1 keeps the surface minimal and correct.
        let mut cb = crate::harness::NoopHarnessCallback;
        harness
            .run(&session_id, &mut cb)
            .await
            .map_err(|e| match e {
                crate::harness::trait_def::HarnessError::Cancelled => FlowError::Cancelled,
                other => FlowError::Internal(format!("harness: {other}")),
            })?;

        // Step 7: read final AssistantMessage text + count assistant turns.
        let records = self
            .session_service
            .get_events(&session_id, None, None)
            .await
            .map_err(|e| FlowError::Internal(format!("session read: {e}")))?;

        let mut final_text = String::new();
        let mut iterations: u32 = 0;
        for r in &records {
            if let SessionEvent::AssistantMessage { content, .. } = &r.event {
                final_text = content.text.clone();
                iterations = iterations.saturating_add(1);
            }
        }

        let _ = events.send(FlowStreamEvent::Complete);
        Ok(FlowOutcome {
            final_text,
            iterations,
        })
    }
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
        BrainRef::Preferred { provider } => Ok(named
            .get(provider)
            .cloned()
            .unwrap_or_else(|| default_provider.clone())),
        BrainRef::Strict { provider, .. } => named
            .get(provider)
            .cloned()
            .ok_or_else(|| FlowError::ProviderUnavailable(provider.clone())),
    }
}
