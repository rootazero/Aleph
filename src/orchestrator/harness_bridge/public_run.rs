//! Isolated public-telecom text run (public session isolation contract §4).
//!
//! A weak public SMS caller must never reach owner state. This module builds
//! everything the run needs from scratch instead of reusing the owner path:
//!
//! - provider: the Core default provider handle only (no per-session model
//!   preference, no MoA handle, no named-brain lookup);
//! - prompt: the fixed [`PUBLIC_SYSTEM_PROMPT`], the caller's current text and
//!   the isolated, already-decrypted `request.history` of *completed public
//!   turns of this binding* (no owner history, memory, profile, project
//!   context or skills);
//! - tools: [`NullToolService`] (nothing offered, nothing callable), or —
//!   only when the owner enabled `[telecom_private_read]` — the single
//!   effect-free `request_private_lookup` signal tool
//!   (`super::public_lookup_tool`);
//! - session: an ephemeral in-memory event store dropped at the end of the run
//!   (no owner `SessionService` write, no durable journal record);
//! - no compactor, trace sink, result store, verifier chain, guardrails,
//!   epoch registrar or tool-signal sink beyond the no-op one.
//!
//! The reply is buffered: the caller receives one complete `final_text` (or
//! an error) and decides whether to deliver it.

use crate::sync_primitives::Arc;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::gateway::telecom::PublicHistoryTurn;
use crate::harness::agent::AgentHarness;
use crate::harness::callback::NoopHarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::HarnessError;
use crate::orchestrator::dispatch::{
    FlowOutcome, FlowStreamEvent, PrivateDraftRequest, PublicRunRequest, PUBLIC_PROMPT_VERSION,
};
use crate::orchestrator::errors::FlowError;
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::session::events::{now_ms, MessageContent, SessionEvent};
use crate::session::in_process::InProcessActorSessionService;
use crate::session::service::SessionService;
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use crate::tools::null::NullToolService;
use crate::tools::service::ToolService;

use super::public_lookup_tool::PrivateLookupToolService;

use super::AgentHarnessRunner;

/// Fixed system prompt of every public telecom run. Versioned by
/// [`PUBLIC_PROMPT_VERSION`]; changing the text requires bumping the version.
pub const PUBLIC_SYSTEM_PROMPT: &str = "You are Aleph answering a text message from an \
unverified public caller. You have no access to the owner's files, memory, contacts, \
calendar, accounts, tools or prior conversations, and you must not claim otherwise or \
reveal anything about the owner. Reply briefly in plain text suitable for SMS, in the \
caller's language. If the request needs the owner, say the owner will need to follow up.";

/// Appended to [`PUBLIC_SYSTEM_PROMPT`] only when the run offers
/// `request_private_lookup` (owner enabled `[telecom_private_read]`). Part of
/// the same [`PUBLIC_PROMPT_VERSION`]: the base text is unchanged and the
/// addition is a deterministic function of the request flag.
pub const PUBLIC_LOOKUP_PROMPT_ADDENDUM: &str = " If, and only if, the caller's request \
cannot be answered without the owner's private notes, call request_private_lookup once \
with a short search query. It returns no data: the owner's operator must approve any \
lookup, and the caller is told separately. Never call it for small talk or general \
questions.";

/// Appended to [`PUBLIC_SYSTEM_PROMPT`] for a private draft (approval A
/// consumed). The draft is reviewed verbatim by the operator (approval B)
/// before anything reaches the caller.
pub const PRIVATE_DRAFT_PROMPT_ADDENDUM: &str = " For this reply only, the owner's operator \
approved a read-only lookup in the owner's notes; its result follows the caller's \
message. Use only what is needed to answer the caller, share nothing else from it, and \
do not mention the lookup or the approval. The operator reviews your exact reply before \
it is sent.";

/// Prefix of the synthetic same-turn message carrying the approved lookup
/// result into a private draft.
const PRIVATE_RESULT_PREFIX: &str = "[Approved private lookup result]\n";

/// Iteration cap of a public run. Without tools one Think turn is the
/// expected shape; with the lookup tool, one call plus one closing turn.
const PUBLIC_MAX_ITERATIONS: usize = 2;

impl AgentHarnessRunner {
    /// Production body of [`crate::orchestrator::dispatch::HarnessRunner::run_public_text`]:
    /// resolves the Core default provider and nothing else from `self`.
    pub(super) async fn run_public_text_impl(
        &self,
        request: PublicRunRequest,
        cancel: CancellationToken,
        events: broadcast::Sender<FlowStreamEvent>,
    ) -> Result<FlowOutcome, FlowError> {
        // The same default provider chain (failover included), wrapped so
        // every call is floor-checked against and recorded under the
        // explicit `Principal::TelecomPublic` — not the ambient owner.
        let llm =
            crate::providers::metering::metered_public_provider(self.default_provider.current());
        run_public_with(llm, request, cancel, events).await
    }

