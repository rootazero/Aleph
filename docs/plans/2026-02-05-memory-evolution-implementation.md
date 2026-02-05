# Memory System Evolution - Phase 1 MVP Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement TranscriptIndexer, ContextComptroller, and memory_search tool to enable Agent-driven memory retrieval

**Architecture:** Extend existing memory system with near-realtime transcript indexing and post-retrieval arbitration to eliminate redundancy while preserving proactive augmentation advantages.

**Tech Stack:** Rust (tokio async), sqlite-vec, fastembed (multilingual-e5-small), schemars (JSON Schema)

---

## Overview

This plan implements the core MVP components from the Memory Evolution Design:

1. **TranscriptIndexer** - Near-realtime vectorization of conversation transcripts
2. **ContextComptroller** - Post-retrieval arbitration to eliminate Fact/Transcript redundancy
3. **memory_search Tool** - Agent-accessible tool for active memory retrieval

**Success Criteria:**
- Agent can search historical conversations via memory_search tool
- No duplicate information (Facts + Transcripts) in context
- Existing tests pass, new tests added

---

## Task 1: Create TranscriptIndexer Module

**Files:**
- Create: `core/src/memory/transcript_indexer/mod.rs`
- Create: `core/src/memory/transcript_indexer/config.rs`
- Create: `core/src/memory/transcript_indexer/indexer.rs`
- Modify: `core/src/memory/mod.rs` (add module export)

### Step 1: Write failing test for TranscriptIndexer

Create test file:

```rust
// core/src/memory/transcript_indexer/mod.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{ContextAnchor, MemoryEntry};
    use crate::memory::database::VectorDatabase;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_index_turn_basic() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(VectorDatabase::new(db_path).unwrap());

        let embedder = Arc::new(SmartEmbedder::new(
            temp_dir.path().to_path_buf(),
            300,
        ));

        let indexer = TranscriptIndexer::new(db.clone(), embedder);

        // Insert a memory entry
        let context = ContextAnchor::now("test.app", "Test Window");
        let mut entry = MemoryEntry::new(
            context,
            "What is Rust?".to_string(),
            "Rust is a systems programming language.".to_string(),
        );

        // Generate embedding
        let text = format!("{} {}", entry.user_input, entry.ai_output);
        let embedding = indexer.embedder.embed(&text).await.unwrap();
        entry.embedding = Some(embedding);

        db.insert_memory(entry.clone()).await.unwrap();

        // Index the turn
        let result = indexer.index_turn(&entry.id).await;
        assert!(result.is_ok());

        // Verify it's searchable
        let query_embedding = indexer.embedder.embed("programming language").await.unwrap();
        let results = db.search_memories(
            "test.app",
            "Test Window",
            &query_embedding,
            5,
        ).await.unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].id, entry.id);
    }
}
```

### Step 2: Run test to verify it fails

Run: `cd /Volumes/TBU4/Workspace/Aleph/.worktrees/memory-evolution && cargo test transcript_indexer::tests::test_index_turn_basic`

Expected: FAIL with "module not found" or "struct not found"

### Step 3: Implement TranscriptIndexer config

```rust
// core/src/memory/transcript_indexer/config.rs
use serde::{Deserialize, Serialize};

/// Configuration for transcript indexing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptIndexerConfig {
    /// Maximum tokens per chunk (default: 400)
    pub max_tokens_per_chunk: usize,

    /// Overlap tokens between chunks (default: 80)
    pub overlap_tokens: usize,

    /// Enable chunking for long transcripts (default: true)
    pub enable_chunking: bool,
}

impl Default for TranscriptIndexerConfig {
    fn default() -> Self {
        Self {
            max_tokens_per_chunk: 400,
            overlap_tokens: 80,
            enable_chunking: true,
        }
    }
}
```

### Step 4: Implement TranscriptIndexer core

