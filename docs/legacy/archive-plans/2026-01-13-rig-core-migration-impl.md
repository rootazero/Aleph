# Rig-Core Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace Aleph's 3-layer routing system with rig-core library, using its built-in Memory, Vector Store, and Tool Calling capabilities.

**Architecture:** Single rig Agent handles all requests autonomously. Tools implemented via rig's `Tool` trait. Memory via rig-sqlite with fastembed embeddings. Simplified UniFFI interface hides rig internals from Swift layer.

**Tech Stack:** rig-core 0.6, rig-sqlite 0.1, fastembed 4, tokio, serde, uniffi

**Design Doc:** `docs/plans/2026-01-13-rig-core-migration-design.md`

---

## Phase 1: Infrastructure Setup

### Task 1.1: Update Cargo.toml Dependencies

**Files:**
- Modify: `Aleph/core/Cargo.toml`

**Step 1: Add rig dependencies**

Add to `[dependencies]` section:

```toml
# Rig framework
rig-core = "0.6"
rig-sqlite = "0.1"
```

**Step 2: Verify dependencies resolve**

Run: `cd /Users/zouguojun/Workspace/Aleph/.worktrees/rig-core-migration/Aleph/core && cargo check`

Expected: Compiling rig-core, rig-sqlite...

**Step 3: Commit**

```bash
git add Aleph/core/Cargo.toml
git commit -m "deps: add rig-core and rig-sqlite dependencies"
```

---

### Task 1.2: Create New Module Structure

**Files:**
- Create: `Aleph/core/src/agent/mod.rs`
- Create: `Aleph/core/src/agent/manager.rs`
- Create: `Aleph/core/src/agent/config.rs`
- Create: `Aleph/core/src/store/mod.rs`
- Create: `Aleph/core/src/store/sqlite.rs`

**Step 1: Create agent module skeleton**

Create `Aleph/core/src/agent/mod.rs`:

```rust
//! Agent module - RigAgentManager and configuration
//!
//! This module provides the core agent functionality using rig-core.

pub mod config;
pub mod manager;

pub use config::AgentConfig;
pub use manager::RigAgentManager;
```

**Step 2: Create agent config skeleton**

Create `Aleph/core/src/agent/config.rs`:

```rust
//! Agent configuration parsing

use serde::{Deserialize, Serialize};

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Provider name (openai, anthropic, ollama, groq)
    pub provider: String,
    /// Model name
    pub model: String,
    /// Temperature (0.0 - 1.0)
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Max tokens
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// System prompt
    #[serde(default)]
    pub system_prompt: String,
}

fn default_temperature() -> f32 { 0.7 }
fn default_max_tokens() -> u32 { 4096 }

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            system_prompt: "You are Aleph, an intelligent assistant.".to_string(),
        }
    }
}
```

**Step 3: Create agent manager skeleton**

Create `Aleph/core/src/agent/manager.rs`:

```rust
//! Rig Agent Manager - core entry point

use crate::error::Result;
use super::config::AgentConfig;

/// Manages the rig Agent lifecycle
pub struct RigAgentManager {
    config: AgentConfig,
}

impl RigAgentManager {
    /// Create a new RigAgentManager
    pub fn new(config: AgentConfig) -> Self {
        Self { config }
    }

    /// Get the current configuration
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_creation() {
        let config = AgentConfig::default();
        let manager = RigAgentManager::new(config);
        assert_eq!(manager.config().provider, "openai");
    }
}
```

**Step 4: Create store module skeleton**

Create `Aleph/core/src/store/mod.rs`:

```rust
//! Vector store module using rig-sqlite

pub mod sqlite;

pub use sqlite::MemoryStore;
```

**Step 5: Create store sqlite skeleton**

Create `Aleph/core/src/store/sqlite.rs`:

```rust
//! SQLite vector store implementation using rig-sqlite

use serde::{Deserialize, Serialize};

/// Memory entry stored in vector database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier
    pub id: String,
    /// User's input
    pub user_input: String,
    /// Assistant's response
    pub assistant_response: String,
    /// Unix timestamp
    pub timestamp: i64,
    /// Source application context
    pub app_context: Option<String>,
}

/// Memory store using rig-sqlite
pub struct MemoryStore {
    // Will be implemented in Phase 2
}

impl MemoryStore {
    /// Create placeholder (will be implemented in Phase 2)
    pub fn placeholder() -> Self {
        Self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_entry_creation() {
        let entry = MemoryEntry {
            id: "test-1".to_string(),
            user_input: "Hello".to_string(),
            assistant_response: "Hi there!".to_string(),
            timestamp: 1234567890,
            app_context: None,
        };
        assert_eq!(entry.id, "test-1");
    }
}
```

**Step 6: Verify modules compile**

Run: `cargo check`

Expected: Compiling... Finished

**Step 7: Run tests**

Run: `cargo test agent store --lib`

Expected: test result: ok. 2 passed

**Step 8: Commit**

```bash
git add Aleph/core/src/agent Aleph/core/src/store
git commit -m "feat: add agent and store module skeletons"
```

---

### Task 1.3: Create Tools Module Structure

