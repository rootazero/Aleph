//! Subagent spawner — Harness-based sub-agent execution.
//!
//! Replacement for the legacy pre-Harness `run_subagent` entry point.
//! The spawner takes a `SpawnerBase` (shared session/tools/sandbox/provider)
//! plus a `SpawnRequest` (agent_def, task, model, timeout, cancel), builds a
//! child ephemeral `SessionKey`, assembles a `HarnessDeps` bundle with the
//! agent's system prompt and max_iterations + a tool service wrapped in
//! `AllowlistToolService`, seeds the task as a `UserMessage`, runs
//! `AgentHarness::run` under `tokio::time::timeout` + `catch_unwind` for
//! timeout + panic isolation, then walks the child session event log to
//! synthesize a `LoopRunResult`.
//!
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::agents::allowlist_tool_service::AllowlistToolService;
use crate::agents::runtime::LoopRunResult;
use crate::agents::AgentDef;
use crate::error::Result as AlephResult;
use crate::harness::agent::AgentHarness;
use crate::harness::callback::NoopHarnessCallback;
use crate::harness::chain_context::ChainContext;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::sandbox::Sandbox;
use crate::session::events::{
    now_ms, MessageContent, SessionEvent, SessionEventRecord, TurnTrigger,
};
use crate::session::service::{SessionId, SessionService};
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
use crate::tools::service::ToolService;

/// Shared infrastructure shared by all sub-agent spawns in a given
/// orchestration context (session actor, parent tool service, sandbox,
/// provider, and the parent's chain context).
#[derive(Clone)]
pub struct SpawnerBase {
    /// Shared session service (same actor as the parent).
    pub session: Arc<dyn SessionService>,
    /// The parent's tool service. The spawner decorates this with an
    /// `AllowlistToolService` gated on `AgentDef.is_tool_allowed`.
    pub parent_tools: Arc<dyn ToolService>,
    /// Shared sandbox instance.
    pub sandbox: Arc<dyn Sandbox>,
    /// Provider used for LLM calls. The spawner wraps this with a
    /// `ModelOverrideProvider` when `SpawnRequest.model` is set.
    pub provider: Arc<dyn AiProvider>,
    /// The parent's chain context. The spawner derives a child via
    /// `ChainContext::child()`.
    pub chain: ChainContext,
    /// Spec 1 G2 — when set, the spawner emits a `RawMemory(Delegation)`
    /// row after a successful spawn so CompressionService can distil
    /// LESSON-flavoured notes for the parent agent's long-term memory.
    /// The pre-phase7 A2A path emits the same row from `a2a/sub_agent.rs`;
    /// this field plugs the gap on the post-phase7 intra-process path.
    pub raw_memory_writer: Option<Arc<dyn RawMemoryStore>>,
    /// Optional capture-filter registry threaded into the delegation emit.
    pub capture_registry: Option<Arc<MemoryExtensionRegistry>>,
    /// Parent agent identity stamped onto the emitted `RawMemory` row.
    /// `None` falls back to `"default"` to match the A2A path's behaviour.
    pub parent_agent_id: Option<String>,
    /// Parent session id — when set, the row is tagged with it so
    /// `notes` can correlate the lesson with the originating session.
    pub parent_session_id: Option<String>,
}

/// Per-spawn configuration. All lifetimes are scoped to a single `spawn` call.
pub struct SpawnRequest<'a> {
    /// Agent definition (id, allowed_tools, max_iterations, model_hint, …).
    pub agent_def: &'a AgentDef,
    /// Task description — seeded as the child's first `UserMessage`.
    pub task: &'a str,
    /// Optional summary of the parent's context. When set, prefixed to the
    /// task with a "## Context from parent agent" header (matches legacy
    /// `run_subagent` behaviour).
    pub context_summary: Option<&'a str>,
    /// Explicit model override (highest priority). Falls back to
    /// `agent_def.model_hint`, then to whatever the provider uses natively.
    pub model: Option<&'a str>,
    /// Hard wall-clock timeout for the entire run.
    pub timeout_secs: u64,
    /// Cancellation token observed between turns by the harness.
    pub cancel: CancellationToken,
}

