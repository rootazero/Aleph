//! Memory search/store adapters exposing the memory system as LoopTool.
//!
//! Defines a `MemoryBackend` trait for testability and two tools:
//! - `MemorySearchTool` — semantic search over long-term memory
//! - `MemoryStoreTool` — persist important information for future recall

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use super::super::tool::{LoopTool, ToolResult};

// =============================================================================
// MemoryBackend trait
// =============================================================================

/// Entry returned from a memory search.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub score: f32,
    pub metadata: Option<Value>,
}

/// Abstraction over the memory store for testability.
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Semantic search returning up to `limit` relevant entries.
    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Store content with optional metadata, returning the new entry's ID.
    async fn store(&self, content: &str, metadata: Option<Value>) -> anyhow::Result<String>;
}

// =============================================================================
// MemorySearchTool
// =============================================================================

/// Tool that searches long-term memory via a `MemoryBackend`.
pub struct MemorySearchTool<M: MemoryBackend> {
    backend: Arc<M>,
}

impl<M: MemoryBackend> MemorySearchTool<M> {
    pub fn new(backend: Arc<M>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<M: MemoryBackend + 'static> LoopTool for MemorySearchTool<M> {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search long-term memory for relevant information. Use when you need to recall past conversations, user preferences, or stored knowledge."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to find relevant memories"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => {
                return ToolResult::Error {
                    error: "missing required parameter: query".into(),
                    retryable: false,
                };
            }
        };

        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;

        match self.backend.search(query, limit).await {
            Ok(entries) => {
                let results: Vec<Value> = entries
                    .iter()
                    .map(|e| {
                        json!({
                            "content": e.content,
                            "relevance": e.score,
                        })
                    })
                    .collect();
                let count = results.len();
                ToolResult::Success {
                    output: json!({ "results": results, "count": count }),
                }
            }
            Err(e) => ToolResult::Error {
                error: format!("memory search failed: {e}"),
                retryable: true,
            },
        }
    }
}

// =============================================================================
// MemoryStoreTool
// =============================================================================

/// Tool that stores information into long-term memory via a `MemoryBackend`.
pub struct MemoryStoreTool<M: MemoryBackend> {
    backend: Arc<M>,
}

impl<M: MemoryBackend> MemoryStoreTool<M> {
    pub fn new(backend: Arc<M>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<M: MemoryBackend + 'static> LoopTool for MemoryStoreTool<M> {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Store important information in long-term memory for future recall. Use for user preferences, key facts, or decisions worth remembering."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The information to store in memory"
                },
                "metadata": {
                    "type": "object",
                    "description": "Optional metadata to attach to the memory entry"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                return ToolResult::Error {
                    error: "missing required parameter: content".into(),
                    retryable: false,
                };
            }
        };

        let metadata = input.get("metadata").cloned();

        match self.backend.store(content, metadata).await {
            Ok(id) => ToolResult::Success {
                output: json!({ "stored": true, "id": id }),
            },
            Err(e) => ToolResult::Error {
                error: format!("memory store failed: {e}"),
                retryable: true,
            },
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::Mutex;

    /// In-memory fake backend for testing.
    struct FakeMemory {
        entries: Mutex<Vec<MemoryEntry>>,
        next_id: Mutex<u64>,
    }

    impl FakeMemory {
        fn new() -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
                next_id: Mutex::new(1),
            }
        }
    }

    #[async_trait]
    impl MemoryBackend for FakeMemory {
        async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
            let entries = self.entries.lock().await;
            let results: Vec<MemoryEntry> = entries
                .iter()
                .filter(|e| e.content.contains(query))
                .take(limit)
                .cloned()
                .collect();
            Ok(results)
        }

        async fn store(&self, content: &str, metadata: Option<Value>) -> anyhow::Result<String> {
            let mut entries = self.entries.lock().await;
            let mut next_id = self.next_id.lock().await;
            let id = format!("mem-{}", *next_id);
            *next_id += 1;
            entries.push(MemoryEntry {
                id: id.clone(),
                content: content.to_string(),
                score: 1.0,
                metadata,
            });
            Ok(id)
        }
    }

    #[tokio::test]
    async fn test_store_tool_success() {
        let backend = Arc::new(FakeMemory::new());
        let tool = MemoryStoreTool::new(Arc::clone(&backend));

        let result = tool
            .execute(json!({ "content": "User prefers dark mode" }))
            .await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["stored"], true);
                assert_eq!(output["id"], "mem-1");
            }
            ToolResult::Error { error, .. } => panic!("expected success, got: {error}"),
        }
    }

    #[tokio::test]
    async fn test_search_tool_finds_stored() {
        let backend = Arc::new(FakeMemory::new());

        // Store something first
        backend
            .store("User prefers dark mode", None)
            .await
            .unwrap();

        let tool = MemorySearchTool::new(Arc::clone(&backend));
        let result = tool.execute(json!({ "query": "dark mode" })).await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["count"], 1);
                let results = output["results"].as_array().unwrap();
                assert_eq!(results[0]["content"], "User prefers dark mode");
                assert_eq!(results[0]["relevance"], 1.0);
            }
            ToolResult::Error { error, .. } => panic!("expected success, got: {error}"),
        }
    }

    #[tokio::test]
    async fn test_search_no_results() {
        let backend = Arc::new(FakeMemory::new());
        let tool = MemorySearchTool::new(Arc::clone(&backend));

        let result = tool
            .execute(json!({ "query": "nonexistent topic" }))
            .await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["count"], 0);
                assert!(output["results"].as_array().unwrap().is_empty());
            }
            ToolResult::Error { error, .. } => panic!("expected success, got: {error}"),
        }
    }

    #[tokio::test]
    async fn test_store_with_metadata() {
        let backend = Arc::new(FakeMemory::new());
        let tool = MemoryStoreTool::new(Arc::clone(&backend));

        let result = tool
            .execute(json!({
                "content": "Meeting at 3pm",
                "metadata": { "category": "schedule", "priority": "high" }
            }))
            .await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["stored"], true);
                assert_eq!(output["id"], "mem-1");
            }
            ToolResult::Error { error, .. } => panic!("expected success, got: {error}"),
        }

        // Verify metadata was stored
        let entries = backend.entries.lock().await;
        assert_eq!(entries.len(), 1);
        let meta = entries[0].metadata.as_ref().unwrap();
        assert_eq!(meta["category"], "schedule");
        assert_eq!(meta["priority"], "high");
    }

    #[tokio::test]
    async fn test_search_missing_query() {
        let backend = Arc::new(FakeMemory::new());
        let tool = MemorySearchTool::new(Arc::clone(&backend));

        let result = tool.execute(json!({})).await;
        match result {
            ToolResult::Error {
                error, retryable, ..
            } => {
                assert!(error.contains("missing required parameter: query"));
                assert!(!retryable);
            }
            ToolResult::Success { .. } => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn test_store_missing_content() {
        let backend = Arc::new(FakeMemory::new());
        let tool = MemoryStoreTool::new(Arc::clone(&backend));

        let result = tool.execute(json!({})).await;
        match result {
            ToolResult::Error {
                error, retryable, ..
            } => {
                assert!(error.contains("missing required parameter: content"));
                assert!(!retryable);
            }
            ToolResult::Success { .. } => panic!("expected error"),
        }
    }
}