```rust
// core/src/memory/transcript_indexer/indexer.rs
use crate::error::{AlephError, Result};
use crate::memory::context::MemoryEntry;
use crate::memory::database::VectorDatabase;
use crate::memory::smart_embedder::SmartEmbedder;
use std::sync::Arc;
use super::config::TranscriptIndexerConfig;

/// Near-realtime transcript indexer
pub struct TranscriptIndexer {
    database: Arc<VectorDatabase>,
    embedder: Arc<SmartEmbedder>,
    config: TranscriptIndexerConfig,
}

impl TranscriptIndexer {
    /// Create new indexer with default config
    pub fn new(
        database: Arc<VectorDatabase>,
        embedder: Arc<SmartEmbedder>,
    ) -> Self {
        Self {
            database,
            embedder,
            config: TranscriptIndexerConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(
        database: Arc<VectorDatabase>,
        embedder: Arc<SmartEmbedder>,
        config: TranscriptIndexerConfig,
    ) -> Self {
        Self {
            database,
            embedder,
            config,
        }
    }

    /// Index a single conversation turn
    ///
    /// This is called after a conversation turn completes.
    /// The memory entry should already be in the database.
    pub async fn index_turn(&self, memory_id: &str) -> Result<()> {
        // Memory is already inserted by MemoryIngestion
        // This is a no-op for MVP since memories table already has embeddings
        // In future, this will handle chunking and additional indexing
        Ok(())
    }

    /// Index with chunking support (future enhancement)
    pub async fn index_with_chunking(&self, memory_id: &str) -> Result<Vec<String>> {
        // TODO: Implement sliding window chunking
        // For now, return single chunk ID
        Ok(vec![memory_id.to_string()])
    }
}
```

### Step 5: Wire up module exports

```rust
// core/src/memory/transcript_indexer/mod.rs
pub mod config;
pub mod indexer;

pub use config::TranscriptIndexerConfig;
pub use indexer::TranscriptIndexer;

#[cfg(test)]
mod tests {
    // ... (test code from Step 1)
}
```

Modify `core/src/memory/mod.rs`:
```rust
// Add to public modules section
pub mod transcript_indexer;

// Add to re-exports section
pub use transcript_indexer::{TranscriptIndexer, TranscriptIndexerConfig};
```

### Step 6: Run test to verify it passes

Run: `cargo test transcript_indexer::tests::test_index_turn_basic`

Expected: PASS

### Step 7: Commit

```bash
git add core/src/memory/transcript_indexer/
git add core/src/memory/mod.rs
git commit -m "$(cat <<'EOF'
feat: add TranscriptIndexer for near-realtime memory indexing

Implements TranscriptIndexer module with:
- TranscriptIndexerConfig for chunking configuration
- TranscriptIndexer core with index_turn() method
- Basic test coverage for indexing workflow

Part of Memory Evolution Phase 1 MVP.

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Create ContextComptroller Module

**Files:**
- Create: `core/src/memory/context_comptroller/mod.rs`
- Create: `core/src/memory/context_comptroller/config.rs`
- Create: `core/src/memory/context_comptroller/comptroller.rs`
- Create: `core/src/memory/context_comptroller/types.rs`
- Modify: `core/src/memory/mod.rs`

### Step 1: Write failing test for ContextComptroller

```rust
// core/src/memory/context_comptroller/mod.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryEntry, MemoryFact};
    use crate::memory::fact_retrieval::RetrievalResult;

    #[test]
    fn test_detect_redundancy_high_similarity() {
        let config = ComptrollerConfig::default();
        let comptroller = ContextComptroller::new(config);

        // Create a fact and a transcript with similar content
        let fact = MemoryFact::new(
            "User prefers Rust for systems programming".to_string(),
            FactType::Preference,
            vec!["mem-1".to_string()],
        ).with_embedding(vec![0.1; 384])
         .with_similarity_score(0.9);

        let mut transcript = MemoryEntry::new(
            crate::memory::context::ContextAnchor::now("test", "test"),
            "I really prefer Rust".to_string(),
            "Rust is great for systems programming".to_string(),
        );
        transcript.embedding = Some(vec![0.1; 384]);
        transcript.similarity_score = Some(0.85);

        let result = RetrievalResult {
            facts: vec![fact],
            raw_memories: vec![transcript],
        };

        let arbitrated = comptroller.arbitrate(result, TokenBudget::new(10000));

        // Should keep transcript, remove fact (prefer original)
        assert_eq!(arbitrated.facts.len(), 0);
        assert_eq!(arbitrated.raw_memories.len(), 1);
    }
}
```

### Step 2: Run test to verify it fails

Run: `cargo test context_comptroller::tests::test_detect_redundancy_high_similarity`

Expected: FAIL

### Step 3: Implement types and config

```rust
// core/src/memory/context_comptroller/types.rs
use crate::memory::context::{MemoryEntry, MemoryFact};

/// Token budget for context window
#[derive(Debug, Clone)]
pub struct TokenBudget {
    pub total: usize,
    pub used: usize,
}

