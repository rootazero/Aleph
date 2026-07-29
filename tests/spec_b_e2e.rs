//! Spec-B E2E integration tests.
//!
//! Each test uses `TestEnv` — a minimal, fully in-memory harness that wires
//! together `SessionSearchTool`, `SummarySynthesizer`, and a controllable
//! `MockSummaryLlm` without touching the real database or network.
//!
//! Tests 14-17 reuse `TestEnv::build()` — do NOT add per-test seeds inside
//! `build()`.

// test-only tuple return type reads clearer inline.
#![allow(clippy::type_complexity)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;

use alephcore::builtin_tools::session_search::SummarySource;
use alephcore::builtin_tools::session_search::{SessionSearchArgs, SessionSearchTool};
use alephcore::gateway::agent_instance::AgentRegistry;
use alephcore::gateway::context::GatewayContext;
use alephcore::gateway::execution_adapter::ExecutionAdapter;
use alephcore::gateway::execution_engine::{ExecutionError, RunRequest, RunState, RunStatus};
use alephcore::gateway::inter_agent_policy::AgentToAgentPolicy;
use alephcore::gateway::router::SessionKey;
use alephcore::gateway::session_manager::SessionState;
use alephcore::gateway::session_store::error::SessionStoreError;
use alephcore::gateway::session_store::types::*;
use alephcore::gateway::session_store::SessionStore;
use alephcore::gateway::{AgentInstance, EventEmitter};
use alephcore::memory::assembler::{
    FeedbackFloorLoader, HybridAssembler, LlmReranker, UserProfileLoader, WorkingMemoryAssembler,
};
use alephcore::memory::embedding_provider::EmbeddingProvider;
use alephcore::memory::note_retrieval::NoteFactRetrieval;
use alephcore::memory::notes::NoteIndexer;
use alephcore::memory::session_resume::reader::SnapshotReader;
use alephcore::memory::session_search_summary::end_hook::SessionEndSummarizer;
use alephcore::memory::session_search_summary::synthesizer::{SummaryLlm, SummarySynthesizer};
use alephcore::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use alephcore::memory::SqliteMemoryBackend;
use alephcore::tools::AlephTool;
use alephcore::AlephError;
use alephcore::AssemblerConfig;

// ---------------------------------------------------------------------------
// NeverCalledReranker — stub reranker; skeleton path never calls it
// ---------------------------------------------------------------------------

struct NeverCalledReranker;

#[async_trait]
impl LlmReranker for NeverCalledReranker {
    async fn complete(&self, _prompt: &str, _model: Option<&str>) -> Result<String, AlephError> {
        Err(AlephError::config(
            "NeverCalledReranker must not be invoked".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// ZeroEmbedder — all embeddings are 768-dim zero vectors
// ---------------------------------------------------------------------------

struct ZeroEmbedder;

#[async_trait]
impl EmbeddingProvider for ZeroEmbedder {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Ok(vec![0.0_f32; 768])
    }
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Ok(texts.iter().map(|_| vec![0.0_f32; 768]).collect())
    }
    fn dimensions(&self) -> usize {
        768
    }
    fn model_name(&self) -> &str {
        "zero"
    }
    fn provider_id(&self) -> &str {
        "zero"
    }
}

// ---------------------------------------------------------------------------
// MockExecutionAdapter — no-op, satisfies the trait bound on GatewayContext
// ---------------------------------------------------------------------------

struct MockExecutionAdapter;

#[async_trait]
impl ExecutionAdapter for MockExecutionAdapter {
    async fn execute(
        &self,
        _request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> std::result::Result<(), ExecutionError> {
        Ok(())
    }

    async fn cancel(&self, run_id: &str) -> std::result::Result<(), ExecutionError> {
        Err(ExecutionError::RunNotFound(run_id.to_string()))
    }

    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        Some(RunStatus {
            run_id: run_id.to_string(),
            state: RunState::Completed,
            started_at: Some(chrono::Utc::now()),
            completed_at: Some(chrono::Utc::now()),
            steps_completed: 0,
            current_tool: None,
        })
    }

    async fn active_run_count(&self) -> usize {
        0
    }
}

// ---------------------------------------------------------------------------
// MockSummaryLlm — controllable LLM with a call counter
// ---------------------------------------------------------------------------

pub struct MockSummaryLlm {
    response: StdMutex<String>,
    call_count: AtomicUsize,
}

impl MockSummaryLlm {
    pub fn new(initial_response: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            response: StdMutex::new(initial_response.into()),
            call_count: AtomicUsize::new(0),
        })
    }