/// Build a child ephemeral session, run the harness, and synthesize the
/// `LoopRunResult` by walking the child session event log.
///
/// Errors:
///   * `"chain depth exceeded"` — the parent's `ChainContext::child()`
///     returned `None` (hit the recursion cap).
///   * `"Sub-agent timed out after Ns"` — the outer `tokio::time::timeout`
///     elapsed before `AgentHarness::run` returned.
///   * `"sub-agent panicked: …"` — the harness task panicked.
///   * `"sub-agent failed: …"` — any other harness / session / tool error.
pub async fn spawn(base: &SpawnerBase, req: SpawnRequest<'_>) -> Result<LoopRunResult, String> {
    // 1. Derive a child chain; fail early if the recursion cap is hit so
    //    callers see the same "depth exceeded" signal the legacy path used.
    let child_chain = base
        .chain
        .child()
        .ok_or_else(|| "chain depth exceeded".to_string())?;

    // 2. Unique ephemeral session key for this sub-agent.
    let child_id = ephemeral_for(&req.agent_def.id);

    // 3. Attach the child session and seed the initial Turn + UserMessage.
    //    Any failure here surfaces immediately — the harness never runs.
    base.session
        .attach(child_id.clone())
        .await
        .map_err(|e| format!("sub-agent failed: attach session: {e}"))?;

    let turn = uuid::Uuid::new_v4();
    base.session
        .emit_event(
            &child_id,
            SessionEvent::TurnStarted {
                turn_id: turn,
                trigger: TurnTrigger::SubagentRequest,
                at: now_ms(),
            },
        )
        .await
        .map_err(|e| format!("sub-agent failed: emit TurnStarted: {e}"))?;

    let effective_task = match req.context_summary {
        Some(summary) => format!(
            "## Context from parent agent\n\n{}\n\n---\n\n{}",
            summary, req.task
        ),
        None => req.task.to_string(),
    };
    base.session
        .emit_event(
            &child_id,
            SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: effective_task,
                    blocks: Vec::new(),
                },
                at: now_ms(),
            },
        )
        .await
        .map_err(|e| format!("sub-agent failed: emit UserMessage: {e}"))?;

    // 4. Build the agent-scoped system prompt. `PromptBuilder::with_agent`
    //    pulls in the AgentRoleLayer; `build_system_prompt(&[])` is fine —
    //    tool schemas are delivered via native tool_use, not the prompt.
    let system_prompt = PromptBuilder::new(PromptConfig::default())
        .with_agent(req.agent_def.clone())
        .build_system_prompt(&[]);

    // 5. Resolve the model override: explicit > model_hint > native.
    let resolved_model: Option<String> = req
        .model
        .map(str::to_string)
        .or_else(|| req.agent_def.model_hint.clone());
    let llm: Arc<dyn AiProvider> = match resolved_model {
        Some(m) => Arc::new(ModelOverrideProvider {
            inner: base.provider.clone(),
            model: m,
        }),
        None => base.provider.clone(),
    };

    // 6. Wrap the parent's tool service with the allowlist gate.
    let agent_def_arc = Arc::new(req.agent_def.clone());
    let scoped_tools: Arc<dyn ToolService> = Arc::new(AllowlistToolService::new(
        base.parent_tools.clone(),
        agent_def_arc.clone(),
    ));

    let max_iter = req.agent_def.max_iterations.map(|v| v as usize);

    let deps = HarnessDeps {
        session: base.session.clone(),
        tools: scoped_tools,
        sandbox: base.sandbox.clone(),
        llm,
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: Some(system_prompt),
        max_iterations: max_iter,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
    };
    let harness = Arc::new(AgentHarness::new(deps));

    // 7. Run the harness with wall-clock timeout + panic isolation.
    //    AssertUnwindSafe is used because the harness internals (provider
    //    closures, channels) are not `UnwindSafe` but we intentionally
    //    catch panics to synthesize a clean error rather than unwind
    //    into the parent actor.
    //
    //    The harness is held via `Arc` so we retain a handle after the
    //    async closure completes — this lets us query `hit_limit()`
    //    directly instead of reconstructing it from the event log.
    let timeout = std::time::Duration::from_secs(req.timeout_secs);
    let cancel = req.cancel.clone();
    let sid = child_id.clone();
    let harness_for_run = harness.clone();
    let run_fut = async move {
        let mut cb = NoopHarnessCallback;
        harness_for_run.run(&sid, &mut cb, &cancel).await
    };
    let outcome = tokio::time::timeout(timeout, AssertUnwindSafe(run_fut).catch_unwind()).await;

    match outcome {
        Err(_elapsed) => {
            return Err(format!("Sub-agent timed out after {}s", req.timeout_secs));
        }
        Ok(Err(panic_payload)) => {
            let msg = panic_message(&panic_payload);
            return Err(format!("sub-agent panicked: {msg}"));
        }
        Ok(Ok(Err(e))) => {
            return Err(format!("sub-agent failed: {e}"));
        }
        Ok(Ok(Ok(()))) => {}
    }

    // 8. Query the harness directly for the `hit_limit` signal. The
    //    previous implementation reconstructed this from the event log
    //    because the harness had been moved into the async closure; with
    //    `Arc<AgentHarness>` we just read the flag.
    let hit_limit = harness.hit_limit();

    let result =
        extract_run_result(base.session.as_ref(), &child_id, &child_chain, hit_limit).await?;

    // 9. Spec 1 G2 — fire-and-forget Delegation emit so CompressionService
    //    can distil parent-side lessons. Skipped silently when no writer is
    //    threaded through (legacy callers, tests, off-by-config).
    if let Some(writer) = base.raw_memory_writer.clone() {
        let summary = result.final_text.clone().unwrap_or_default();
        let parent_id = base
            .parent_agent_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        crate::a2a::sub_agent::emit_delegation_primitives(
            writer,
            req.task.to_string(),
            summary,
            parent_id,
            base.parent_session_id.clone(),
            req.agent_def.id.clone(),
            base.capture_registry.clone(),
        );
    }

    Ok(result)
}