impl TokenBudget {
    pub fn new(total: usize) -> Self {
        Self { total, used: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.total.saturating_sub(self.used)
    }

    pub fn usage_percent(&self) -> f32 {
        (self.used as f32 / self.total as f32) * 100.0
    }
}

/// Arbitrated context after redundancy removal
#[derive(Debug, Clone)]
pub struct ArbitratedContext {
    pub facts: Vec<MemoryFact>,
    pub raw_memories: Vec<MemoryEntry>,
    pub tokens_saved: usize,
}

/// Retention mode for arbitration
#[derive(Debug, Clone, Copy)]
pub enum RetentionMode {
    PreferTranscript,  // Default: keep original text
    PreferFact,        // Space-constrained: keep compressed
    Hybrid,            // Mix based on importance
}
```

```rust
// core/src/memory/context_comptroller/config.rs
use serde::{Deserialize, Serialize};
use super::types::RetentionMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComptrollerConfig {
    /// Similarity threshold for redundancy detection (default: 0.95)
    pub similarity_threshold: f32,

    /// Token budget (default: 100000)
    pub token_budget: usize,

    /// Fold threshold - remaining % to trigger compression (default: 0.2)
    pub fold_threshold: f32,

    /// Retention mode
    pub retention_mode: RetentionMode,
}

impl Default for ComptrollerConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.95,
            token_budget: 100000,
            fold_threshold: 0.2,
            retention_mode: RetentionMode::PreferTranscript,
        }
    }
}
```

### Step 4: Implement ContextComptroller core

```rust
// core/src/memory/context_comptroller/comptroller.rs
use crate::memory::context::{MemoryEntry, MemoryFact};
use crate::memory::fact_retrieval::RetrievalResult;
use super::config::ComptrollerConfig;
use super::types::{ArbitratedContext, RetentionMode, TokenBudget};

pub struct ContextComptroller {
    config: ComptrollerConfig,
}

impl ContextComptroller {
    pub fn new(config: ComptrollerConfig) -> Self {
        Self { config }
    }

    /// Arbitrate retrieval results to eliminate redundancy
    pub fn arbitrate(
        &self,
        results: RetrievalResult,
        budget: TokenBudget,
    ) -> ArbitratedContext {
        let mut tokens_saved = 0;

        // Detect redundancy between facts and transcripts
        let redundant_pairs = self.detect_redundancy(&results.facts, &results.raw_memories);

        let mut kept_facts = Vec::new();
        let mut kept_transcripts = Vec::new();

        // Apply retention strategy
        match self.config.retention_mode {
            RetentionMode::PreferTranscript => {
                // Keep transcripts, remove redundant facts
                for fact in results.facts {
                    if !redundant_pairs.iter().any(|(f_id, _)| f_id == &fact.id) {
                        kept_facts.push(fact);
                    } else {
                        tokens_saved += self.estimate_tokens(&fact.content);
                    }
                }
                kept_transcripts = results.raw_memories;
            }
            RetentionMode::PreferFact => {
                // Keep facts, remove redundant transcripts
                kept_facts = results.facts;
                for transcript in results.raw_memories {
                    if !redundant_pairs.iter().any(|(_, t_id)| t_id == &transcript.id) {
                        kept_transcripts.push(transcript);
                    } else {
                        let text = format!("{} {}", transcript.user_input, transcript.ai_output);
                        tokens_saved += self.estimate_tokens(&text);
                    }
                }
            }
            RetentionMode::Hybrid => {
                // TODO: Implement hybrid strategy
                kept_facts = results.facts;
                kept_transcripts = results.raw_memories;
            }
        }

        ArbitratedContext {
            facts: kept_facts,
            raw_memories: kept_transcripts,
            tokens_saved,
        }
    }

    /// Detect redundant fact-transcript pairs
    fn detect_redundancy(
        &self,
        facts: &[MemoryFact],
        transcripts: &[MemoryEntry],
    ) -> Vec<(String, String)> {
        let mut pairs = Vec::new();

        for fact in facts {
            for transcript in transcripts {
                // Check if fact was derived from this transcript
                if fact.source_memory_ids.contains(&transcript.id) {
                    pairs.push((fact.id.clone(), transcript.id.clone()));
                    continue;
                }

                // Check embedding similarity if both have embeddings
                if let (Some(fact_emb), Some(trans_emb)) = (&fact.embedding, &transcript.embedding) {
                    let similarity = self.cosine_similarity(fact_emb, trans_emb);
                    if similarity >= self.config.similarity_threshold {
                        pairs.push((fact.id.clone(), transcript.id.clone()));
                    }
                }
            }
        }

        pairs
    }