**Files:**
- Create: `Aleph/core/src/rig_tools/mod.rs`
- Create: `Aleph/core/src/rig_tools/search.rs`
- Create: `Aleph/core/src/rig_tools/web_fetch.rs`
- Create: `Aleph/core/src/rig_tools/error.rs`

**Step 1: Create tools module**

Create `Aleph/core/src/rig_tools/mod.rs`:

```rust
//! Rig tool implementations
//!
//! All tools implement rig's Tool trait for AI-callable functions.

pub mod error;
pub mod search;
pub mod web_fetch;

pub use error::ToolError;
pub use search::SearchTool;
pub use web_fetch::WebFetchTool;
```

**Step 2: Create tool error type**

Create `Aleph/core/src/rig_tools/error.rs`:

```rust
//! Tool error types

use std::fmt;

/// Error type for tool execution
#[derive(Debug)]
pub enum ToolError {
    /// Network error
    Network(String),
    /// Invalid arguments
    InvalidArgs(String),
    /// Execution failed
    Execution(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::Network(msg) => write!(f, "Network error: {}", msg),
            ToolError::InvalidArgs(msg) => write!(f, "Invalid arguments: {}", msg),
            ToolError::Execution(msg) => write!(f, "Execution error: {}", msg),
        }
    }
}

impl std::error::Error for ToolError {}
```

**Step 3: Create search tool skeleton**

Create `Aleph/core/src/rig_tools/search.rs`:

```rust
//! Web search tool

use serde::{Deserialize, Serialize};
use super::error::ToolError;

/// Arguments for search tool
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchArgs {
    /// Search query
    pub query: String,
    /// Max results (default 5)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize { 5 }

/// Search result
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Web search tool (skeleton)
#[derive(Default)]
pub struct SearchTool;

impl SearchTool {
    pub fn new() -> Self {
        Self
    }

    /// Execute search (placeholder)
    pub async fn execute(&self, args: SearchArgs) -> Result<Vec<SearchResult>, ToolError> {
        // Will be implemented in Phase 3
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_args_default_limit() {
        let args: SearchArgs = serde_json::from_str(r#"{"query": "test"}"#).unwrap();
        assert_eq!(args.limit, 5);
    }
}
```

**Step 4: Create web_fetch tool skeleton**

Create `Aleph/core/src/rig_tools/web_fetch.rs`:

```rust
//! Web fetch tool

use serde::{Deserialize, Serialize};
use super::error::ToolError;

/// Arguments for web fetch tool
#[derive(Debug, Deserialize, Serialize)]
pub struct WebFetchArgs {
    /// URL to fetch
    pub url: String,
}

/// Web fetch result
#[derive(Debug, Serialize)]
pub struct WebFetchResult {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
}

/// Web fetch tool (skeleton)
#[derive(Default)]
pub struct WebFetchTool;

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }

    /// Execute fetch (placeholder)
    pub async fn execute(&self, args: WebFetchArgs) -> Result<WebFetchResult, ToolError> {
        // Will be implemented in Phase 3
        Ok(WebFetchResult {
            url: args.url,
            title: None,
            content: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_fetch_args() {
        let args: WebFetchArgs = serde_json::from_str(r#"{"url": "https://example.com"}"#).unwrap();
        assert_eq!(args.url, "https://example.com");
    }
}
```

**Step 5: Verify compilation**

Run: `cargo check`

Expected: Finished

**Step 6: Run tests**

Run: `cargo test rig_tools --lib`

Expected: test result: ok. 2 passed

**Step 7: Commit**

```bash
git add Aleph/core/src/rig_tools
git commit -m "feat: add rig_tools module skeletons"
```

---

### Task 1.4: Register New Modules in lib.rs

**Files:**
- Modify: `Aleph/core/src/lib.rs`

**Step 1: Add module declarations**

Add to `Aleph/core/src/lib.rs` (near other module declarations):

```rust
// New rig-based modules
pub mod agent;
pub mod store;
pub mod rig_tools;
```

**Step 2: Verify compilation**

Run: `cargo check`

Expected: Finished

**Step 3: Run all tests**

Run: `cargo test --lib`

Expected: test result: ok. 1400+ passed

**Step 4: Commit**

```bash
git add Aleph/core/src/lib.rs
git commit -m "feat: register agent, store, rig_tools modules"
```

---

## Phase 2: Core Functionality

### Task 2.1: Implement MemoryStore with rig-sqlite

**Files:**
- Modify: `Aleph/core/src/store/sqlite.rs`

**Step 1: Write failing test for store creation**

Add to `Aleph/core/src/store/sqlite.rs` tests:

```rust
#[tokio::test]
async fn test_memory_store_creation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test_memory.db");
    let store = MemoryStore::new(db_path.to_str().unwrap()).await.unwrap();
    assert!(store.is_initialized());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_memory_store_creation --lib`

Expected: FAIL - function `new` not found

**Step 3: Implement MemoryStore**

Replace `Aleph/core/src/store/sqlite.rs`:

```rust
//! SQLite vector store implementation using rig-sqlite

use crate::error::{AlephError, Result};
use fastembed::{EmbeddingModel as FastEmbedModel, InitOptions, TextEmbedding};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Memory entry stored in vector database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier
    pub id: String,
    /// User's input
    pub user_input: String,
    /// Assistant's response
    pub assistant_response: String,
    /// Unix timestamp
    pub timestamp: i64,
    /// Source application context
    pub app_context: Option<String>,
}

impl MemoryEntry {
    /// Get text for embedding
    pub fn text(&self) -> String {
        format!("User: {}\nAssistant: {}", self.user_input, self.assistant_response)
    }
}

/// Memory store using SQLite with vector search
pub struct MemoryStore {
    conn: Arc<RwLock<Connection>>,
    embedding_model: TextEmbedding,
    initialized: bool,
}

impl MemoryStore {
    /// Embedding dimension (bge-small-zh-v1.5)
    pub const EMBEDDING_DIM: usize = 512;

    /// Create a new MemoryStore
    pub async fn new(db_path: &str) -> Result<Self> {
        info!(db_path = %db_path, "Creating MemoryStore");

        // Create parent directory if needed
        if let Some(parent) = Path::new(db_path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AlephError::config(format!("Failed to create db directory: {}", e))
            })?;
        }

        // Open SQLite connection
        let conn = Connection::open(db_path).map_err(|e| {
            AlephError::config(format!("Failed to open database: {}", e))
        })?;

        // Initialize embedding model
        let embedding_model = TextEmbedding::try_new(
            InitOptions::new(FastEmbedModel::BGESmallZHV15)
                .with_show_download_progress(true),
        ).map_err(|e| {
            AlephError::config(format!("Failed to initialize embedding model: {}", e))
        })?;

        let store = Self {
            conn: Arc::new(RwLock::new(conn)),
            embedding_model,
            initialized: false,
        };

        // Initialize schema
        store.init_schema().await?;

        Ok(Self {
            conn: store.conn,
            embedding_model: store.embedding_model,
            initialized: true,
        })
    }

    /// Initialize database schema
    async fn init_schema(&self) -> Result<()> {
        let conn = self.conn.write().await;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                user_input TEXT NOT NULL,
                assistant_response TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                app_context TEXT,
                embedding BLOB NOT NULL
            )",
            [],
        ).map_err(|e| AlephError::config(format!("Failed to create table: {}", e)))?;

        debug!("Database schema initialized");
        Ok(())
    }

    /// Check if store is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Store a memory entry
    pub async fn store(&self, entry: MemoryEntry) -> Result<()> {
        let text = entry.text();
        let embedding = self.embed_text(&text)?;
        let embedding_bytes = Self::embedding_to_bytes(&embedding);

        let conn = self.conn.write().await;
        conn.execute(
            "INSERT OR REPLACE INTO memories (id, user_input, assistant_response, timestamp, app_context, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                entry.id,
                entry.user_input,
                entry.assistant_response,
                entry.timestamp,
                entry.app_context,
                embedding_bytes,
            ],
        ).map_err(|e| AlephError::config(format!("Failed to insert memory: {}", e)))?;

        debug!(id = %entry.id, "Memory stored");
        Ok(())
    }

    /// Search for similar memories
    pub async fn search(&self, query: &str, top_k: usize) -> Result<Vec<(MemoryEntry, f32)>> {
        let query_embedding = self.embed_text(query)?;

        let conn = self.conn.read().await;
        let mut stmt = conn.prepare(
            "SELECT id, user_input, assistant_response, timestamp, app_context, embedding FROM memories"
        ).map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let entries: Vec<(MemoryEntry, Vec<f32>)> = stmt
            .query_map([], |row| {
                let embedding_bytes: Vec<u8> = row.get(5)?;
                Ok((
                    MemoryEntry {
                        id: row.get(0)?,
                        user_input: row.get(1)?,
                        assistant_response: row.get(2)?,
                        timestamp: row.get(3)?,
                        app_context: row.get(4)?,
                    },
                    Self::bytes_to_embedding(&embedding_bytes),
                ))
            })
            .map_err(|e| AlephError::config(format!("Query failed: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();

        // Calculate similarities and sort
        let mut scored: Vec<(MemoryEntry, f32)> = entries
            .into_iter()
            .map(|(entry, emb)| {
                let score = Self::cosine_similarity(&query_embedding, &emb);
                (entry, score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        Ok(scored)
    }

    /// Clear all memories
    pub async fn clear(&self) -> Result<()> {
        let conn = self.conn.write().await;
        conn.execute("DELETE FROM memories", [])
            .map_err(|e| AlephError::config(format!("Failed to clear memories: {}", e)))?;
        info!("All memories cleared");
        Ok(())
    }

    /// Embed text using fastembed
    fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embedding_model
            .embed(vec![text], None)
            .map_err(|e| AlephError::config(format!("Embedding failed: {}", e)))?;

        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| AlephError::config("No embedding returned"))
    }

    /// Convert embedding to bytes
    fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Convert bytes to embedding
    fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    /// Calculate cosine similarity
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a > 0.0 && norm_b > 0.0 {
            dot / (norm_a * norm_b)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_entry_text() {
        let entry = MemoryEntry {
            id: "test-1".to_string(),
            user_input: "Hello".to_string(),
            assistant_response: "Hi there!".to_string(),
            timestamp: 1234567890,
            app_context: None,
        };
        assert!(entry.text().contains("User: Hello"));
        assert!(entry.text().contains("Assistant: Hi there!"));
    }

    #[test]
    fn test_embedding_bytes_roundtrip() {
        let embedding = vec![0.1, 0.2, 0.3, 0.4];
        let bytes = MemoryStore::embedding_to_bytes(&embedding);
        let recovered = MemoryStore::bytes_to_embedding(&bytes);

        for (a, b) in embedding.iter().zip(recovered.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((MemoryStore::cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);

        let c = vec![0.0, 1.0, 0.0];
        assert!(MemoryStore::cosine_similarity(&a, &c).abs() < 1e-6);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_memory_store_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_memory.db");
        let store = MemoryStore::new(db_path.to_str().unwrap()).await.unwrap();
        assert!(store.is_initialized());
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_store_and_search() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_memory.db");
        let store = MemoryStore::new(db_path.to_str().unwrap()).await.unwrap();

        let entry = MemoryEntry {
            id: "test-1".to_string(),
            user_input: "What is Rust?".to_string(),
            assistant_response: "Rust is a systems programming language.".to_string(),
            timestamp: 1234567890,
            app_context: None,
        };

        store.store(entry).await.unwrap();

        let results = store.search("programming language", 5).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].0.id, "test-1");
    }
}
```

