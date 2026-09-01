#[cfg(test)]
mod tests {
    use super::super::*;
    use super::super::{build_effective_task, ephemeral_for, extract_run_result};

    use crate::sync_primitives::Mutex;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::agents::{AgentDef, AgentMode};
    use crate::error::Result as AlephResult;
    use crate::harness::chain_context::ChainContext;
    use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
    use crate::providers::AiProvider;
    use crate::session::events::{
        now_ms, MessageContent, SessionEvent, ToolOutput, ToolOutputMetadata, TurnTrigger,
    };
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use crate::session::{SessionId, SessionService};
    use crate::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

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
                        thought_signature: None,
                        id: format!("call-{n}"),
                        name: "noop".into(),
                        arguments: json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::ToolUse,
                    truncated_tool_call: None,
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
                        thought_signature: None,
                        id: format!("call-{n}"),
                        name: tool_name,
                        arguments: json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::ToolUse,
                    truncated_tool_call: None,
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
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            std::sync::Arc::from([])
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
            provider,
            chain: ChainContext::new(),
            raw_memory_writer: None,
            capture_registry: None,
            parent_agent_id: None,
            parent_session_id: None,
            guardrails: None,
            // Stage A (P1):
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
            trace_sink: None,
            // P3 Stage I:
            plugin_registry: None,
            subagent_semaphore: None,
            routing_store: None,
            // B15 — unset here on purpose: the fallback cap must hold even when
            // no runner default is threaded through.
            default_max_iterations: None,
            parallel_tool_concurrency: None,
            // Context management off in the fixture: these tests assert spawn
            // mechanics, and a wired compactor would put a side-channel LLM
            // call behind the scripted provider.
            context_budget_config: None,
            context_budget_refiner: None,
            primary_context_window: None,
            cheap_summary_provider: None,
            verifier_chain: None,
        }
    }

    // -- Context management ------------------------------------------------

    /// A subagent used to run with `context_budget` / `context_compactor` /
    /// `preflight_pipeline` all hardcoded `None`, i.e. with no context
    /// management whatsoever: the child prompt replays the whole child log
    /// every turn, nothing compacted it, and a `prompt_too_long` had no
    /// compactor to rescue with — the reactive drain went straight to
    /// `ReactiveCompactExhausted` and the whole child run died.
    ///
    /// Pins the all-or-nothing gating `HarnessDeps` documents: with a config
    /// all three are wired; without one all three stay absent (matching the
    /// main harness when `[context_budget]` is disabled). A compactor without
    /// a preflight pipeline would pay for LLM summarisation where free
    /// structural pruning was available.
    #[test]
    fn context_triple_is_all_or_nothing_and_follows_the_config() {
        let llm: Arc<dyn AiProvider> = Arc::new(crate::providers::mock::MockProvider::new("ok"));

        let (budget, compactor, preflight) = super::super::build_context_triple(
            None,
            &llm,
            None,
            "child-agent",
            &ephemeral_for("child-agent", None),
            CancellationToken::new(),
        );
        assert!(
            budget.is_none() && compactor.is_none() && preflight.is_none(),
            "no [context_budget] config → child stays unmanaged, like the main harness",
        );

        let cfg = crate::context::budget::ContextBudgetConfig {
            token_budget: 10_000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 3.5,
            fresh_tail_count: 6,
            summarizer_input_budget: 48_000,
            circuit_breaker_max: 3,
            max_splits: 3,
        };
        let child_id = ephemeral_for("child-agent", None);
        let (budget, compactor, preflight) = super::super::build_context_triple(
            Some(&cfg),
            &llm,
            None,
            "child-agent",
            &child_id,
            CancellationToken::new(),
        );
        assert!(
            budget.is_some() && compactor.is_some() && preflight.is_some(),
            "a configured child must get budget AND compactor AND preflight — \
             any one missing is the gap this test exists for",
        );
        assert_eq!(
            compactor.as_ref().and_then(|c| c.monitor_scope()),
            Some(
                crate::thinker::prompt_builder::cache_monitor::cache_scope(
                    "child-agent",
                    Some(&child_id.to_key_string())
                )
                .as_str()
            ),
            "the child's compactor must scope its cache-watchdog reset to its OWN \
             conversation — an unscoped reset zeroes every other agent's miss streak \
             in a swarm, and an agent-only scope zeroes its own siblings', which in a \
             fan-out are all the same agent id",
        );
    }

    /// The child's compactor must summarize on the parent's flash-tier provider.
    ///
    /// Asserts the *routing* (`summarizer_name`), not that a builder was called:
    /// the whole defect class this guards against is a second construction site
    /// that looks wired and bills the wrong model.
    #[test]
    fn a_child_compactor_summarizes_on_the_inherited_cheap_tier() {
        let llm: Arc<dyn AiProvider> = Arc::new(crate::providers::mock::MockProvider::new("ok"));
        let cheap: Arc<dyn AiProvider> =
            Arc::new(crate::providers::mock::MockProvider::new("ok").with_name("flash-tier"));
        let cfg = crate::context::budget::ContextBudgetConfig {
            token_budget: 10_000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 3.5,
            fresh_tail_count: 6,
            summarizer_input_budget: 48_000,
            circuit_breaker_max: 3,
            max_splits: 3,
        };
        let child_id = ephemeral_for("child-agent", None);

        let (_, with_cheap, _) = super::super::build_context_triple(
            Some(&cfg),
            &llm,
            Some(&cheap),
            "child-agent",
            &child_id,
            CancellationToken::new(),
        );
        assert_eq!(
            with_cheap.as_ref().map(|c| c.summarizer_name()),
            Some("flash-tier"),
            "an inherited cheap tier must actually be what summarization is \
             billed to — otherwise every child compaction silently charges the \
             operator's main reasoning model, multiplied by the fan-out width",
        );

        // No cheap tier resolved upstream → fall back to the child's own LLM,
        // exactly as before this was threaded.
        let (_, without, _) = super::super::build_context_triple(
            Some(&cfg),
            &llm,
            None,
            "child-agent",
            &child_id,
            CancellationToken::new(),
        );
        assert_eq!(
            without.as_ref().map(|c| c.summarizer_name()),
            Some(llm.name()),
            "no cheap tier → main LLM, unchanged legacy behaviour",
        );
    }

    fn agent_with_allowed(id: &str, tools: Vec<&str>) -> AgentDef {
        AgentDef::new(id, AgentMode::SubAgent)
            .with_allowed_tools(tools.into_iter().map(String::from).collect())
    }

    // -- Fake RawMemoryStore (used by the G2 emit test) -------------------

    /// In-memory `RawMemoryStore` capturing every insert for assertions.
    #[derive(Default)]
    struct FakeWriter(tokio::sync::Mutex<Vec<crate::memory::store::raw_memory::RawMemory>>);

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
        ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, crate::error::AlephError>
        {
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
        ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, crate::error::AlephError>
        {
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
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        assert_eq!(result.final_text.as_deref(), Some("hi from child"));
        assert_eq!(result.iterations, 1);
        assert_eq!(result.tool_calls_made, 0);
        assert!(!result.hit_limit);
    }

    #[tokio::test]
    async fn spawn_reports_provider_token_usage() {
        use crate::providers::adapter::{StopReason, TokenUsage};
        let provider = ScriptedProvider::new(vec![ProviderResponse {
            text: Some("done".to_string()),
            tool_calls: vec![],
            thinking: None,
            thinking_signature: None,
            stop_reason: StopReason::EndTurn,
            truncated_tool_call: None,
            usage: Some(TokenUsage {
                input_tokens: 12,
                output_tokens: 30,
                cache_read_tokens: Some(4),
                cache_creation_tokens: Some(2),
                thinking_tokens: None,
                cost: None,
            }),
        }]);
        let base = make_base(provider);

        let agent = agent_with_allowed("echo", vec!["*"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "say hi",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        // 12 + 30 + 4 + 2 = 48.
        assert_eq!(result.total_tokens, 48);
    }

    #[tokio::test]
    async fn spawn_blocks_when_semaphore_exhausted() {
        use tokio::sync::Semaphore;
        let sem = Arc::new(Semaphore::new(1));
        let provider = ScriptedProvider::new(vec![
            ProviderResponse::text_only("ok".into()),
            ProviderResponse::text_only("ok".into()),
        ]);
        let mut base = make_base(provider);
        base.subagent_semaphore = Some(sem.clone());

        let agent = agent_with_allowed("capped", vec!["*"]);

        // Exhaust the single permit by hand.
        let held = sem.clone().acquire_owned().await.unwrap();

        // spawn() must block on acquire — wrap in a short timeout.
        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            spawn(
                &base,
                SpawnRequest {
                    agent_def: &agent,
                    task: "noop",
                    context_summary: None,
                    model: None,
                    timeout_secs: 5,
                    cancel: CancellationToken::new(),
                    spawn_context: None,
                    fork_source: None,
                    isolation: None,
                    strategy: None,
                    session_mode: None,
                    request_id: None,
                },
            ),
        )
        .await;
        assert!(
            blocked.is_err(),
            "spawn must block while the semaphore is exhausted"
        );

        // Release the permit; the next spawn proceeds promptly.
        drop(held);
        let ran = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            spawn(
                &base,
                SpawnRequest {
                    agent_def: &agent,
                    task: "noop",
                    context_summary: None,
                    model: None,
                    timeout_secs: 5,
                    cancel: CancellationToken::new(),
                    spawn_context: None,
                    fork_source: None,
                    isolation: None,
                    strategy: None,
                    session_mode: None,
                    request_id: None,
                },
            ),
        )
        .await;
        assert!(ran.is_ok(), "spawn must proceed once a permit frees up");
        ran.unwrap().expect("spawn ok");
    }

    /// W10 — a sub-agent QUEUED behind the concurrency semaphore must observe
    /// its cancel token while parked on `acquire_owned`. Before the fix the
    /// acquire was a bare `.await`, so cancelling a queued child did nothing
    /// until every sub-agent ahead of it in the queue had finished running:
    /// cancel latency equal to the whole queue's runtime, with no error and
    /// nothing observable happening.
    #[tokio::test]
    async fn spawn_queued_behind_semaphore_honours_cancel() {
        use tokio::sync::Semaphore;
        let sem = Arc::new(Semaphore::new(1));
        let provider = ScriptedProvider::new(vec![ProviderResponse::text_only("ok".into())]);
        let mut base = make_base(provider);
        base.subagent_semaphore = Some(sem.clone());

        let agent = agent_with_allowed("capped", vec!["*"]);
        // Hold the only permit for the whole test — nothing frees it, so the
        // ONLY way this spawn returns is via the cancel arm.
        let _held = sem.clone().acquire_owned().await.unwrap();

        let cancel = CancellationToken::new();
        let fire = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            fire.cancel();
        });

        let out = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            spawn(
                &base,
                SpawnRequest {
                    agent_def: &agent,
                    task: "noop",
                    context_summary: None,
                    model: None,
                    timeout_secs: 60,
                    cancel,
                    spawn_context: None,
                    fork_source: None,
                    isolation: None,
                    strategy: None,
                    session_mode: None,
                    request_id: None,
                },
            ),
        )
        .await
        .expect("cancel must unpark the queued spawn well before the timeout");

        // Byte-exact: `background_tracker::lifecycle_from_outcome` matches this
        // string by EQUALITY, so any other wording settles the panel node as
        // `Failed` instead of `Cancelled`.
        assert_eq!(
            out.expect_err("a cancelled queue wait is an error, not a run"),
            "sub-agent failed: cancelled"
        );
    }

    /// W10 companion — a token cancelled BEFORE the spawn must not wait for a
    /// permit at all. `biased` puts the cancel arm first so an already-fired
    /// token wins outright; without the whole select the call parks on a
    /// permit that (here) is never released.
    #[tokio::test]
    async fn spawn_with_pre_cancelled_token_does_not_queue_for_a_permit() {
        use tokio::sync::Semaphore;
        let sem = Arc::new(Semaphore::new(1));
        let provider = ScriptedProvider::new(vec![ProviderResponse::text_only("ok".into())]);
        let mut base = make_base(provider);
        base.subagent_semaphore = Some(sem.clone());

        let agent = agent_with_allowed("capped", vec!["*"]);
        // The only permit is held for the whole test and never released.
        let _held = sem.clone().acquire_owned().await.unwrap();

        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            spawn(
                &base,
                SpawnRequest {
                    agent_def: &agent,
                    task: "noop",
                    context_summary: None,
                    model: None,
                    timeout_secs: 60,
                    cancel,
                    spawn_context: None,
                    fork_source: None,
                    isolation: None,
                    strategy: None,
                    session_mode: None,
                    request_id: None,
                },
            ),
        )
        .await
        .expect("a pre-cancelled spawn must not queue behind an unavailable permit")
        .expect_err("a pre-cancelled spawn must not run");
        assert_eq!(err, "sub-agent failed: cancelled");
    }

    #[tokio::test]
    async fn spawn_multi_turn_counts_iterations_and_tool_calls() {
        let provider = ScriptedProvider::new(vec![
            // Turn 1: the agent calls a tool.
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    thought_signature: None,
                    id: "call-1".into(),
                    name: "noop".into(),
                    arguments: json!({}),
                }],
                thinking: None,
                thinking_signature: None,
                stop_reason: StopReason::ToolUse,
                truncated_tool_call: None,
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
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
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
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
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
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
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
        // PermissionDenied on every turn. The harness records the error
        // as a ToolError event and continues looping until max_iterations
        // is reached (harness design: tool failures are non-fatal).
        let provider: Arc<dyn AiProvider> = Arc::new(SingleCallProvider {
            tool_name: "forbidden_tool".into(),
            called: AtomicUsize::new(0),
        });
        let base = make_base(provider);

        let agent = AgentDef::new("gated", AgentMode::SubAgent)
            .with_allowed_tools(vec!["noop".into()])
            .with_max_iterations(2);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "please don't call forbidden_tool",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        assert!(
            result.hit_limit,
            "expected hit_limit=true when allowlist blocks every tool call"
        );
        assert_eq!(
            result.iterations, 2,
            "expected max_iterations to cap the loop"
        );
        assert!(
            result.tool_calls_made > 0,
            "expected at least one attempted tool call"
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
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
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
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        assert_eq!(result.final_text.as_deref(), Some("no writer here"));
        // No assertion on captured rows — the absence of a writer is the
        // contract, and if any silent default fires the test above will catch
        // it (single row expected) before this test would.
    }

    // -- Stage H Task 8: isolation field API surface lock-in ------------------

    /// Compile-time lock: `SpawnRequest.isolation` exists and defaults to
    /// `None` at every construction site.  If the field is removed or renamed
    /// this test will not compile, catching the regression immediately.
    #[test]
    fn spawn_request_isolation_field_exists_and_defaults_none() {
        let agent = agent_with_allowed("isolation-probe", vec!["*"]);
        let cancel = CancellationToken::new();
        let req = SpawnRequest {
            agent_def: &agent,
            task: "noop",
            context_summary: None,
            model: None,
            timeout_secs: 1,
            cancel,
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };
        assert!(req.isolation.is_none());
    }

    // -- extract_run_result edge-case regression tests ------------------------

    /// Seed a session with the given sequence of `AssistantMessage` text
    /// variants (Some("…") = non-empty text turn, None = pure tool_use turn).
    async fn seed_session_with_assistant_texts(
        session: &Arc<dyn SessionService>,
        child_id: &SessionId,
        texts: &[Option<&str>],
    ) -> uuid::Uuid {
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
                            thinking: None,
                            thinking_signature: None,
                        },
                        usage: None,
                        at: now_ms(),
                    },
                )
                .await
                .unwrap();
        }
        turn
    }

    /// A forked child's tallies must cover only what the child itself did.
    ///
    /// `context=fork` seeds the child's log with a verbatim copy of the
    /// parent's transcript, and `extract_run_result` reads that same log. Two
    /// failures follow from counting the whole thing, and the second is the
    /// dangerous one:
    ///
    /// * `iterations` / `tool_calls_made` charge the parent's turns to the
    ///   child — a one-turn child forked off a busy parent reporting double
    ///   digits;
    /// * a child that produced **no** assistant message of its own (immediate
    ///   error, cancelled before its first Think) hands back *the parent's last
    ///   answer* as its finding, in a success shape. A sub-agent that quotes the
    ///   question back as its result is worse than one that fails.
    #[tokio::test]
    async fn a_forked_child_reports_only_its_own_work() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
        let child_id = ephemeral_for("forked", None);

        // Stand in for `fork::seed`: the parent's turn, copied in verbatim,
        // under the PARENT's turn id.
        let parent_turn = uuid::Uuid::new_v4();
        session.attach(child_id.clone()).await.unwrap();
        for (call, text) in [("p1", "parent's own conclusion")] {
            session
                .emit_event(
                    &child_id,
                    SessionEvent::ToolCallRequested {
                        turn_id: parent_turn,
                        call_id: call.to_string(),
                        name: "bash".into(),
                        input: json!({}),
                        at: now_ms(),
                    },
                )
                .await
                .unwrap();
            session
                .emit_event(
                    &child_id,
                    SessionEvent::AssistantMessage {
                        turn_id: parent_turn,
                        content: MessageContent {
                            text: text.to_string(),
                            blocks: Vec::new(),
                            thinking: None,
                            thinking_signature: None,
                        },
                        usage: None,
                        at: now_ms(),
                    },
                )
                .await
                .unwrap();
        }

        // Now the child's own turn — which does exactly one thing.
        let own_turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                &child_id,
                SessionEvent::TurnStarted {
                    turn_id: own_turn,
                    trigger: TurnTrigger::SubagentRequest,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        session
            .emit_event(
                &child_id,
                SessionEvent::AssistantMessage {
                    turn_id: own_turn,
                    content: MessageContent {
                        text: "the child's finding".to_string(),
                        blocks: Vec::new(),
                        thinking: None,
                        thinking_signature: None,
                    },
                    usage: None,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();

        let result = extract_run_result(session.as_ref(), &child_id, false, 0, own_turn)
            .await
            .expect("extract ok");

        assert_eq!(
            result.iterations, 1,
            "the parent's assistant turns must not be charged to the child"
        );
        assert_eq!(
            result.tool_calls_made, 0,
            "the parent's tool calls must not be charged to the child"
        );
        assert_eq!(result.final_text.as_deref(), Some("the child's finding"));
    }

    /// The other half of the same boundary: a forked child that never produced
    /// an assistant message must report *nothing*, not the parent's answer.
    #[tokio::test]
    async fn a_forked_child_that_said_nothing_does_not_borrow_the_parents_answer() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
        let child_id = ephemeral_for("forked-silent", None);

        session.attach(child_id.clone()).await.unwrap();
        let parent_turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                &child_id,
                SessionEvent::AssistantMessage {
                    turn_id: parent_turn,
                    content: MessageContent {
                        text: "PARENT-ANSWER".to_string(),
                        blocks: Vec::new(),
                        thinking: None,
                        thinking_signature: None,
                    },
                    usage: None,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();

        let own_turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                &child_id,
                SessionEvent::TurnStarted {
                    turn_id: own_turn,
                    trigger: TurnTrigger::SubagentRequest,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();

        let result = extract_run_result(session.as_ref(), &child_id, false, 0, own_turn)
            .await
            .expect("extract ok");

        assert_eq!(result.iterations, 0);
        assert_eq!(
            result.final_text, None,
            "a silent child must return nothing, never the parent's own words"
        );
    }

    /// When the final AssistantMessage has empty text but earlier turns had
    /// text, `final_text` must be cleared so stale mid-run narration is never
    /// presented as the final answer (the subagent tool reports the cap via
    /// `hit_iteration_limit` alongside).
    #[tokio::test]
    async fn final_text_cleared_when_last_assistant_is_empty() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
        let child_id = ephemeral_for("edge", None);

        // Turn 1: "thinking..." (real text). Turn 2: pure tool_use (empty).
        let turn =
            seed_session_with_assistant_texts(&session, &child_id, &[Some("thinking..."), None])
                .await;

        let result = extract_run_result(session.as_ref(), &child_id, true, 777, turn)
            .await
            .expect("extract ok");

        assert_eq!(result.iterations, 2, "should count both assistant turns");
        assert!(
            result.final_text.is_none(),
            "final_text must be None when the last assistant turn is empty (got {:?})",
            result.final_text
        );
        assert!(result.hit_limit, "hit_limit must propagate from caller");
        assert_eq!(
            result.total_tokens, 777,
            "total_tokens must propagate from caller"
        );
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
        let child_id = ephemeral_for("happy", None);

        // Turn 1: pure tool_use (empty). Turn 2: terminal text.
        let turn =
            seed_session_with_assistant_texts(&session, &child_id, &[None, Some("final answer")])
                .await;

        let result = extract_run_result(session.as_ref(), &child_id, false, 0, turn)
            .await
            .expect("extract ok");
        assert_eq!(result.final_text.as_deref(), Some("final answer"));
        assert!(!result.hit_limit);
        assert_eq!(
            result.total_tokens, 0,
            "total_tokens propagates the zero path"
        );
    }

    // -- P3 Stage I: McpScope provision tests ---------------------------------

    /// Spawn with a non-empty `mcp_servers` referencing an unknown name must
    /// fail loud with an error containing "mcp scope" and the missing name.
    /// This also validates the fail-loud path when `plugin_registry` is
    /// `Some(empty)` — every reference lookup will fail as "not found".
    #[tokio::test]
    async fn spawn_mcp_scope_unknown_reference_fails_loud() {
        use crate::agents::McpServerSpec;

        let agent = agent_with_allowed("scoped", vec!["*"]).with_mcp_servers(vec![
            McpServerSpec::Reference {
                name: "missing".into(),
            },
        ]);

        let provider = ScriptedProvider::new(vec![ProviderResponse::text_only(
            "should not reach".to_string(),
        )]);
        let mut base = make_base(provider);
        // Provide an empty registry — every reference lookup returns "not found".
        base.plugin_registry = Some(Arc::new(tokio::sync::RwLock::new(
            crate::extension::registry::PluginRegistry::new(),
        )));

        let cancel = CancellationToken::new();
        let req = SpawnRequest {
            agent_def: &agent,
            task: "noop",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel,
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };
        let err = spawn(&base, req).await.expect_err("must fail loud");
        assert!(
            err.contains("mcp scope") && err.contains("missing"),
            "got error: {err}"
        );
    }

    // -- Stage J-pre: MeteringProvider wrap -----------------------------------

    /// A provider that returns a `ProviderResponse` with `usage: Some(...)`.
    /// Used to verify that the MeteringProvider wrap in `spawn()` emits a
    /// `LoopTraceEvent::ProviderUsage` event labelled with the subagent id.
    struct UsageProvider;

    impl AiProvider for UsageProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            Box::pin(async move {
                Ok(ProviderResponse {
                    text: Some("done".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::EndTurn,
                    truncated_tool_call: None,
                    usage: Some(crate::providers::adapter::TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_tokens: Some(7),
                        cache_creation_tokens: Some(3),
                        thinking_tokens: None,
                        cost: None,
                    }),
                })
            })
        }

        fn name(&self) -> &str {
            "usage-provider"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    struct CapturingSink(std::sync::Mutex<Vec<crate::harness::trace::LoopTraceEvent>>);

    impl crate::harness::TraceSink for CapturingSink {
        fn on_trace(&self, event: &crate::harness::trace::LoopTraceEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
        fn flush(&self) {}
    }

    #[tokio::test]
    async fn subagent_spawn_emits_provider_usage_with_agent_id() {
        let provider: Arc<dyn AiProvider> = Arc::new(UsageProvider);
        let sink = Arc::new(CapturingSink(std::sync::Mutex::new(vec![])));
        let mut base = make_base(provider);
        base.trace_sink = Some(sink.clone() as Arc<dyn crate::harness::TraceSink>);

        let agent = agent_with_allowed("test-subagent-id", vec!["*"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "say hi",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };

        spawn(&base, req).await.expect("spawn ok");

        let events = sink.0.lock().unwrap();
        let usage_events: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                crate::harness::trace::LoopTraceEvent::ProviderUsage {
                    agent_id,
                    cache_read_tokens,
                    ..
                } => Some((agent_id.clone(), *cache_read_tokens)),
                _ => None,
            })
            .collect();

        assert!(
            !usage_events.is_empty(),
            "expected at least one ProviderUsage event, got none"
        );
        let (agent_id, cache_read) = &usage_events[0];
        assert_eq!(agent_id, "test-subagent-id", "agent_id label mismatch");
        assert_eq!(*cache_read, Some(7), "cache_read_tokens mismatch");
    }

    // -- VESR v1.1 (b): routing capture helpers ------------------------------

    struct NoopTraceSink;
    impl crate::harness::TraceSink for NoopTraceSink {
        fn on_trace(&self, _e: &crate::harness::trace::LoopTraceEvent) {}
        fn flush(&self) {}
    }

    struct SpawnStubEmbedder;
    #[async_trait::async_trait]
    impl crate::memory::EmbeddingProvider for SpawnStubEmbedder {
        async fn embed(&self, _t: &str) -> AlephResult<Vec<f32>> {
            Ok({
                let mut v = vec![0.0f32; 768];
                v[0] = 1.0;
                v
            })
        }
        async fn embed_batch(&self, t: &[&str]) -> AlephResult<Vec<Vec<f32>>> {
            Ok(t.iter()
                .map(|_| {
                    let mut v = vec![0.0f32; 768];
                    v[0] = 1.0;
                    v
                })
                .collect())
        }
        fn dimensions(&self) -> usize {
            768
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    fn routing_store_for_test() -> (
        tempfile::TempDir,
        Arc<crate::routing::RoutingExperienceStore>,
    ) {
        let (scratch, dir) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        let backend = Arc::new(
            crate::memory::store::sqlite::SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap(),
        );
        let embedder: Arc<dyn crate::memory::EmbeddingProvider> = Arc::new(SpawnStubEmbedder);
        (
            scratch,
            Arc::new(crate::routing::RoutingExperienceStore::new(
                backend, embedder,
            )),
        )
    }

    async fn drain_routing_row(
        store: &crate::routing::RoutingExperienceStore,
        agent: &str,
    ) -> Vec<crate::memory::store::sqlite::routing_experience::RoutingNeighbor> {
        let q = {
            let mut v = vec![0.0f32; 768];
            v[0] = 0.0;
            v
        };
        let mut got = Vec::new();
        for _ in 0..200 {
            tokio::task::yield_now().await;
            got = store.recall(agent, &q, 5).await.unwrap();
            if !got.is_empty() {
                break;
            }
        }
        got
    }

    #[tokio::test]
    async fn spawn_captures_routing_experience_under_child_agent_id() {
        let (_scratch, store) = routing_store_for_test();
        let provider = ScriptedProvider::new(vec![ProviderResponse::text_only("done".to_string())]);
        let mut base = make_base(provider);
        base.routing_store = Some(store.clone());
        base.trace_sink = Some(Arc::new(NoopTraceSink) as Arc<dyn crate::harness::TraceSink>);

        // Explicit model_hint + provider_hint → precise attribution.
        let agent = agent_with_allowed("reviewer", vec!["*"])
            .with_model_hint("claude-sonnet-4-6")
            .with_provider_hint("anthropic");
        let req = SpawnRequest {
            agent_def: &agent,
            task: "review the diff",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };

        spawn(&base, req).await.expect("spawn ok");

        let got = drain_routing_row(&store, "reviewer").await;
        assert_eq!(got.len(), 1, "subagent run recorded under child agent_id");
        assert_eq!(got[0].model_id, "claude-sonnet-4-6"); // from model_hint
        assert_eq!(got[0].provider_id, "anthropic"); // from provider_hint
    }

    // -- VESR v1.1 (b): production threading test ----------------------------

    #[tokio::test]
    async fn agent_runtime_threads_routing_store_to_capture() {
        use crate::agents::runtime::{AgentRuntime, AgentRuntimeConfig};

        let (_scratch, store) = routing_store_for_test();
        let provider = ScriptedProvider::new(vec![ProviderResponse::text_only("ok".to_string())]);

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let event_store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn SessionService> =
            Arc::new(InProcessActorSessionService::new(event_store));

        // child_chain must be descended (depth > 0) — execute_via_harness debug_asserts it.
        let chain = ChainContext::new().child().expect("descended chain");

        let runtime = AgentRuntime::new(
            provider,
            chain,
            CancellationToken::new(),
            session,
            Arc::new(AlwaysOkTools),
        )
        .with_trace_sink(Arc::new(NoopTraceSink) as Arc<dyn crate::harness::TraceSink>)
        .with_routing_store(store.clone());

        let config = AgentRuntimeConfig {
            agent_def: agent_with_allowed("planner", vec!["*"])
                .with_model_hint("claude-opus-4-8")
                .with_provider_hint("anthropic"),
            task: "plan it".to_string(),
            context_summary: None,
            spawn_context: None,
            fork_source: None,
            model: None,
            timeout_secs: 5,
            request_id: None,
        };

        runtime.run(config).await.expect("spawn ok");

        let got = drain_routing_row(&store, "planner").await;
        assert_eq!(
            got.len(),
            1,
            "production threading reaches the spawn-seam observer"
        );
        assert_eq!(got[0].model_id, "claude-opus-4-8");
        assert_eq!(got[0].provider_id, "anthropic");
    }

    // -- B5: the starting-context mode is authoritative -----------------------

    /// An isolated child must not receive the caller's summary — that is the
    /// mode's entire content — but the caller must still be told, or a
    /// 2 KB briefing vanishes with an off-target answer and nothing to
    /// correlate it to.
    #[test]
    fn build_effective_task_isolated_drops_the_summary_but_announces_it() {
        use crate::agents::SpawnContext;
        let t = build_effective_task(Some("SECRET-CONTEXT"), SpawnContext::Isolated, "do work");
        assert!(t.starts_with("do work"));
        assert!(!t.contains("SECRET-CONTEXT"), "the summary leaked: {t}");
        assert!(!t.contains("Context from parent agent"));
        assert!(
            t.contains("context=isolated") && t.contains("context=\\\"summary\\\"")
                || t.contains("context=\"summary\""),
            "a dropped summary must name the mode that dropped it and the way \
             back: {t}"
        );
    }

    #[test]
    fn build_effective_task_summary_mode_prepends_summary() {
        use crate::agents::SpawnContext;
        let t = build_effective_task(Some("PARENT-CTX"), SpawnContext::Summary, "do work");
        assert!(t.contains("Context from parent agent"));
        assert!(t.contains("PARENT-CTX"));
        assert!(t.ends_with("do work"));
    }

    /// A fork already hands the child the parent's real transcript; a précis of
    /// the same events layered on top would be a second, lossier account that
    /// contradicts the first wherever they disagree.
    #[test]
    fn build_effective_task_fork_drops_the_summary_as_redundant() {
        use crate::agents::SpawnContext;
        let t = build_effective_task(
            Some("SECRET-CONTEXT"),
            SpawnContext::Fork { turns: None },
            "do work",
        );
        assert!(!t.contains("SECRET-CONTEXT"), "the summary leaked: {t}");
        assert!(t.contains("context=fork"), "the reason must be named: {t}");
    }

    #[test]
    fn build_effective_task_no_summary_is_bare_task() {
        use crate::agents::SpawnContext;
        for mode in [
            crate::agents::SpawnContext::Isolated,
            SpawnContext::Summary,
            SpawnContext::Fork { turns: None },
        ] {
            assert_eq!(
                build_effective_task(None, mode, "just this"),
                "just this",
                "no summary means no annotation, in every mode"
            );
        }
    }

    /// The per-call knob defaults to the agent definition's declared mode, so
    /// every caller that predates it behaves exactly as before.
    #[test]
    fn an_omitted_context_argument_falls_back_to_the_agent_default() {
        use crate::agents::{ContextMode, SpawnContext};
        assert_eq!(
            SpawnContext::from_context_mode(&ContextMode::Fresh),
            SpawnContext::Isolated
        );
        assert_eq!(
            SpawnContext::from_context_mode(&ContextMode::Summary),
            SpawnContext::Summary
        );
    }

    // -- B4-01: parent_session_id_of must read flat key-strings ------------

    #[test]
    fn parent_session_id_of_round_trips_session_key_to_key_string() {
        use crate::agents::subagent_spawner::parent_session_id_of;
        use crate::routing::session_key::SessionKey;

        let key = SessionKey::Main {
            agent_id: "main".into(),
            main_key: "main".into(),
            epoch: 0,
        };
        let raw = key.to_key_string();
        let parsed = parent_session_id_of(&raw).expect("must parse flat key-string");

        assert_eq!(parsed, key);
    }

    #[test]
    fn parent_session_id_of_rejects_garbage() {
        use crate::agents::subagent_spawner::parent_session_id_of;
        assert!(parent_session_id_of("not a session key").is_none());
        assert!(parent_session_id_of("").is_none());
    }

    // -- E1: strategy weld reaches the inline system prompt ------------------

    /// Provider that records the system prompt of the first request, then
    /// returns a terminal text response so the harness stops after one turn.
    struct SystemPromptCapture(Mutex<Option<String>>);

    impl AiProvider for SystemPromptCapture {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            {
                let mut g = self.0.lock().unwrap();
                if g.is_none() {
                    *g = Some(payload.system_prompt.unwrap_or_default().to_string());
                }
            }
            Box::pin(async move { Ok(ProviderResponse::text_only("done".to_string())) })
        }

        fn name(&self) -> &str {
            "capture"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    #[tokio::test]
    async fn spawn_request_strategy_reaches_inline_system_prompt() {
        let provider = Arc::new(SystemPromptCapture(Mutex::new(None)));
        let base = make_base(provider.clone());
        let agent = agent_with_allowed("explore", vec![]);

        let req = SpawnRequest {
            agent_def: &agent,
            task: "do the thing",
            context_summary: None,
            model: None,
            timeout_secs: 30,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: Some("Objective: weld it.\nGuardrails:\n- no shortcuts"),
            session_mode: None,
            request_id: None,
        };
        let _ = spawn(&base, req).await.expect("spawn ok");

        let captured = provider.0.lock().unwrap().clone().expect("captured prompt");
        assert!(captured.contains("<strategy>"));
        assert!(captured.contains("weld it."));
    }

    /// The parent's usage mode is welded into the child prompt through the
    /// same post-pipeline seam as the strategy, so a chat/code-mode child
    /// knows why families are missing from its inherited surface — and that
    /// `tool_search` promotes them. `None` (and Work, which the wiring site
    /// skips) keeps the prompt byte-identical.
    #[tokio::test]
    async fn spawn_request_mode_reaches_inline_system_prompt() {
        let provider = Arc::new(SystemPromptCapture(Mutex::new(None)));
        let base = make_base(provider.clone());
        let agent = agent_with_allowed("explore", vec![]);

        let req = SpawnRequest {
            agent_def: &agent,
            task: "do the thing",
            context_summary: None,
            model: None,
            timeout_secs: 30,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: Some(crate::config::types::policies::SessionMode::Chat),
            request_id: None,
        };
        let _ = spawn(&base, req).await.expect("spawn ok");

        let captured = provider.0.lock().unwrap().clone().expect("captured prompt");
        assert!(captured.contains("Usage mode: chat"));
        assert!(captured.contains("tool_search"));
        // The child line must NOT carry the user-switching contract — that
        // belongs to the parent conversation, not an ephemeral child session.
        assert!(!captured.contains("session_set_mode"));
    }

    #[tokio::test]
    async fn spawn_request_without_strategy_omits_block() {
        let provider = Arc::new(SystemPromptCapture(Mutex::new(None)));
        let base = make_base(provider.clone());
        let agent = agent_with_allowed("explore", vec![]);

        let req = SpawnRequest {
            agent_def: &agent,
            task: "do the thing",
            context_summary: None,
            model: None,
            timeout_secs: 30,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };
        let _ = spawn(&base, req).await.expect("spawn ok");

        let captured = provider.0.lock().unwrap().clone().expect("captured prompt");
        assert!(!captured.contains("<strategy>"));
    }

    // -- B15: the spawned loop is never left uncapped -------------------------

    /// An `AgentDef` with no `max_iterations` (the built-in "default" role, and
    /// every user role whose frontmatter omits the key) used to reach
    /// `HarnessDeps` as `None` = *unbounded*: the child then span until the
    /// wall-clock spawn timeout killed it and discarded the run. With the
    /// runner's default threaded through, the cap fires instead — `hit_limit`
    /// plus a completed run rather than `Err("Sub-agent timed out")`.
    #[tokio::test]
    async fn spawn_caps_capless_agent_with_runner_default() {
        let provider: Arc<dyn AiProvider> = Arc::new(AlwaysToolCallProvider {
            calls: AtomicUsize::new(0),
        });
        let mut base = make_base(provider);
        base.default_max_iterations = Some(2);

        // No `.with_max_iterations(...)` — exactly the built-in "default" role.
        let agent = agent_with_allowed("capless", vec!["*"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "loop forever",
            context_summary: None,
            model: None,
            timeout_secs: 10,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        assert!(
            result.hit_limit,
            "a capless AgentDef must inherit the runner's cap, not run unbounded"
        );
        assert_eq!(
            result.iterations, 2,
            "inherited cap must be the one enforced"
        );
    }

    /// A zero cap — `[execution] max_iterations = 0` or `max_iterations: 0` in
    /// frontmatter — must be read as "unset" and fall through, never as
    /// `Some(0)` (which would kill every subagent after zero turns). This is the
    /// trap `resolve_max_iterations` exists to avoid; a bare `.or(...)` here
    /// would walk straight into it.
    #[tokio::test]
    async fn spawn_zero_cap_does_not_degrade_to_zero_iterations() {
        let provider = ScriptedProvider::new(vec![
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    thought_signature: None,
                    id: "call-0".into(),
                    name: "noop".into(),
                    arguments: json!({}),
                }],
                thinking: None,
                thinking_signature: None,
                stop_reason: StopReason::ToolUse,
                truncated_tool_call: None,
                usage: None,
            },
            ProviderResponse::text_only("finished anyway".to_string()),
        ]);
        let mut base = make_base(provider);
        base.default_max_iterations = Some(0);

        let agent = agent_with_allowed("zero-capped", vec!["*"]).with_max_iterations(0);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "do a tool call then answer",
            context_summary: None,
            model: None,
            timeout_secs: 10,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };

        let result = spawn(&base, req).await.expect("spawn ok");
        assert!(
            !result.hit_limit,
            "a zero cap must fall through to the fallback, not cap the loop at 0"
        );
        assert_eq!(result.final_text.as_deref(), Some("finished anyway"));
    }

    // -- D6: subagent tool signals reach the raw-memory sink -------------------

    /// Every tool a subagent runs must land in `raw_memories` as a
    /// `ToolInvocation` row — that is what feeds the `insights.tools` RPC. The
    /// spawner used to install a hardcoded `NoopToolSignalSink` even in
    /// production (where `raw_memory_writer` is `Some`), so subagent tool usage
    /// was invisible. Attribution is the sub-role's id, not the parent's.
    #[tokio::test]
    async fn spawn_records_tool_signals_under_sub_role_id() {
        use crate::memory::store::raw_memory::RawMemorySource;

        let provider: Arc<dyn AiProvider> = Arc::new(AlwaysToolCallProvider {
            calls: AtomicUsize::new(0),
        });
        let mut base = make_base(provider);
        let fake = Arc::new(FakeWriter::default());
        base.raw_memory_writer =
            Some(fake.clone() as Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>);
        base.parent_agent_id = Some("parent-007".to_string());

        let agent = AgentDef::new("tool-runner", AgentMode::SubAgent)
            .with_allowed_tools(vec!["*".into()])
            .with_max_iterations(1);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "call the tool",
            context_summary: None,
            model: None,
            timeout_secs: 10,
            cancel: CancellationToken::new(),
            spawn_context: None,
            fork_source: None,
            isolation: None,
            strategy: None,
            session_mode: None,
            request_id: None,
        };

        spawn(&base, req).await.expect("spawn ok");
        // The sink fires from a detached task (`push_tool_invocation`).
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let captured = fake.0.lock().await;
        let invocation = captured
            .iter()
            .find(|row| matches!(row.source, RawMemorySource::ToolInvocation { .. }))
            .expect("subagent tool call must produce a ToolInvocation raw memory row");
        match &invocation.source {
            RawMemorySource::ToolInvocation { tool_name, .. } => assert_eq!(tool_name, "noop"),
            other => panic!("expected ToolInvocation, got {other:?}"),
        }
        assert_eq!(
            invocation.agent_id, "tool-runner",
            "tool signals attribute to the sub-role that ran the tool, not the parent"
        );
    }

    // ---------------------------------------------------------------------
    // §2.3 — the environment envelope a sub-agent is given about itself.
    //
    // Before this round the child's prompt threaded no `ResolvedContext` at
    // all, so none of these layers could speak: `<environment_context>` (cwd /
    // repo / branch / model / hour), `## Operating Envelope` (writable roots,
    // network posture, run id) and the sandbox posture bullets in
    // `## Security & Constraints`. The child ran `bash` and edited files in a
    // tree it was never told about, while the parent — same tools, same tree —
    // was told all of it.
    // ---------------------------------------------------------------------

    use super::super::child_environment_context;

    fn render_child_prompt(ctx: crate::thinker::context::ResolvedContext) -> Vec<String> {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
        PromptBuilder::new(PromptConfig::default())
            .with_agent(AgentDef::new("explorer", AgentMode::SubAgent))
            .with_resolved_context(ctx)
            .build_system_prompt_parts(&[])
            .into_iter()
            .map(|p| p.content)
            .collect()
    }

    #[test]
    fn a_worktree_isolated_child_is_told_where_it_may_write() {
        // A real directory, because a provisioned worktree is one — and
        // because `RuntimeContext` now (correctly) refuses to advertise a path
        // that is not a directory.
        let tmp = tempfile::tempdir().expect("tempdir");
        let wt = tmp.path().join("aleph-6f1c2e9a");
        std::fs::create_dir(&wt).expect("mkdir worktree");
        let wt_str = wt.display().to_string();
        let ctx = child_environment_context(
            "claude-sonnet-4-5",
            Some(&wt),
            Some("agent:parent:main"),
            Some("req-42"),
            None,
        );
        let whole = render_child_prompt(ctx).join("");

        // The three facts a child running shell commands cannot get any other way.
        assert!(
            whole.contains(&format!("<cwd>{wt_str}</cwd>")),
            "the child must be told the worktree it actually executes in:\n{whole}"
        );
        assert!(
            whole.contains("<model>claude-sonnet-4-5</model>"),
            "the child must be told which model is answering:\n{whole}"
        );
        assert!(
            whole.contains(&format!("Writable roots: {wt_str}")),
            "the child must be told the root it may write to:\n{whole}"
        );
        // The two fields that had a renderer, a doc and tests but never a
        // producer, so neither string could appear for any input.
        assert!(
            whole.contains("<parent kind=\"subagent\">agent:parent:main</parent>"),
            "the parent binding never had a production writer until now:\n{whole}"
        );
        assert!(
            whole.contains("- Run id: `req-42`"),
            "the run id never had a production writer until now:\n{whole}"
        );
    }

    /// A detached background child cannot name its directory —
    /// `EXEC_WORKSPACE` is a `tokio::task_local` and does not survive
    /// `tokio::spawn` — so it must be told nothing rather than told the
    /// daemon's own directory, which is the defect §2.3 exists to prevent.
    #[test]
    fn a_child_that_cannot_name_its_directory_states_none() {
        let ctx = child_environment_context("m", None, None, None, None);
        let whole = render_child_prompt(ctx).join("");
        assert!(
            !whole.contains("<cwd>"),
            "an unknowable cwd must be omitted, never defaulted:\n{whole}"
        );
        // Silence about the directory is not silence about everything: the
        // facts that ARE knowable still ship.
        assert!(whole.contains("<model>m</model>"), "{whole}");
        assert!(whole.contains("<environment_context>"), "{whole}");
    }

    /// The reason the child prompt is split rather than handed over whole.
    ///
    /// A `context=fork` fan-out exists to give N children a warm shared
    /// prefix. Every fact this round adds differs per child, so in an
    /// undivided block each member would rewrite the entire scaffold. The
    /// stable half must be byte-identical across siblings that differ only in
    /// their per-child facts; the dynamic half is where they may differ.
    #[test]
    fn siblings_in_a_fan_out_share_a_byte_identical_cached_prefix() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wt_a = tmp.path().join("aleph-aaaa");
        let wt_b = tmp.path().join("aleph-bbbb");
        std::fs::create_dir(&wt_a).expect("mkdir a");
        std::fs::create_dir(&wt_b).expect("mkdir b");
        let a = render_child_prompt(child_environment_context(
            "m",
            Some(&wt_a),
            Some("agent:p:main"),
            Some("req-a"),
            Some(crate::config::types::policies::ExecTier::Ask),
        ));
        let b = render_child_prompt(child_environment_context(
            "m",
            Some(&wt_b),
            Some("agent:p:main"),
            Some("req-b"),
            Some(crate::config::types::policies::ExecTier::Ask),
        ));
        assert_eq!(a.len(), 2);
        assert_eq!(
            a[0], b[0],
            "the cached prefix must not carry per-child bytes — a fan-out would \
             pay cache_creation N times for an otherwise identical prefix"
        );
        assert_ne!(
            a[1], b[1],
            "the per-child facts must actually be present in the uncached half"
        );
        assert!(a[1].contains("req-a") && b[1].contains("req-b"));
        assert!(a[1].contains("aleph-aaaa") && b[1].contains("aleph-bbbb"));
    }

    /// The child inherits the sub-agent-specific wording for strategy and
    /// usage mode from the hand-welds, and the resolved context must not state
    /// either fact a second time (§2.3 rule ③: one question, one voice).
    #[test]
    fn the_resolved_context_does_not_restate_the_welded_facts() {
        let ctx = child_environment_context("m", None, None, None, None);
        assert!(
            ctx.strategy.is_none() && ctx.strategy_guardrails.is_none(),
            "the strategy is welded post-pipeline; setting it here states it twice"
        );
        assert!(
            ctx.session_mode.is_none(),
            "the usage mode is welded post-pipeline with sub-agent wording"
        );
        assert!(
            ctx.approval_tier.is_none(),
            "a caller that could not ask the enforcer must state no regime at \
             all — a default here would describe a gate nobody applies"
        );
    }

    /// The child must be told the approval regime its own calls will meet.
    ///
    /// It runs on the PARENT's `ScopedToolService`, so the tier and the very
    /// same `PlanGate` are enforced on every call it makes — and until this
    /// round its prompt said nothing about either. `Plan` is the tier that
    /// makes the silence expensive rather than merely untidy: it REFUSES
    /// instead of asking, and `subagent` is deliberately reachable under it
    /// (`PLAN_REACHABLE_TOOLS`) so a plan for a large codebase can be
    /// researched by delegation. A child that is not told spends its whole
    /// iteration budget discovering by refusal.
    #[test]
    fn a_child_is_told_the_approval_regime_its_calls_will_meet() {
        use crate::config::types::policies::ExecTier;

        for tier in [
            ExecTier::Plan,
            ExecTier::Ask,
            ExecTier::Auto,
            ExecTier::Full,
        ] {
            let whole =
                render_child_prompt(child_environment_context("m", None, None, None, Some(tier)))
                    .join("");
            assert!(
                whole.contains(tier.approval_prompt_line()),
                "a child under {} must read the regime its calls meet:\n{whole}",
                tier.id()
            );
        }

        // …and a caller with no answer states none, rather than the default
        // tier. Stating a regime nobody enforces is the expensive half.
        let silent =
            render_child_prompt(child_environment_context("m", None, None, None, None)).join("");
        assert!(
            !silent.contains("Approval mode:"),
            "an unknowable tier must be omitted, never defaulted:\n{silent}"
        );
    }

    /// The tier line must ride the DYNAMIC half.
    ///
    /// It is a per-turn knob (the composer pill, `chat.send{exec_tier}`, a
    /// human releasing the plan gate mid-turn), and the same reason it was
    /// moved out of `SecurityLayer` on the main path applies here: a byte that
    /// changes mid-conversation must not sit in the cacheable prefix, or a
    /// fan-out of N children rewrites the shared scaffold N times.
    #[test]
    fn the_approval_regime_does_not_land_in_the_cached_prefix() {
        use crate::config::types::policies::ExecTier;

        let ask = render_child_prompt(child_environment_context(
            "m",
            None,
            None,
            None,
            Some(ExecTier::Ask),
        ));
        let plan = render_child_prompt(child_environment_context(
            "m",
            None,
            None,
            None,
            Some(ExecTier::Plan),
        ));
        assert_eq!(ask.len(), 2);
        assert_eq!(
            ask[0], plan[0],
            "the tier must not reach the cacheable prefix — flipping a pill \
             would then re-WRITE the whole conversation's cached prefix at \
             1.25x instead of READing it at 0.1x"
        );
        assert!(ask[1].contains(ExecTier::Ask.approval_prompt_line()));
        assert!(plan[1].contains(ExecTier::Plan.approval_prompt_line()));
    }

    /// The effect, not the wire: a child that crossed a `tokio::spawn` must
    /// name — and jail to — the root its run was authorised for.
    ///
    /// The negative half is asserted in the same test because it is the whole
    /// reason `CarriedAttribution` has this leg: a bare spawn still loses it,
    /// and a child that lost it does not degrade to "slightly wrong cwd", it
    /// degrades to an empty `workspaces/<hash>` directory that
    /// `WorkspaceSandbox` creates on first use.
    #[tokio::test]
    async fn a_detached_child_names_the_root_it_actually_executes_in() {
        use crate::sandbox::context::with_exec_workspace;

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("authorised-root");
        std::fs::create_dir(&root).expect("mkdir root");

        let (bare, carried) = with_exec_workspace(Some(root.clone()), async {
            let bare =
                tokio::spawn(async { child_environment_context("m", None, None, None, None) })
                    .await
                    .expect("join bare");
            let carrier = crate::scope::CarriedAttribution::capture();
            let carried = tokio::spawn(
                carrier
                    .reestablish(async { child_environment_context("m", None, None, None, None) }),
            )
            .await
            .expect("join carried");
            (bare, carried)
        })
        .await;

        assert!(
            bare.runtime_context
                .as_ref()
                .and_then(|rc| rc.working_dir.as_ref())
                .is_none(),
            "a bare spawn must still lose the root — if this ever passes, the \
             carrier leg this test guards is dead weight"
        );
        assert_eq!(
            carried
                .runtime_context
                .as_ref()
                .and_then(|rc| rc.working_dir.clone())
                .as_deref(),
            Some(root.as_path()),
            "a detached child must be told the root it will actually execute \
             in, not silence and not the daemon's cwd"
        );
    }

    /// A sub-agent running inside a project room must be told who else is in
    /// the room.
    ///
    /// `ResolvedContext` has exactly two production construction sites — the
    /// gateway turn (`harness_bridge::prompt_build`) and this one — and
    /// `room_roster` was populated only at the first. The scope task-local it
    /// derives from is carried across the spawn by
    /// [`crate::scope::CarriedAttribution`] and always was, so the data was
    /// reachable the whole time: what was missing is a reader on this path.
    /// `RoomRosterLayer` sits in the pipeline BOTH paths run, so the failure
    /// mode is not an error — it is the layer rendering nothing, forever.
    ///
    /// The negative half is asserted in the same test because "always `Some`"
    /// passes the positive half while re-keying the cached prefix of every
    /// single-human deployment on earth.
    #[tokio::test]
    async fn a_child_in_a_room_inherits_the_rooms_roster() {
        use crate::scope::{ScopeAttribution, ScopeId};

        let store = crate::projects::ProjectStore::shared();
        let project = store
            .create("roster-inheritance", Some("u-owner"), None)
            .expect("create room");
        // `create` seeds the owner; a room of one deliberately renders nothing,
        // so a second member is what makes this fixture a room at all.
        store
            .add_member(&project.id, "u-bob")
            .expect("add second member");

        let in_room = crate::scope::with_scope(
            Some(ScopeAttribution {
                owner_user_id: "u-owner".to_string(),
                scope: ScopeId::Project(project.id.clone()),
            }),
            async { child_environment_context("m", None, None, None, None) },
        )
        .await;

        let roster = in_room.room_roster.as_deref().unwrap_or_else(|| {
            panic!(
                "a sub-agent delegated inside a project room was told nothing \
                 about the room. Its prompt cannot name a single teammate, so \
                 it addresses work to people it does not know exist."
            )
        });
        assert!(
            roster.contains("u-bob"),
            "the roster reached the child but is missing a member: {roster:?}"
        );
        assert!(
            roster.contains("(owner)"),
            "the owner mark is part of the rendered line's contract: {roster:?}"
        );

        let personal =
            crate::scope::with_scope(Some(ScopeAttribution::personal("u-solo")), async {
                child_environment_context("m", None, None, None, None)
            })
            .await;
        assert!(
            personal.room_roster.is_none(),
            "a personal session has no room. Emitting a block here would add \
             per-run bytes to the STABLE prefix of every single-human \
             deployment — the symptom of which appears only on the bill"
        );
    }

    /// Every `ResolvedContext` field is classified for the child, and the
    /// classification is DERIVED from the type rather than listed here.
    ///
    /// `ResolvedContext` has two production construction sites — the gateway
    /// turn and [`child_environment_context`] — and the struct's own doc names
    /// only the first ("filled in afterwards by
    /// `harness_bridge::prompt_build::resolve_prompt_context`"). So a field
    /// added for the gateway inherits **silence** here, and silence is
    /// indistinguishable from a deliberate decision not to inherit it. That is
    /// exactly how `room_roster` shipped dead: nothing was broken, a layer just
    /// rendered nothing on one of the two paths for its whole existence.
    ///
    /// The exhaustive destructure below is the mechanism: adding a field to
    /// `ResolvedContext` is a COMPILE ERROR in this test, which puts the
    /// question in front of the person adding it while they still have the
    /// answer. Do not repair such a break by adding `..` — that converts this
    /// guard back into the silence it exists to remove.
    #[tokio::test]
    async fn every_resolved_context_field_is_classified_for_the_child() {
        use crate::scope::{ScopeAttribution, ScopeId};
        use crate::thinker::context::{ResolvedContext, VoiceContext};

        let store = crate::projects::ProjectStore::shared();
        let project = store
            .create("field-census", Some("u-owner"), None)
            .expect("create room");
        store.add_member(&project.id, "u-bob").expect("add member");

        let ctx = crate::scope::with_scope(
            Some(ScopeAttribution {
                owner_user_id: "u-owner".to_string(),
                scope: ScopeId::Project(project.id.clone()),
            }),
            async {
                child_environment_context("m", None, Some("sess-parent"), Some("run-7"), None)
            },
        )
        .await;

        let ResolvedContext {
            // -- (a) The child computes its own -------------------------------
            // Facts about THIS run: its process environment, its jail, its
            // place in the delegation tree. Inheriting the parent's would be
            // actively wrong, not merely incomplete.
            environment_contract: _,
            runtime_context,
            sandbox_summary: _,
            envelope_parent,
            run_id,
            approval_tier: _,

            // -- (b) Derived from a task-local the spawn carries ---------------
            // `CarriedAttribution` re-establishes the scope across every spawn,
            // so these are reachable on BOTH paths and a child that lacks one
            // is a wire nobody connected — not a design decision.
            room_roster,

            // -- (c) The PARENT SESSION's state, deliberately not inherited ----
            // A sub-agent runs on its own ephemeral session (`ephemeral_for`).
            // These describe the parent's session, and handing them to a child
            // would tell it that the parent's plan / goal / loop / strategy /
            // voice turn is its own. Assert they stay absent so "inherit
            // everything" cannot be someone's shortcut fix to a future bug.
            runtime_state_blocks,
            execution_plan,
            standing_goal,
            graph_topology,
            timer_loop,
            strategy,
            strategy_guardrails,
            voice,
            voice_vocabulary,
            session_mode,
            memory_muted,
        } = ctx;

        // (a) — proof the fixture really reached the child site, so the (c)
        // assertions below are not vacuously green on a default-constructed
        // struct.
        assert!(runtime_context.is_some(), "the child names its own runtime");
        assert!(envelope_parent.is_some(), "the child names its parent");
        assert_eq!(run_id.as_deref(), Some("run-7"));

        // (b)
        assert!(
            room_roster.is_some(),
            "the room is ambient, and a child in one must be told who is in it"
        );

        // (c)
        assert!(runtime_state_blocks.is_empty());
        assert!(execution_plan.is_none());
        assert!(standing_goal.is_none());
        assert!(graph_topology.is_none());
        assert!(timer_loop.is_none());
        assert!(
            strategy.is_none(),
            "threaded by the builder, not the envelope"
        );
        assert!(strategy_guardrails.is_none());
        assert!(matches!(voice, VoiceContext::Off), "a child has no channel");
        assert!(voice_vocabulary.is_none());
        assert!(session_mode.is_none());
        assert!(!memory_muted);
    }
}
