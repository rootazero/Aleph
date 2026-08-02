//! Spec B — filter for restricting `HybridAssembler` results by `FactSource`.
//!
//! Default behaviour (`Any`) is byte-for-byte identical to pre-Spec-B
//! assembler output. `Only(_)` and `Excluding(_)` are non-default values
//! used by `session_search` (and only `session_search`) to physically
//! separate session summaries from wiki/note retrieval.

use crate::memory::context::FactSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FactSourceFilter {
    #[default]
    Any,
    Only(FactSource),
    Excluding(FactSource),
}

impl FactSourceFilter {
    /// Predicate evaluated row-by-row when filtering candidate facts.
    #[must_use]
    pub fn matches(&self, source: FactSource) -> bool {
        match self {
            Self::Any => true,
            Self::Only(want) => *want == source,
            Self::Excluding(skip) => *skip != source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_matches_everything() {
        let f = FactSourceFilter::Any;
        assert!(f.matches(FactSource::SessionCompressed));
        assert!(f.matches(FactSource::Extracted));
    }

    #[test]
    fn only_matches_target_only() {
        let f = FactSourceFilter::Only(FactSource::SessionCompressed);
        assert!(f.matches(FactSource::SessionCompressed));
        assert!(!f.matches(FactSource::Extracted));
    }

    #[test]
    fn excluding_skips_target_only() {
        let f = FactSourceFilter::Excluding(FactSource::SessionCompressed);
        assert!(!f.matches(FactSource::SessionCompressed));
        assert!(f.matches(FactSource::Extracted));
    }

    #[test]
    fn default_is_any() {
        assert_eq!(FactSourceFilter::default(), FactSourceFilter::Any);
    }
}

// ---------------------------------------------------------------------------
// Integration test — requires async runtime + real SQLite backend.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::error::AlephError;
    use crate::memory::assembler::hybrid::LlmReranker;
    use crate::memory::assembler::{
        AssemblyBudget, FeedbackFloorLoader, HybridAssembler, UserProfileLoader,
        WorkingMemoryAssembler,
    };
    use crate::memory::context::FactSource;
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::note_retrieval::NoteFactRetrieval;
    use crate::memory::notes::NoteIndexer;
    use crate::memory::session_resume::reader::SnapshotReader;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::MemoryBackend;
    use crate::memory::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use async_trait::async_trait;

    struct NeverCalledReranker;

    #[async_trait]
    impl LlmReranker for NeverCalledReranker {
        async fn complete(&self, _: &str, _: Option<&str>) -> Result<String, AlephError> {
            Err(AlephError::config("NeverCalledReranker".to_string()))
        }
    }

    struct ZeroEmbedder;

    #[async_trait]
    impl EmbeddingProvider for ZeroEmbedder {
        async fn embed(&self, _: &str) -> Result<Vec<f32>, AlephError> {
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

    fn build_assembler(backend: MemoryBackend, notes_dir: std::path::PathBuf) -> HybridAssembler {
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(ZeroEmbedder);
        let indexer = Arc::new(NoteIndexer::new(notes_dir.clone(), backend.clone()));
        let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
        let snap_dir = notes_dir.join("_snapshots");
        std::fs::create_dir_all(&snap_dir).unwrap();
        let snapshots = Arc::new(SnapshotReader::new(&snap_dir));
        let feedback_floor = FeedbackFloorLoader::new(notes_dir.clone());
        let profile = UserProfileLoader::new(notes_dir);
        let reranker: Arc<dyn LlmReranker> = Arc::new(NeverCalledReranker);
        let cfg = crate::config::types::memory::AssemblerConfig {
            enabled: true,
            candidate_pool_limit: 20,
            rerank_timeout_ms: 200,
            rerank_model: None,
            render_style: Default::default(),
            force_fallback: true, // skeleton path — no LLM needed
            fallback_skeleton: Default::default(),
            assembly_log: Default::default(),
            project_scoped: false,
            retrieval_scoring: Default::default(),
            rerank: Default::default(),
            expansion: Default::default(),
        };
        HybridAssembler::new(
            retrieval,
            snapshots,
            backend,
            profile,
            feedback_floor,
            reranker,
            cfg,
        )
    }

    /// Seed: 2 Transcript raw memories + 1 SessionCompressed raw memory.
    /// Call assemble with `Only(SessionCompressed)`.
    /// Assert: the envelope contains the SessionCompressed fact's content
    /// and does NOT contain the Transcript content.
    #[tokio::test]
    async fn only_session_compressed_excludes_transcripts() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("mem.db");
        let notes_dir = tmp.path().join("notes");
        tokio::fs::create_dir_all(&notes_dir).await.unwrap();

        let backend: MemoryBackend = Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());

        // Seed 2 Transcript fragments with matching content
        for i in 0..2u32 {
            let raw = RawMemory::new(format!("Transcript body {i}"), RawMemorySource::Transcript)
                .with_agent("agent-filter-test")
                .with_session("sess-filter");
            backend.insert_raw_memory(&raw).await.unwrap();
        }

        // Seed 1 SessionCompressed fact — the key target
        let compressed = RawMemory::new(
            "User asked about deployment strategies".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("agent-filter-test")
        .with_session("sess-filter")
        .with_path("aleph://session/sess-filter/d0/0");
        backend.insert_raw_memory(&compressed).await.unwrap();

        let assembler = build_assembler(backend, notes_dir);

        // session_id = None forces the cross-session fetch path for
        // Only(SessionCompressed), which queries by source rather than path prefix.
        let envelope = assembler
            .assemble(
                "deployment",
                "agent-filter-test",
                None,
                AssemblyBudget { total_tokens: 4000 },
                FactSourceFilter::Only(FactSource::SessionCompressed),
            )
            .await
            .expect("assemble must succeed");

        // The envelope must contain the SessionCompressed content.
        let all_content: String = envelope
            .slots
            .iter()
            .flat_map(|s| s.items.iter())
            .map(|i| i.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            all_content.contains("User asked about deployment strategies"),
            "SessionCompressed fact must appear in envelope; got: {all_content:?}"
        );

        // The Transcript content must NOT appear.
        assert!(
            !all_content.contains("Transcript body"),
            "Transcript fragments must be filtered out; got: {all_content:?}"
        );
    }
}
