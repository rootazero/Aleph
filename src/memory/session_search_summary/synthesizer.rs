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
//!
//! Steps 1–5 are expensive (one LLM call) and must not run twice for the same
//! session. They therefore run under a per-`(agent_id, session_id)` single
//! flight: the first caller for a key is the *leader* and pays for the LLM
//! call, concurrent callers for the same key are *followers* that wait for the
//! leader's outcome instead of issuing their own LLM call. Without the flight,
//! the Markdown-backed store (whose operations run on `spawn_blocking`) let
//! every concurrent caller pass the fast-path read before the first writer
//! committed, so N concurrent callers meant N LLM calls.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::watch;

use crate::sync_primitives::{Arc, Mutex as StdMutex, MutexGuard};

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
// In-flight single flight
// ---------------------------------------------------------------------------

/// Stable key of the in-flight table: the `(agent_id, session_id)` pair.
///
/// The agent id is part of the key because summary rows are unique per
/// `(agent_id, path)`, while their canonical path omits the agent id.
/// A tuple preserves both ids without assuming any reserved separator.
fn flight_key(agent_id: &str, session_id: &str) -> (String, String) {
    (agent_id.to_owned(), session_id.to_owned())
}

/// Terminal outcome of one synthesis flight, shared with the followers.
#[derive(Clone)]
enum FlightOutcome {
    /// The leader found or wrote the summary row.
    Ready(RawMemory),
    /// The leader failed. Only the message is shared: `AlephError` is not
    /// `Clone`, so the exact variant cannot be handed to the followers.
    Failed(String),
    /// The leader was cancelled (or panicked) before publishing an outcome.
    Aborted,
}

/// One in-flight synthesis.
///
/// The leader owns the work; the followers wait on the `watch` channel for
/// the terminal outcome. The outcome lives in the channel itself, so a
/// follower that subscribes after the publish still observes it.
struct InFlight {
    /// Terminal outcome; `None` until the leader publishes.
    outcome_tx: watch::Sender<Option<FlightOutcome>>,
    /// Ensures the terminal outcome is published exactly once (leader
    /// completion vs. leader drop).
    published: AtomicBool,
}

impl InFlight {
    fn new() -> Self {
        let (outcome_tx, _rx) = watch::channel(None);
        Self {
            outcome_tx,
            published: AtomicBool::new(false),
        }
    }

    /// Publish the terminal outcome. The first publisher wins; later calls
    /// are no-ops.
    fn publish(&self, outcome: FlightOutcome) {
        if self.published.swap(true, Ordering::SeqCst) {
            return;
        }
        // `send_replace` stores the value even when no follower is subscribed
        // yet, so a follower that subscribes later still sees the outcome.
        self.outcome_tx.send_replace(Some(outcome));
    }

    /// Wait for the leader's outcome. `None` means the leader vanished
    /// without publishing — the leader's drop guard prevents that, it
    /// publishes `Aborted` first.
    async fn wait(&self) -> Option<FlightOutcome> {
        let mut rx = self.outcome_tx.subscribe();
        // The borrow is dropped before the await below: a `watch::Ref` must
        // not be held across an await point.
        let current: Option<FlightOutcome> = rx.borrow().clone();
        if current.is_some() {
            return current;
        }
        match rx.changed().await {
            Ok(()) => {
                let current: Option<FlightOutcome> = rx.borrow().clone();
                current
            }
            Err(_) => None,
        }
    }
}

/// In-flight synthesis table: flight key -> flight.
type InFlightTable = HashMap<(String, String), Arc<InFlight>>;

/// Lock the in-flight table. A poisoned mutex is recovered from rather than
/// propagated: a panic in one flight must not turn every later lookup into a
/// panic.
fn lock_inflight(table: &StdMutex<InFlightTable>) -> MutexGuard<'_, InFlightTable> {
    table
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Removes the flight from the in-flight table and guarantees that the
/// followers are woken — even when the leader task is cancelled mid-flight.
struct LeaderGuard {
    key: (String, String),
    flight: Arc<InFlight>,
    table: Arc<StdMutex<InFlightTable>>,
}