    pub fn set_response(&self, s: impl Into<String>) {
        *self.response.lock().unwrap() = s.into();
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SummaryLlm for MockSummaryLlm {
    async fn complete(&self, _prompt: &str) -> Result<String, AlephError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.lock().unwrap().clone())
    }
}

// ---------------------------------------------------------------------------
// E2eSessionStore — in-memory SessionStore supporting both load_window and
// search_messages so the lazy fallback path in call_impl works end-to-end.
// ---------------------------------------------------------------------------

pub struct E2eSessionStore {
    // (agent_id, session_id) -> Vec<(role, content)>
    pub messages: StdMutex<HashMap<(String, String), Vec<(String, String)>>>,
}

impl E2eSessionStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            messages: StdMutex::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl SessionStore for E2eSessionStore {
    // --- load_window: used by SummarySynthesizer ---
    async fn load_window(
        &self,
        agent_id: &str,
        session_id: &str,
        max_turns: usize,
        max_chars: usize,
    ) -> Result<Vec<(String, String)>, SessionStoreError> {
        let map = self.messages.lock().unwrap();
        let key = (agent_id.to_string(), session_id.to_string());
        let msgs = match map.get(&key) {
            Some(v) => v.clone(),
            None => return Ok(vec![]),
        };
        drop(map);

        let start = if msgs.len() > max_turns {
            msgs.len() - max_turns
        } else {
            0
        };
        let slice = &msgs[start..];
        let mut result = Vec::with_capacity(slice.len());
        let mut total = 0usize;
        for (role, content) in slice {
            let chars = content.len();
            if total + chars > max_chars && !result.is_empty() {
                break;
            }
            total += chars;
            result.push((role.clone(), content.clone()));
        }
        Ok(result)
    }