    /// Production body of [`crate::orchestrator::dispatch::HarnessRunner::run_private_draft`]:
    /// same provider resolution and isolation as the public run.
    pub(super) async fn run_private_draft_impl(
        &self,
        request: PrivateDraftRequest,
        cancel: CancellationToken,
        events: broadcast::Sender<FlowStreamEvent>,
    ) -> Result<FlowOutcome, FlowError> {
        let llm =
            crate::providers::metering::metered_public_provider(self.default_provider.current());
        run_private_draft_with(llm, request, cancel, events).await
    }
}

/// Drive one isolated public run against `llm`. Free function so tests can
/// inject a spy provider without constructing a full runner.
///
/// With `request.allow_private_lookup` the model is offered exactly one
/// effect-free tool, `request_private_lookup`; a recorded query comes back
/// in [`FlowOutcome::private_lookup`] and the reply text of such a run is
/// only advisory (the telecom driver replaces it with a fixed notice).
pub(crate) async fn run_public_with(
    llm: Arc<dyn AiProvider>,
    request: PublicRunRequest,
    cancel: CancellationToken,
    // Buffered reply: no partial text is streamed to any sink.
    _events: broadcast::Sender<FlowStreamEvent>,
) -> Result<FlowOutcome, FlowError> {
    check_version(request.prompt_version)?;
    let (tools, lookup): (Arc<dyn ToolService>, Option<Arc<PrivateLookupToolService>>) =
        if request.allow_private_lookup {
            let svc = Arc::new(PrivateLookupToolService::new());
            (svc.clone(), Some(svc))
        } else {
            (Arc::new(NullToolService::new()), None)
        };
    let system_prompt = if lookup.is_some() {
        format!("{PUBLIC_SYSTEM_PROMPT}{PUBLIC_LOOKUP_PROMPT_ADDENDUM}")
    } else {
        PUBLIC_SYSTEM_PROMPT.to_string()
    };
    let run = run_isolated(
        llm,
        &request.run_id,
        &request.history,
        &request.text,
        None,
        system_prompt,
        tools,
        &cancel,
    )
    .await;
    let private_lookup = lookup.and_then(|svc| svc.take_query());
    match (run, private_lookup) {
        // A recorded lookup is the run's result even if the closing turn
        // produced no text: the driver discards the public reply anyway.
        (Ok(mut outcome), Some(q)) => {
            outcome.private_lookup = Some(q);
            Ok(outcome)
        }
        // The closing turn failed after the query was recorded (e.g. the
        // iteration cap): the request itself still stands; it only asks the
        // operator. A cancelled run never yields a request.
        (Err(e), Some(q)) if !matches!(e, FlowError::Cancelled) && !cancel.is_cancelled() => {
            Ok(FlowOutcome {
                private_lookup: Some(q),
                ..FlowOutcome::default()
            })
        }
        (run, _) => {
            let outcome = run?;
            if outcome.final_text.is_empty() {
                return Err(FlowError::Internal("public run produced no reply".into()));
            }
            Ok(outcome)
        }
    }
}

/// Draft the reply of an operator-approved private lookup against `llm`.
/// Same isolation as [`run_public_with`] (no tools at all, ephemeral
/// session); the lookup result rides one synthetic same-turn message.
pub(crate) async fn run_private_draft_with(
    llm: Arc<dyn AiProvider>,
    request: PrivateDraftRequest,
    cancel: CancellationToken,
    _events: broadcast::Sender<FlowStreamEvent>,
) -> Result<FlowOutcome, FlowError> {
    check_version(request.prompt_version)?;
    let carrier = format!(
        "{PRIVATE_RESULT_PREFIX}query: {}\n{}",
        request.query, request.private_result
    );
    let outcome = run_isolated(
        llm,
        &request.run_id,
        &request.history,
        &request.text,
        Some(carrier),
        format!("{PUBLIC_SYSTEM_PROMPT}{PRIVATE_DRAFT_PROMPT_ADDENDUM}"),
        Arc::new(NullToolService::new()),
        &cancel,
    )
    .await?;
    if outcome.final_text.is_empty() {
        return Err(FlowError::Internal("private draft produced no text".into()));
    }
    Ok(outcome)
}

fn check_version(version: &str) -> Result<(), FlowError> {
    if version != PUBLIC_PROMPT_VERSION {
        return Err(FlowError::InvalidConfig(format!(
            "unknown public prompt version {version}"
        )));
    }
    Ok(())
}