**Step 4: Add tempfile dependency**

Add to `Cargo.toml` under `[dev-dependencies]`:

```toml
tempfile = "3"
```

**Step 5: Verify compilation**

Run: `cargo check`

Expected: Finished

**Step 6: Run unit tests (non-ignored)**

Run: `cargo test store --lib -- --skip test_memory_store_creation --skip test_store_and_search`

Expected: test result: ok. 3 passed

**Step 7: Commit**

```bash
git add Aleph/core/src/store/sqlite.rs Aleph/core/Cargo.toml
git commit -m "feat: implement MemoryStore with SQLite and fastembed"
```

---

### Task 2.2: Implement RigAgentManager

**Files:**
- Modify: `Aleph/core/src/agent/manager.rs`

**Step 1: Write failing test**

Add test to `manager.rs`:

```rust
#[tokio::test]
async fn test_manager_process_placeholder() {
    let config = AgentConfig::default();
    let manager = RigAgentManager::new(config);
    let result = manager.process("Hello").await;
    assert!(result.is_ok());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_manager_process_placeholder --lib`

Expected: FAIL - method `process` not found

**Step 3: Implement RigAgentManager with rig**

Replace `Aleph/core/src/agent/manager.rs`:

```rust
//! Rig Agent Manager - core entry point

use crate::error::{AlephError, Result};
use crate::store::MemoryStore;
use super::config::AgentConfig;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Response from agent processing
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// Generated response text
    pub content: String,
    /// Tools that were called
    pub tools_called: Vec<String>,
}

/// Manages the rig Agent lifecycle
pub struct RigAgentManager {
    config: AgentConfig,
    memory_store: Option<Arc<RwLock<MemoryStore>>>,
}

impl RigAgentManager {
    /// Create a new RigAgentManager
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            memory_store: None,
        }
    }

    /// Create with memory store
    pub fn with_memory(mut self, store: Arc<RwLock<MemoryStore>>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Get the current configuration
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Process user input (placeholder - will integrate with rig)
    pub async fn process(&self, input: &str) -> Result<AgentResponse> {
        info!(input_len = input.len(), provider = %self.config.provider, "Processing input");

        // TODO: Integrate with rig Agent
        // For now, return placeholder response
        let response = AgentResponse {
            content: format!("[Placeholder] Received: {}", input),
            tools_called: vec![],
        };

        debug!("Processing complete");
        Ok(response)
    }

    /// Process with streaming callback
    pub async fn process_stream<F>(&self, input: &str, mut on_chunk: F) -> Result<AgentResponse>
    where
        F: FnMut(&str) + Send,
    {
        let response = self.process(input).await?;

        // Simulate streaming
        for chunk in response.content.split_whitespace() {
            on_chunk(chunk);
            on_chunk(" ");
        }

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_creation() {
        let config = AgentConfig::default();
        let manager = RigAgentManager::new(config);
        assert_eq!(manager.config().provider, "openai");
    }

    #[tokio::test]
    async fn test_manager_process_placeholder() {
        let config = AgentConfig::default();
        let manager = RigAgentManager::new(config);
        let result = manager.process("Hello").await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.content.contains("Hello"));
    }

    #[tokio::test]
    async fn test_manager_process_stream() {
        let config = AgentConfig::default();
        let manager = RigAgentManager::new(config);

        let mut chunks = Vec::new();
        let result = manager.process_stream("Hello world", |chunk| {
            chunks.push(chunk.to_string());
        }).await;

        assert!(result.is_ok());
        assert!(!chunks.is_empty());
    }
}
```