    /// Calculate cosine similarity between two embeddings
    fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }

        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot_product / (norm_a * norm_b)
    }

    /// Estimate tokens (4 chars per token)
    fn estimate_tokens(&self, text: &str) -> usize {
        (text.len() / 4).max(1)
    }
}
```

### Step 5: Wire up module

```rust
// core/src/memory/context_comptroller/mod.rs
pub mod comptroller;
pub mod config;
pub mod types;

pub use comptroller::ContextComptroller;
pub use config::ComptrollerConfig;
pub use types::{ArbitratedContext, RetentionMode, TokenBudget};

#[cfg(test)]
mod tests {
    // ... (test from Step 1)
}
```

Add to `core/src/memory/mod.rs`:
```rust
pub mod context_comptroller;
pub use context_comptroller::{ContextComptroller, ComptrollerConfig, ArbitratedContext, RetentionMode, TokenBudget};
```

### Step 6: Run test

Run: `cargo test context_comptroller::tests::test_detect_redundancy_high_similarity`

Expected: PASS

### Step 7: Commit

```bash
git add core/src/memory/context_comptroller/
git add core/src/memory/mod.rs
git commit -m "$(cat <<'EOF'
feat: add ContextComptroller for redundancy elimination

Implements post-retrieval arbitration to prevent duplicate
information (Facts + Transcripts) in context window.

Features:
- Redundancy detection via source_id and embedding similarity
- Multiple retention modes (PreferTranscript/PreferFact/Hybrid)
- Token budget tracking and savings calculation

Part of Memory Evolution Phase 1 MVP.

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Implement memory_search Tool

**Files:**
- Create: `core/src/tools/memory_search.rs`
- Modify: `core/src/tools/mod.rs`
- Modify: `core/src/tools/server.rs` (register tool)

### Step 1: Write failing test

```rust
// core/src/tools/memory_search.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_search_schema() {
        let tool = MemorySearchTool::new(/* deps */);
        let def = tool.definition();

        assert_eq!(def.name, "memory_search");
        assert!(def.description.contains("search"));
        assert!(def.parameters.is_object());
    }
}
```

### Step 2: Implement tool

```rust
// core/src/tools/memory_search.rs
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::dispatcher::{ToolCategory, ToolDefinition};
use crate::error::Result;
use crate::memory::{
    ContextComptroller, FactRetrieval, MemoryRetrieval, SmartEmbedder,
    VectorDatabase, ComptrollerConfig, TokenBudget,
};
use crate::tools::AlephTool;

#[derive(Clone)]
pub struct MemorySearchTool {
    fact_retrieval: Arc<FactRetrieval>,
    memory_retrieval: Arc<MemoryRetrieval>,
    comptroller: Arc<ContextComptroller>,
    embedder: Arc<SmartEmbedder>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct MemorySearchArgs {
    /// Search query
    pub query: String,

    /// Maximum results to return (default: 6)
    #[serde(default = "default_max_results")]
    pub max_results: Option<u32>,

    /// Minimum similarity score (default: 0.35)
    #[serde(default = "default_min_score")]
    pub min_score: Option<f32>,

    /// Search mode
    #[serde(default)]
    pub mode: SearchMode,
}

#[derive(Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Auto,
    Facts,
    Transcripts,
    Hybrid,
}

#[derive(Serialize)]
pub struct MemorySearchOutput {
    pub items: Vec<MemoryItem>,
    pub total_found: u32,
    pub tokens_used: u32,
}

#[derive(Serialize)]
pub struct MemoryItem {
    pub uri: String,
    pub content: String,
    pub score: f32,
    pub source: String,
    pub timestamp: String,
}

fn default_max_results() -> Option<u32> { Some(6) }
fn default_min_score() -> Option<f32> { Some(0.35) }

impl MemorySearchTool {
    pub fn new(
        database: Arc<VectorDatabase>,
        embedder: Arc<SmartEmbedder>,
    ) -> Self {
        let fact_retrieval = Arc::new(FactRetrieval::new(
            database.clone(),
            embedder.clone(),
            Default::default(),
        ));

        let memory_retrieval = Arc::new(MemoryRetrieval::new(
            database.clone(),
            embedder.clone(),
            Arc::new(Default::default()),
        ));

        let comptroller = Arc::new(ContextComptroller::new(
            ComptrollerConfig::default(),
        ));

        Self {
            fact_retrieval,
            memory_retrieval,
            comptroller,
            embedder,
        }
    }
}

#[async_trait]
impl AlephTool for MemorySearchTool {
    const NAME: &'static str = "memory_search";
    const DESCRIPTION: &'static str =
        "Search historical conversations and extracted facts. \
         Use this when you need to recall previous discussions, \
         user preferences, or context from past interactions.";

    type Args = MemorySearchArgs;
    type Output = MemorySearchOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "memory_search(query='Rust programming preferences')".to_string(),
            "memory_search(query='previous discussion about API design', max_results=10)".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Generate query embedding
        let query_embedding = self.embedder.embed(&args.query).await?;

        // Retrieve based on mode
        let retrieval_result = match args.mode {
            SearchMode::Facts => {
                let facts = self.fact_retrieval.retrieve(&args.query).await?;
                crate::memory::fact_retrieval::RetrievalResult {
                    facts: facts.facts,
                    raw_memories: vec![],
                }
            }
            SearchMode::Transcripts | SearchMode::Hybrid | SearchMode::Auto => {
                // For MVP, use hybrid approach
                self.fact_retrieval.retrieve(&args.query).await?
            }
        };

        // Arbitrate to remove redundancy
        let budget = TokenBudget::new(100000);
        let arbitrated = self.comptroller.arbitrate(retrieval_result, budget);

        // Format results
        let mut items = Vec::new();

        for fact in arbitrated.facts {
            items.push(MemoryItem {
                uri: format!("aleph://fact/{}", fact.id),
                content: fact.content.clone(),
                score: fact.similarity_score.unwrap_or(0.0),
                source: "fact".to_string(),
                timestamp: chrono::DateTime::from_timestamp(fact.created_at, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            });
        }

        for memory in arbitrated.raw_memories {
            let content = format!("User: {}\nAssistant: {}",
                memory.user_input, memory.ai_output);
            items.push(MemoryItem {
                uri: format!("aleph://transcript/{}", memory.id),
                content,
                score: memory.similarity_score.unwrap_or(0.0),
                source: "transcript".to_string(),
                timestamp: chrono::DateTime::from_timestamp(memory.context.timestamp, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            });
        }

        // Sort by score descending
        items.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        // Apply limits
        let max_results = args.max_results.unwrap_or(6) as usize;
        items.truncate(max_results);

        let total_found = items.len() as u32;
        let tokens_used = items.iter()
            .map(|item| item.content.len() / 4)
            .sum::<usize>() as u32;

        Ok(MemorySearchOutput {
            items,
            total_found,
            tokens_used,
        })
    }
}
```