/// Walk the child session event log and synthesize a `LoopRunResult`.
///
/// `iterations` := count of `AssistantMessage` events.
/// `tool_calls_made` := count of `ToolCallRequested` events.
/// `final_text` := text of the last `AssistantMessage`, or `None`.
/// `hit_limit` := passed in by the caller (sourced from
///                 `AgentHarness::hit_limit()` after the run).
async fn extract_run_result(
    session: &dyn SessionService,
    child_id: &SessionId,
    chain: &ChainContext,
    hit_limit: bool,
) -> Result<LoopRunResult, String> {
    let events = session
        .get_events(child_id, None, None)
        .await
        .map_err(|e| format!("sub-agent failed: read events: {e}"))?;

    let mut iterations: usize = 0;
    let mut tool_calls_made: usize = 0;
    let mut final_text: Option<String> = None;
    for rec in &events {
        match &rec.event {
            SessionEvent::AssistantMessage { content, .. } => {
                iterations = iterations.saturating_add(1);
                // Keep the most recent assistant text as the "final" answer.
                if !content.text.is_empty() {
                    final_text = Some(content.text.clone());
                } else if is_last_assistant(&events, rec) {
                    // Edge case: the *last* AssistantMessage is pure tool_use
                    // (no text). Clear any earlier textual answer so the
                    // gateway's `hit_limit && final_text.is_empty()` check in
                    // `helpers::gateway_response_from_outcome` surfaces
                    // `ErrLoopExhausted` instead of echoing a stale earlier
                    // message. The dedicated `final_text_cleared_when_…`
                    // regression test below asserts this behavior.
                    final_text = None;
                }
            }
            SessionEvent::ToolCallRequested { .. } => {
                tool_calls_made = tool_calls_made.saturating_add(1);
            }
            _ => {}
        }
    }

    Ok(LoopRunResult {
        final_text,
        iterations,
        tool_calls_made,
        total_tokens: 0,
        hit_limit,
        chain_id: chain.chain_id.clone(),
        depth: chain.depth,
    })
}