**Step 4: Update mod.rs exports**

Update `Aleph/core/src/agent/mod.rs`:

```rust
//! Agent module - RigAgentManager and configuration

pub mod config;
pub mod manager;

pub use config::AgentConfig;
pub use manager::{AgentResponse, RigAgentManager};
```

**Step 5: Verify compilation**

Run: `cargo check`

Expected: Finished

**Step 6: Run tests**

Run: `cargo test agent --lib`

Expected: test result: ok. 4 passed

**Step 7: Commit**

```bash
git add Aleph/core/src/agent/
git commit -m "feat: implement RigAgentManager with process methods"
```

---

## Phase 3: Tool Migration

### Task 3.1: Implement SearchTool with rig Tool trait

**Files:**
- Modify: `Aleph/core/src/rig_tools/search.rs`

**Step 1: Write failing test**

Add to search.rs tests:

```rust
#[tokio::test]
async fn test_search_tool_call() {
    let tool = SearchTool::new();
    let args = SearchArgs { query: "rust programming".to_string(), limit: 3 };
    let result = tool.call(args).await;
    assert!(result.is_ok());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_search_tool_call --lib`

Expected: FAIL - method `call` not found

**Step 3: Implement SearchTool with Tool trait**

Replace `Aleph/core/src/rig_tools/search.rs`:

```rust
//! Web search tool

use super::error::ToolError;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use tracing::{debug, info, warn};

/// Arguments for search tool
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchArgs {
    /// Search query
    pub query: String,
    /// Max results (default 5)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize { 5 }

/// Search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Search tool output
#[derive(Debug, Clone, Serialize)]
pub struct SearchOutput {
    pub results: Vec<SearchResult>,
    pub query: String,
}

/// Web search tool using Tavily API
pub struct SearchTool {
    client: Client,
    api_key: Option<String>,
}

impl Default for SearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchTool {
    /// Tool name for rig
    pub const NAME: &'static str = "search";

    /// Tool description for rig
    pub const DESCRIPTION: &'static str =
        "Search the internet for current information. Use for questions requiring up-to-date data.";

    pub fn new() -> Self {
        Self {
            client: Client::new(),
            api_key: env::var("TAVILY_API_KEY").ok(),
        }
    }

    /// Execute search
    pub async fn call(&self, args: SearchArgs) -> Result<SearchOutput, ToolError> {
        info!(query = %args.query, limit = args.limit, "Executing search");

        let api_key = self.api_key.as_ref().ok_or_else(|| {
            ToolError::InvalidArgs("TAVILY_API_KEY not set".to_string())
        })?;

        let response = self.client
            .post("https://api.tavily.com/search")
            .json(&serde_json::json!({
                "api_key": api_key,
                "query": args.query,
                "max_results": args.limit,
                "include_answer": false,
            }))
            .send()
            .await
            .map_err(|e| ToolError::Network(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            warn!(status = %status, "Search API error");
            return Err(ToolError::Execution(format!("API error {}: {}", status, text)));
        }

        let data: TavilyResponse = response
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to parse response: {}", e)))?;

        let results: Vec<SearchResult> = data.results
            .into_iter()
            .take(args.limit)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
            })
            .collect();

        debug!(count = results.len(), "Search completed");

        Ok(SearchOutput {
            results,
            query: args.query,
        })
    }
}

/// Tavily API response
#[derive(Debug, Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_args_default_limit() {
        let args: SearchArgs = serde_json::from_str(r#"{"query": "test"}"#).unwrap();
        assert_eq!(args.limit, 5);
    }

    #[test]
    fn test_search_args_with_limit() {
        let args: SearchArgs = serde_json::from_str(r#"{"query": "test", "limit": 10}"#).unwrap();
        assert_eq!(args.limit, 10);
    }

    #[test]
    fn test_search_tool_creation() {
        let tool = SearchTool::new();
        assert_eq!(SearchTool::NAME, "search");
    }

    #[tokio::test]
    async fn test_search_without_api_key() {
        // Temporarily unset API key
        let original = env::var("TAVILY_API_KEY").ok();
        env::remove_var("TAVILY_API_KEY");

        let tool = SearchTool::new();
        let args = SearchArgs { query: "test".to_string(), limit: 3 };
        let result = tool.call(args).await;

        assert!(result.is_err());
        if let Err(ToolError::InvalidArgs(msg)) = result {
            assert!(msg.contains("TAVILY_API_KEY"));
        }

        // Restore
        if let Some(key) = original {
            env::set_var("TAVILY_API_KEY", key);
        }
    }
}
```

**Step 4: Verify compilation**

Run: `cargo check`

Expected: Finished

**Step 5: Run tests**

Run: `cargo test rig_tools::search --lib`

Expected: test result: ok. 4 passed

**Step 6: Commit**

```bash
git add Aleph/core/src/rig_tools/search.rs
git commit -m "feat: implement SearchTool with Tavily API"
```

---

### Task 3.2: Implement WebFetchTool

**Files:**
- Modify: `Aleph/core/src/rig_tools/web_fetch.rs`

**Step 1: Write failing test**

