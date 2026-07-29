//! Spec B — `on_session_end` hook handler. Produces (or skips) a
//! `/end-summary` `RawMemory` when a session truly ends.
//!
//! Behaviour priority:
//!   1. Short-circuit if /end-summary already exists.
//!   2. Reuse highest-depth <aleph://session/{sid}/d{depth}/{seq>} fact (zero-LLM).
//!   3. Fall back to `SummarySynthesizer::lazy_for` (one LLM call).

use crate::sync_primitives::Arc;

use crate::error::AlephError;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

use super::synthesizer::SummarySynthesizer;

#[derive(Clone)]
pub struct SessionEndSummarizer {
    pub store: Arc<dyn RawMemoryStore>,
    pub synthesizer: Arc<SummarySynthesizer>,
}

impl SessionEndSummarizer {
    /// Idempotent. Returns Ok(()) on success even if nothing was written
    /// (i.e. the /end-summary already existed).
    pub async fn produce(&self, agent_id: &str, session_id: &str) -> Result<(), AlephError> {
        // Step 0 — short-circuit.
        if super::lookup::retrieve_summary_fact(&self.store, agent_id, session_id)
            .await?
            .is_some()
        {
            return Ok(());
        }

        // Step 1 — reuse highest-depth compactor fact, if any.
        if let Some(reused) = self.reuse_highest_depth_fact(agent_id, session_id).await? {
            self.store.insert_raw_memory_or_ignore(&reused).await?;
            return Ok(());
        }

        // Step 2 — fall back to lazy synthesis.
        let _ = self.synthesizer.lazy_for(agent_id, session_id).await?;
        Ok(())
    }

