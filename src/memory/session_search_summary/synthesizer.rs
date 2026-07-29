//! Spec B — Lazy on-read summary synthesis.
//!
//! When `session_search` is invoked for a session that has no `/end-summary`
//! row yet (no compactor pass, no on-session-end firing), `SummarySynthesizer`
//! generates one on demand:
//!
//! 1. Idempotent fast-path: re-check for an existing row (may have been
//!    written between our last check and this call).
//! 2. Load a windowed transcript slice (capped at `LAZY_INPUT_MAX_TURNS` /
//!    `LAZY_INPUT_MAX_CHARS`).
//! 3. Build prompt via `build_summary_prompt` and call the LLM once.
//! 4. Strip the analysis block and write via `INSERT OR IGNORE` (first writer
//!    wins in a concurrent race).
//! 5. Re-read to return the winning row.

use crate::sync_primitives::Arc;

use async_trait::async_trait;

use crate::error::AlephError;
use crate::gateway::session_store::SessionStore;
use crate::memory::session_compactor::fallback::FallbackLevel;
use crate::memory::session_compactor::summary_engine::{
    build_summary_prompt, strip_analysis_block,
};
use crate::memory::session_search_summary::lookup::retrieve_summary_fact;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

/// Maximum number of turns loaded for lazy synthesis.
/// ~8000 tokens at approximately 4 chars/token ≈ 32 000 chars.
pub const LAZY_INPUT_MAX_TURNS: usize = 50;

/// Maximum total content characters loaded for lazy synthesis.
/// Approximates 8 000 LLM tokens at 4 chars/token.
pub const LAZY_INPUT_MAX_CHARS: usize = 32_000;

// ---------------------------------------------------------------------------
// LLM provider abstraction (thin — reuses the same trait as HybridAssembler)
// ---------------------------------------------------------------------------

/// Minimal LLM interface required by the synthesizer.
///
/// Mirrors `crate::memory::assembler::hybrid::LlmReranker` so that the same
/// `AiProviderReranker` wrapper can be injected without any extra indirection.
#[async_trait]
pub trait SummaryLlm: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String, AlephError>;
}

// ---------------------------------------------------------------------------
// SummarySynthesizer
// ---------------------------------------------------------------------------

/// Generates `/end-summary` rows lazily when none exists yet.
pub struct SummarySynthesizer {
    store: Arc<dyn RawMemoryStore>,
    session_store: Arc<dyn SessionStore>,
    llm: Arc<dyn SummaryLlm>,
}

impl SummarySynthesizer {
    /// Construct a new synthesizer.
    pub fn new(
        store: Arc<dyn RawMemoryStore>,
        session_store: Arc<dyn SessionStore>,
        llm: Arc<dyn SummaryLlm>,
    ) -> Self {
        Self {
            store,
            session_store,
            llm,
        }
    }

    /// Return the canonical `/end-summary` `RawMemory` for `(agent_id,
    /// session_id)`, synthesizing it from the live transcript if necessary.
    ///
    /// The function is safe to call concurrently: `INSERT OR IGNORE` ensures
    /// exactly one row survives, and the final re-read returns the winner.
    pub async fn lazy_for(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<RawMemory, AlephError> {
        // 1. Idempotent fast-path — a concurrent caller may have already written
        //    the summary since our last check.
        if let Some(existing) = retrieve_summary_fact(&self.store, agent_id, session_id).await? {
            return Ok(existing);
        }

        // 2. Load transcript window.
        let transcript = self
            .session_store
            .load_window(
                agent_id,
                session_id,
                LAZY_INPUT_MAX_TURNS,
                LAZY_INPUT_MAX_CHARS,
            )
            .await
            .map_err(|e| AlephError::other(format!("load_window failed: {e}")))?;

        if transcript.is_empty() {
            return Err(AlephError::other(format!(
                "no transcript for session {session_id}"
            )));
        }

        // 3. Build prompt and call LLM once.
        let prompt = build_summary_prompt(&transcript, 0, None, FallbackLevel::Normal);
        let llm_output = self.llm.complete(&prompt).await?;

        // 4. Strip the analysis block and build the RawMemory.
        let summary_text = strip_analysis_block(&llm_output);
        let raw = RawMemory::new(summary_text, RawMemorySource::SessionCompressed)
            .with_agent(agent_id)
            .with_session(session_id)
            .with_path(format!("aleph://session/{session_id}/end-summary"));

        // 5. Write — first writer wins.
        self.store.insert_raw_memory_or_ignore(&raw).await?;

        // 6. Re-read to return the winning row (handles the concurrent-write race).
        retrieve_summary_fact(&self.store, agent_id, session_id)
            .await?
            .ok_or_else(|| {
                AlephError::other(format!(
                    "end-summary missing after write for session {session_id}"
                ))
            })
    }
}

// ---------------------------------------------------------------------------
// Test support — pub(crate) stubs reused by integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod test_support {
    use crate::sync_primitives::Mutex as StdMutex;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use crate::error::AlephError;
    use crate::gateway::router::SessionKey;
    use crate::gateway::session_manager::SessionState;
    use crate::gateway::session_store::error::SessionStoreError;
    use crate::gateway::session_store::types::*;
    use crate::gateway::session_store::SessionStore;
    use crate::sync_primitives::Arc;