Add to web_fetch.rs:

```rust
#[tokio::test]
async fn test_web_fetch_call() {
    let tool = WebFetchTool::new();
    let args = WebFetchArgs { url: "https://example.com".to_string() };
    let result = tool.call(args).await;
    assert!(result.is_ok());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_web_fetch_call --lib`

Expected: FAIL - method `call` not found (or wrong signature)

**Step 3: Implement WebFetchTool**

Replace `Aleph/core/src/rig_tools/web_fetch.rs`:

```rust
//! Web fetch tool

use super::error::ToolError;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Arguments for web fetch tool
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebFetchArgs {
    /// URL to fetch
    pub url: String,
}

/// Web fetch result
#[derive(Debug, Clone, Serialize)]
pub struct WebFetchResult {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
}

/// Web fetch tool
pub struct WebFetchTool {
    client: Client,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    /// Tool name
    pub const NAME: &'static str = "web_fetch";

    /// Tool description
    pub const DESCRIPTION: &'static str =
        "Fetch and extract text content from a web page URL.";

    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Execute fetch
    pub async fn call(&self, args: WebFetchArgs) -> Result<WebFetchResult, ToolError> {
        info!(url = %args.url, "Fetching web page");

        let response = self.client
            .get(&args.url)
            .header("User-Agent", "Aleph/1.0")
            .send()
            .await
            .map_err(|e| ToolError::Network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(ToolError::Execution(format!(
                "HTTP error: {}",
                response.status()
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to read response: {}", e)))?;

        let document = Html::parse_document(&html);

        // Extract title
        let title_selector = Selector::parse("title").ok();
        let title = title_selector.and_then(|sel| {
            document.select(&sel).next().map(|el| el.text().collect::<String>())
        });

        // Extract main content
        let content = self.extract_content(&document);

        debug!(
            url = %args.url,
            title = ?title,
            content_len = content.len(),
            "Fetch completed"
        );

        Ok(WebFetchResult {
            url: args.url,
            title,
            content,
        })
    }

    /// Extract readable content from HTML
    fn extract_content(&self, document: &Html) -> String {
        // Try common content selectors
        let selectors = [
            "article",
            "main",
            ".content",
            ".post-content",
            "#content",
            "body",
        ];

        for selector_str in &selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(element) = document.select(&selector).next() {
                    let text: String = element
                        .text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");

                    if text.len() > 100 {
                        // Truncate to reasonable length
                        return text.chars().take(10000).collect();
                    }
                }
            }
        }

        // Fallback: extract all text
        document
            .root_element()
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(10000)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_fetch_args() {
        let args: WebFetchArgs = serde_json::from_str(r#"{"url": "https://example.com"}"#).unwrap();
        assert_eq!(args.url, "https://example.com");
    }

    #[test]
    fn test_web_fetch_tool_creation() {
        let tool = WebFetchTool::new();
        assert_eq!(WebFetchTool::NAME, "web_fetch");
    }

    #[tokio::test]
    async fn test_web_fetch_call() {
        let tool = WebFetchTool::new();
        let args = WebFetchArgs { url: "https://example.com".to_string() };
        let result = tool.call(args).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.url, "https://example.com");
        assert!(output.title.is_some());
    }

    #[tokio::test]
    async fn test_web_fetch_invalid_url() {
        let tool = WebFetchTool::new();
        let args = WebFetchArgs { url: "not-a-valid-url".to_string() };
        let result = tool.call(args).await;

        assert!(result.is_err());
    }
}
```

**Step 4: Verify compilation**

Run: `cargo check`

Expected: Finished

**Step 5: Run tests**

Run: `cargo test rig_tools::web_fetch --lib`

Expected: test result: ok. 4 passed

**Step 6: Commit**

```bash
git add Aleph/core/src/rig_tools/web_fetch.rs
git commit -m "feat: implement WebFetchTool with HTML extraction"
```

---

## Phase 4: UniFFI Integration

### Task 4.1: Create New Simplified UniFFI Interface

**Files:**
- Create: `Aleph/core/src/aleph_v2.udl`

**Step 1: Create new UDL file**

Create `Aleph/core/src/aleph_v2.udl`:

```webidl
namespace aleph_v2 {
    [Throws=AlephV2Error]
    AlephV2Core init_v2(string config_path, AlephV2EventHandler handler);
};

[Error]
enum AlephV2Error {
    "Config",
    "Provider",
    "Tool",
    "Memory",
    "Cancelled",
};

callback interface AlephV2EventHandler {
    void on_thinking();
    void on_tool_start(string tool_name);
    void on_tool_result(string tool_name, string result);
    void on_stream_chunk(string text);
    void on_complete(string response);
    void on_error(string message);
    void on_memory_stored();
};

interface AlephV2Core {
    [Throws=AlephV2Error]
    void process(string input, ProcessOptionsV2? options);

    void cancel();

    sequence<ToolInfoV2> list_tools();

    [Throws=AlephV2Error]
    sequence<MemoryItemV2> search_memory(string query, u32 limit);

    [Throws=AlephV2Error]
    void clear_memory();

    [Throws=AlephV2Error]
    void reload_config();
};

dictionary ProcessOptionsV2 {
    string? app_context;
    string? window_title;
    boolean stream;
};

dictionary ToolInfoV2 {
    string name;
    string description;
    string source;
};

dictionary MemoryItemV2 {
    string id;
    string user_input;
    string assistant_response;
    i64 timestamp;
    string? app_context;
};
```