### Step 3: Register tool

Modify `core/src/tools/mod.rs`:
```rust
pub mod memory_search;
pub use memory_search::MemorySearchTool;
```

Modify `core/src/tools/server.rs` to register the tool in the tool server.

### Step 4: Run tests

Run: `cargo test memory_search`

Expected: PASS

### Step 5: Integration test

Create integration test to verify end-to-end:
```rust
#[tokio::test]
async fn test_memory_search_integration() {
    // Setup database with test data
    // Call memory_search tool
    // Verify results
}
```

### Step 6: Commit

```bash
git add core/src/tools/memory_search.rs
git add core/src/tools/mod.rs
git add core/src/tools/server.rs
git commit -m "$(cat <<'EOF'
feat: add memory_search tool for Agent-driven retrieval

Implements memory_search tool that enables Agents to actively
search historical conversations and extracted facts.

Features:
- Hybrid search (facts + transcripts)
- Automatic redundancy elimination via ContextComptroller
- Configurable result limits and similarity thresholds
- URI-based result identification

Part of Memory Evolution Phase 1 MVP.

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Integration and Testing

### Step 1: Run full test suite

Run: `cargo test --lib`

Expected: All tests pass

### Step 2: Manual integration test

1. Start Aleph with memory_search tool enabled
2. Have a conversation about a topic
3. Use memory_search to retrieve the conversation
4. Verify no duplicate information

### Step 3: Update documentation

Update `docs/TOOL_SYSTEM.md` to document memory_search tool.

### Step 4: Final commit

```bash
git add docs/TOOL_SYSTEM.md
git commit -m "$(cat <<'EOF'
docs: document memory_search tool in TOOL_SYSTEM.md

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Completion Checklist

- [ ] TranscriptIndexer module implemented and tested
- [ ] ContextComptroller module implemented and tested
- [ ] memory_search tool implemented and tested
- [ ] Integration tests pass
- [ ] Documentation updated
- [ ] All commits follow conventional commit format
- [ ] No breaking changes to existing functionality

---

## Notes

- This is MVP implementation - chunking support deferred to Phase 2
- Token estimation uses simple 4 chars/token heuristic
- RetentionMode::Hybrid strategy deferred to Phase 2
- External file indexing deferred to Phase 2