    // --- search_messages: used by call_impl lazy fallback ---
    async fn search_messages(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, SessionStoreError> {
        let map = self.messages.lock().unwrap();
        let query_lc = query.to_lowercase();
        let mut hits = Vec::new();
        for ((agent_id, session_id), msgs) in map.iter() {
            for (role, content) in msgs {
                if content.to_lowercase().contains(&query_lc) {
                    hits.push(SearchHit {
                        session_key: session_id.clone(),
                        agent_id: agent_id.clone(),
                        role: role.clone(),
                        content: content.clone(),
                        timestamp: 0,
                        topic: None,
                    });
                    if hits.len() >= max_results {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    }

    // --- All other methods are stubs ---
    async fn get_or_create(&self, _key: &SessionKey) -> Result<SessionMetadata, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn get_metadata(
        &self,
        _key: &SessionKey,
    ) -> Result<Option<SessionMetadata>, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn list_sessions(
        &self,
        _filter: SessionFilter,
    ) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn delete_session(&self, _key: &SessionKey) -> Result<DeleteResult, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn reset_session(&self, _key: &SessionKey) -> Result<bool, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn append_message(
        &self,
        _key: &SessionKey,
        _msg: MessageRecord,
    ) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn get_history(
        &self,
        _key: &SessionKey,
        _limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn list_checkpoints(
        &self,
        _key: &SessionKey,
    ) -> Result<Vec<CheckpointSummary>, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn branch_from_checkpoint(
        &self,
        _key: &SessionKey,
        _checkpoint_id: &str,
        _new_key: &SessionKey,
    ) -> Result<SessionMetadata, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn restore_checkpoint(
        &self,
        _key: &SessionKey,
        _checkpoint_id: &str,
    ) -> Result<SessionMetadata, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn close_session(
        &self,
        _key: &SessionKey,
        _topic: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn set_topic(&self, _key: &SessionKey, _topic: &str) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn set_state(
        &self,
        _key: &SessionKey,
        _state: SessionState,
    ) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn get_state(&self, _key: &SessionKey) -> Result<SessionState, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn get_identity_context(
        &self,
        _session_key: &str,
        _source_channel: &str,
    ) -> Result<aleph_protocol::IdentityContext, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn get_current_epoch(&self, _base_key_pattern: &str) -> Result<u32, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn get_session_topic(
        &self,
        _key: &SessionKey,
    ) -> Result<Option<String>, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn cleanup_expired(&self) -> Result<usize, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn patch_session(
        &self,
        _key: &SessionKey,
        _patch: &SessionPatch,
    ) -> Result<bool, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn update_session_usage(
        &self,
        _key: &SessionKey,
        _input_tokens: i64,
        _output_tokens: i64,
        _cost_usd: f64,
        _model: Option<&str>,
        _model_provider: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn get_session_preview(
        &self,
        _key: &SessionKey,
        _message_limit: usize,
    ) -> Result<SessionPreview, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn count_by_state(&self, _state: SessionState) -> Result<usize, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn list_by_state(
        &self,
        _state: SessionState,
    ) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn set_error(
        &self,
        _key: &SessionKey,
        _error_msg: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn stop(&self, _key: &SessionKey) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
    async fn set_idle(&self, _key: &SessionKey) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::DatabaseError("stub".into()))
    }
}

// ---------------------------------------------------------------------------
// TestEnv — reusable harness for Tasks 13-17
// ---------------------------------------------------------------------------

/// Shared test harness. `build()` creates an empty environment; callers seed
/// data with `seed_session()` / `seed_raw_memory()` before each test.
pub struct TestEnv {
    pub raw_store: Arc<SqliteMemoryBackend>,
    pub session_store: Arc<E2eSessionStore>,
    pub mock_llm: Arc<MockSummaryLlm>,
    pub synthesizer: Arc<SummarySynthesizer>,
    pub assembler: Arc<dyn WorkingMemoryAssembler>,
    pub context: Arc<GatewayContext>,
}

impl TestEnv {
    pub async fn build() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("raw.db");
        let notes_dir = tmp.path().join("notes");
        std::fs::create_dir_all(&notes_dir).expect("notes dir");
        let raw_store = Arc::new(SqliteMemoryBackend::new(&db_path).expect("SqliteMemoryBackend"));
        // Keep the tempdir alive for the lifetime of the TestEnv.
        // We leak it deliberately — the OS will reclaim it after the test process exits.
        std::mem::forget(tmp);
        let session_store = E2eSessionStore::new();
        let mock_llm = MockSummaryLlm::new("[default response — override with set_response]");

        let synthesizer = Arc::new(SummarySynthesizer::new(
            raw_store.clone() as Arc<dyn RawMemoryStore>,
            session_store.clone() as Arc<dyn SessionStore>,
            mock_llm.clone(),
        ));

        // Build HybridAssembler with force_fallback=true (skeleton path, zero LLM calls).
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(ZeroEmbedder);
        let indexer = Arc::new(NoteIndexer::new(notes_dir.clone(), raw_store.clone()));
        let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
        let snap_dir = notes_dir.join("_snapshots");
        std::fs::create_dir_all(&snap_dir).expect("snap dir");
        let snapshots = Arc::new(SnapshotReader::new(&snap_dir));
        let profile = UserProfileLoader::new(notes_dir.clone());
        let feedback_floor = FeedbackFloorLoader::new(notes_dir);
        let reranker: Arc<dyn LlmReranker> = Arc::new(NeverCalledReranker);
        let assembler_cfg = AssemblerConfig {
            enabled: true,
            total_budget_tokens: 4000,
            candidate_pool_limit: 20,
            rerank_timeout_ms: 200,
            rerank_model: None,
            render_style: Default::default(),
            force_fallback: true,
            fallback_skeleton: Default::default(),
            assembly_log: Default::default(),
            project_scoped: false,
            retrieval_scoring: Default::default(),
            rerank: Default::default(),
            expansion: Default::default(),
        };
        let assembler: Arc<dyn WorkingMemoryAssembler> = Arc::new(HybridAssembler::new(
            retrieval,
            snapshots,
            raw_store.clone(),
            profile,
            feedback_floor,
            reranker,
            assembler_cfg,
        ));

        let agent_registry = Arc::new(AgentRegistry::new());
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let a2a_policy = Arc::new(AgentToAgentPolicy::permissive());

        let context = Arc::new(GatewayContext::new(
            session_store.clone() as Arc<dyn SessionStore>,
            agent_registry,
            execution_adapter,
            a2a_policy,
        ));

        Self {
            raw_store,
            session_store,
            mock_llm,
            synthesizer,
            assembler,
            context,
        }
    }

    /// Build a `SessionSearchTool` for the given caller agent with no assembler.
    /// Passes `assembler = None` so the primary summary path is skipped and the
    /// lazy FTS5 fallback path (which exercises `SummarySynthesizer::lazy_for`)
    /// is taken.
    pub fn build_tool_no_assembler(&self, caller: &str) -> SessionSearchTool {
        SessionSearchTool::new(
            self.context.clone(),
            caller,
            None,                           // skip primary assembler path
            Some(self.synthesizer.clone()), // enable lazy synthesis
        )
    }

    /// Build a `SessionSearchTool` for the given caller agent with the assembler wired in.
    /// Exercises the primary summary-driven retrieval path (Task 14+).
    pub fn build_tool_with_assembler(&self, caller: &str) -> SessionSearchTool {
        SessionSearchTool::new(
            self.context.clone(),
            caller,
            Some(self.assembler.clone()),
            Some(self.synthesizer.clone()),
        )
    }

    /// Seed transcript messages into `E2eSessionStore`.
    /// Both `search_messages` (FTS5 fallback trigger) and `load_window`
    /// (synthesizer transcript loader) read from here.
    pub fn seed_session(&self, agent_id: &str, session_id: &str, msgs: &[(&str, &str)]) {
        let mut map = self.session_store.messages.lock().unwrap();
        map.insert(
            (agent_id.to_string(), session_id.to_string()),
            msgs.iter()
                .map(|(r, c)| (r.to_string(), c.to_string()))
                .collect(),
        );
    }

    /// Seed a pre-synthesized RawMemory fact directly into the raw store,
    /// bypassing the LLM synthesizer (e.g. for Task 14's compactor scenario).
    pub async fn seed_raw_memory(&self, raw: RawMemory) {
        self.raw_store
            .insert_raw_memory(&raw)
            .await
            .expect("seed_raw_memory");
    }

    /// Create a SessionEndSummarizer wired with this environment's store and synthesizer.
    /// Used to test the session-end hook in isolation (Task 15).
    pub fn session_end_summarizer(&self) -> Arc<SessionEndSummarizer> {
        Arc::new(SessionEndSummarizer {
            store: self.raw_store.clone(),
            synthesizer: self.synthesizer.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Task 13 — fresh_short_session_lazy_synthesis
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fresh_short_session_lazy_synthesis() {
    let env = TestEnv::build().await;

    // Seed a short session with no pre-existing /end-summary fact.
    env.seed_session(
        "agent-1",
        "session-A",
        &[
            ("user", "How do I deploy to production?"),
            ("assistant", "kubectl apply -f k8s/"),
        ],
    );

    // Prime the mock LLM with a canned response.
    env.mock_llm.set_response(
        "## Primary Request\nDeployment guidance via kubectl.\n## Key Outcome\nUser learned kubectl apply.",
    );

    let tool = env.build_tool_no_assembler("agent-1");

    // --- First call: should trigger lazy synthesis ---
    let result = tool
        .call(SessionSearchArgs {
            query: "deploy".into(),
            max_results: 5,
        })
        .await
        .expect("first call failed");

    let hit = result
        .hits
        .iter()
        .find(|h| h.session_key == "session-A")
        .expect("expected a hit for session-A");

    assert_eq!(
        hit.source,
        SummarySource::Lazy,
        "first call must be Lazy (no pre-existing /end-summary)"
    );
    assert!(
        hit.summary.contains("kubectl") || hit.summary.contains("Deployment"),
        "summary should contain transcript content, got: {}",
        hit.summary
    );
    assert_eq!(
        env.mock_llm.call_count(),
        1,
        "expected exactly one LLM call on first invocation"
    );

    // --- Second call: must hit the cached /end-summary, NO extra LLM call ---
    let result2 = tool
        .call(SessionSearchArgs {
            query: "deploy".into(),
            max_results: 5,
        })
        .await
        .expect("second call failed");

    assert!(
        result2.hits.iter().any(|h| h.session_key == "session-A"),
        "session-A should still appear on second call"
    );
    assert_eq!(
        env.mock_llm.call_count(),
        1,
        "no additional LLM call on second invocation — cache must be hit"
    );
}

// ---------------------------------------------------------------------------
// Task 14 — compactor_session_uses_compressed_facts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compactor_session_uses_compressed_facts() {
    let env = TestEnv::build().await;

    // Seed a single d1-style compactor fact for "long-sess".
    // agent_id must match the caller so fetch_session_compressed finds it.
    let raw = RawMemory::new(
        "Rust deployment uses cargo and kubectl in the production path".to_string(),
        RawMemorySource::SessionCompressed,
    )
    .with_agent("agent-1")
    .with_session("long-sess")
    .with_path("aleph://session/long-sess/d1/0");
    env.seed_raw_memory(raw).await;

    let tool = env.build_tool_with_assembler("agent-1");

    let result = tool
        .call(SessionSearchArgs {
            query: "deployment".into(),
            max_results: 5,
        })
        .await
        .expect("call");

    let hit = result
        .hits
        .iter()
        .find(|h| h.session_key == "long-sess")
        .expect("expected a hit for long-sess");

    assert_eq!(
        hit.source,
        SummarySource::Compactor,
        "expected Compactor source, got {:?}",
        hit.source
    );
    assert!(
        hit.summary.contains("deployment") || hit.summary.contains("Rust"),
        "expected summary to mention seeded content, got: {}",
        hit.summary
    );
    assert_eq!(
        env.mock_llm.call_count(),
        0,
        "no LLM call expected for compactor-source hit; got {}",
        env.mock_llm.call_count()
    );
}

// ---------------------------------------------------------------------------
// Task 15 — session_end_hook_produces_summary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_end_hook_produces_summary() {
    let env = TestEnv::build().await;
    env.seed_session(
        "agent-1",
        "ended-sess",
        &[("user", "Status report?"), ("assistant", "All green.")],
    );
    env.mock_llm
        .set_response("<summary>\n## Primary Request\nStatus report\n</summary>");

    // Step 1: directly fire the session-end hook.
    let summarizer = env.session_end_summarizer();
    summarizer
        .produce("agent-1", "ended-sess")
        .await
        .expect("produce");
    assert_eq!(
        env.mock_llm.call_count(),
        1,
        "session_end fired one LLM call"
    );

    // Step 2: now session_search should serve the SessionEnd-source hit, no further LLM call.
    let tool = env.build_tool_with_assembler("agent-1");
    let result = tool
        .call(SessionSearchArgs {
            query: "status".into(),
            max_results: 5,
        })
        .await
        .expect("call");

    let hit = result
        .hits
        .iter()
        .find(|h| h.session_key == "ended-sess")
        .expect("ended-sess hit");
    assert_eq!(
        hit.source,
        SummarySource::SessionEnd,
        "expected SessionEnd source (matched /end-summary path), got {:?}",
        hit.source
    );
    assert_eq!(
        env.mock_llm.call_count(),
        1,
        "no extra LLM call from search"
    );
}

// ---------------------------------------------------------------------------
// Helper for Task 17 — build a GatewayContext that only allows self-access.
// Uses AgentToAgentPolicy::disabled() so agent-A cannot read agent-B's data.
// ---------------------------------------------------------------------------

fn build_restricted_context(env: &TestEnv) -> Arc<GatewayContext> {
    let a2a_policy = Arc::new(AgentToAgentPolicy::disabled());
    Arc::new(GatewayContext::new(
        env.session_store.clone() as Arc<dyn SessionStore>,
        Arc::new(AgentRegistry::new()),
        Arc::new(MockExecutionAdapter) as Arc<dyn ExecutionAdapter>,
        a2a_policy,
    ))
}

// ---------------------------------------------------------------------------
// Task 16 — per_session_dedup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn per_session_dedup() {
    let env = TestEnv::build().await;

    // Seed 5 d0-style compactor facts in the same session, all matching the query.
    for i in 0..5 {
        let raw = RawMemory::new(
            format!("Chunk {i} discusses deployment strategies"),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("agent-1")
        .with_session("big-sess")
        .with_path(format!("aleph://session/big-sess/d0/{i}"));
        env.seed_raw_memory(raw).await;
    }

    let tool = env.build_tool_with_assembler("agent-1");
    let result = tool
        .call(SessionSearchArgs {
            query: "deployment".into(),
            max_results: 10,
        })
        .await
        .expect("call");

    let big_sess_count = result
        .hits
        .iter()
        .filter(|h| h.session_key == "big-sess")
        .count();
    assert_eq!(
        big_sess_count, 1,
        "session must appear at most once after dedup; got {} hits",
        big_sess_count
    );
}

// ---------------------------------------------------------------------------
// Task 17 — a2a_filter_preserved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a2a_filter_preserved() {
    let env = TestEnv::build().await;

    // Seed a SessionCompressed fact owned by agent-B.
    let raw = RawMemory::new(
        "Sensitive content from agent-B about deployment".to_string(),
        RawMemorySource::SessionCompressed,
    )
    .with_agent("agent-B")
    .with_session("agent-b-sess")
    .with_path("aleph://session/agent-b-sess/d0/0");
    env.seed_raw_memory(raw).await;

    // Build a tool whose caller is agent-A with a restrictive (disabled) A2A policy.
    // AgentToAgentPolicy::disabled() allows same-agent access only, so agent-A
    // must NOT be able to see agent-B's data.
    let restricted_ctx = build_restricted_context(&env);
    let tool = SessionSearchTool::new(
        restricted_ctx,
        "agent-A",
        Some(env.assembler.clone()),
        Some(env.synthesizer.clone()),
    );

    let result = tool
        .call(SessionSearchArgs {
            query: "deployment".into(),
            max_results: 5,
        })
        .await
        .expect("call");

    assert!(
        result.hits.iter().all(|h| h.agent_id != "agent-B"),
        "A2A filter must drop agent-B hits when caller is agent-A. Got hits: {:?}",
        result
            .hits
            .iter()
            .map(|h| (&h.agent_id, &h.session_key))
            .collect::<Vec<_>>()
    );
}