    /// Returns a freshly-constructed `RawMemory` at /end-summary path, with
    /// content reused from the highest-depth d{depth}/{seq} row in this
    /// session. Returns Ok(None) if no d* rows exist.
    async fn reuse_highest_depth_fact(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<Option<RawMemory>, AlephError> {
        let prefix = format!("aleph://session/{session_id}/d");
        let candidates = self
            .store
            .get_raw_by_path_prefix(&prefix, agent_id, 64)
            .await?;

        let best = candidates
            .into_iter()
            .filter(|r| matches!(r.source, RawMemorySource::SessionCompressed))
            .max_by_key(|r| extract_depth_from_path(r.path.as_deref().unwrap_or("")));

        Ok(best.map(|src| {
            RawMemory::new(src.content.clone(), RawMemorySource::SessionCompressed)
                .with_agent(agent_id)
                .with_session(session_id)
                .with_path(format!("aleph://session/{session_id}/end-summary"))
        }))
    }
}

/// Parse depth from a `aleph://session/{sid}/d{depth}/{seq}` path.
/// Returns 0 for paths that don't match.
///
/// Note: `session_compactor::summary_source::extract_depth` exists but is
/// private (`fn`, not `pub fn`), so we inline an equivalent helper here.
fn extract_depth_from_path(path: &str) -> u32 {
    // Split on "/d" — the last occurrence is the depth segment.
    // Path shape: aleph://session/<sid>/d<depth>/<seq>
    // Use the LAST "/d" so a session id containing "/d" can never shadow the
    // real depth segment. "/d" is ASCII, so `idx + 2` is a char boundary.
    path.rfind("/d")
        .and_then(|idx| path[idx + 2..].split_once('/'))
        .and_then(|(d, _)| d.parse::<u32>().ok())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sync_primitives::Mutex as StdMutex;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use crate::gateway::router::SessionKey;
    use crate::gateway::session_manager::SessionState;
    use crate::gateway::session_store::error::SessionStoreError;
    use crate::gateway::session_store::types::*;
    use crate::gateway::session_store::SessionStore;
    use crate::memory::session_search_summary::lookup::retrieve_summary_fact;
    use crate::memory::session_search_summary::synthesizer::SummaryLlm;
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    // -----------------------------------------------------------------------
    // MockSummaryLlm — counts calls, returns a fixed response
    // -----------------------------------------------------------------------

    struct MockSummaryLlm {
        response: String,
        call_count: AtomicUsize,
    }

    impl MockSummaryLlm {
        fn with_response(s: impl Into<String>) -> Arc<Self> {
            Arc::new(Self {
                response: s.into(),
                call_count: AtomicUsize::new(0),
            })
        }

        fn call_count(&self) -> usize {
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
    // InMemorySessionStore — minimal stub; only load_window is meaningful
    // -----------------------------------------------------------------------

    struct InMemorySessionStore {
        messages: StdMutex<HashMap<(String, String), Vec<(String, String)>>>,
    }

    impl InMemorySessionStore {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                messages: StdMutex::new(HashMap::new()),
            })
        }

        fn with_messages(
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
    }

    // -----------------------------------------------------------------------
    // Helper
    // -----------------------------------------------------------------------

    fn make_store() -> Arc<dyn RawMemoryStore> {
        Arc::new(SqliteMemoryBackend::in_memory().expect("in-memory backend"))
    }

    fn make_summarizer(
        store: Arc<dyn RawMemoryStore>,
        session_store: Arc<dyn SessionStore>,
        llm: Arc<dyn SummaryLlm>,
    ) -> SessionEndSummarizer {
        let synthesizer = Arc::new(SummarySynthesizer::new(store.clone(), session_store, llm));
        SessionEndSummarizer { store, synthesizer }
    }

    // -----------------------------------------------------------------------
    // Test 1: short_circuits_when_summary_exists
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn short_circuits_when_summary_exists() {
        let store = make_store();
        let session_store = InMemorySessionStore::new();
        let llm = MockSummaryLlm::with_response("LLM should not be called");

        // Pre-seed an /end-summary row.
        let raw = RawMemory::new(
            "Existing summary".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("agent-1")
        .with_session("sk1")
        .with_path("aleph://session/sk1/end-summary");
        store.insert_raw_memory(&raw).await.unwrap();

        let summarizer = make_summarizer(store.clone(), session_store, llm.clone());
        summarizer.produce("agent-1", "sk1").await.unwrap();

        // LLM must not be called.
        assert_eq!(
            llm.call_count(),
            0,
            "LLM must not be called when /end-summary already exists"
        );

        // Existing row must be unchanged.
        let existing = retrieve_summary_fact(&store, "agent-1", "sk1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(existing.content, "Existing summary");
    }

    // -----------------------------------------------------------------------
    // Test 2: reuses_highest_depth_fact_without_llm
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn reuses_highest_depth_fact_without_llm() {
        let store = make_store();
        let session_store = InMemorySessionStore::new();
        let llm = MockSummaryLlm::with_response("LLM should not be called");

        // Seed d0 and d2 rows.
        let d0 = RawMemory::new("d0 detail".to_string(), RawMemorySource::SessionCompressed)
            .with_agent("agent-1")
            .with_session("sk2")
            .with_path("aleph://session/sk2/d0/0");
        store.insert_raw_memory(&d0).await.unwrap();

        let d2 = RawMemory::new(
            "d2 abstract".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("agent-1")
        .with_session("sk2")
        .with_path("aleph://session/sk2/d2/0");
        store.insert_raw_memory(&d2).await.unwrap();

        let summarizer = make_summarizer(store.clone(), session_store, llm.clone());
        summarizer.produce("agent-1", "sk2").await.unwrap();

        // Written row's content must be from d2 (highest depth wins).
        let written = retrieve_summary_fact(&store, "agent-1", "sk2")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            written.content, "d2 abstract",
            "highest-depth fact must be reused"
        );
        assert_eq!(
            written.path.as_deref(),
            Some("aleph://session/sk2/end-summary")
        );

        // LLM must not be called.
        assert_eq!(
            llm.call_count(),
            0,
            "LLM must not be called when d* facts exist"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: falls_back_to_synthesizer_when_no_d_facts
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn falls_back_to_synthesizer_when_no_d_facts() {
        let store = make_store();
        let session_store = InMemorySessionStore::new().with_messages(
            "agent-1",
            "sk3",
            &[("user", "Hello"), ("assistant", "Hi there")],
        );
        let llm = MockSummaryLlm::with_response("Fallback summary text");

        let summarizer = make_summarizer(store.clone(), session_store, llm.clone());
        summarizer.produce("agent-1", "sk3").await.unwrap();

        // /end-summary must be written with the LLM output.
        let written = retrieve_summary_fact(&store, "agent-1", "sk3")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(written.content, "Fallback summary text");

        // LLM must be called exactly once.
        assert_eq!(
            llm.call_count(),
            1,
            "LLM must be called exactly once for fallback"
        );
    }

    // -----------------------------------------------------------------------
    // Unit tests for extract_depth_from_path
    // -----------------------------------------------------------------------

    #[test]
    fn extract_depth_parses_d0() {
        assert_eq!(extract_depth_from_path("aleph://session/sid/d0/0"), 0);
    }

    #[test]
    fn extract_depth_parses_d2() {
        assert_eq!(extract_depth_from_path("aleph://session/sid/d2/0"), 2);
    }

    #[test]
    fn extract_depth_returns_0_for_end_summary() {
        assert_eq!(
            extract_depth_from_path("aleph://session/sid/end-summary"),
            0
        );
    }
}