    use super::SummaryLlm;

    // -----------------------------------------------------------------------
    // MockSummaryLlm
    // -----------------------------------------------------------------------

    pub(crate) struct MockSummaryLlm {
        pub(crate) response: String,
        pub(crate) call_count: AtomicUsize,
    }

    impl MockSummaryLlm {
        pub(crate) fn with_response(s: impl Into<String>) -> Arc<Self> {
            Arc::new(Self {
                response: s.into(),
                call_count: AtomicUsize::new(0),
            })
        }

        pub(crate) fn call_count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SummaryLlm for MockSummaryLlm {
        async fn complete(&self, _prompt: &str) -> Result<String, AlephError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
        }
    }

    // -----------------------------------------------------------------------
    // InMemorySessionStore
    // -----------------------------------------------------------------------

    /// Minimal in-memory SessionStore for tests.
    pub(crate) struct InMemorySessionStore {
        // (agent_id, session_id) -> Vec<(role, content)>
        pub(crate) messages: StdMutex<HashMap<(String, String), Vec<(String, String)>>>,
    }

    impl InMemorySessionStore {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                messages: StdMutex::new(HashMap::new()),
            })
        }

        pub(crate) fn with_messages(
            self: Arc<Self>,
            agent_id: &str,
            session_id: &str,
            msgs: &[(&str, &str)],
        ) -> Arc<Self> {
            let mut map = self.messages.lock().unwrap();
            map.insert(
                (agent_id.to_string(), session_id.to_string()),
                msgs.iter()
                    .map(|(r, c)| (r.to_string(), c.to_string()))
                    .collect(),
            );
            drop(map);
            self
        }
    }

    // We only need to implement load_window; all other methods are stubs that
    // return errors — they should never be called in these tests.
    #[async_trait]
    impl SessionStore for InMemorySessionStore {
        async fn get_or_create(
            &self,
            _key: &SessionKey,
        ) -> Result<SessionMetadata, SessionStoreError> {
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
        async fn delete_session(
            &self,
            _key: &SessionKey,
        ) -> Result<DeleteResult, SessionStoreError> {
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
        async fn search_messages(
            &self,
            _query: &str,
            _max_results: usize,
        ) -> Result<Vec<SearchHit>, SessionStoreError> {
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
        async fn set_topic(
            &self,
            _key: &SessionKey,
            _topic: &str,
        ) -> Result<(), SessionStoreError> {
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
        async fn get_current_epoch(
            &self,
            _base_key_pattern: &str,
        ) -> Result<u32, SessionStoreError> {
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

        // Override load_window: look up by (agent_id, session_id) directly.
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

            // Take most-recent max_turns, then cap by chars (chronological order).
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
    }

    // -----------------------------------------------------------------------
    // Helper: in-memory raw memory store (pub(crate) for integration tests)
    // -----------------------------------------------------------------------

    pub(crate) fn make_store() -> Arc<dyn crate::memory::store::raw_memory::RawMemoryStore> {
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        Arc::new(SqliteMemoryBackend::in_memory().expect("in-memory backend"))
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::test_support::{make_store, InMemorySessionStore, MockSummaryLlm};
    use super::*;

    use crate::gateway::session_store::SessionStore;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};

    // -----------------------------------------------------------------------
    // Test 1: returns existing fact when summary already present
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn returns_existing_fact_when_already_present() {
        let store = make_store();
        let session_store = InMemorySessionStore::new();
        let llm = MockSummaryLlm::with_response("LLM should not be called");

        // Pre-seed an end-summary row.
        let raw = RawMemory::new(
            "Pre-existing summary".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("agent-1")
        .with_session("sess-existing")
        .with_path("aleph://session/sess-existing/end-summary");
        store.insert_raw_memory(&raw).await.unwrap();

        let synthesizer = SummarySynthesizer::new(store, session_store, llm.clone());
        let result = synthesizer
            .lazy_for("agent-1", "sess-existing")
            .await
            .unwrap();

        assert_eq!(result.content, "Pre-existing summary");
        assert_eq!(
            llm.call_count(),
            0,
            "LLM must not be called when summary exists"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: synthesizes when no existing summary
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn synthesizes_when_no_existing_summary() {
        let store = make_store();
        let session_store = InMemorySessionStore::new().with_messages(
            "agent-2",
            "sess-new",
            &[("user", "Hello world"), ("assistant", "Hi there!")],
        );
        let llm = MockSummaryLlm::with_response("Synthesized summary text");

        let synthesizer = SummarySynthesizer::new(store.clone(), session_store, llm.clone());

        // First call — should invoke LLM once.
        let result = synthesizer.lazy_for("agent-2", "sess-new").await.unwrap();
        assert_eq!(result.content, "Synthesized summary text");
        assert_eq!(llm.call_count(), 1, "LLM must be called exactly once");

        // Second call — idempotent fast-path, LLM count stays at 1.
        let result2 = synthesizer.lazy_for("agent-2", "sess-new").await.unwrap();
        assert_eq!(result2.content, "Synthesized summary text");
        assert_eq!(
            llm.call_count(),
            1,
            "second call must short-circuit via fast-path"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: returns error when transcript empty
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn returns_error_when_transcript_empty() {
        let store = make_store();
        let session_store = InMemorySessionStore::new(); // no messages seeded
        let llm = MockSummaryLlm::with_response("never");

        let synthesizer = SummarySynthesizer::new(store, session_store, llm);
        let err = synthesizer
            .lazy_for("agent-3", "sess-empty")
            .await
            .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("no transcript"),
            "expected 'no transcript' in error, got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: concurrent calls produce one fact
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn concurrent_calls_produce_one_fact() {
        let store = make_store();
        let session_store = InMemorySessionStore::new().with_messages(
            "agent-4",
            "sess-concurrent",
            &[
                ("user", "message one"),
                ("assistant", "reply one"),
                ("user", "message two"),
            ],
        );
        let llm = MockSummaryLlm::with_response("Concurrent summary");
        let synthesizer = Arc::new(SummarySynthesizer::new(
            store.clone(),
            session_store as Arc<dyn SessionStore>,
            llm.clone(),
        ));

        let mut handles = Vec::new();
        for _ in 0..10 {
            let syn = synthesizer.clone();
            handles.push(tokio::spawn(async move {
                syn.lazy_for("agent-4", "sess-concurrent").await
            }));
        }

        let mut contents = Vec::new();
        for h in handles {
            let r = h.await.expect("task panicked").expect("lazy_for failed");
            contents.push(r.content);
        }

        // All callers must get the same content.
        assert!(
            contents.iter().all(|c| c == "Concurrent summary"),
            "all callers must see the same summary"
        );

        // SQL-level uniqueness: with the UNIQUE(agent_id, path) partial index,
        // exactly ONE row at the canonical /end-summary path must exist after
        // 10 concurrent writers contended. (Without the index, INSERT OR
        // IGNORE is inert and the table would hold up to 10 rows.)
        let path = "aleph://session/sess-concurrent/end-summary";
        let rows = store
            .get_raw_by_path_prefix(path, "agent-4", 100)
            .await
            .unwrap();
        let exact: Vec<_> = rows
            .into_iter()
            .filter(|r| r.path.as_deref() == Some(path))
            .collect();
        assert_eq!(
            exact.len(),
            1,
            "expected exactly one /end-summary row, found {}",
            exact.len()
        );

        // With the UNIQUE(agent_id, path) partial index in place, the race
        // window is genuinely small: only callers that pass the read fast-path
        // before any writer commits will issue an LLM call. In a single-thread
        // tokio runtime serving a make_store() in-memory backend, the
        // expected count is 1 (≤ 2 allows a tiny scheduling jitter cushion).
        assert!(
            llm.call_count() <= 2,
            "too many LLM calls: {}",
            llm.call_count()
        );
    }
}