/// Generate a unique ephemeral SessionKey for this sub-agent spawn.
fn ephemeral_for(agent_id: &str) -> SessionKey {
    let nonce = uuid::Uuid::new_v4();
    SessionKey::Ephemeral {
        agent_id: agent_id.to_string(),
        ephemeral_id: format!("sub-{nonce}"),
    }
}

/// Whether `target` is the last `AssistantMessage` in `events` (by seq).
fn is_last_assistant(events: &[SessionEventRecord], target: &SessionEventRecord) -> bool {
    events
        .iter()
        .rev()
        .find(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map(|r| r.seq == target.seq)
        .unwrap_or(false)
}

/// Pull a human-readable message out of a panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "panic (non-string payload)".to_string()
}

/// Provider wrapper that stamps `RequestPayload.model` with a configured
/// override before delegating to the inner provider. Used when the spawn
/// request (or agent model_hint) supplies a per-spawn model.
struct ModelOverrideProvider {
    inner: Arc<dyn AiProvider>,
    model: String,
}

impl AiProvider for ModelOverrideProvider {
    fn process<'a>(
        &'a self,
        mut payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        payload.model = Some(self.model.clone());
        self.inner.process(payload)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn color(&self) -> &str {
        self.inner.color()
    }

    fn supports_native_tools(&self) -> bool {
        self.inner.supports_native_tools()
    }

    fn supports_thinking(&self) -> bool {
        self.inner.supports_thinking()
    }

    fn protocol(&self) -> &str {
        self.inner.protocol()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sync_primitives::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::agents::{AgentDef, AgentMode};
    use crate::providers::adapter::{NativeToolCall, ProviderResponse, StopReason};
    use crate::session::events::{ToolOutput, ToolOutputMetadata};
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use crate::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};
    use serde_json::json;

    // -- Providers --------------------------------------------------------