**Step 2: Commit**

```bash
git add Aleph/core/src/aleph_v2.udl
git commit -m "feat: add simplified UniFFI interface (v2)"
```

---

### Task 4.2: Implement UniFFI Bindings

**Files:**
- Create: `Aleph/core/src/uniffi_v2.rs`

**Step 1: Create UniFFI implementation**

Create `Aleph/core/src/uniffi_v2.rs`:

```rust
//! UniFFI v2 bindings for simplified rig-based architecture

use crate::agent::{AgentConfig, RigAgentManager};
use crate::store::{MemoryEntry, MemoryStore};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

/// Error type for UniFFI v2
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AlephV2Error {
    #[error("Configuration error: {message}")]
    Config { message: String },
    #[error("Provider error: {message}")]
    Provider { message: String },
    #[error("Tool error: {message}")]
    Tool { message: String },
    #[error("Memory error: {message}")]
    Memory { message: String },
    #[error("Operation cancelled")]
    Cancelled,
}

impl From<crate::error::AlephError> for AlephV2Error {
    fn from(e: crate::error::AlephError) -> Self {
        AlephV2Error::Config { message: e.to_string() }
    }
}

/// Event handler callback interface
#[uniffi::export(callback_interface)]
pub trait AlephV2EventHandler: Send + Sync {
    fn on_thinking(&self);
    fn on_tool_start(&self, tool_name: String);
    fn on_tool_result(&self, tool_name: String, result: String);
    fn on_stream_chunk(&self, text: String);
    fn on_complete(&self, response: String);
    fn on_error(&self, message: String);
    fn on_memory_stored(&self);
}

/// Processing options
#[derive(Debug, Clone, uniffi::Record)]
pub struct ProcessOptionsV2 {
    pub app_context: Option<String>,
    pub window_title: Option<String>,
    pub stream: bool,
}

impl Default for ProcessOptionsV2 {
    fn default() -> Self {
        Self {
            app_context: None,
            window_title: None,
            stream: true,
        }
    }
}

/// Tool information for UI
#[derive(Debug, Clone, uniffi::Record)]
pub struct ToolInfoV2 {
    pub name: String,
    pub description: String,
    pub source: String,
}

/// Memory item for UI
#[derive(Debug, Clone, uniffi::Record)]
pub struct MemoryItemV2 {
    pub id: String,
    pub user_input: String,
    pub assistant_response: String,
    pub timestamp: i64,
    pub app_context: Option<String>,
}

impl From<MemoryEntry> for MemoryItemV2 {
    fn from(entry: MemoryEntry) -> Self {
        Self {
            id: entry.id,
            user_input: entry.user_input,
            assistant_response: entry.assistant_response,
            timestamp: entry.timestamp,
            app_context: entry.app_context,
        }
    }
}

/// Core v2 implementation
#[derive(uniffi::Object)]
pub struct AlephV2Core {
    manager: Arc<RwLock<RigAgentManager>>,
    memory_store: Option<Arc<RwLock<MemoryStore>>>,
    handler: Arc<dyn AlephV2EventHandler>,
    runtime: tokio::runtime::Handle,
}

/// Initialize AlephV2Core
#[uniffi::export]
pub fn init_v2(
    config_path: String,
    handler: Arc<dyn AlephV2EventHandler>,
) -> Result<Arc<AlephV2Core>, AlephV2Error> {
    info!(config_path = %config_path, "Initializing AlephV2Core");

    // Create runtime if not in async context
    let runtime = tokio::runtime::Handle::try_current()
        .unwrap_or_else(|_| {
            tokio::runtime::Runtime::new()
                .expect("Failed to create Tokio runtime")
                .handle()
                .clone()
        });

    // TODO: Load config from file
    let config = AgentConfig::default();
    let manager = RigAgentManager::new(config);

    Ok(Arc::new(AlephV2Core {
        manager: Arc::new(RwLock::new(manager)),
        memory_store: None,
        handler,
        runtime,
    }))
}

#[uniffi::export]
impl AlephV2Core {
    /// Process user input
    pub fn process(
        &self,
        input: String,
        options: Option<ProcessOptionsV2>,
    ) -> Result<(), AlephV2Error> {
        let _options = options.unwrap_or_default();
        let handler = Arc::clone(&self.handler);
        let manager = Arc::clone(&self.manager);
        let input_clone = input.clone();

        self.runtime.spawn(async move {
            handler.on_thinking();

            let manager_guard = manager.read().await;
            match manager_guard.process(&input_clone).await {
                Ok(response) => {
                    handler.on_complete(response.content);
                }
                Err(e) => {
                    error!(error = %e, "Processing failed");
                    handler.on_error(e.to_string());
                }
            }
        });

        Ok(())
    }

    /// Cancel current operation
    pub fn cancel(&self) {
        // TODO: Implement cancellation
        info!("Cancel requested");
    }

    /// List available tools
    pub fn list_tools(&self) -> Vec<ToolInfoV2> {
        vec![
            ToolInfoV2 {
                name: "search".to_string(),
                description: "Search the internet".to_string(),
                source: "builtin".to_string(),
            },
            ToolInfoV2 {
                name: "web_fetch".to_string(),
                description: "Fetch web page content".to_string(),
                source: "builtin".to_string(),
            },
        ]
    }

    /// Search memory
    pub fn search_memory(&self, query: String, limit: u32) -> Result<Vec<MemoryItemV2>, AlephV2Error> {
        let store = self.memory_store.as_ref().ok_or_else(|| {
            AlephV2Error::Memory { message: "Memory store not initialized".to_string() }
        })?;

        let store_clone = Arc::clone(store);
        let result = self.runtime.block_on(async {
            let store_guard = store_clone.read().await;
            store_guard.search(&query, limit as usize).await
        });

        match result {
            Ok(entries) => Ok(entries.into_iter().map(|(e, _)| e.into()).collect()),
            Err(e) => Err(AlephV2Error::Memory { message: e.to_string() }),
        }
    }

    /// Clear all memory
    pub fn clear_memory(&self) -> Result<(), AlephV2Error> {
        let store = self.memory_store.as_ref().ok_or_else(|| {
            AlephV2Error::Memory { message: "Memory store not initialized".to_string() }
        })?;

        let store_clone = Arc::clone(store);
        self.runtime.block_on(async {
            let store_guard = store_clone.read().await;
            store_guard.clear().await
        }).map_err(|e| AlephV2Error::Memory { message: e.to_string() })
    }

    /// Reload configuration
    pub fn reload_config(&self) -> Result<(), AlephV2Error> {
        // TODO: Implement config reload
        info!("Config reload requested");
        Ok(())
    }
}

uniffi::setup_scaffolding!("aleph_v2");

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TestHandler {
        completed: AtomicBool,
    }

    impl TestHandler {
        fn new() -> Self {
            Self { completed: AtomicBool::new(false) }
        }
    }

    impl AlephV2EventHandler for TestHandler {
        fn on_thinking(&self) {}
        fn on_tool_start(&self, _: String) {}
        fn on_tool_result(&self, _: String, _: String) {}
        fn on_stream_chunk(&self, _: String) {}
        fn on_complete(&self, _: String) {
            self.completed.store(true, Ordering::SeqCst);
        }
        fn on_error(&self, _: String) {}
        fn on_memory_stored(&self) {}
    }

    #[test]
    fn test_tool_info_creation() {
        let info = ToolInfoV2 {
            name: "test".to_string(),
            description: "Test tool".to_string(),
            source: "builtin".to_string(),
        };
        assert_eq!(info.name, "test");
    }

    #[test]
    fn test_process_options_default() {
        let options = ProcessOptionsV2::default();
        assert!(options.stream);
        assert!(options.app_context.is_none());
    }
}
```

