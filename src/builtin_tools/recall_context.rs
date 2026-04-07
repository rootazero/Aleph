//! Semantic recovery tool for post-compression context retrieval.
//!
//! Allows the LLM to retrieve raw conversation details stored in SQLite
//! before context compression occurred. Searches by path prefix scoped to
//! the current session's raw chunks.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::store::types::SearchFilter;
use crate::memory::store::{MemoryBackend, MemoryStore};

fn default_max_results() -> usize {
    3
}

/// Arguments for the recall_context tool.
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

/// Output from the recall_context tool.
#[derive(Debug, Clone, Serialize)]
pub struct RecallContextResult {
    /// Retrieved fragments ordered by storage order.
    pub fragments: Vec<RecalledFragment>,
    /// The original query used for retrieval.
    pub query: String,
}

/// Tool that retrieves pre-compression conversation details from SQLite.
///
/// Raw conversation chunks are stored under `aleph://session/{session_id}/raw/`
/// by the session compression pipeline (Task 14). This tool lets the LLM
/// recover specific code, error messages, or decision details that were
/// present before the context was compressed.
pub struct RecallContextTool {
    database: MemoryBackend,
    session_id: String,
}

impl RecallContextTool {
    /// Tool identifier registered with the agent runtime.
    pub const NAME: &'static str = "recall_context";

    /// Tool description shown to the LLM in its system prompt.
    pub const DESCRIPTION: &'static str =
        "Retrieve pre-compression conversation details. Use when you need to recall \
         specific code, error messages, or decision details from earlier in the conversation.";

    /// Create a new RecallContextTool for the given session.
    pub fn new(database: MemoryBackend, session_id: impl Into<String>) -> Self {
        Self {
            database,
            session_id: session_id.into(),
        }
    }

    /// Execute the recall search against the session-scoped raw chunk store.
    ///
    /// Searches the SQLite path prefix `aleph://session/{session_id}/raw/`
    /// and returns up to `args.max_results` fragments. The query string is
    /// preserved in the result for the LLM's reference.
    pub async fn call_impl(&self, args: RecallContextArgs) -> anyhow::Result<RecallContextResult> {
        let path_prefix = format!("aleph://session/{}/raw/", self.session_id);

        let filter = SearchFilter::new().with_path_prefix(&path_prefix);

        let facts = self
            .database
            .get_facts_by_path_prefix(&path_prefix, &filter, args.max_results)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to retrieve raw context chunks: {}", e))?;

        let fragments = facts
            .into_iter()
            .map(|f| RecalledFragment {
                content: f.content,
                relevance_score: f.confidence,
                source_path: f.path,
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
}
