# Tool-as-Resource Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement dynamic tool discovery using semantic vector search, enabling on-demand tool schema hydration to solve the "scale-context" paradox.

**Architecture:** Tools are stored as `MemoryFact` with `FactType::Tool` for semantic retrieval. A `ToolIndexCoordinator` synchronizes tools from `ToolRegistry` to Memory. `ToolRetrieval` provides dual-threshold semantic search, and `HydrationPipeline` injects relevant tool schemas before LLM inference.

**Tech Stack:** Rust, fastembed (bge-small-zh-v1.5), sqlite-vec, tokio, serde

---

## Task 1: Extend FactType with Tool Variant

**Files:**
- Modify: `core/src/memory/context.rs:118-162`

**Step 1: Write the failing test**

Add to `core/src/memory/context.rs` (at the end of file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fact_type_tool_roundtrip() {
        let tool_type = FactType::Tool;
        assert_eq!(tool_type.as_str(), "tool");
        assert_eq!(FactType::from_str("tool"), FactType::Tool);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_fact_type_tool_roundtrip --lib 2>&1 | tail -20`
Expected: FAIL with "no variant named `Tool`"

**Step 3: Write minimal implementation**

In `core/src/memory/context.rs`, modify the `FactType` enum (around line 121):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum FactType {
    /// User preferences (likes, habits, style choices)
    Preference,
    /// User plans, goals, or intentions
    Plan,
    /// Learning or skill-related information
    Learning,
    /// Project or work-related information
    Project,
    /// Personal information (non-sensitive)
    Personal,
    /// Tool capability (procedural knowledge)
    Tool,
    /// Other facts that don't fit above categories
    #[default]
    Other,
}
```

Update `as_str()` method (around line 138):

```rust
impl FactType {
    pub fn as_str(&self) -> &str {
        match self {
            FactType::Preference => "preference",
            FactType::Plan => "plan",
            FactType::Learning => "learning",
            FactType::Project => "project",
            FactType::Personal => "personal",
            FactType::Tool => "tool",
            FactType::Other => "other",
        }
    }
```

Update `from_str()` method (around line 152):

```rust
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "preference" => FactType::Preference,
            "plan" => FactType::Plan,
            "learning" => FactType::Learning,
            "project" => FactType::Project,
            "personal" => FactType::Personal,
            "tool" => FactType::Tool,
            _ => FactType::Other,
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_fact_type_tool_roundtrip --lib 2>&1 | tail -10`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/context.rs
git commit -m "feat(memory): add Tool variant to FactType for procedural knowledge"
```

---

## Task 2: Create ToolIndexConfig

**Files:**
- Create: `core/src/dispatcher/tool_index/mod.rs`
- Create: `core/src/dispatcher/tool_index/config.rs`
- Modify: `core/src/dispatcher/mod.rs`

**Step 1: Write the failing test**

Create `core/src/dispatcher/tool_index/config.rs`:

```rust
//! Tool Index Configuration
//!
//! Configuration for tool retrieval thresholds and behavior.

use serde::{Deserialize, Serialize};

/// Configuration for tool retrieval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRetrievalConfig {
    /// Hard threshold for noise filtering (default: 0.4)
    /// Tools below this score are always discarded
    pub hard_threshold: f32,

    /// Soft threshold for confidence boundary (default: 0.6)
    /// Tools between hard and soft thresholds get summary only
    pub soft_threshold: f32,

    /// High confidence threshold (default: 0.7)
    /// Tools above this get full schema injection
    pub high_confidence: f32,

    /// Maximum number of tools to retrieve (default: 5)
    pub top_k: usize,

    /// Core tools that are always available in index form
    pub core_tools: Vec<String>,
}

impl Default for ToolRetrievalConfig {
    fn default() -> Self {
        Self {
            hard_threshold: 0.4,
            soft_threshold: 0.6,
            high_confidence: 0.7,
            top_k: 5,
            core_tools: vec![
                "search".to_string(),
                "file_read".to_string(),
                "file_write".to_string(),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ToolRetrievalConfig::default();
        assert_eq!(config.hard_threshold, 0.4);
        assert_eq!(config.soft_threshold, 0.6);
        assert_eq!(config.high_confidence, 0.7);
        assert_eq!(config.top_k, 5);
        assert!(!config.core_tools.is_empty());
    }

    #[test]
    fn test_threshold_ordering() {
        let config = ToolRetrievalConfig::default();
        // Thresholds should be ordered: hard < soft < high
        assert!(config.hard_threshold < config.soft_threshold);
        assert!(config.soft_threshold < config.high_confidence);
    }
}
```

**Step 2: Create module file**

Create `core/src/dispatcher/tool_index/mod.rs`:

```rust
//! Tool Index System
//!
//! Provides semantic tool discovery and on-demand schema hydration.
//!
//! # Architecture
//!
//! - `ToolIndexCoordinator`: Syncs tools from ToolRegistry to Memory
//! - `SemanticPurposeInferrer`: Generates semantic descriptions (L0/L1/L2)
//! - `ToolRetrieval`: Dual-threshold semantic search
//! - `ToolRetrievalConfig`: Configuration for thresholds

mod config;

pub use config::ToolRetrievalConfig;
```

**Step 3: Update dispatcher mod.rs**

In `core/src/dispatcher/mod.rs`, add after the existing mod declarations:

```rust
pub mod tool_index;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore tool_index::config --lib 2>&1 | tail -10`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/dispatcher/tool_index/
git add core/src/dispatcher/mod.rs
git commit -m "feat(dispatcher): add ToolRetrievalConfig for tool index system"
```

---

## Task 3: Create SemanticPurposeInferrer (L0 + L1)

**Files:**
- Create: `core/src/dispatcher/tool_index/inference.rs`
- Modify: `core/src/dispatcher/tool_index/mod.rs`

**Step 1: Write the failing test**

Create `core/src/dispatcher/tool_index/inference.rs`:

```rust
//! Semantic Purpose Inference Engine
//!
//! Generates semantic descriptions for tools using a ranked inference strategy:
//! - L0: Extract from structured_meta (zero latency)
//! - L1: Rule-based template inference (minimal latency)
//! - L2: Async LLM enhancement (future, eventual consistency)

use crate::dispatcher::types::UnifiedTool;

/// Optimization level for tool descriptions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizationLevel {
    /// L0: From structured_meta.use_when or capabilities
    L0,
    /// L1: Rule-based template inference
    L1,
    /// L2: LLM-enhanced (async)
    L2,
}

impl OptimizationLevel {
    pub fn as_str(&self) -> &str {
        match self {
            OptimizationLevel::L0 => "L0",
            OptimizationLevel::L1 => "L1",
            OptimizationLevel::L2 => "L2",
        }
    }
}

/// Result of semantic inference
#[derive(Debug, Clone)]
pub struct InferenceResult {
    /// Generated content for MemoryFact
    pub content: String,
    /// Optimization level achieved
    pub level: OptimizationLevel,
}

/// Semantic purpose inferrer for tool descriptions
pub struct SemanticPurposeInferrer;

impl SemanticPurposeInferrer {
    /// Create a new inferrer
    pub fn new() -> Self {
        Self
    }

    /// Generate semantic content for a tool
    ///
    /// Attempts L0 first (structured_meta), falls back to L1 (templates).
    pub fn generate_content(&self, tool: &UnifiedTool) -> InferenceResult {
        // L0: Try structured_meta
        if let Some(purpose) = self.try_level_0(tool) {
            return InferenceResult {
                content: self.format_content(tool, &purpose),
                level: OptimizationLevel::L0,
            };
        }

        // L1: Template inference
        let purpose = self.level_1_inference(tool);
        InferenceResult {
            content: self.format_content(tool, &purpose),
            level: OptimizationLevel::L1,
        }
    }

    /// L0: Extract from structured_meta
    fn try_level_0(&self, tool: &UnifiedTool) -> Option<String> {
        let meta = tool.structured_meta.as_ref()?;

        // Try use_when first
        if !meta.use_when.is_empty() {
            return Some(meta.use_when.join("; "));
        }

        // Try capabilities
        if !meta.capabilities.is_empty() {
            let purposes: Vec<String> = meta
                .capabilities
                .iter()
                .map(|c| format!("{} {} in {}", c.action, c.target, c.scope))
                .collect();
            return Some(purposes.join("; "));
        }

        None
    }

    /// L1: Rule-based template inference
    fn level_1_inference(&self, tool: &UnifiedTool) -> String {
        let name_lower = tool.name.to_lowercase();
        let topic = self.extract_topic(&tool.name);

        // Verb-based templates
        if name_lower.starts_with("list")
            || name_lower.starts_with("get")
            || name_lower.starts_with("read")
            || name_lower.starts_with("fetch")
            || name_lower.starts_with("query")
        {
            return format!("retrieve information about {}", topic);
        }

        if name_lower.starts_with("set")
            || name_lower.starts_with("update")
            || name_lower.starts_with("write")
            || name_lower.starts_with("modify")
        {
            return format!("modify or save {}", topic);
        }

        if name_lower.starts_with("create")
            || name_lower.starts_with("add")
            || name_lower.starts_with("new")
        {
            return format!("create new {}", topic);
        }

        if name_lower.starts_with("delete")
            || name_lower.starts_with("remove")
            || name_lower.starts_with("drop")
        {
            return format!("remove or delete {}", topic);
        }

        if name_lower.starts_with("search") || name_lower.starts_with("find") {
            return format!("search for {}", topic);
        }

        if name_lower.starts_with("send")
            || name_lower.starts_with("post")
            || name_lower.starts_with("notify")
        {
            return format!("send or notify {}", topic);
        }

        // Default fallback
        format!("perform {} operations", topic)
    }

    /// Extract topic from tool name
    fn extract_topic(&self, name: &str) -> String {
        // Split by common separators
        let parts: Vec<&str> = name.split(['_', '-', ':']).collect();

        if parts.len() > 1 {
            // Skip verb prefix, join rest
            parts[1..].join(" ")
        } else {
            name.to_string()
        }
    }

    /// Format final content for MemoryFact
    fn format_content(&self, tool: &UnifiedTool, purpose: &str) -> String {
        format!(
            "[Tool] {}: {}. Use this tool when you need to {}.",
            tool.name, tool.description, purpose
        )
    }
}

impl Default for SemanticPurposeInferrer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::types::{ToolSource, StructuredToolMeta, Capability};

    fn create_test_tool(name: &str, description: &str) -> UnifiedTool {
        UnifiedTool::new(
            format!("test:{}", name),
            name,
            description,
            ToolSource::Builtin,
        )
    }

    #[test]
    fn test_l1_inference_read_verb() {
        let inferrer = SemanticPurposeInferrer::new();
        let tool = create_test_tool("read_file", "Read contents of a file");

        let result = inferrer.generate_content(&tool);
        assert_eq!(result.level, OptimizationLevel::L1);
        assert!(result.content.contains("retrieve information"));
        assert!(result.content.contains("[Tool] read_file"));
    }

    #[test]
    fn test_l1_inference_create_verb() {
        let inferrer = SemanticPurposeInferrer::new();
        let tool = create_test_tool("create_branch", "Create a git branch");

        let result = inferrer.generate_content(&tool);
        assert_eq!(result.level, OptimizationLevel::L1);
        assert!(result.content.contains("create new"));
    }

    #[test]
    fn test_l1_inference_delete_verb() {
        let inferrer = SemanticPurposeInferrer::new();
        let tool = create_test_tool("delete_file", "Delete a file");

        let result = inferrer.generate_content(&tool);
        assert_eq!(result.level, OptimizationLevel::L1);
        assert!(result.content.contains("remove or delete"));
    }

    #[test]
    fn test_l0_inference_with_use_when() {
        let inferrer = SemanticPurposeInferrer::new();
        let mut tool = create_test_tool("git_commit", "Commit changes");

        tool.structured_meta = Some(StructuredToolMeta {
            capabilities: vec![],
            not_suitable_for: vec![],
            differentiation: vec![],
            use_when: vec!["saving work to version control".to_string()],
        });

        let result = inferrer.generate_content(&tool);
        assert_eq!(result.level, OptimizationLevel::L0);
        assert!(result.content.contains("saving work to version control"));
    }

    #[test]
    fn test_l0_inference_with_capabilities() {
        let inferrer = SemanticPurposeInferrer::new();
        let mut tool = create_test_tool("search_web", "Search the web");

        tool.structured_meta = Some(StructuredToolMeta {
            capabilities: vec![Capability::new("search", "information", "web", "results")],
            not_suitable_for: vec![],
            differentiation: vec![],
            use_when: vec![],
        });

        let result = inferrer.generate_content(&tool);
        assert_eq!(result.level, OptimizationLevel::L0);
        assert!(result.content.contains("search information in web"));
    }

    #[test]
    fn test_format_content() {
        let inferrer = SemanticPurposeInferrer::new();
        let tool = create_test_tool("test_tool", "A test tool");

        let content = inferrer.format_content(&tool, "do testing");
        assert!(content.starts_with("[Tool] test_tool:"));
        assert!(content.contains("A test tool"));
        assert!(content.contains("Use this tool when you need to do testing"));
    }
}
```

**Step 2: Update mod.rs**

In `core/src/dispatcher/tool_index/mod.rs`:

```rust
//! Tool Index System
//!
//! Provides semantic tool discovery and on-demand schema hydration.

mod config;
mod inference;

pub use config::ToolRetrievalConfig;
pub use inference::{InferenceResult, OptimizationLevel, SemanticPurposeInferrer};
```

**Step 3: Run test to verify it passes**

Run: `cargo test -p alephcore tool_index::inference --lib 2>&1 | tail -20`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/dispatcher/tool_index/
git commit -m "feat(dispatcher): add SemanticPurposeInferrer with L0/L1 ranked inference"
```

---

## Task 4: Create ToolIndexCoordinator

**Files:**
- Create: `core/src/dispatcher/tool_index/coordinator.rs`
- Modify: `core/src/dispatcher/tool_index/mod.rs`

**Step 1: Write the implementation**

Create `core/src/dispatcher/tool_index/coordinator.rs`:

```rust
//! Tool Index Coordinator
//!
//! Synchronizes tools from ToolRegistry to Memory as ToolFacts.
//! Uses event-driven updates when tools are added/removed.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::inference::{OptimizationLevel, SemanticPurposeInferrer};
use crate::dispatcher::registry::ToolRegistry;
use crate::dispatcher::types::UnifiedTool;
use crate::error::{AlephError, Result};
use crate::memory::context::{FactSpecificity, FactType, MemoryFact, TemporalScope};
use crate::memory::database::core::VectorDatabase;

/// Coordinator for synchronizing tools to Memory
pub struct ToolIndexCoordinator {
    /// Memory database for storing tool facts
    memory: Arc<VectorDatabase>,
    /// Inferrer for generating semantic descriptions
    inferrer: SemanticPurposeInferrer,
    /// Embedding function (from memory system)
    embed_fn: Option<Arc<dyn Fn(&str) -> Result<Vec<f32>> + Send + Sync>>,
}

impl ToolIndexCoordinator {
    /// Create a new coordinator
    pub fn new(memory: Arc<VectorDatabase>) -> Self {
        Self {
            memory,
            inferrer: SemanticPurposeInferrer::new(),
            embed_fn: None,
        }
    }

    /// Set the embedding function
    pub fn with_embed_fn(
        mut self,
        f: Arc<dyn Fn(&str) -> Result<Vec<f32>> + Send + Sync>,
    ) -> Self {
        self.embed_fn = Some(f);
        self
    }

    /// Generate tool fact ID from tool ID
    fn tool_fact_id(tool_id: &str) -> String {
        format!("tool:{}", tool_id)
    }

    /// Build metadata for a tool fact
    fn build_metadata(&self, tool: &UnifiedTool, level: OptimizationLevel) -> Value {
        let category = match &tool.source {
            crate::dispatcher::types::ToolSource::Builtin => "builtin",
            crate::dispatcher::types::ToolSource::Native => "native",
            crate::dispatcher::types::ToolSource::Mcp { .. } => "mcp",
            crate::dispatcher::types::ToolSource::Skill { .. } => "skill",
            crate::dispatcher::types::ToolSource::Custom { .. } => "custom",
        };

        let server = match &tool.source {
            crate::dispatcher::types::ToolSource::Mcp { server } => Some(server.clone()),
            _ => None,
        };

        json!({
            "tool_id": tool.id,
            "server": server,
            "category": category,
            "optimization_level": level.as_str(),
        })
    }

    /// Sync a single tool to Memory
    pub async fn sync_tool(&self, tool: &UnifiedTool) -> Result<()> {
        let inference = self.inferrer.generate_content(tool);
        let fact_id = Self::tool_fact_id(&tool.id);

        debug!(
            "Syncing tool {} with {} inference",
            tool.id,
            inference.level.as_str()
        );

        // Generate embedding if function is available
        let embedding = if let Some(ref embed_fn) = self.embed_fn {
            match embed_fn(&inference.content) {
                Ok(emb) => Some(emb),
                Err(e) => {
                    warn!("Failed to generate embedding for tool {}: {}", tool.id, e);
                    None
                }
            }
        } else {
            None
        };

        // Create the fact
        let mut fact = MemoryFact::with_id(fact_id, inference.content, FactType::Tool)
            .with_specificity(FactSpecificity::Principle)
            .with_temporal_scope(TemporalScope::Permanent);

        if let Some(emb) = embedding {
            fact = fact.with_embedding(emb);
        }

        // Upsert to database
        // First try to delete existing, then insert
        let _ = self
            .memory
            .delete_fact_permanent(&Self::tool_fact_id(&tool.id))
            .await;
        self.memory.insert_fact(fact).await?;

        Ok(())
    }

    /// Remove a tool from Memory
    pub async fn remove_tool(&self, tool_id: &str) -> Result<()> {
        let fact_id = Self::tool_fact_id(tool_id);
        debug!("Removing tool fact: {}", fact_id);

        self.memory.delete_fact_permanent(&fact_id).await
    }

    /// Sync all tools from registry to Memory
    pub async fn sync_all(&self, registry: &ToolRegistry) -> Result<SyncStats> {
        let tools = registry.list_all().await;
        let total = tools.len();
        let mut synced = 0;
        let mut failed = 0;

        info!("Starting full tool sync: {} tools", total);

        for tool in tools {
            match self.sync_tool(&tool).await {
                Ok(()) => synced += 1,
                Err(e) => {
                    warn!("Failed to sync tool {}: {}", tool.id, e);
                    failed += 1;
                }
            }
        }

        info!(
            "Tool sync complete: {} synced, {} failed",
            synced, failed
        );

        Ok(SyncStats {
            total,
            synced,
            failed,
        })
    }

    /// Get all tool facts from Memory
    pub async fn get_tool_facts(&self) -> Result<Vec<MemoryFact>> {
        let all_facts = self.memory.get_all_facts(false).await?;
        Ok(all_facts
            .into_iter()
            .filter(|f| f.fact_type == FactType::Tool)
            .collect())
    }
}

/// Statistics from a sync operation
#[derive(Debug, Clone)]
pub struct SyncStats {
    pub total: usize,
    pub synced: usize,
    pub failed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::types::ToolSource;
    use tempfile::tempdir;

    async fn create_test_coordinator() -> (ToolIndexCoordinator, Arc<VectorDatabase>) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = VectorDatabase::open(db_path.to_str().unwrap())
            .await
            .unwrap();
        let db = Arc::new(db);
        let coordinator = ToolIndexCoordinator::new(Arc::clone(&db));
        (coordinator, db)
    }

    fn create_test_tool(name: &str) -> UnifiedTool {
        UnifiedTool::new(
            format!("test:{}", name),
            name,
            format!("{} description", name),
            ToolSource::Builtin,
        )
    }

    #[tokio::test]
    async fn test_tool_fact_id_generation() {
        let id = ToolIndexCoordinator::tool_fact_id("mcp:github:git_commit");
        assert_eq!(id, "tool:mcp:github:git_commit");
    }

    #[tokio::test]
    async fn test_sync_tool() {
        let (coordinator, db) = create_test_coordinator().await;
        let tool = create_test_tool("test_tool");

        coordinator.sync_tool(&tool).await.unwrap();

        // Verify fact was created
        let fact = db.get_fact("tool:test:test_tool").await.unwrap();
        assert!(fact.is_some());

        let fact = fact.unwrap();
        assert_eq!(fact.fact_type, FactType::Tool);
        assert!(fact.content.contains("[Tool] test_tool"));
        assert_eq!(fact.specificity, FactSpecificity::Principle);
        assert_eq!(fact.temporal_scope, TemporalScope::Permanent);
    }

    #[tokio::test]
    async fn test_remove_tool() {
        let (coordinator, db) = create_test_coordinator().await;
        let tool = create_test_tool("to_remove");

        // Sync first
        coordinator.sync_tool(&tool).await.unwrap();
        assert!(db.get_fact("tool:test:to_remove").await.unwrap().is_some());

        // Remove
        coordinator.remove_tool("test:to_remove").await.unwrap();
        assert!(db.get_fact("tool:test:to_remove").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_get_tool_facts() {
        let (coordinator, _db) = create_test_coordinator().await;

        // Sync some tools
        coordinator
            .sync_tool(&create_test_tool("tool1"))
            .await
            .unwrap();
        coordinator
            .sync_tool(&create_test_tool("tool2"))
            .await
            .unwrap();

        let facts = coordinator.get_tool_facts().await.unwrap();
        assert_eq!(facts.len(), 2);
        assert!(facts.iter().all(|f| f.fact_type == FactType::Tool));
    }
}
```

**Step 2: Update mod.rs**

In `core/src/dispatcher/tool_index/mod.rs`:

```rust
//! Tool Index System
//!
//! Provides semantic tool discovery and on-demand schema hydration.

mod config;
mod coordinator;
mod inference;

pub use config::ToolRetrievalConfig;
pub use coordinator::{SyncStats, ToolIndexCoordinator};
pub use inference::{InferenceResult, OptimizationLevel, SemanticPurposeInferrer};
```

**Step 3: Run test to verify it passes**

Run: `cargo test -p alephcore tool_index::coordinator --lib 2>&1 | tail -20`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/dispatcher/tool_index/
git commit -m "feat(dispatcher): add ToolIndexCoordinator for Memory synchronization"
```

---

## Task 5: Create ToolRetrieval with Dual-Threshold Logic

**Files:**
- Create: `core/src/dispatcher/tool_index/retrieval.rs`
- Modify: `core/src/dispatcher/tool_index/mod.rs`

**Step 1: Write the implementation**

Create `core/src/dispatcher/tool_index/retrieval.rs`:

```rust
//! Tool Retrieval
//!
//! Semantic tool search with dual-threshold filtering.

use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::debug;

use super::config::ToolRetrievalConfig;
use crate::dispatcher::registry::ToolRegistry;
use crate::dispatcher::types::UnifiedTool;
use crate::error::Result;
use crate::memory::context::{FactType, MemoryFact};
use crate::memory::database::core::VectorDatabase;

/// Hydration level for retrieved tools
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HydrationLevel {
    /// Full JSON Schema injected
    FullSchema,
    /// Only name + description (summary)
    SummaryOnly,
}

/// A tool with its hydration level
#[derive(Debug, Clone)]
pub struct HydratedTool {
    pub tool: UnifiedTool,
    pub hydration_level: HydrationLevel,
    pub score: f32,
}

/// Confidence level for retrieval results
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceLevel {
    High,
    Medium,
    Low,
    None,
}

/// Result of tool retrieval
#[derive(Debug, Clone)]
pub struct RetrievalResult {
    /// Tools with full schema (high + medium confidence)
    pub hydrated: Vec<HydratedTool>,
    /// Tools with summary only (low confidence)
    pub summaries: Vec<HydratedTool>,
    /// Overall confidence level
    pub confidence: ConfidenceLevel,
}

impl RetrievalResult {
    /// Get all tools (both hydrated and summaries)
    pub fn all_tools(&self) -> Vec<&HydratedTool> {
        self.hydrated.iter().chain(self.summaries.iter()).collect()
    }

    /// Check if any tools were found
    pub fn is_empty(&self) -> bool {
        self.hydrated.is_empty() && self.summaries.is_empty()
    }

    /// Total tool count
    pub fn len(&self) -> usize {
        self.hydrated.len() + self.summaries.len()
    }
}

/// Tool retrieval service
pub struct ToolRetrieval {
    memory: Arc<VectorDatabase>,
    registry: Arc<RwLock<ToolRegistry>>,
    config: ToolRetrievalConfig,
    embed_fn: Option<Arc<dyn Fn(&str) -> Result<Vec<f32>> + Send + Sync>>,
}

impl ToolRetrieval {
    /// Create a new retrieval service
    pub fn new(
        memory: Arc<VectorDatabase>,
        registry: Arc<RwLock<ToolRegistry>>,
        config: ToolRetrievalConfig,
    ) -> Self {
        Self {
            memory,
            registry,
            config,
            embed_fn: None,
        }
    }

    /// Set the embedding function
    pub fn with_embed_fn(
        mut self,
        f: Arc<dyn Fn(&str) -> Result<Vec<f32>> + Send + Sync>,
    ) -> Self {
        self.embed_fn = Some(f);
        self
    }

    /// Retrieve relevant tools for a query
    pub async fn retrieve(&self, query: &str) -> Result<RetrievalResult> {
        // Generate query embedding
        let query_embedding = match &self.embed_fn {
            Some(f) => f(query)?,
            None => {
                return Ok(RetrievalResult {
                    hydrated: vec![],
                    summaries: vec![],
                    confidence: ConfidenceLevel::None,
                });
            }
        };

        // Search Memory for tool facts
        let facts = self
            .memory
            .hybrid_search_facts(
                &query_embedding,
                query,
                0.7, // vector weight
                0.3, // text weight
                self.config.hard_threshold,
                self.config.top_k * 2, // fetch more candidates
                self.config.top_k,
            )
            .await?;

        // Filter to only tool facts
        let tool_facts: Vec<MemoryFact> = facts
            .into_iter()
            .filter(|f| f.fact_type == FactType::Tool)
            .collect();

        if tool_facts.is_empty() {
            return Ok(RetrievalResult {
                hydrated: vec![],
                summaries: vec![],
                confidence: ConfidenceLevel::None,
            });
        }

        // Classify by confidence level
        let mut high_confidence = Vec::new();
        let mut medium_confidence = Vec::new();
        let mut low_confidence = Vec::new();

        for fact in tool_facts {
            let score = fact.similarity_score.unwrap_or(0.0);
            let tool_id = self.extract_tool_id(&fact.id);

            if score > self.config.high_confidence {
                high_confidence.push((tool_id, score));
            } else if score > self.config.soft_threshold {
                medium_confidence.push((tool_id, score));
            } else if score > self.config.hard_threshold {
                low_confidence.push((tool_id, score));
            }
            // Below hard_threshold: discard
        }

        debug!(
            "Tool retrieval: {} high, {} medium, {} low confidence",
            high_confidence.len(),
            medium_confidence.len(),
            low_confidence.len()
        );

        // Fetch full tools from registry
        let registry = self.registry.read().await;

        let mut hydrated = Vec::new();
        let mut summaries = Vec::new();

        // High + medium confidence get full schema
        for (tool_id, score) in high_confidence.iter().chain(medium_confidence.iter()) {
            if let Some(tool) = registry.get_by_id(tool_id).await {
                hydrated.push(HydratedTool {
                    tool,
                    hydration_level: HydrationLevel::FullSchema,
                    score: *score,
                });
            }
        }

        // Low confidence: only top-1 with summary
        if let Some((tool_id, score)) = low_confidence.first() {
            if let Some(tool) = registry.get_by_id(tool_id).await {
                summaries.push(HydratedTool {
                    tool,
                    hydration_level: HydrationLevel::SummaryOnly,
                    score: *score,
                });
            }
        }

        // Determine overall confidence
        let confidence = if !high_confidence.is_empty() {
            ConfidenceLevel::High
        } else if !medium_confidence.is_empty() {
            ConfidenceLevel::Medium
        } else if !low_confidence.is_empty() {
            ConfidenceLevel::Low
        } else {
            ConfidenceLevel::None
        };

        Ok(RetrievalResult {
            hydrated,
            summaries,
            confidence,
        })
    }

    /// Extract tool ID from fact ID
    fn extract_tool_id(&self, fact_id: &str) -> String {
        // fact_id format: "tool:{tool_id}"
        fact_id
            .strip_prefix("tool:")
            .unwrap_or(fact_id)
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tool_id() {
        let retrieval = ToolRetrieval::new(
            Arc::new(VectorDatabase::in_memory().unwrap()),
            Arc::new(RwLock::new(ToolRegistry::new())),
            ToolRetrievalConfig::default(),
        );

        assert_eq!(
            retrieval.extract_tool_id("tool:mcp:github:git_commit"),
            "mcp:github:git_commit"
        );
        assert_eq!(
            retrieval.extract_tool_id("tool:builtin:search"),
            "builtin:search"
        );
    }

    #[test]
    fn test_retrieval_result_methods() {
        let result = RetrievalResult {
            hydrated: vec![],
            summaries: vec![],
            confidence: ConfidenceLevel::None,
        };

        assert!(result.is_empty());
        assert_eq!(result.len(), 0);
        assert!(result.all_tools().is_empty());
    }

    #[test]
    fn test_confidence_level_ordering() {
        // High > Medium > Low > None
        assert_ne!(ConfidenceLevel::High, ConfidenceLevel::Medium);
        assert_ne!(ConfidenceLevel::Medium, ConfidenceLevel::Low);
        assert_ne!(ConfidenceLevel::Low, ConfidenceLevel::None);
    }
}
```

**Step 2: Update mod.rs**

In `core/src/dispatcher/tool_index/mod.rs`:

```rust
//! Tool Index System
//!
//! Provides semantic tool discovery and on-demand schema hydration.

mod config;
mod coordinator;
mod inference;
mod retrieval;

pub use config::ToolRetrievalConfig;
pub use coordinator::{SyncStats, ToolIndexCoordinator};
pub use inference::{InferenceResult, OptimizationLevel, SemanticPurposeInferrer};
pub use retrieval::{ConfidenceLevel, HydratedTool, HydrationLevel, RetrievalResult, ToolRetrieval};
```

**Step 3: Run test to verify it passes**

Run: `cargo test -p alephcore tool_index::retrieval --lib 2>&1 | tail -20`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/dispatcher/tool_index/
git commit -m "feat(dispatcher): add ToolRetrieval with dual-threshold semantic search"
```

---

## Task 6: Add VectorDatabase::in_memory() for Testing

**Files:**
- Modify: `core/src/memory/database/core.rs`

**Step 1: Check if method exists**

Run: `grep -n "in_memory" core/src/memory/database/core.rs`

If not found, add the method.

**Step 2: Add in_memory constructor if needed**

In `core/src/memory/database/core.rs`, add:

```rust
impl VectorDatabase {
    /// Create an in-memory database for testing
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, AlephError> {
        use rusqlite::Connection;

        let conn = Connection::open_in_memory()
            .map_err(|e| AlephError::config(format!("Failed to open in-memory database: {}", e)))?;

        // Run migrations
        Self::run_migrations(&conn)?;

        Ok(Self {
            conn: std::sync::Mutex::new(conn),
            path: ":memory:".to_string(),
        })
    }
}
```

**Step 3: Run test to verify**

Run: `cargo test -p alephcore tool_index --lib 2>&1 | tail -20`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/memory/database/core.rs
git commit -m "feat(memory): add VectorDatabase::in_memory() for testing"
```

---

## Task 7: Integration Test - Full Pipeline

**Files:**
- Create: `core/src/dispatcher/tool_index/tests.rs`
- Modify: `core/src/dispatcher/tool_index/mod.rs`

**Step 1: Write integration test**

Create `core/src/dispatcher/tool_index/tests.rs`:

```rust
//! Integration tests for Tool Index System

#[cfg(test)]
mod integration {
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::dispatcher::registry::ToolRegistry;
    use crate::dispatcher::tool_index::{
        ToolIndexCoordinator, ToolRetrieval, ToolRetrievalConfig,
    };
    use crate::dispatcher::types::{ToolSource, UnifiedTool};
    use crate::memory::context::FactType;
    use crate::memory::database::core::VectorDatabase;

    fn create_test_tool(name: &str, description: &str) -> UnifiedTool {
        UnifiedTool::new(
            format!("test:{}", name),
            name,
            description,
            ToolSource::Builtin,
        )
    }

    #[tokio::test]
    async fn test_full_sync_and_retrieval_pipeline() {
        // Setup
        let db = Arc::new(VectorDatabase::in_memory().unwrap());
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));

        // Register some tools
        {
            let reg = registry.write().await;
            reg.register_with_conflict_resolution(create_test_tool(
                "git_commit",
                "Commit staged changes to the repository",
            ))
            .await;
            reg.register_with_conflict_resolution(create_test_tool(
                "git_push",
                "Push commits to remote repository",
            ))
            .await;
            reg.register_with_conflict_resolution(create_test_tool(
                "file_read",
                "Read contents of a file",
            ))
            .await;
        }

        // Sync to Memory
        let coordinator = ToolIndexCoordinator::new(Arc::clone(&db));
        let stats = coordinator.sync_all(&*registry.read().await).await.unwrap();

        assert_eq!(stats.total, 3);
        assert_eq!(stats.synced, 3);
        assert_eq!(stats.failed, 0);

        // Verify facts were created
        let facts = coordinator.get_tool_facts().await.unwrap();
        assert_eq!(facts.len(), 3);
        assert!(facts.iter().all(|f| f.fact_type == FactType::Tool));
        assert!(facts.iter().all(|f| f.content.contains("[Tool]")));
    }

    #[tokio::test]
    async fn test_coordinator_sync_and_remove() {
        let db = Arc::new(VectorDatabase::in_memory().unwrap());
        let coordinator = ToolIndexCoordinator::new(Arc::clone(&db));

        let tool = create_test_tool("temp_tool", "A temporary tool");

        // Sync
        coordinator.sync_tool(&tool).await.unwrap();
        let facts = coordinator.get_tool_facts().await.unwrap();
        assert_eq!(facts.len(), 1);

        // Remove
        coordinator.remove_tool("test:temp_tool").await.unwrap();
        let facts = coordinator.get_tool_facts().await.unwrap();
        assert_eq!(facts.len(), 0);
    }
}
```

**Step 2: Update mod.rs**

In `core/src/dispatcher/tool_index/mod.rs`, add at the end:

```rust
#[cfg(test)]
mod tests;
```

**Step 3: Run test to verify**

Run: `cargo test -p alephcore tool_index::tests --lib 2>&1 | tail -20`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/dispatcher/tool_index/
git commit -m "test(dispatcher): add integration tests for tool index pipeline"
```

---

## Task 8: Update lib.rs Exports

**Files:**
- Modify: `core/src/lib.rs`

**Step 1: Add public exports**

In `core/src/lib.rs`, find the dispatcher exports section and add:

```rust
// In the pub use dispatcher section, add:
pub use dispatcher::tool_index::{
    ConfidenceLevel, HydratedTool, HydrationLevel, InferenceResult, OptimizationLevel,
    RetrievalResult, SemanticPurposeInferrer, SyncStats, ToolIndexCoordinator, ToolRetrieval,
    ToolRetrievalConfig,
};
```

**Step 2: Run build to verify**

Run: `cargo build -p alephcore 2>&1 | tail -10`
Expected: Build succeeds

**Step 3: Commit**

```bash
git add core/src/lib.rs
git commit -m "feat(lib): export tool index types from alephcore"
```

---

## Task 9: Update Design Document Status

**Files:**
- Modify: `docs/plans/2026-02-05-tool-as-resource-design.md`

**Step 1: Update status**

Change the status line from `Draft` to `In Progress` and add implementation notes.

**Step 2: Commit**

```bash
git add docs/plans/2026-02-05-tool-as-resource-design.md
git commit -m "docs: update Tool-as-Resource design status to In Progress"
```

---

## Summary

This plan implements the core Tool-as-Resource infrastructure:

| Task | Component | Description |
|------|-----------|-------------|
| 1 | FactType::Tool | Extend Memory fact types |
| 2 | ToolRetrievalConfig | Configuration for thresholds |
| 3 | SemanticPurposeInferrer | L0/L1 ranked inference |
| 4 | ToolIndexCoordinator | Registry → Memory sync |
| 5 | ToolRetrieval | Dual-threshold search |
| 6 | VectorDatabase::in_memory | Testing support |
| 7 | Integration Tests | Full pipeline test |
| 8 | lib.rs exports | Public API |
| 9 | Documentation | Status update |

**Next Phase (not in this plan):**
- MCP event listener integration
- HydrationPipeline in Dispatcher
- L2 async LLM enhancement
- PromptBuilder dynamic injection
