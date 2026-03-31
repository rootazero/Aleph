//! Compactor probe test harness.
//!
//! Wraps `SessionCompactor`, `MemoryBackend` (LanceDB in a temp dir),
//! and `MockLlmProvider` with helpers for creating messages, querying
//! session-local facts, and running compression.

#![allow(dead_code)]

use std::sync::Arc;

use tempfile::TempDir;

use alephcore::memory::context::{MemoryFact, MemoryScope};
use alephcore::memory::session_compactor::{SessionCompactor, SessionCompactorConfig};
use alephcore::memory::store::lance::LanceMemoryBackend;
use alephcore::memory::store::types::SearchFilter;
use alephcore::memory::store::{MemoryBackend, MemoryStore};
use alephcore::providers::message::UnifiedMessage;

use super::mock_llm::MockLlmProvider;

// =============================================================================
// CompactorProbeHarness
// =============================================================================

/// Test harness for session compactor in-process probe tests.
///
/// Provides a `SessionCompactor` wired to a fresh LanceDB instance and a
/// `MockLlmProvider`, plus helpers for generating messages and querying facts.
pub struct CompactorProbeHarness {
    pub compactor: SessionCompactor,
    pub database: MemoryBackend,
    pub mock_llm: Arc<MockLlmProvider>,
    pub _temp_dir: TempDir,
}

impl CompactorProbeHarness {
    /// Create a harness with the default (working) MockLlmProvider.
    pub async fn new() -> Self {
        Self::build(MockLlmProvider::new()).await
    }

    /// Create a harness with a custom config.
    pub async fn with_config(config: SessionCompactorConfig) -> Self {
        let mock_llm = Arc::new(MockLlmProvider::new());
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let lance = Arc::new(
            LanceMemoryBackend::open_or_create(temp_dir.path())
                .await
                .expect("Failed to create LanceDB backend"),
        );

        let compactor =
            SessionCompactor::new(lance.clone(), config).with_provider(mock_llm.clone());

        Self {
            compactor,
            database: lance,
            mock_llm,
            _temp_dir: temp_dir,
        }
    }

    /// Create a harness with a MockLlmProvider configured to always fail.
    pub async fn with_failing_llm() -> Self {
        Self::build(MockLlmProvider::failing()).await
    }

    /// Internal builder: create temp dir, LanceDB, compactor.
    async fn build(mock: MockLlmProvider) -> Self {
        let mock_llm = Arc::new(mock);
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let lance = Arc::new(
            LanceMemoryBackend::open_or_create(temp_dir.path())
                .await
                .expect("Failed to create LanceDB backend"),
        );

        let config = SessionCompactorConfig {
            enabled: true,
            // Use small values for fast tests
            fresh_tail_count: 4,
            context_threshold: 0.75,
            leaf_chunk_tokens: 200,
            d1_min_fanout: 4,
            d2_min_fanout: 3,
            max_summary_depth: 2,
            token_estimate_ratio: 3.5,
            session_fact_retention_hours: 24,
            promote_confidence_threshold: 0.8,
        };

        let compactor =
            SessionCompactor::new(lance.clone(), config).with_provider(mock_llm.clone());

        Self {
            compactor,
            database: lance,
            mock_llm,
            _temp_dir: temp_dir,
        }
    }

    // -------------------------------------------------------------------------
    // Message generation helpers
    // -------------------------------------------------------------------------

    /// Generate `count` user/assistant conversation turn pairs on a given topic.
    ///
    /// Returns `(role, content)` pairs suitable for chunking and compression.
    pub fn make_messages(count: usize, topic: &str) -> Vec<(String, String)> {
        let mut pairs = Vec::with_capacity(count * 2);
        for i in 0..count {
            pairs.push((
                "user".to_string(),
                format!(
                    "Turn {}: Tell me about {} — question part {}",
                    i + 1,
                    topic,
                    i + 1
                ),
            ));
            pairs.push((
                "assistant".to_string(),
                format!(
                    "Turn {}: Here is detailed information about {} for question {}. \
                     The implementation involves several components including module A, \
                     module B, and module C. Each component handles a specific aspect \
                     of the {} subsystem.",
                    i + 1,
                    topic,
                    i + 1,
                    topic
                ),
            ));
        }
        pairs
    }

