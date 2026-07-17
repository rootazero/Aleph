//! Semantic recovery tool for post-compression context retrieval.
//!
//! Allows the LLM to retrieve raw conversation details stored in `SQLite`
//! before context compression occurred. Searches by path prefix scoped to
//! the current session's raw chunks.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::MemoryBackend;

const fn default_max_results() -> usize {
    3
}

/// Arguments for the `recall_context` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RecallContextArgs {
    /// Description of what to recall from before compression.
    pub query: String,
    /// Maximum number of results to return.
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

/// A single recalled fragment from pre-compression storage.
#[derive(Debug, Clone, Serialize)]
pub struct RecalledFragment {
    /// Raw content of the stored chunk.
    pub content: String,
    /// Confidence score of the stored fact.
    pub relevance_score: f32,
    /// VFS path where this fragment is stored.
    pub source_path: String,
}

/// Output from the `recall_context` tool.
#[derive(Debug, Clone, Serialize)]
pub struct RecallContextResult {
    /// Retrieved fragments ordered by storage order.
    pub fragments: Vec<RecalledFragment>,
    /// The original query used for retrieval.
    pub query: String,
}

/// Tool that retrieves pre-compression conversation details from `SQLite`.
///
/// Raw conversation chunks are stored under `aleph://session/{session_id}/raw/`
/// by the session compression pipeline (Task 14). This tool lets the LLM
/// recover specific code, error messages, or decision details that were
/// present before the context was compressed.
pub struct RecallContextTool {
    database: MemoryBackend,
    session_id: String,
    /// Resolved (optionally project-scoped) agent id the compaction pipeline
    /// writes raw chunks under — must match `session_compactor::store_raw_chunk`,
    /// which stopped hardcoding `"default"` when the writer gained the real id.
    agent_id: String,
}

impl RecallContextTool {
    /// Tool identifier registered with the agent runtime.
    pub const NAME: &'static str = "recall_context";

    /// Tool description shown to the LLM in its system prompt.
    pub const DESCRIPTION: &'static str =
        "Retrieve pre-compression conversation details. Use when you need to recall \
         specific code, error messages, or decision details from earlier in the conversation.";

    /// Create a new `RecallContextTool` for the given session.
    ///
    /// `agent_id` must be the same resolved id under which the compaction
    /// pipeline stores the session's raw chunks (see
    /// `session_compactor::store_raw_chunk`); reading under a different id
    /// silently returns nothing.
    pub fn new(
        database: MemoryBackend,
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            database,
            session_id: session_id.into(),
            agent_id: agent_id.into(),
        }
    }

    /// Execute the recall search against the session-scoped raw chunk store.
    ///
    /// Searches the `SQLite` path prefix `aleph://session/{session_id}/raw/`
    /// and returns up to `args.max_results` fragments. The query string is
    /// preserved in the result for the LLM's reference.
    pub async fn call_impl(&self, args: RecallContextArgs) -> anyhow::Result<RecallContextResult> {
        let path_prefix = format!("aleph://session/{}/raw/", self.session_id);

        let raws = self
            .database
            .get_raw_by_path_prefix(&path_prefix, &self.agent_id, args.max_results)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to retrieve raw context chunks: {e}"))?;

        let fragments = raws
            .into_iter()
            .map(|r| RecalledFragment {
                content: r.content,
                relevance_score: 1.0,
                source_path: r.path.unwrap_or_default(),
            })
            .collect();

        Ok(RecallContextResult {
            fragments,
            query: args.query,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_and_description() {
        assert_eq!(RecallContextTool::NAME, "recall_context");
        assert!(!RecallContextTool::DESCRIPTION.is_empty());
    }

    #[test]
    fn args_deserialize() {
        let json = r#"{"query": "config.rs error", "max_results": 5}"#;
        let args: RecallContextArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.query, "config.rs error");
        assert_eq!(args.max_results, 5);
    }

    #[test]
    fn args_default_max_results() {
        let json = r#"{"query": "test"}"#;
        let args: RecallContextArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.max_results, 3);
    }

    /// Regression: the reader used to hardcode agent `"default"` while the
    /// compaction writer (`session_compactor::store_raw_chunk`) stores chunks
    /// under the resolved agent id — recall for any non-default agent silently
    /// returned nothing.
    #[tokio::test]
    async fn recall_reads_under_the_writers_agent_scope() {
        use crate::memory::store::raw_memory::RawMemoryStore;
        use crate::memory::store::{RawMemory, RawMemorySource};
        use crate::sync_primitives::Arc;

        let temp = tempfile::tempdir().unwrap();
        let database: MemoryBackend =
            Arc::new(crate::memory::store::sqlite::SqliteMemoryBackend::new(temp.path()).unwrap());
        let raw = RawMemory::new(
            "[user]: the API key lives in .env.local".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("alice")
        .with_session("sess-1")
        .with_path("aleph://session/sess-1/raw/0");
        database.insert_raw_memory(&raw).await.unwrap();

        let tool = RecallContextTool::new(database.clone(), "sess-1", "alice");
        let out = tool
            .call_impl(RecallContextArgs {
                query: "API key".into(),
                max_results: 3,
            })
            .await
            .unwrap();
        assert_eq!(out.fragments.len(), 1, "chunk visible under the writer id");

        let wrong_scope = RecallContextTool::new(database, "sess-1", "default");
        let out = wrong_scope
            .call_impl(RecallContextArgs {
                query: "API key".into(),
                max_results: 3,
            })
            .await
            .unwrap();
        assert!(
            out.fragments.is_empty(),
            "reading under a different agent id must not see the chunk"
        );
    }
}