impl Drop for LeaderGuard {
    fn drop(&mut self) {
        // A cancelled leader never published: wake the followers instead of
        // leaving them waiting forever. `publish` is a no-op when the leader
        // already published a real outcome.
        self.flight.publish(FlightOutcome::Aborted);

        let mut table = lock_inflight(&self.table);
        // Only drop our own entry — a later flight for the same key may have
        // replaced it in the meantime.
        if table
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.flight))
        {
            table.remove(&self.key);
        }
    }
}

// ---------------------------------------------------------------------------
// SummarySynthesizer
// ---------------------------------------------------------------------------

/// Generates `/end-summary` rows lazily when none exists yet.
pub struct SummarySynthesizer {
    store: Arc<dyn RawMemoryStore>,
    session_store: Arc<dyn SessionStore>,
    llm: Arc<dyn SummaryLlm>,
    /// In-flight synthesis flights, keyed by `flight_key(agent_id, session_id)`.
    ///
    /// The lock is only ever held for a `HashMap` lookup or insert — never
    /// across an `await`.
    inflight: Arc<StdMutex<InFlightTable>>,
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
            inflight: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    /// Return the canonical `/end-summary` `RawMemory` for `(agent_id,
    /// session_id)`, synthesizing it from the live transcript if necessary.
    ///
    /// The function is safe to call concurrently: the per-`(agent_id,
    /// session_id)` single flight makes exactly one caller perform the
    /// synthesis (one LLM call), and the followers share its outcome.
    pub async fn lazy_for(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<RawMemory, AlephError> {
        let key = flight_key(agent_id, session_id);

        // Join the flight for this `(agent_id, session_id)` or start one. The
        // lock is held for a map lookup/insert only — no `await` under it.
        let (flight, is_leader) = {
            let mut table = lock_inflight(&self.inflight);
            match table.get(&key) {
                Some(current) => (Arc::clone(current), false),
                None => {
                    let flight = Arc::new(InFlight::new());
                    table.insert(key.clone(), Arc::clone(&flight));
                    (flight, true)
                }
            }
        };

        if !is_leader {
            // Follower: share the leader's outcome. No store read, no LLM call.
            return match flight.wait().await {
                Some(FlightOutcome::Ready(raw)) => Ok(raw),
                Some(FlightOutcome::Failed(message)) => Err(AlephError::other(format!(
                    "end-summary synthesis failed for session {session_id}: {message}"
                ))),
                Some(FlightOutcome::Aborted) => Err(AlephError::other(format!(
                    "end-summary synthesis was aborted for session {session_id}"
                ))),
                None => Err(AlephError::other(format!(
                    "end-summary synthesis for session {session_id} ended without a result"
                ))),
            };
        }

        // Leader: the guard wakes the followers and removes the in-flight
        // entry even if this task is cancelled before the outcome is published.
        let _guard = LeaderGuard {
            key,
            flight: Arc::clone(&flight),
            table: Arc::clone(&self.inflight),
        };

        match self.synthesize(agent_id, session_id).await {
            Ok(raw) => {
                flight.publish(FlightOutcome::Ready(raw.clone()));
                Ok(raw)
            }
            Err(err) => {
                // The followers cannot receive the error value itself
                // (`AlephError` is not `Clone`), only its message.
                flight.publish(FlightOutcome::Failed(err.to_string()));
                Err(err)
            }
        }
    }

    /// Produce the canonical `/end-summary` row for `(agent_id, session_id)`.
    ///
    /// Only the flight leader runs this — it is the only path that reads the
    /// store or calls the LLM.
    async fn synthesize(&self, agent_id: &str, session_id: &str) -> Result<RawMemory, AlephError> {
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
        fn transcript_file(&self, _key: &SessionKey) -> Option<std::path::PathBuf> {
            None
        }
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
        async fn stamp_and_bill_in_range(
            &self,
            _key: &SessionKey,
            _after_seq: u64,
            _before_seq: u64,
            _metadata: &serde_json::Value,
            _bill: Option<&crate::gateway::session_store::RunBill>,
        ) -> Result<crate::gateway::session_store::StampOutcome, SessionStoreError> {
            // No row here carries a source seq, so no row is in any range —
            // with or without a bill: nothing is stamped, so nothing is billed.
            Ok(crate::gateway::session_store::StampOutcome::NoRowInRange)
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
    // Helper: Markdown-backed raw memory store with an owned scratch directory
    // -----------------------------------------------------------------------

    pub(crate) async fn make_store() -> (
        tempfile::TempDir,
        Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) {
        use crate::memory::store::markdown::MarkdownSessionStore;
        let dir = tempfile::tempdir().expect("raw memory scratch directory");
        let store = Arc::new(
            MarkdownSessionStore::new(dir.path())
                .await
                .expect("Markdown raw store"),
        );
        (dir, store)
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

    #[test]
    fn flight_keys_preserve_both_ids() {
        assert_ne!(flight_key("a\u{1f}b", "c"), flight_key("a", "b\u{1f}c"));
        assert_ne!(
            flight_key("agent-a", "session"),
            flight_key("agent-b", "session")
        );
    }

    struct GatedLlm {
        calls: std::sync::atomic::AtomicUsize,
        started: tokio::sync::Notify,
        release: tokio::sync::Semaphore,
    }

    #[async_trait]
    impl SummaryLlm for GatedLlm {
        async fn complete(&self, _prompt: &str) -> Result<String, AlephError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.release.acquire().await.unwrap().forget();
            Ok("Retried summary".to_owned())
        }
    }

    #[tokio::test]
    async fn cancelling_the_leader_wakes_followers_and_allows_retry() {
        let (_dir, store) = make_store().await;
        let sessions = InMemorySessionStore::new().with_messages(
            "agent",
            "session",
            &[("user", "Summarize this")],
        );
        let llm = Arc::new(GatedLlm {
            calls: std::sync::atomic::AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        let synthesizer = Arc::new(SummarySynthesizer::new(store, sessions, llm.clone()));
        let leader = tokio::spawn({
            let synthesizer = synthesizer.clone();
            async move { synthesizer.lazy_for("agent", "session").await }
        });
        let timeout = std::time::Duration::from_secs(10);
        tokio::time::timeout(timeout, llm.started.notified())
            .await
            .unwrap();
        let follower = tokio::spawn({
            let synthesizer = synthesizer.clone();
            async move { synthesizer.lazy_for("agent", "session").await }
        });
        tokio::time::timeout(timeout, async {
            loop {
                let joined = lock_inflight(&synthesizer.inflight)
                    .get(&flight_key("agent", "session"))
                    .is_some_and(|flight| flight.outcome_tx.receiver_count() > 0);
                if joined {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        leader.abort();
        assert!(leader.await.unwrap_err().is_cancelled());
        let error = tokio::time::timeout(timeout, follower)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("aborted"));
        assert!(lock_inflight(&synthesizer.inflight).is_empty());

        llm.release.add_permits(1);
        let retried = tokio::time::timeout(timeout, synthesizer.lazy_for("agent", "session"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retried.content, "Retried summary");
        assert_eq!(llm.calls.load(Ordering::SeqCst), 2);
        assert!(lock_inflight(&synthesizer.inflight).is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 1: returns existing fact when summary already present
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn returns_existing_fact_when_already_present() {
        let (_raw_dir, store) = make_store().await;
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
        let (_raw_dir, store) = make_store().await;
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
        let (_raw_dir, store) = make_store().await;
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
        let (_raw_dir, store) = make_store().await;
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

        assert_eq!(
            llm.call_count(),
            1,
            "concurrent callers must share one LLM call"
        );
        assert!(lock_inflight(&synthesizer.inflight).is_empty());
    }
}