/// Shared isolated harness run. Returns the (possibly empty) trimmed reply
/// of the current turn.
#[allow(clippy::too_many_arguments)]
async fn run_isolated(
    llm: Arc<dyn AiProvider>,
    run_id: &str,
    history: &[PublicHistoryTurn],
    text: &str,
    same_turn_carrier: Option<String>,
    system_prompt: String,
    tools: Arc<dyn ToolService>,
    cancel: &CancellationToken,
) -> Result<FlowOutcome, FlowError> {
    if cancel.is_cancelled() {
        return Err(FlowError::Cancelled);
    }

    let started = std::time::Instant::now();
    let provider_name = llm.name().to_string();

    let (session, sid, current_turn) =
        ephemeral_session(run_id, history, text, same_turn_carrier).await?;

    let deps = HarnessDeps {
        tool_descriptor_lookup: None,
        session: session.clone(),
        tools,
        llm,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: Some(system_prompt),
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        max_iterations: Some(PUBLIC_MAX_ITERATIONS),
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let mut cb = NoopHarnessCallback;
    match harness.run(&sid, &mut cb, cancel).await {
        Ok(()) => {}
        Err(HarnessError::Cancelled) => return Err(FlowError::Cancelled),
        Err(e) => return Err(super::error::classify_harness_error(e, &provider_name)),
    }
    if cancel.is_cancelled() {
        return Err(FlowError::Cancelled);
    }

    let records = session
        .get_events(&sid, None, None)
        .await
        .map_err(|e| FlowError::Internal(format!("public session read failed: {e}")))?;
    let mut final_text = String::new();
    let mut iterations: u32 = 0;
    for r in &records {
        // Seeded history turns carry their own assistant rows; only this
        // run's turn may produce the reply (no stale answer from history).
        if let SessionEvent::AssistantMessage {
            turn_id, content, ..
        } = &r.event
        {
            if *turn_id != current_turn {
                continue;
            }
            iterations += 1;
            if !content.text.trim().is_empty() {
                final_text = content.text.clone();
            }
        }
    }

    Ok(FlowOutcome {
        final_text: final_text.trim().to_string(),
        iterations,
        total_tokens: u32::try_from(harness.total_tokens()).unwrap_or(u32::MAX),
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ..FlowOutcome::default()
    })
}

/// Fresh in-memory session seeded with the isolated completed `history`
/// turns (oldest first) followed by one open user turn: the caller's current
/// text. Returns the open turn's id. Dropped with the run.
async fn ephemeral_session(
    run_id: &str,
    history: &[PublicHistoryTurn],
    text: &str,
    same_turn_carrier: Option<String>,
) -> Result<(Arc<dyn SessionService>, SessionKey, uuid::Uuid), FlowError> {
    let internal = |e: String| FlowError::Internal(format!("public session setup failed: {e}"));
    let conn = rusqlite::Connection::open_in_memory().map_err(|e| internal(e.to_string()))?;
    migrate_add_session_events(&conn).map_err(|e| internal(e.to_string()))?;
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
    let sid = SessionKey::ephemeral(&format!("telecom-public-{run_id}"));
    session
        .attach(sid.clone())
        .await
        .map_err(|e| internal(e.to_string()))?;
    let content = |text: &str| MessageContent {
        text: text.to_string(),
        blocks: vec![],
        thinking: None,
        thinking_signature: None,
    };
    for past in history {
        let past_turn = uuid::Uuid::new_v4();
        let at = now_ms();
        let [started, user] = SessionEvent::user_turn(past_turn, content(&past.user), None, at);
        for event in [
            started,
            user,
            SessionEvent::AssistantMessage {
                turn_id: past_turn,
                content: content(&past.assistant),
                usage: None,
                at,
            },
        ] {
            session
                .emit_event(&sid, event)
                .await
                .map_err(|e| internal(e.to_string()))?;
        }
    }
    let turn = uuid::Uuid::new_v4();
    let [started, user] = SessionEvent::user_turn(turn, content(text), None, now_ms());
    let carrier = same_turn_carrier.map(|c| SessionEvent::synthetic_user(turn, c));
    for event in [started, user].into_iter().chain(carrier) {
        session
            .emit_event(&sid, event)
            .await
            .map_err(|e| internal(e.to_string()))?;
    }
    Ok((session, sid, turn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result as AlephResult;
    use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
    use crate::providers::message::UnifiedMessage;
    use crate::sync_primitives::Mutex;
    use std::future::Future;
    use std::pin::Pin;

    const CANARY: &str = "OWNER-PRIVATE-CANARY-7f3a";

    #[derive(Default)]
    struct Captured {
        system: Vec<Option<String>>,
        messages: Vec<String>,
        tools_offered: Vec<usize>,
        payloads: Vec<Vec<UnifiedMessage>>,
    }

    struct SpyProvider {
        label: &'static str,
        reply: &'static str,
        captured: Arc<Mutex<Captured>>,
    }

    impl AiProvider for SpyProvider {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            {
                let mut c = self.captured.lock().unwrap();
                c.system.push(payload.system_prompt.map(str::to_string));
                c.messages.push(format!("{:?}", payload.messages));
                c.tools_offered.push(payload.tools.map_or(0, <[_]>::len));
                c.payloads.push(payload.messages.to_vec());
            }
            let reply = self.reply;
            Box::pin(async move {
                Ok(ProviderResponse {
                    text: Some(reply.to_string()),
                    stop_reason: StopReason::EndTurn,
                    truncated_tool_call: None,
                    provider_error: None,
                    usage: None,
                    ..Default::default()
                })
            })
        }
        fn name(&self) -> &str {
            self.label
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    fn sample_ctx() -> crate::gateway::telecom::TelecomPublicContext {
        crate::gateway::telecom::public_context::test_support::sample_public_context("bind-a")
    }

    fn request(text: &str) -> PublicRunRequest {
        PublicRunRequest {
            ctx: sample_ctx(),
            text: text.to_string(),
            history: Vec::new(),
            run_id: "run-1".into(),
            prompt_version: PUBLIC_PROMPT_VERSION,
            allow_private_lookup: false,
        }
    }

    fn spy(reply: &'static str) -> (Arc<dyn AiProvider>, Arc<Mutex<Captured>>) {
        named_spy("spy", reply)
    }

    fn named_spy(
        label: &'static str,
        reply: &'static str,
    ) -> (Arc<dyn AiProvider>, Arc<Mutex<Captured>>) {
        let captured = Arc::new(Mutex::new(Captured::default()));
        (
            Arc::new(SpyProvider {
                label,
                reply,
                captured: captured.clone(),
            }),
            captured,
        )
    }

    /// Emit native calls even though the public request offers no tools, then
    /// answer after the harness has returned the errors through the adapter.
    struct MaliciousProvider {
        calls: Vec<NativeToolCall>,
        captured: Arc<Mutex<Captured>>,
    }

    impl AiProvider for MaliciousProvider {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let first = {
                let mut c = self.captured.lock().unwrap();
                let first = c.payloads.is_empty();
                c.system.push(payload.system_prompt.map(str::to_string));
                c.messages.push(format!("{:?}", payload.messages));
                c.tools_offered.push(payload.tools.map_or(0, <[_]>::len));
                c.payloads.push(payload.messages.to_vec());
                first
            };
            let calls = self.calls.clone();
            Box::pin(async move {
                Ok(if first {
                    ProviderResponse {
                        tool_calls: calls,
                        stop_reason: StopReason::ToolUse,
                        ..Default::default()
                    }
                } else {
                    ProviderResponse {
                        text: Some("public refusal only".into()),
                        stop_reason: StopReason::EndTurn,
                        ..Default::default()
                    }
                })
            })
        }
        fn name(&self) -> &str {
            "malicious-public-test"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    #[tokio::test]
    async fn provider_sees_only_fixed_prompt_and_current_text_no_tools() {
        let (llm, captured) = spy("hello caller");
        let (tx, _rx) = broadcast::channel(4);
        let out = run_public_with(
            llm,
            request("what time is it"),
            CancellationToken::new(),
            tx,
        )
        .await
        .expect("public run ok");
        assert_eq!(out.final_text, "hello caller");

        let c = captured.lock().unwrap();
        assert!(!c.system.is_empty(), "provider must be called");
        for s in &c.system {
            assert_eq!(s.as_deref(), Some(PUBLIC_SYSTEM_PROMPT));
        }
        for m in &c.messages {
            assert!(m.contains("what time is it"));
            assert!(!m.contains(CANARY));
        }
        assert!(c.tools_offered.iter().all(|n| *n == 0), "no tools offered");
    }

    /// The owner-path runner holds owner history/memory in its own session
    /// service; the public path must not read it even through the runner.
    #[tokio::test]
    #[serial_test::serial(spend_global_policy)]
    async fn runner_public_path_ignores_owner_session_canary() {
        let (llm, captured) = spy("ok");
        let (runner, owner) =
            super::super::tests::runner_with_provider_for_public(llm, CANARY).await;
        let owner_before = super::super::tests::owner_event_count(&runner, &owner).await;
        assert!(owner_before > 0, "canary planted");
        let (tx, _rx) = broadcast::channel(4);
        let out = crate::orchestrator::dispatch::HarnessRunner::run_public_text(
            &runner,
            request("hi there"),
            CancellationToken::new(),
            tx,
        )
        .await
        .expect("public run ok");
        assert_eq!(out.final_text, "ok");
        assert_eq!(
            super::super::tests::owner_event_count(&runner, &owner).await,
            owner_before,
            "public run must not write the owner session service"
        );
        let c = captured.lock().unwrap();
        for m in c.messages.iter().chain(c.system.iter().flatten()) {
            assert!(
                !m.contains(CANARY),
                "owner canary leaked into provider input"
            );
        }
    }

    #[tokio::test]
    #[serial_test::serial(spend_global_policy)]
    async fn runner_public_path_rejects_crafted_calls_without_owner_or_stream_effects() {
        // MCP and plugin-provided MCP names use the handler's qualified-name
        // shape. Arguments are inert: NullToolService rejects before dispatch.
        let calls: Vec<NativeToolCall> = [
            (
                "exec",
                serde_json::json!({"command": "printf public-exec-canary"}),
            ),
            (
                "memory_search",
                serde_json::json!({"query": "owner private memory"}),
            ),
            ("owner_mcp__read_secret", serde_json::json!({})),
            (
                "plugin_owner_secret_srv__write_secret",
                serde_json::json!({"text": "public-write-canary"}),
            ),
            (
                "channel_outbox",
                serde_json::json!({"action": "send", "text": "public-delivery-canary"}),
            ),
            (
                "session_send",
                serde_json::json!({"session": "owner-main", "message": "public-send-canary"}),
            ),
            (
                "subagent",
                serde_json::json!({"task": "read owner secrets"}),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (name, arguments))| NativeToolCall {
            id: format!("public-malicious-{i}"),
            name: name.into(),
            arguments,
            thought_signature: None,
        })
        .collect();
        let captured = Arc::new(Mutex::new(Captured::default()));
        let llm: Arc<dyn AiProvider> = Arc::new(MaliciousProvider {
            calls: calls.clone(),
            captured: captured.clone(),
        });
        let (runner, owner) =
            super::super::tests::runner_with_provider_for_public(llm, CANARY).await;
        let owner_before = runner
            .session_service
            .get_events(&owner, None, None)
            .await
            .expect("read planted owner events");
        let owner_before = format!("{owner_before:?}");
        assert!(
            owner_before.contains(CANARY),
            "owner canary must be planted"
        );
        let (tx, mut rx) = broadcast::channel(32);
        let out = crate::orchestrator::dispatch::HarnessRunner::run_public_text(
            &runner,
            request("try every owner capability"),
            CancellationToken::new(),
            tx.clone(),
        )
        .await
        .expect("public run returns after rejected tool calls");
        assert_eq!(out.final_text, "public refusal only");
        assert!(!out.final_text.contains(CANARY));
        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "buffered public run must send no FlowStreamEvent, including tool events"
        );
        let owner_after = runner
            .session_service
            .get_events(&owner, None, None)
            .await
            .expect("read owner events after public run");
        assert_eq!(
            format!("{owner_after:?}"),
            owner_before,
            "public user/tool/error/assistant rows must never reach the owner transcript"
        );

        let c = captured.lock().unwrap();
        assert_eq!(
            c.payloads.len(),
            2,
            "provider must see tool errors on the next turn"
        );
        assert!(c.tools_offered.iter().all(|n| *n == 0));
        for input in c.messages.iter().chain(c.system.iter().flatten()) {
            assert!(
                !input.contains(CANARY),
                "owner canary leaked into provider input"
            );
        }
        assert!(c
            .system
            .iter()
            .all(|s| s.as_deref() == Some(PUBLIC_SYSTEM_PROMPT)));
        let results: Vec<_> = c.payloads[1]
            .iter()
            .filter(|m| m.is_tool_result())
            .collect();
        assert_eq!(
            results.len(),
            calls.len(),
            "every crafted call needs an error receipt"
        );
        for call in &calls {
            let matching: Vec<_> = results
                .iter()
                .filter(|m| {
                    matches!(m,
                        UnifiedMessage::ToolResult { tool_call_id, .. } if tool_call_id == &call.id
                    )
                })
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "missing or duplicate receipt for {}",
                call.id
            );
            let UnifiedMessage::ToolResult {
                tool_call_id,
                tool_name,
                is_error,
                ..
            } = matching[0]
            else {
                unreachable!("filtered tool result")
            };
            assert_eq!(tool_call_id, &call.id);
            assert_eq!(tool_name, &call.name);
            assert!(*is_error, "{} must not execute successfully", call.name);
            assert!(
                matching[0]
                    .text_content()
                    .contains(&format!("tool not found: {}", call.name)),
                "{} must be rejected by the null-service seam, not silently dropped",
                call.name
            );
        }
    }

    #[tokio::test]
    async fn sequential_public_runs_do_not_retain_unremembered_user_text() {
        const FIRST_USER_CANARY: &str = "FIRST-PUBLIC-USER-EPHEMERAL-a913";
        let (first_llm, first_captured) = spy("first public answer");
        let (tx, mut rx) = broadcast::channel(8);
        let first = run_public_with(
            first_llm,
            request(FIRST_USER_CANARY),
            CancellationToken::new(),
            tx.clone(),
        )
        .await
        .expect("first public run");
        assert_eq!(first.final_text, "first public answer");
        assert!(
            first_captured
                .lock()
                .unwrap()
                .messages
                .iter()
                .any(|m| m.contains(FIRST_USER_CANARY)),
            "first run must actually see its canary"
        );

        // Reuse the same binding and run id; isolation must come from a fresh
        // session, not from choosing a different caller key or provider history.
        let (second_llm, second_captured) = spy("second public answer");
        let second = run_public_with(
            second_llm,
            request("second question without history"),
            CancellationToken::new(),
            tx.clone(),
        )
        .await
        .expect("second public run");
        assert_eq!(second.final_text, "second public answer");
        assert!(!second.final_text.contains(FIRST_USER_CANARY));
        assert_eq!(second.iterations, 1);
        let c = second_captured.lock().unwrap();
        assert_eq!(c.payloads.len(), 1);
        assert!(c.messages[0].contains("second question without history"));
        for input in c.messages.iter().chain(c.system.iter().flatten()) {
            assert!(!input.contains(FIRST_USER_CANARY));
            assert!(!input.contains("first public answer"));
        }
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    #[serial_test::serial(spend_global_policy)]
    async fn runner_public_path_uses_core_default_not_owner_agent_provider_hint() {
        let (default_llm, default_captured) = named_spy("core-default", "default public answer");
        let (override_llm, override_captured) =
            named_spy("owner-override", "owner override answer");
        let (mut runner, owner) =
            super::super::tests::runner_with_provider_for_public(default_llm, CANARY).await;
        runner
            .named_providers
            .insert("owner-override".into(), override_llm);
        runner.agent_registry.register(
            crate::agents::AgentDef::new("main", crate::agents::AgentMode::Primary)
                .with_provider_hint("owner-override")
                .with_model_hint("owner-private-model"),
        );
        let owner_before = super::super::tests::owner_event_count(&runner, &owner).await;
        let (tx, mut rx) = broadcast::channel(8);
        let out = crate::orchestrator::dispatch::HarnessRunner::run_public_text(
            &runner,
            request("use the public default"),
            CancellationToken::new(),
            tx.clone(),
        )
        .await
        .expect("public default provider run");
        assert_eq!(out.final_text, "default public answer");
        assert_eq!(
            super::super::tests::owner_event_count(&runner, &owner).await,
            owner_before
        );
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        let c = default_captured.lock().unwrap();
        assert_eq!(
            c.payloads.len(),
            1,
            "default provider must actually be reached"
        );
        for input in c.messages.iter().chain(c.system.iter().flatten()) {
            assert!(!input.contains(CANARY));
            assert!(!input.contains("owner-private-model"));
        }
        assert!(
            override_captured.lock().unwrap().payloads.is_empty(),
            "owner named provider must never be reached"
        );
    }

    #[tokio::test]
    async fn pre_cancelled_run_never_calls_provider() {
        let (llm, captured) = spy("should not appear");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (tx, _rx) = broadcast::channel(4);
        let err = run_public_with(llm, request("hi"), cancel, tx)
            .await
            .unwrap_err();
        assert!(matches!(err, FlowError::Cancelled));
        assert!(captured.lock().unwrap().system.is_empty());
    }

    #[tokio::test]
    async fn unknown_prompt_version_refused() {
        let (llm, _c) = spy("x");
        let mut req = request("hi");
        req.prompt_version = "other";
        let (tx, _rx) = broadcast::channel(4);
        let err = run_public_with(llm, req, CancellationToken::new(), tx)
            .await
            .unwrap_err();
        assert!(matches!(err, FlowError::InvalidConfig(_)));
    }

    #[tokio::test]
    async fn slash_text_is_plain_caller_text() {
        let (llm, captured) = spy("ok");
        let (tx, _rx) = broadcast::channel(4);
        run_public_with(llm, request("/model opus"), CancellationToken::new(), tx)
            .await
            .expect("plain text run");
        let c = captured.lock().unwrap();
        assert!(c.messages.iter().all(|m| m.contains("/model opus")));
        assert!(c
            .system
            .iter()
            .all(|s| s.as_deref() == Some(PUBLIC_SYSTEM_PROMPT)));
    }

    #[tokio::test]
    async fn history_reaches_provider_in_order_before_current_text() {
        let (llm, captured) = spy("fresh reply");
        let mut req = request("current question");
        req.history = vec![
            PublicHistoryTurn {
                user: "earlier-user-lilac".into(),
                assistant: "earlier-assistant-amber".into(),
            },
            PublicHistoryTurn {
                user: "second-user".into(),
                assistant: "second-assistant".into(),
            },
        ];
        let (tx, _rx) = broadcast::channel(4);
        let out = run_public_with(llm, req, CancellationToken::new(), tx)
            .await
            .expect("public run ok");
        assert_eq!(out.final_text, "fresh reply");
        assert_eq!(out.iterations, 1, "history rows are not this run's work");
        let c = captured.lock().unwrap();
        let m = &c.messages[0];
        let pos = |needle: &str| m.find(needle).unwrap_or_else(|| panic!("{needle} missing"));
        assert!(pos("earlier-user-lilac") < pos("earlier-assistant-amber"));
        assert!(pos("earlier-assistant-amber") < pos("second-user"));
        assert!(pos("second-assistant") < pos("current question"));
        assert!(c.tools_offered.iter().all(|n| *n == 0));
        assert!(!m.contains(CANARY));
    }

    #[tokio::test]
    async fn empty_new_reply_never_resurfaces_a_history_reply() {
        let (llm, _c) = spy("   ");
        let mut req = request("current question");
        req.history = vec![PublicHistoryTurn {
            user: "old-q".into(),
            assistant: "STALE-HISTORY-REPLY".into(),
        }];
        let (tx, _rx) = broadcast::channel(4);
        let err = run_public_with(llm, req, CancellationToken::new(), tx)
            .await
            .unwrap_err();
        assert!(matches!(err, FlowError::Internal(m) if !m.contains("STALE")));
    }

    // -- Private lookup (owner enabled `[telecom_private_read]`). --

    fn lookup_call(id: &str, name: &str, query: &str) -> NativeToolCall {
        NativeToolCall {
            id: id.into(),
            name: name.into(),
            arguments: serde_json::json!({ "query": query }),
            thought_signature: None,
        }
    }

    fn lookup_request(text: &str) -> PublicRunRequest {
        let mut r = request(text);
        r.allow_private_lookup = true;
        r
    }

    #[tokio::test]
    async fn disabled_lookup_offers_no_tool_and_records_nothing() {
        let captured = Arc::new(Mutex::new(Captured::default()));
        let llm: Arc<dyn AiProvider> = Arc::new(MaliciousProvider {
            calls: vec![lookup_call("c1", "request_private_lookup", "dentist")],
            captured: captured.clone(),
        });
        let (tx, _rx) = broadcast::channel(4);
        let out = run_public_with(llm, request("when is it"), CancellationToken::new(), tx)
            .await
            .expect("run ok");
        assert_eq!(out.private_lookup, None);
        let c = captured.lock().unwrap();
        assert!(c.tools_offered.iter().all(|n| *n == 0));
        assert!(c
            .system
            .iter()
            .all(|s| s.as_deref() == Some(PUBLIC_SYSTEM_PROMPT)));
        assert!(c.payloads[1].iter().any(|m| m.is_tool_result()
            && m.text_content()
                .contains("tool not found: request_private_lookup")));
    }

    #[tokio::test]
    async fn enabled_lookup_offers_one_tool_and_records_query_only() {
        let captured = Arc::new(Mutex::new(Captured::default()));
        let llm: Arc<dyn AiProvider> = Arc::new(MaliciousProvider {
            calls: vec![
                lookup_call("c1", "request_private_lookup", "dentist appointment"),
                lookup_call("c2", "memory_search", "owner private memory"),
                lookup_call("c3", "exec", "printf canary"),
            ],
            captured: captured.clone(),
        });
        let (tx, mut rx) = broadcast::channel(4);
        let out = run_public_with(
            llm,
            lookup_request("when is my appointment"),
            CancellationToken::new(),
            tx.clone(),
        )
        .await
        .expect("run ok");
        assert_eq!(out.private_lookup.as_deref(), Some("dentist appointment"));
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        let c = captured.lock().unwrap();
        assert!(c.tools_offered.iter().all(|n| *n == 1));
        let want = format!("{PUBLIC_SYSTEM_PROMPT}{PUBLIC_LOOKUP_PROMPT_ADDENDUM}");
        assert!(c.system.iter().all(|s| s.as_deref() == Some(want.as_str())));
        let results: Vec<_> = c.payloads[1]
            .iter()
            .filter(|m| m.is_tool_result())
            .collect();
        assert_eq!(results.len(), 3);
        for (name, ok) in [
            ("request_private_lookup", true),
            ("memory_search", false),
            ("exec", false),
        ] {
            let r = results
                .iter()
                .find(|m| matches!(m, UnifiedMessage::ToolResult { tool_name, .. } if tool_name == name))
                .expect("receipt");
            let UnifiedMessage::ToolResult { is_error, .. } = r else {
                unreachable!()
            };
            assert_eq!(!*is_error, ok, "{name}");
            if !ok {
                assert!(r
                    .text_content()
                    .contains(&format!("tool not found: {name}")));
            } else {
                // The fixed receipt never echoes the query back.
                assert!(!r.text_content().contains("dentist"));
            }
        }
    }

    #[tokio::test]
    async fn enabled_lookup_without_call_is_a_plain_public_reply() {
        let (llm, captured) = spy("just chatting");
        let (tx, _rx) = broadcast::channel(4);
        let out = run_public_with(llm, lookup_request("hi"), CancellationToken::new(), tx)
            .await
            .expect("run ok");
        assert_eq!(out.final_text, "just chatting");
        assert_eq!(out.private_lookup, None);
        assert_eq!(captured.lock().unwrap().tools_offered, vec![1]);
    }

    #[tokio::test]
    async fn cancelled_lookup_run_yields_no_request() {
        let (llm, captured) = spy("x");
        let (tx, _rx) = broadcast::channel(4);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = run_public_with(llm, lookup_request("hi"), cancel, tx)
            .await
            .unwrap_err();
        assert!(matches!(err, FlowError::Cancelled));
        assert!(captured.lock().unwrap().payloads.is_empty());
    }

    fn draft_request(result: &str) -> PrivateDraftRequest {
        PrivateDraftRequest {
            ctx: sample_ctx(),
            text: "when is my appointment".into(),
            history: vec![PublicHistoryTurn {
                user: "earlier question".into(),
                assistant: "earlier answer".into(),
            }],
            run_id: "run-draft".into(),
            prompt_version: PUBLIC_PROMPT_VERSION,
            query: "dentist appointment".into(),
            private_result: Arc::from(result),
        }
    }

    #[tokio::test]
    async fn private_draft_carries_result_same_turn_with_no_tools() {
        let (llm, captured) = spy("Tuesday 10:00");
        let (tx, mut rx) = broadcast::channel(4);
        let out = run_private_draft_with(
            llm,
            draft_request("PRIVATE-RESULT-CANARY dentist Tue 10:00"),
            CancellationToken::new(),
            tx.clone(),
        )
        .await
        .expect("draft ok");
        assert_eq!(out.final_text, "Tuesday 10:00");
        assert_eq!(out.private_lookup, None);
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        let c = captured.lock().unwrap();
        assert_eq!(c.tools_offered, vec![0]);
        let want = format!("{PUBLIC_SYSTEM_PROMPT}{PRIVATE_DRAFT_PROMPT_ADDENDUM}");
        assert_eq!(c.system[0].as_deref(), Some(want.as_str()));
        let msgs = &c.messages[0];
        let hist = msgs.find("earlier question").expect("history");
        let cur = msgs.find("when is my appointment").expect("current text");
        let res = msgs.find("PRIVATE-RESULT-CANARY").expect("result carrier");
        assert!(
            hist < cur && cur < res,
            "history, then caller text, then result"
        );
        assert!(msgs.contains("query: dentist appointment"));
    }

    #[tokio::test]
    async fn private_draft_empty_text_and_bad_version_fail() {
        let (llm, _c) = spy("");
        let (tx, _rx) = broadcast::channel(4);
        let err = run_private_draft_with(llm, draft_request("r"), CancellationToken::new(), tx)
            .await
            .unwrap_err();
        assert!(matches!(err, FlowError::Internal(_)));
        let (llm, c) = spy("x");
        let (tx, _rx) = broadcast::channel(4);
        let mut req = draft_request("r");
        req.prompt_version = "other";
        assert!(
            run_private_draft_with(llm, req, CancellationToken::new(), tx)
                .await
                .is_err()
        );
        assert!(c.lock().unwrap().payloads.is_empty());
    }

    #[test]
    fn private_draft_debug_redacts_query_and_result() {
        let dbg = format!("{:?}", draft_request("PRIVATE-RESULT-CANARY"));
        assert!(!dbg.contains("PRIVATE-RESULT-CANARY"));
        assert!(!dbg.contains("dentist"));
    }
}