    /// Generate `count` messages that include large tool results.
    ///
    /// Produces `UnifiedMessage` instances with tool_result blocks containing
    /// substantial output text, simulating real tool calls.
    pub fn make_tool_messages(count: usize) -> Vec<UnifiedMessage> {
        let mut messages = Vec::with_capacity(count * 3);
        for i in 0..count {
            // User asks for something
            messages.push(UnifiedMessage::user(format!(
                "Please run diagnostic tool #{}",
                i + 1
            )));
            // Tool result with large output
            let tool_output = format!(
                "Tool #{} output:\n\
                 ========================================\n\
                 Processing item alpha... OK (234ms)\n\
                 Processing item beta... OK (187ms)\n\
                 Processing item gamma... OK (312ms)\n\
                 Processing item delta... WARN: slow response (1204ms)\n\
                 \n\
                 Summary: 4 items processed, 1 warning.\n\
                 Detailed metrics: CPU 45%, MEM 2.1GB, DISK 120MB/s\n\
                 Log entries: 847 lines analyzed, 3 anomalies detected.\n\
                 ========================================",
                i + 1
            );
            messages.push(UnifiedMessage::tool_result(
                format!("call_{}", i + 1),
                format!("diagnostic_{}", i + 1),
                tool_output,
                false,
            ));
            // Assistant summarizes the tool result
            messages.push(UnifiedMessage::assistant(format!(
                "Tool #{} completed successfully with 1 warning about slow response on delta item.",
                i + 1
            )));
        }
        messages
    }

    // -------------------------------------------------------------------------
    // Fact query helpers
    // -------------------------------------------------------------------------

    /// Query LanceDB for valid (`is_valid=true`) SessionLocal facts matching
    /// the given session key.
    pub async fn query_session_facts(&self, session_key: &str) -> Vec<MemoryFact> {
        let path_prefix = format!("aleph://session/{}/", session_key);
        let filter = SearchFilter::new()
            .with_valid_only()
            .with_scope(MemoryScope::SessionLocal)
            .with_path_prefix(&path_prefix);

        self.database
            .get_facts_by_path_prefix(&path_prefix, &filter, 500)
            .await
            .unwrap_or_default()
    }

    /// Query LanceDB for ALL SessionLocal facts (including invalidated ones)
    /// matching the given session key.
    pub async fn query_all_session_facts(&self, session_key: &str) -> Vec<MemoryFact> {
        let path_prefix = format!("aleph://session/{}/", session_key);
        let filter = SearchFilter::new()
            .with_scope(MemoryScope::SessionLocal)
            .with_path_prefix(&path_prefix);

        self.database
            .get_facts_by_path_prefix(&path_prefix, &filter, 500)
            .await
            .unwrap_or_default()
    }

    /// Count valid facts at a specific depth for a session.
    pub async fn count_facts_at_depth(&self, session_key: &str, depth: u32) -> usize {
        let path_prefix = format!("aleph://session/{}/d{}/", session_key, depth);
        let filter = SearchFilter::new()
            .with_valid_only()
            .with_scope(MemoryScope::SessionLocal)
            .with_path_prefix(&path_prefix);

        self.database.count_facts(&filter).await.unwrap_or(0)
    }

    /// Insert a pre-built MemoryFact directly into the database.
    ///
    /// Useful for seeding test state before running compactor operations.
    pub async fn insert_fact(&self, fact: &MemoryFact) {
        self.database
            .insert_fact(fact)
            .await
            .expect("Failed to insert fact");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn harness_creates_successfully() {
        let h = CompactorProbeHarness::new().await;
        assert!(h.compactor.config().enabled);
        assert_eq!(h.mock_llm.call_count(), 0);
    }

    #[tokio::test]
    async fn make_messages_produces_correct_count() {
        let msgs = CompactorProbeHarness::make_messages(5, "testing");
        assert_eq!(msgs.len(), 10); // 5 turns * 2 messages each
        assert!(msgs[0].1.contains("testing"));
        assert_eq!(msgs[0].0, "user");
        assert_eq!(msgs[1].0, "assistant");
    }

    #[tokio::test]
    async fn make_tool_messages_produces_correct_count() {
        let msgs = CompactorProbeHarness::make_tool_messages(3);
        assert_eq!(msgs.len(), 9); // 3 iterations * 3 messages each
    }

    #[tokio::test]
    async fn empty_session_returns_no_facts() {
        let h = CompactorProbeHarness::new().await;
        let facts = h.query_session_facts("nonexistent-session").await;
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn failing_llm_harness() {
        let h = CompactorProbeHarness::with_failing_llm().await;
        assert!(h.compactor.config().enabled);
    }

    #[tokio::test]
    async fn with_config_harness() {
        let config = SessionCompactorConfig {
            enabled: true,
            fresh_tail_count: 10,
            ..SessionCompactorConfig::default()
        };
        let h = CompactorProbeHarness::with_config(config).await;
        assert_eq!(h.compactor.config().fresh_tail_count, 10);
    }
}