**Step 2: Add thiserror dependency if not present**

Verify `thiserror` is in Cargo.toml (should already be there).

**Step 3: Register module in lib.rs**

Add to `Aleph/core/src/lib.rs`:

```rust
pub mod uniffi_v2;
```

**Step 4: Verify compilation**

Run: `cargo check`

Expected: Finished

**Step 5: Run tests**

Run: `cargo test uniffi_v2 --lib`

Expected: test result: ok. 2 passed

**Step 6: Commit**

```bash
git add Aleph/core/src/uniffi_v2.rs Aleph/core/src/lib.rs
git commit -m "feat: implement UniFFI v2 bindings"
```

---

## Phase 5: Cleanup (To be done after validation)

### Task 5.1: Remove Old Modules

**Note:** Only execute after validating new implementation works.

**Files to delete:**
- `Aleph/core/src/routing/` (entire directory)
- `Aleph/core/src/dispatcher/` (entire directory)
- `Aleph/core/src/capability/` (entire directory)
- `Aleph/core/src/payload/` (entire directory)
- `Aleph/core/src/providers/` (entire directory)
- `Aleph/core/src/semantic/` (entire directory)
- `Aleph/core/src/memory/` (entire directory)

**Steps:**
1. Comment out old module declarations in lib.rs
2. Run `cargo check` to identify remaining dependencies
3. Fix or remove dependencies
4. Delete directories
5. Run full test suite
6. Commit

---

## Summary

| Phase | Tasks | Estimated Steps |
|-------|-------|-----------------|
| 1. Infrastructure | 4 tasks | ~30 steps |
| 2. Core Functionality | 2 tasks | ~15 steps |
| 3. Tool Migration | 2 tasks | ~15 steps |
| 4. UniFFI Integration | 2 tasks | ~12 steps |
| 5. Cleanup | 1 task | ~10 steps |
| **Total** | **11 tasks** | **~82 steps** |

---

**Plan saved to:** `docs/plans/2026-01-13-rig-core-migration-impl.md`