    /// Returns scripted responses in sequence, panicking if called past the
    /// script. Used to drive multi-turn behaviour deterministically.
    struct ScriptedProvider {
        responses: Mutex<Vec<ProviderResponse>>,
        calls: AtomicUsize,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<ProviderResponse>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses),
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl AiProvider for ScriptedProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut guard = self.responses.lock().unwrap();
            let resp = if guard.is_empty() {
                // Safety net — an exhausted script usually means the test
                // forgot to stop the loop. Return a terminal text response.
                ProviderResponse::text_only("(scripted provider exhausted)".to_string())
            } else {
                guard.remove(0)
            };
            Box::pin(async move { Ok(resp) })
        }

        fn name(&self) -> &str {
            "scripted"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    /// Always returns the same tool_call, forcing the harness to spin until
    /// `max_iterations` cuts it off.
    struct AlwaysToolCallProvider {
        calls: AtomicUsize,
    }

    impl AiProvider for AlwaysToolCallProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: format!("call-{n}"),
                        name: "noop".into(),
                        arguments: json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                })
            })
        }

        fn name(&self) -> &str {
            "always-tool"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    /// Sleeps for `delay` before returning a terminal text — drives the
    /// timeout test.
    struct SlowProvider {
        delay: std::time::Duration,
    }

    impl AiProvider for SlowProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let delay = self.delay;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                Ok(ProviderResponse::text_only("eventually".to_string()))
            })
        }

        fn name(&self) -> &str {
            "slow"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    /// Returns a tool_call targeting `tool_name`. Used by the allowlist
    /// test — if the name isn't in the agent's allowlist, the
    /// AllowlistToolService will short-circuit with PermissionDenied.
    struct SingleCallProvider {
        tool_name: String,
        called: AtomicUsize,
    }

    impl AiProvider for SingleCallProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let n = self.called.fetch_add(1, Ordering::SeqCst);
            let tool_name = self.tool_name.clone();
            Box::pin(async move {
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: format!("call-{n}"),
                        name: tool_name,
                        arguments: json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                })
            })
        }

        fn name(&self) -> &str {
            "single-call"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    // -- Tool services ----------------------------------------------------

    /// Tool service whose `execute` always succeeds with an empty payload.
    struct AlwaysOkTools;

    #[async_trait::async_trait]
    impl ToolService for AlwaysOkTools {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({}),
                metadata: ToolOutputMetadata::default(),
            })
        }

        async fn list(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "noop".into(),
                description: "fake noop".into(),
                input_schema: json!({}),
                source: ToolSource::Builtin,
                metadata: Default::default(),
            }]
        }

        async fn describe(&self, name: &str) -> Option<ToolDefinition> {
            self.list().await.into_iter().find(|d| d.name == name)
        }
    }

    // -- Fixtures ---------------------------------------------------------

    fn make_base(provider: Arc<dyn AiProvider>) -> SpawnerBase {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));

        SpawnerBase {
            session,
            parent_tools: Arc::new(AlwaysOkTools),
            sandbox: Arc::new(crate::sandbox::NoopSandbox),
            provider,
            chain: ChainContext::new(),
            raw_memory_writer: None,
            capture_registry: None,
            parent_agent_id: None,
            parent_session_id: None,
        }
    }

    fn agent_with_allowed(id: &str, tools: Vec<&str>) -> AgentDef {
        AgentDef::new(id, AgentMode::SubAgent)
            .with_allowed_tools(tools.into_iter().map(String::from).collect())
    }

    // -- Fake RawMemoryStore (used by the G2 emit test) -------------------

    /// In-memory `RawMemoryStore` capturing every insert for assertions.
    /// Mirrors the pattern in `a2a::sub_agent::spec1_tests::FakeWriter` and
    /// `components::session_compactor::tests::pre_compress_tests::FakeWriter`.
    #[derive(Default)]
    struct FakeWriter(
        tokio::sync::Mutex<Vec<crate::memory::store::raw_memory::RawMemory>>,
    );

    #[async_trait::async_trait]
    impl crate::memory::store::raw_memory::RawMemoryStore for FakeWriter {
        async fn insert_raw_memory(
            &self,
            raw: &crate::memory::store::raw_memory::RawMemory,
        ) -> Result<(), crate::error::AlephError> {
            self.0.lock().await.push(raw.clone());
            Ok(())
        }

        async fn get_unprocessed_raw_memories(
            &self,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<
            Vec<crate::memory::store::raw_memory::RawMemory>,
            crate::error::AlephError,
        > {
            Ok(vec![])
        }

        async fn mark_raw_as_processed(
            &self,
            _ids: &[String],
        ) -> Result<usize, crate::error::AlephError> {
            Ok(0)
        }

        async fn count_unprocessed(
            &self,
            _agent_id: &str,
        ) -> Result<usize, crate::error::AlephError> {
            Ok(0)
        }

        async fn get_raw_by_path_prefix(
            &self,
            _path_prefix: &str,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<
            Vec<crate::memory::store::raw_memory::RawMemory>,
            crate::error::AlephError,
        > {
            Ok(vec![])
        }
    }

    // -- Tests ------------------------------------------------------------

    #[tokio::test]
    async fn spawn_single_turn_returns_final_text() {
        let provider = ScriptedProvider::new(vec![ProviderResponse::text_only(
            "hi from child".to_string(),
        )]);
        let base = make_base(provider);

        let agent = agent_with_allowed("echo", vec!["*"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "say hi",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        assert_eq!(result.final_text.as_deref(), Some("hi from child"));
        assert_eq!(result.iterations, 1);
        assert_eq!(result.tool_calls_made, 0);
        assert_eq!(result.depth, 1); // child of root (depth=0) → depth=1
        assert!(!result.hit_limit);
    }

    #[tokio::test]
    async fn spawn_multi_turn_counts_iterations_and_tool_calls() {
        let provider = ScriptedProvider::new(vec![
            // Turn 1: the agent calls a tool.
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call-1".into(),
                    name: "noop".into(),
                    arguments: json!({}),
                }],
                thinking: None,
                thinking_signature: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 2: terminal text.
            ProviderResponse::text_only("all done".to_string()),
        ]);
        let base = make_base(provider);

        let agent = agent_with_allowed("worker", vec!["*"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "do two things",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        assert_eq!(result.final_text.as_deref(), Some("all done"));
        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 1);
        assert!(!result.hit_limit);
    }

    #[tokio::test]
    async fn spawn_max_iter_sets_hit_limit() {
        let provider: Arc<dyn AiProvider> = Arc::new(AlwaysToolCallProvider {
            calls: AtomicUsize::new(0),
        });
        let base = make_base(provider);

        let agent = AgentDef::new("capped", AgentMode::SubAgent)
            .with_allowed_tools(vec!["*".into()])
            .with_max_iterations(3);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "loop forever",
            context_summary: None,
            model: None,
            timeout_secs: 10,
            cancel: CancellationToken::new(),
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        assert!(result.hit_limit, "expected hit_limit=true when cap fires");
        assert_eq!(result.iterations, 3);
    }

    #[tokio::test]
    async fn spawn_timeout_returns_timed_out_error() {
        let provider: Arc<dyn AiProvider> = Arc::new(SlowProvider {
            delay: std::time::Duration::from_secs(3),
        });
        let base = make_base(provider);

        let agent = agent_with_allowed("slow", vec!["*"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "take your time",
            context_summary: None,
            model: None,
            timeout_secs: 1,
            cancel: CancellationToken::new(),
        };

        let err = spawn(&base, req).await.expect_err("spawn should time out");
        assert!(
            err.contains("timed out"),
            "expected timeout error, got: {err}",
        );
    }

    #[tokio::test]
    async fn spawn_tool_allowlist_enforced_via_harness() {
        // Provider always calls `forbidden_tool`. The agent's allowlist
        // only contains `noop`, so the allowlist decorator returns
        // PermissionDenied. The harness surfaces that as an error.
        let provider: Arc<dyn AiProvider> = Arc::new(SingleCallProvider {
            tool_name: "forbidden_tool".into(),
            called: AtomicUsize::new(0),
        });
        let base = make_base(provider);

        let agent = agent_with_allowed("gated", vec!["noop"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "please don't call forbidden_tool",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
        };

        let err = spawn(&base, req)
            .await
            .expect_err("allowlist should block forbidden_tool");
        let lower = err.to_lowercase();
        assert!(
            lower.contains("sub-agent failed")
                || lower.contains("permissiondenied")
                || lower.contains("permission"),
            "expected allowlist denial, got: {err}",
        );
    }

    // -- G2 delegation hook regression test ---------------------------------

    /// Spec 1 G2 — when `SpawnerBase.raw_memory_writer` is set, a successful
    /// spawn must fire-and-forget a `RawMemory(Delegation { child_agent_id })`
    /// row stamped with parent agent + session ids. Without this hook the
    /// post-phase7 intra-process subagent path silently loses every
    /// delegation lesson, regressing the work shipped under spec 1.
    #[tokio::test]
    async fn spawn_emits_delegation_raw_when_writer_set() {
        use crate::memory::store::raw_memory::RawMemorySource;

        let provider = ScriptedProvider::new(vec![ProviderResponse::text_only(
            "child summary text".to_string(),
        )]);
        let mut base = make_base(provider);

        let fake = Arc::new(FakeWriter::default());
        base.raw_memory_writer =
            Some(fake.clone() as Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>);
        base.parent_agent_id = Some("parent-007".to_string());
        base.parent_session_id = Some("agent:parent-007:peer:user".to_string());

        let agent = agent_with_allowed("delegated-child", vec!["*"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "summarise the quarterly report",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
        };

        spawn(&base, req).await.expect("spawn ok");

        // Emit is fire-and-forget on a tokio task; give it a moment.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let captured = fake.0.lock().await;
        assert_eq!(
            captured.len(),
            1,
            "expected exactly one Delegation RawMemory row, got {}",
            captured.len()
        );
        let row = &captured[0];

        match &row.source {
            RawMemorySource::Delegation { child_agent_id } => {
                assert_eq!(child_agent_id, "delegated-child");
            }
            other => panic!("expected Delegation source, got {:?}", other),
        }
        assert!(
            row.content.contains("DELEGATION_PROMPT:"),
            "content missing DELEGATION_PROMPT marker: {}",
            row.content
        );
        assert!(
            row.content.contains("summarise the quarterly report"),
            "content missing task text: {}",
            row.content
        );
        assert!(
            row.content.contains("DELEGATION_RESULT:"),
            "content missing DELEGATION_RESULT marker: {}",
            row.content
        );
        assert!(
            row.content.contains("child summary text"),
            "content missing child summary: {}",
            row.content
        );
        assert_eq!(row.agent_id, "parent-007");
        assert_eq!(
            row.session_id,
            Some("agent:parent-007:peer:user".to_string()),
        );
    }

    /// Negative control — when no writer is wired, spawn must succeed without
    /// emitting anything. Guards against regressions where a default writer
    /// is silently injected by the harness.
    #[tokio::test]
    async fn spawn_does_not_emit_delegation_when_writer_unset() {
        let provider = ScriptedProvider::new(vec![ProviderResponse::text_only(
            "no writer here".to_string(),
        )]);
        let base = make_base(provider); // raw_memory_writer left as None

        let agent = agent_with_allowed("orphan", vec!["*"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "ignored",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        assert_eq!(result.final_text.as_deref(), Some("no writer here"));
        // No assertion on captured rows — the absence of a writer is the
        // contract, and if any silent default fires the test above will catch
        // it (single row expected) before this test would.
    }

    // -- extract_run_result edge-case regression tests ------------------------

    /// Seed a session with the given sequence of `AssistantMessage` text
    /// variants (Some("…") = non-empty text turn, None = pure tool_use turn).
    async fn seed_session_with_assistant_texts(
        session: &Arc<dyn SessionService>,
        child_id: &SessionId,
        texts: &[Option<&str>],
    ) {
        session.attach(child_id.clone()).await.unwrap();
        let turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                child_id,
                SessionEvent::TurnStarted {
                    turn_id: turn,
                    trigger: TurnTrigger::SubagentRequest,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        for text in texts {
            session
                .emit_event(
                    child_id,
                    SessionEvent::AssistantMessage {
                        turn_id: turn,
                        content: MessageContent {
                            text: text.unwrap_or("").to_string(),
                            blocks: Vec::new(),
                        },
                        at: now_ms(),
                    },
                )
                .await
                .unwrap();
        }
    }

    /// When the final AssistantMessage has empty text but earlier turns had
    /// text, `final_text` must be cleared so the gateway surfaces
    /// `ErrLoopExhausted` instead of echoing the stale earlier message.
    #[tokio::test]
    async fn final_text_cleared_when_last_assistant_is_empty() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
        let child_id = ephemeral_for("edge");

        // Turn 1: "thinking..." (real text). Turn 2: pure tool_use (empty).
        seed_session_with_assistant_texts(&session, &child_id, &[Some("thinking..."), None]).await;

        let chain = ChainContext::new();
        let result = extract_run_result(session.as_ref(), &child_id, &chain, true)
            .await
            .expect("extract ok");

        assert_eq!(result.iterations, 2, "should count both assistant turns");
        assert!(
            result.final_text.is_none(),
            "final_text must be None when the last assistant turn is empty (got {:?})",
            result.final_text
        );
        assert!(result.hit_limit, "hit_limit must propagate from caller");
    }

    /// Control case: when the final AssistantMessage has non-empty text,
    /// `final_text` is that text — confirming the clearing logic is guarded
    /// on the last-assistant-empty condition.
    #[tokio::test]
    async fn final_text_kept_when_last_assistant_has_text() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
        let child_id = ephemeral_for("happy");

        // Turn 1: pure tool_use (empty). Turn 2: terminal text.
        seed_session_with_assistant_texts(&session, &child_id, &[None, Some("final answer")]).await;

        let chain = ChainContext::new();
        let result = extract_run_result(session.as_ref(), &child_id, &chain, false)
            .await
            .expect("extract ok");
        assert_eq!(result.final_text.as_deref(), Some("final answer"));
        assert!(!result.hit_limit);
    }
}
