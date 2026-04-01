//! Cross-session search tool using FTS5 full-text search.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::error::ToolError;
use crate::error::Result;
use crate::gateway::session_manager::SessionManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionSearchArgs {
    /// Full-text search query to find in past conversations
    pub query: String,
    /// Maximum number of matching messages to return (default 5)
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_max_results() -> usize {
    5
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSearchHit {
    pub session_key: String,
    pub agent_id: String,
    pub topic: Option<String>,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSearchOutput {
    pub query: String,
    pub hits: Vec<SessionSearchHit>,
    pub total_hits: usize,
}

#[derive(Clone)]
pub struct SessionSearchTool {
    session_manager: Arc<SessionManager>,
}

impl SessionSearchTool {
    pub fn new(session_manager: Arc<SessionManager>) -> Self {
        Self { session_manager }
    }

    async fn call_impl(
        &self,
        args: SessionSearchArgs,
    ) -> std::result::Result<SessionSearchOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        let args_summary = format!("搜索历史对话: {}", &args.query);
        notify_tool_start("session_search", &args_summary);

        let results = self
            .session_manager
            .search_messages(&args.query, args.max_results)
            .await
            .map_err(|e| ToolError::Execution(format!("Session search failed: {}", e)))?;

        let total_hits = results.len();
        let hits: Vec<SessionSearchHit> = results
            .into_iter()
            .map(|r| SessionSearchHit {
                session_key: r.session_key,
                agent_id: r.agent_id,
                topic: r.topic,
                role: r.role,
                content: r.content,
                timestamp: r.timestamp,
            })
            .collect();

        let result_summary = format!("找到 {} 条历史对话匹配", total_hits);
        notify_tool_result("session_search", &result_summary, true);

        Ok(SessionSearchOutput {
            query: args.query,
            hits,
            total_hits,
        })
    }
}

#[async_trait]
impl AlephTool for SessionSearchTool {
    const NAME: &'static str = "session_search";
    const DESCRIPTION: &'static str =
        "Search past conversation transcripts across all sessions using full-text search. \
        Use this when the user references something from a prior conversation, \
        or when you suspect relevant context exists in past sessions. \
        Prefer this over asking the user to repeat themselves.";

    type Args = SessionSearchArgs;
    type Output = SessionSearchOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "session_search(query='Rust async patterns')".to_string(),
            "session_search(query='deployment configuration', max_results=3)".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_deserialization() {
        let json = r#"{"query": "test search"}"#;
        let args: SessionSearchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.query, "test search");
        assert_eq!(args.max_results, 5);
    }

    #[test]
    fn args_with_max_results() {
        let json = r#"{"query": "test", "max_results": 3}"#;
        let args: SessionSearchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.max_results, 3);
    }
}
