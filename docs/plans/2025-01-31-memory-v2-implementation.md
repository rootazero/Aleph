# Aether Memory v2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Upgrade Aether's memory system with signal-based compression, hybrid retrieval, dynamic association clustering, and memory decay.

**Architecture:** Five priority phases (P0-P4) with TDD approach. Each phase builds on the previous, with P0 (Signal Detector) and P1 (Hybrid Retrieval) parallelizable.

**Tech Stack:** Rust, rusqlite (FTS5), sqlite-vec, tokio, serde

**Reference Design:** `docs/plans/2025-01-31-memory-v2-design.md`

---

## Phase Overview

| Phase | Component | Files | Dependencies |
|-------|-----------|-------|--------------|
| P0 | Signal Detector + Smart Compression | signal_detector.rs, service.rs | None |
| P1 | FTS5 + Hybrid Retrieval | core.rs, hybrid.rs, strategy.rs | None (parallel with P0) |
| P2 | Structured Extraction + Conflict v2 | extractor.rs, conflict.rs, context.rs | P0 |
| P3 | Dynamic Association | association.rs | P1 |
| P4 | Memory Decay | decay.rs, facts.rs | P2 |

---

## Phase P0: Signal Detector + Smart Compression

### Task P0.1: Create Signal Types and Keywords

**Files:**
- Create: `core/src/memory/compression/signal_detector.rs`
- Modify: `core/src/memory/compression/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/memory/compression/signal_detector.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_learning_signal_detection() {
        let detector = SignalDetector::new();
        let result = detector.detect("记住，我喜欢用 Rust 写代码");

        assert!(result.should_compress);
        assert!(matches!(result.priority, CompressionPriority::Deferred));
        assert!(result.signals.iter().any(|s| matches!(s, CompressionSignal::Learning { .. })));
    }

    #[test]
    fn test_correction_signal_detection() {
        let detector = SignalDetector::new();
        let result = detector.detect("不对，我说的是 Python 不是 JavaScript");

        assert!(result.should_compress);
        assert!(matches!(result.priority, CompressionPriority::Immediate));
        assert!(result.signals.iter().any(|s| matches!(s, CompressionSignal::Correction { .. })));
    }

    #[test]
    fn test_milestone_signal_detection() {
        let detector = SignalDetector::new();
        let result = detector.detect("好了，这个功能终于完成了");

        assert!(result.should_compress);
        assert!(matches!(result.priority, CompressionPriority::Batch));
        assert!(result.signals.iter().any(|s| matches!(s, CompressionSignal::Milestone { .. })));
    }

    #[test]
    fn test_no_signal_for_normal_conversation() {
        let detector = SignalDetector::new();
        let result = detector.detect("今天天气怎么样？");

        assert!(!result.should_compress);
        assert!(result.signals.is_empty());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore signal_detector::tests --no-default-features`
Expected: FAIL with "cannot find module `signal_detector`"

**Step 3: Write minimal implementation**

```rust
// core/src/memory/compression/signal_detector.rs

//! Signal Detector for Smart Compression Triggers
//!
//! Detects learning, correction, milestone, and context-switch signals
//! in user messages to trigger intelligent memory compression.

use serde::{Deserialize, Serialize};

/// Compression signal types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionSignal {
    /// Learning signal: user expressing preference/rule
    Learning {
        trigger_phrase: String,
        confidence: f32,
    },
    /// Correction signal: user correcting AI misunderstanding
    Correction {
        original_understanding: String,
        corrected_to: String,
        confidence: f32,
    },
    /// Milestone signal: task/project completion
    Milestone {
        task_description: String,
        completion_indicator: String,
    },
    /// Context switch signal: topic change
    ContextSwitch {
        from_topic: String,
        to_topic: String,
    },
}

/// Compression priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionPriority {
    /// Immediate compression (correction signals)
    Immediate,
    /// Deferred compression (learning signals, wait for dialogue to stabilize)
    Deferred,
    /// Batch compression (milestone, context switch)
    Batch,
}

/// Signal detection result
#[derive(Debug, Clone, Default)]
pub struct DetectionResult {
    pub signals: Vec<CompressionSignal>,
    pub should_compress: bool,
    pub priority: CompressionPriority,
}

impl Default for CompressionPriority {
    fn default() -> Self {
        Self::Batch
    }
}

/// Signal keywords for each type
pub struct SignalKeywords {
    pub learning: Vec<&'static str>,
    pub correction: Vec<&'static str>,
    pub milestone: Vec<&'static str>,
}

impl Default for SignalKeywords {
    fn default() -> Self {
        Self {
            learning: vec![
                // Chinese
                "记住", "以后", "偏好", "喜欢用", "习惯", "总是", "一直",
                "我喜欢", "我讨厌", "我倾向", "默认用", "优先用",
                // English
                "remember", "always", "prefer", "I like", "I hate",
                "from now on", "by default", "going forward",
            ],
            correction: vec![
                // Chinese
                "不对", "搞错", "错了", "我说的是", "不是这个意思",
                "你理解错了", "应该是", "纠正一下",
                // English
                "wrong", "incorrect", "no,", "not what I meant",
                "I meant", "actually", "let me clarify",
            ],
            milestone: vec![
                // Chinese
                "完成", "搞定", "结束", "做完了", "好了", "成功",
                "告一段落", "收工",
                // English
                "done", "finished", "completed", "that's it",
                "wrap up", "all set",
            ],
        }
    }
}

/// Signal detector for smart compression triggers
pub struct SignalDetector {
    keywords: SignalKeywords,
}

impl SignalDetector {
    /// Create a new signal detector with default keywords
    pub fn new() -> Self {
        Self {
            keywords: SignalKeywords::default(),
        }
    }

    /// Create with custom keywords
    pub fn with_keywords(keywords: SignalKeywords) -> Self {
        Self { keywords }
    }

    /// Detect signals in user message
    pub fn detect(&self, message: &str) -> DetectionResult {
        let message_lower = message.to_lowercase();
        let mut signals = Vec::new();
        let mut highest_priority = CompressionPriority::Batch;

        // Check correction signals first (highest priority)
        for keyword in &self.keywords.correction {
            if message_lower.contains(&keyword.to_lowercase()) {
                signals.push(CompressionSignal::Correction {
                    original_understanding: String::new(), // To be filled by LLM
                    corrected_to: String::new(),
                    confidence: 0.8,
                });
                highest_priority = CompressionPriority::Immediate;
                break;
            }
        }

        // Check learning signals
        for keyword in &self.keywords.learning {
            if message_lower.contains(&keyword.to_lowercase()) {
                signals.push(CompressionSignal::Learning {
                    trigger_phrase: keyword.to_string(),
                    confidence: 0.7,
                });
                if highest_priority != CompressionPriority::Immediate {
                    highest_priority = CompressionPriority::Deferred;
                }
                break;
            }
        }

        // Check milestone signals
        for keyword in &self.keywords.milestone {
            if message_lower.contains(&keyword.to_lowercase()) {
                signals.push(CompressionSignal::Milestone {
                    task_description: String::new(), // To be filled by LLM
                    completion_indicator: keyword.to_string(),
                });
                // Keep existing priority if already set
                break;
            }
        }

        DetectionResult {
            should_compress: !signals.is_empty(),
            priority: highest_priority,
            signals,
        }
    }
}

impl Default for SignalDetector {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 4: Update mod.rs**

```rust
// core/src/memory/compression/mod.rs - add at top
pub mod signal_detector;

// Add to re-exports
pub use signal_detector::{
    CompressionPriority, CompressionSignal, DetectionResult, SignalDetector, SignalKeywords,
};
```

**Step 5: Run test to verify it passes**

Run: `cargo test -p aethecore signal_detector::tests --no-default-features`
Expected: PASS (4 tests)

**Step 6: Commit**

```bash
git add core/src/memory/compression/signal_detector.rs core/src/memory/compression/mod.rs
git commit -m "feat(memory): add signal detector for smart compression triggers

Implements keyword-based detection for:
- Learning signals (preferences, rules)
- Correction signals (user corrections)
- Milestone signals (task completion)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task P0.2: Integrate Signal Detector into Compression Service

**Files:**
- Modify: `core/src/memory/compression/service.rs`

**Step 1: Write the failing test**

```rust
// Add to core/src/memory/compression/service.rs tests module

#[tokio::test]
async fn test_signal_triggered_compression() {
    let (service, database) = create_test_service().await;

    // Store a memory with learning signal
    let context = ContextAnchor::now("test.app".to_string(), "test.txt".to_string());
    database.insert_memory(
        &MemoryEntry::new(
            "mem-1".to_string(),
            context,
            "记住，我喜欢用 Vim".to_string(),
            "好的，我记住了".to_string(),
        )
    ).await.unwrap();

    // Check with signal detection
    let message = "记住，我喜欢用 Vim";
    let result = service.check_and_compress_with_signal(message).await.unwrap();

    // Should trigger compression due to learning signal
    assert!(result.is_some());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_signal_triggered_compression --no-default-features`
Expected: FAIL with "method `check_and_compress_with_signal` not found"

**Step 3: Modify service.rs**

```rust
// core/src/memory/compression/service.rs

// Add import at top
use super::signal_detector::{CompressionPriority, DetectionResult, SignalDetector};

// Add field to CompressionService struct
pub struct CompressionService {
    // ... existing fields ...
    signal_detector: SignalDetector,
}

// Update new() method
impl CompressionService {
    pub fn new(
        database: Arc<VectorDatabase>,
        provider: Arc<dyn AiProvider>,
        embedder: SmartEmbedder,
        config: CompressionConfig,
    ) -> Self {
        // ... existing code ...

        Self {
            database,
            extractor,
            conflict_detector,
            scheduler,
            config,
            provider_name,
            signal_detector: SignalDetector::new(), // Add this
        }
    }

    /// Check for signal-based compression trigger
    pub async fn check_and_compress_with_signal(
        &self,
        user_message: &str,
    ) -> Result<Option<CompressionResult>, AetherError> {
        // Detect signals in message
        let detection = self.signal_detector.detect(user_message);

        if detection.should_compress {
            tracing::info!(
                signals = ?detection.signals,
                priority = ?detection.priority,
                "Signal-triggered compression"
            );

            match detection.priority {
                CompressionPriority::Immediate => {
                    // Compress immediately
                    let result = self.compress().await?;
                    Ok(Some(result))
                }
                CompressionPriority::Deferred => {
                    // Record turn and let scheduler decide
                    self.scheduler.increment_turns();
                    self.check_and_compress().await
                }
                CompressionPriority::Batch => {
                    // Just record activity, batch later
                    self.scheduler.record_activity();
                    Ok(None)
                }
            }
        } else {
            // Fall back to existing scheduler-based check
            self.check_and_compress().await
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_signal_triggered_compression --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/compression/service.rs
git commit -m "feat(memory): integrate signal detector into compression service

Adds check_and_compress_with_signal() method that:
- Detects learning/correction/milestone signals
- Triggers immediate/deferred/batch compression based on priority
- Falls back to scheduler-based check when no signal detected

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task P0.3: Add Context Switch Detection (Vector Distance)

**Files:**
- Modify: `core/src/memory/compression/signal_detector.rs`

**Step 1: Write the failing test**

```rust
// Add to signal_detector.rs tests

#[tokio::test]
async fn test_context_switch_detection() {
    let detector = SignalDetector::new();

    // Simulate previous embedding (about programming)
    let prev_embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5];

    // Current message about cooking (very different)
    let current_embedding = vec![0.9, 0.8, 0.7, 0.6, 0.5];

    let result = detector.detect_context_switch(&prev_embedding, &current_embedding, 0.5);

    assert!(result.is_some());
    assert!(matches!(result.unwrap(), CompressionSignal::ContextSwitch { .. }));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_context_switch_detection --no-default-features`
Expected: FAIL with "method `detect_context_switch` not found"

**Step 3: Add context switch detection**

```rust
// Add to SignalDetector impl in signal_detector.rs

impl SignalDetector {
    // ... existing methods ...

    /// Detect context switch based on embedding distance
    ///
    /// Returns Some(ContextSwitch) if the cosine distance exceeds the threshold
    pub fn detect_context_switch(
        &self,
        prev_embedding: &[f32],
        current_embedding: &[f32],
        threshold: f32,
    ) -> Option<CompressionSignal> {
        if prev_embedding.len() != current_embedding.len() {
            return None;
        }

        let distance = Self::cosine_distance(prev_embedding, current_embedding);

        if distance > threshold {
            Some(CompressionSignal::ContextSwitch {
                from_topic: String::new(), // To be summarized by LLM
                to_topic: String::new(),
            })
        } else {
            None
        }
    }

    /// Calculate cosine distance between two vectors
    fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 1.0; // Max distance for zero vectors
        }

        let similarity = dot / (norm_a * norm_b);
        1.0 - similarity // Convert similarity to distance
    }

    /// Combined detection with context switch
    pub fn detect_with_context(
        &self,
        message: &str,
        prev_embedding: Option<&[f32]>,
        current_embedding: Option<&[f32]>,
        context_switch_threshold: f32,
    ) -> DetectionResult {
        let mut result = self.detect(message);

        // Check for context switch if embeddings provided
        if let (Some(prev), Some(curr)) = (prev_embedding, current_embedding) {
            if let Some(switch_signal) = self.detect_context_switch(prev, curr, context_switch_threshold) {
                result.signals.push(switch_signal);
                if !result.should_compress {
                    result.should_compress = true;
                    result.priority = CompressionPriority::Batch;
                }
            }
        }

        result
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_context_switch_detection --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/compression/signal_detector.rs
git commit -m "feat(memory): add context switch detection via vector distance

Detects topic changes by calculating cosine distance between
previous and current message embeddings. Triggers batch compression
when distance exceeds threshold (default 0.5).

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase P1: FTS5 + Hybrid Retrieval

### Task P1.1: Add FTS5 Tables to Database Schema

**Files:**
- Modify: `core/src/memory/database/core.rs`

**Step 1: Write the failing test**

```rust
// Add to core/src/memory/database/core.rs tests

#[test]
fn test_fts5_tables_created() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).unwrap();

    let conn = db.conn.lock().unwrap();

    // Check memories_fts table exists
    let memories_fts_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memories_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(memories_fts_exists, "memories_fts table should exist");

    // Check facts_fts table exists
    let facts_fts_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='facts_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(facts_fts_exists, "facts_fts table should exist");
}

#[test]
fn test_fts5_sync_triggers_exist() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).unwrap();

    let conn = db.conn.lock().unwrap();

    // Check insert trigger exists
    let trigger_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='trigger' AND name='memories_fts_insert'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(trigger_exists, "memories_fts_insert trigger should exist");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_fts5_tables_created --no-default-features`
Expected: FAIL with "memories_fts table should exist"

**Step 3: Add FTS5 schema**

```rust
// Modify core/src/memory/database/core.rs
// Add to the CREATE TABLE batch in VectorDatabase::new()

// After the existing vec0 tables, add:

            -- ================================================================
            -- FTS5 Full-Text Search Tables (Hybrid Search)
            -- ================================================================

            -- Full-text index for memories
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                user_input,
                ai_output,
                id UNINDEXED,
                content='memories',
                content_rowid='rowid'
            );

            -- Full-text index for facts
            CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
                content,
                fact_type UNINDEXED,
                id UNINDEXED,
                content='memory_facts',
                content_rowid='rowid'
            );

            -- Sync trigger: memories insert
            CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, user_input, ai_output, id)
                VALUES (new.rowid, new.user_input, new.ai_output, new.id);
            END;

            -- Sync trigger: memories delete
            CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, user_input, ai_output, id)
                VALUES ('delete', old.rowid, old.user_input, old.ai_output, old.id);
            END;

            -- Sync trigger: facts insert
            CREATE TRIGGER IF NOT EXISTS facts_fts_insert AFTER INSERT ON memory_facts BEGIN
                INSERT INTO facts_fts(rowid, content, fact_type, id)
                VALUES (new.rowid, new.content, new.fact_type, new.id);
            END;

            -- Sync trigger: facts delete
            CREATE TRIGGER IF NOT EXISTS facts_fts_delete AFTER DELETE ON memory_facts BEGIN
                INSERT INTO facts_fts(facts_fts, rowid, content, fact_type, id)
                VALUES ('delete', old.rowid, old.content, old.fact_type, old.id);
            END;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_fts5_tables_created test_fts5_sync_triggers_exist --no-default-features`
Expected: PASS (2 tests)

**Step 5: Commit**

```bash
git add core/src/memory/database/core.rs
git commit -m "feat(memory): add FTS5 full-text search tables

Adds memories_fts and facts_fts virtual tables with:
- Automatic sync triggers for insert/delete
- Content sync with main tables via content= parameter
- BM25 ranking support for hybrid search

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task P1.2: Create Hybrid Retrieval Module

**Files:**
- Create: `core/src/memory/retrieval/mod.rs`
- Create: `core/src/memory/retrieval/hybrid.rs`
- Create: `core/src/memory/retrieval/strategy.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/memory/retrieval/hybrid.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hybrid_config_default() {
        let config = HybridSearchConfig::default();
        assert!((config.vector_weight - 0.7).abs() < 0.01);
        assert!((config.text_weight - 0.3).abs() < 0.01);
        assert!((config.min_score - 0.35).abs() < 0.01);
        assert_eq!(config.max_results, 10);
    }

    #[test]
    fn test_combined_score_calculation() {
        let config = HybridSearchConfig::default();

        // Vector score 0.8, text score 0.6
        let combined = config.calculate_combined_score(Some(0.8), Some(0.6));

        // 0.7 * 0.8 + 0.3 * 0.6 = 0.56 + 0.18 = 0.74
        assert!((combined - 0.74).abs() < 0.01);
    }

    #[test]
    fn test_combined_score_vector_only() {
        let config = HybridSearchConfig::default();
        let combined = config.calculate_combined_score(Some(0.8), None);

        // 0.7 * 0.8 + 0.0 = 0.56
        assert!((combined - 0.56).abs() < 0.01);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore hybrid::tests --no-default-features`
Expected: FAIL with "cannot find module"

**Step 3: Create the retrieval module structure**

```rust
// core/src/memory/retrieval/mod.rs

//! Memory Retrieval Module
//!
//! Provides hybrid search (vector + FTS5), layered retrieval strategies,
//! and dynamic association clustering.

pub mod hybrid;
pub mod strategy;

pub use hybrid::{HybridRetrieval, HybridSearchConfig};
pub use strategy::RetrievalStrategy;
```

```rust
// core/src/memory/retrieval/hybrid.rs

//! Hybrid Search Engine
//!
//! Combines vector similarity (sqlite-vec) with full-text search (FTS5 BM25)
//! for improved retrieval precision.

use serde::{Deserialize, Serialize};

/// Hybrid search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchConfig {
    /// Weight for vector similarity score (default: 0.7)
    pub vector_weight: f32,
    /// Weight for text/BM25 score (default: 0.3)
    pub text_weight: f32,
    /// Minimum combined score threshold (default: 0.35)
    pub min_score: f32,
    /// Maximum results to return (default: 10)
    pub max_results: usize,
    /// Candidate pool multiplier (default: 4)
    pub candidate_multiplier: usize,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            vector_weight: 0.7,
            text_weight: 0.3,
            min_score: 0.35,
            max_results: 10,
            candidate_multiplier: 4,
        }
    }
}

impl HybridSearchConfig {
    /// Calculate combined score from vector and text scores
    pub fn calculate_combined_score(
        &self,
        vector_score: Option<f32>,
        text_score: Option<f32>,
    ) -> f32 {
        let vs = vector_score.unwrap_or(0.0);
        let ts = text_score.unwrap_or(0.0);
        self.vector_weight * vs + self.text_weight * ts
    }
}

/// Hybrid retrieval engine
pub struct HybridRetrieval {
    config: HybridSearchConfig,
}

impl HybridRetrieval {
    /// Create a new hybrid retrieval engine
    pub fn new(config: HybridSearchConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(HybridSearchConfig::default())
    }

    /// Get current configuration
    pub fn config(&self) -> &HybridSearchConfig {
        &self.config
    }
}

impl Default for HybridRetrieval {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hybrid_config_default() {
        let config = HybridSearchConfig::default();
        assert!((config.vector_weight - 0.7).abs() < 0.01);
        assert!((config.text_weight - 0.3).abs() < 0.01);
        assert!((config.min_score - 0.35).abs() < 0.01);
        assert_eq!(config.max_results, 10);
    }

    #[test]
    fn test_combined_score_calculation() {
        let config = HybridSearchConfig::default();
        let combined = config.calculate_combined_score(Some(0.8), Some(0.6));
        assert!((combined - 0.74).abs() < 0.01);
    }

    #[test]
    fn test_combined_score_vector_only() {
        let config = HybridSearchConfig::default();
        let combined = config.calculate_combined_score(Some(0.8), None);
        assert!((combined - 0.56).abs() < 0.01);
    }

    #[test]
    fn test_combined_score_text_only() {
        let config = HybridSearchConfig::default();
        let combined = config.calculate_combined_score(None, Some(1.0));
        assert!((combined - 0.3).abs() < 0.01);
    }
}
```

```rust
// core/src/memory/retrieval/strategy.rs

//! Layered Retrieval Strategies
//!
//! Defines strategies for searching facts vs memories.

use serde::{Deserialize, Serialize};

/// Layered retrieval strategy
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum RetrievalStrategy {
    /// Only search Layer 2 (facts) - fast mode
    FactsOnly,
    /// Search facts first, then memories if not enough - default mode
    #[default]
    FactsFirst {
        /// Minimum facts required before searching memories
        min_facts: usize,
    },
    /// Search both layers simultaneously, merge results - deep mode
    BothLayers,
}

impl RetrievalStrategy {
    /// Create FactsFirst strategy with default min_facts
    pub fn facts_first() -> Self {
        Self::FactsFirst { min_facts: 3 }
    }

    /// Create FactsFirst with custom min_facts
    pub fn facts_first_with_min(min_facts: usize) -> Self {
        Self::FactsFirst { min_facts }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_strategy() {
        let strategy = RetrievalStrategy::default();
        assert!(matches!(strategy, RetrievalStrategy::FactsFirst { min_facts: 3 }));
    }

    #[test]
    fn test_facts_first_custom() {
        let strategy = RetrievalStrategy::facts_first_with_min(5);
        if let RetrievalStrategy::FactsFirst { min_facts } = strategy {
            assert_eq!(min_facts, 5);
        } else {
            panic!("Expected FactsFirst");
        }
    }
}
```

**Step 4: Update memory/mod.rs**

```rust
// core/src/memory/mod.rs - add after existing modules

pub mod retrieval_v2; // Renamed to avoid conflict with existing retrieval.rs

// In the file, we should rename to avoid conflict. Let's use a different approach:
// Keep existing retrieval.rs as is, add new module with different name
```

Actually, let's create the module properly:

```rust
// Modify core/src/memory/mod.rs - add line
pub mod hybrid_retrieval; // New hybrid retrieval module

// Add re-exports
pub use hybrid_retrieval::{HybridRetrieval, HybridSearchConfig, RetrievalStrategy};
```

Wait, the design says to create `retrieval/` directory. Let's stick with the original plan but rename to avoid conflict:

```rust
// Create core/src/memory/hybrid_retrieval/mod.rs instead
// This avoids conflict with existing retrieval.rs
```

**Step 4 (revised): Create hybrid_retrieval module**

```rust
// core/src/memory/hybrid_retrieval/mod.rs

//! Hybrid Memory Retrieval Module
//!
//! Provides hybrid search (vector + FTS5), layered retrieval strategies,
//! and dynamic association clustering.

pub mod hybrid;
pub mod strategy;

pub use hybrid::{HybridRetrieval, HybridSearchConfig};
pub use strategy::RetrievalStrategy;
```

Move the files:
- `core/src/memory/hybrid_retrieval/hybrid.rs`
- `core/src/memory/hybrid_retrieval/strategy.rs`

Update `core/src/memory/mod.rs`:

```rust
// Add after existing modules
pub mod hybrid_retrieval;

// Add re-exports
pub use hybrid_retrieval::{HybridRetrieval, HybridSearchConfig, RetrievalStrategy};
```

**Step 5: Run test to verify it passes**

Run: `cargo test -p aethecore hybrid::tests strategy::tests --no-default-features`
Expected: PASS (6 tests)

**Step 6: Commit**

```bash
git add core/src/memory/hybrid_retrieval/
git add core/src/memory/mod.rs
git commit -m "feat(memory): add hybrid retrieval module with FTS5 support

Adds hybrid_retrieval module with:
- HybridSearchConfig for vector/text weight tuning
- RetrievalStrategy enum (FactsOnly, FactsFirst, BothLayers)
- Combined score calculation (70% vector + 30% BM25)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task P1.3: Implement Hybrid Search SQL Query

**Files:**
- Modify: `core/src/memory/database/facts.rs`
- Modify: `core/src/memory/hybrid_retrieval/hybrid.rs`

**Step 1: Write the failing test**

```rust
// Add to hybrid.rs tests

#[tokio::test]
async fn test_hybrid_search_facts() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let database = Arc::new(VectorDatabase::new(db_path).unwrap());

    // Insert test facts
    let fact = MemoryFact::new(
        "The user prefers Rust for systems programming".to_string(),
        FactType::Preference,
        vec!["mem-1".to_string()],
    ).with_embedding(vec![0.1; 384]);

    database.insert_fact(fact).await.unwrap();

    let hybrid = HybridRetrieval::new(HybridSearchConfig::default(), database);

    // Search with query
    let query_embedding = vec![0.1; 384];
    let results = hybrid.search_facts(&query_embedding, "Rust programming").await.unwrap();

    assert!(!results.is_empty());
    assert!(results[0].content.contains("Rust"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_hybrid_search_facts --no-default-features`
Expected: FAIL with "method `search_facts` not found"

**Step 3: Implement hybrid search**

```rust
// Modify core/src/memory/hybrid_retrieval/hybrid.rs

use crate::error::AetherError;
use crate::memory::context::MemoryFact;
use crate::memory::database::VectorDatabase;
use std::sync::Arc;

/// Hybrid retrieval engine
pub struct HybridRetrieval {
    config: HybridSearchConfig,
    database: Arc<VectorDatabase>,
}

impl HybridRetrieval {
    /// Create a new hybrid retrieval engine
    pub fn new(config: HybridSearchConfig, database: Arc<VectorDatabase>) -> Self {
        Self { config, database }
    }

    /// Search facts using hybrid vector + FTS5 search
    pub async fn search_facts(
        &self,
        query_embedding: &[f32],
        query_text: &str,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        self.database
            .hybrid_search_facts(
                query_embedding,
                query_text,
                self.config.vector_weight,
                self.config.text_weight,
                self.config.min_score,
                self.config.max_results * self.config.candidate_multiplier,
                self.config.max_results,
            )
            .await
    }
}
```

```rust
// Add to core/src/memory/database/facts.rs

impl VectorDatabase {
    /// Hybrid search combining vector similarity and FTS5 BM25
    pub async fn hybrid_search_facts(
        &self,
        query_embedding: &[f32],
        query_text: &str,
        vector_weight: f32,
        text_weight: f32,
        min_score: f32,
        candidate_limit: usize,
        result_limit: usize,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        let embedding_bytes = Self::serialize_embedding(query_embedding);
        let conn = self.conn.lock().map_err(|e| {
            AetherError::config(format!("Failed to lock database: {}", e))
        })?;

        // Prepare FTS5 query (tokenize and AND together)
        let fts_query = Self::prepare_fts_query(query_text);

        let mut stmt = conn.prepare(
            r#"
            WITH vec_hits AS (
                SELECT rowid, distance FROM facts_vec
                WHERE embedding MATCH ?1
                ORDER BY distance
                LIMIT ?2
            ),
            fts_hits AS (
                SELECT rowid, bm25(facts_fts) as rank FROM facts_fts
                WHERE facts_fts MATCH ?3
                ORDER BY rank
                LIMIT ?2
            )
            SELECT
                f.id, f.content, f.fact_type, f.embedding, f.source_memory_ids,
                f.created_at, f.updated_at, f.confidence, f.is_valid, f.invalidation_reason,
                (COALESCE(?4 / (1.0 + v.distance), 0) +
                 COALESCE(?5 / (1.0 - COALESCE(fts.rank, -1000)), 0)) as combined_score
            FROM memory_facts f
            LEFT JOIN vec_hits v ON f.rowid = v.rowid
            LEFT JOIN fts_hits fts ON f.rowid = fts.rowid
            WHERE (v.rowid IS NOT NULL OR fts.rowid IS NOT NULL)
              AND f.is_valid = 1
            ORDER BY combined_score DESC
            LIMIT ?6
            "#,
        ).map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let facts: Vec<MemoryFact> = stmt
            .query_map(
                rusqlite::params![
                    embedding_bytes,
                    candidate_limit,
                    fts_query,
                    vector_weight,
                    text_weight,
                    result_limit,
                ],
                |row| {
                    let source_ids_json: String = row.get(4)?;
                    let source_ids: Vec<String> = serde_json::from_str(&source_ids_json)
                        .unwrap_or_default();

                    let embedding_bytes: Option<Vec<u8>> = row.get(3)?;
                    let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));

                    let combined_score: f64 = row.get(10)?;

                    Ok(MemoryFact {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        fact_type: crate::memory::context::FactType::from_str(&row.get::<_, String>(2)?),
                        embedding,
                        source_memory_ids: source_ids,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                        confidence: row.get(7)?,
                        is_valid: row.get::<_, i32>(8)? == 1,
                        invalidation_reason: row.get(9)?,
                        similarity_score: Some(combined_score as f32),
                    })
                },
            )
            .map_err(|e| AetherError::config(format!("Failed to execute query: {}", e)))?
            .filter_map(|r| r.ok())
            .filter(|f| f.similarity_score.unwrap_or(0.0) >= min_score)
            .collect();

        Ok(facts)
    }

    /// Prepare FTS5 query from natural language
    fn prepare_fts_query(text: &str) -> String {
        // Tokenize and create AND query
        // "rust programming" -> "rust" AND "programming"
        let tokens: Vec<&str> = text
            .split_whitespace()
            .filter(|t| t.len() > 1) // Skip single chars
            .collect();

        if tokens.is_empty() {
            return "*".to_string(); // Match all if no valid tokens
        }

        tokens
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" AND ")
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_hybrid_search_facts --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/database/facts.rs core/src/memory/hybrid_retrieval/hybrid.rs
git commit -m "feat(memory): implement hybrid search with vector + FTS5

Adds hybrid_search_facts() combining:
- sqlite-vec vector similarity (70% weight)
- FTS5 BM25 text ranking (30% weight)
- Combined score filtering and ranking

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase P2: Structured Extraction + Conflict v2

### Task P2.1: Add Specificity and TemporalScope to MemoryFact

**Files:**
- Modify: `core/src/memory/context.rs`
- Modify: `core/src/memory/database/core.rs`

**Step 1: Write the failing test**

```rust
// Add to context.rs tests

#[test]
fn test_fact_specificity() {
    let fact = MemoryFact::new(
        "User prefers Rust".to_string(),
        FactType::Preference,
        vec!["mem-1".to_string()],
    )
    .with_specificity(FactSpecificity::Pattern)
    .with_temporal_scope(TemporalScope::Permanent);

    assert_eq!(fact.specificity, FactSpecificity::Pattern);
    assert_eq!(fact.temporal_scope, TemporalScope::Permanent);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_fact_specificity --no-default-features`
Expected: FAIL with "cannot find type `FactSpecificity`"

**Step 3: Add new types to context.rs**

```rust
// Add to core/src/memory/context.rs after FactType

/// Fact specificity level (prevents too vague or too detailed facts)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FactSpecificity {
    /// Principle level: "User prefers functional programming"
    Principle,
    /// Pattern level: "User uses Result instead of panic for error handling"
    #[default]
    Pattern,
    /// Instance level: "User used anyhow in 2025-01-15 project"
    Instance,
}

impl FactSpecificity {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Principle => "principle",
            Self::Pattern => "pattern",
            Self::Instance => "instance",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "principle" => Self::Principle,
            "instance" => Self::Instance,
            _ => Self::Pattern,
        }
    }
}

/// Temporal scope of a fact
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TemporalScope {
    /// Long-term valid: "User's native language is Chinese"
    Permanent,
    /// Context-related: "User is working on Aether project"
    #[default]
    Contextual,
    /// Short-term valid: "User wants to focus on docs today"
    Ephemeral,
}

impl TemporalScope {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Permanent => "permanent",
            Self::Contextual => "contextual",
            Self::Ephemeral => "ephemeral",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "permanent" => Self::Permanent,
            "ephemeral" => Self::Ephemeral,
            _ => Self::Contextual,
        }
    }
}

// Update MemoryFact struct
pub struct MemoryFact {
    // ... existing fields ...

    /// Fact specificity level
    pub specificity: FactSpecificity,
    /// Temporal scope
    pub temporal_scope: TemporalScope,
}

// Update MemoryFact impl
impl MemoryFact {
    pub fn new(content: String, fact_type: FactType, source_ids: Vec<String>) -> Self {
        // ... existing code ...
        Self {
            // ... existing fields ...
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
        }
    }

    /// Set specificity
    pub fn with_specificity(mut self, specificity: FactSpecificity) -> Self {
        self.specificity = specificity;
        self
    }

    /// Set temporal scope
    pub fn with_temporal_scope(mut self, scope: TemporalScope) -> Self {
        self.temporal_scope = scope;
        self
    }
}
```

**Step 4: Update database schema (core.rs)**

```sql
-- Add to memory_facts table creation (in execute_batch)
-- After existing columns:

-- Note: For existing databases, we need migration
-- ALTER TABLE memory_facts ADD COLUMN specificity TEXT DEFAULT 'pattern';
-- ALTER TABLE memory_facts ADD COLUMN temporal_scope TEXT DEFAULT 'contextual';
```

**Step 5: Run test to verify it passes**

Run: `cargo test -p aethecore test_fact_specificity --no-default-features`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/memory/context.rs core/src/memory/database/core.rs
git commit -m "feat(memory): add FactSpecificity and TemporalScope to MemoryFact

Adds structured fact metadata:
- FactSpecificity: Principle | Pattern | Instance
- TemporalScope: Permanent | Contextual | Ephemeral

Enables filtering out vague facts and managing temporal validity.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task P2.2: Upgrade Conflict Resolution to Three Strategies

**Files:**
- Modify: `core/src/memory/compression/conflict.rs`

**Step 1: Write the failing test**

```rust
// Add to conflict.rs tests

#[test]
fn test_merge_strategy() {
    let resolution = ConflictResolution::Merge {
        old_id: "fact-1".to_string(),
        new_content: "User likes Rust and Go".to_string(),
        merge_strategy: MergeStrategy::Enumerate,
    };

    assert!(matches!(resolution, ConflictResolution::Merge { .. }));
}

#[test]
fn test_reject_strategy() {
    let resolution = ConflictResolution::Reject {
        rejected_content: "User dislikes Rust".to_string(),
        reason: "Contradicts high-confidence fact".to_string(),
    };

    assert!(matches!(resolution, ConflictResolution::Reject { .. }));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_merge_strategy --no-default-features`
Expected: FAIL with "variant `Merge` not found"

**Step 3: Update conflict.rs**

```rust
// core/src/memory/compression/conflict.rs

/// Result of conflict resolution
#[derive(Debug, Clone)]
pub enum ConflictResolution {
    /// No conflict detected
    NoConflict,
    /// Override: new fact replaces old (default for correction signals)
    Override {
        invalidated_id: String,
        reason: String,
    },
    /// Reject: keep old fact, discard new (confidence comparison)
    Reject {
        rejected_content: String,
        reason: String,
    },
    /// Merge: combine into more precise statement
    Merge {
        old_id: String,
        new_content: String,
        merge_strategy: MergeStrategy,
    },
}

/// Strategy for merging facts
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Generalize: "likes Rust" + "likes Go" → "likes systems languages"
    Generalize,
    /// Specialize: "likes coffee" + "likes dark roast" → "likes dark roast coffee"
    Specialize,
    /// Enumerate: "likes Rust, Go, and Zig"
    Enumerate,
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_merge_strategy test_reject_strategy --no-default-features`
Expected: PASS (2 tests)

**Step 5: Commit**

```bash
git add core/src/memory/compression/conflict.rs
git commit -m "feat(memory): add three-way conflict resolution

Upgrades ConflictResolution enum with:
- Override: new fact replaces old
- Reject: keep old, discard new
- Merge: combine with Generalize/Specialize/Enumerate strategies

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase P3: Dynamic Association Clustering

### Task P3.1: Create Association Module

**Files:**
- Create: `core/src/memory/hybrid_retrieval/association.rs`
- Modify: `core/src/memory/hybrid_retrieval/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/memory/hybrid_retrieval/association.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_association_config_default() {
        let config = AssociationConfig::default();
        assert!((config.expansion_radius - 0.4).abs() < 0.01);
        assert_eq!(config.max_associations, 5);
        assert_eq!(config.min_cluster_size, 2);
    }

    #[test]
    fn test_cluster_creation() {
        let center = MemoryFact::new(
            "User likes Rust".to_string(),
            FactType::Preference,
            vec![],
        );

        let related = vec![
            MemoryFact::new("User uses Cargo".to_string(), FactType::Learning, vec![]),
        ];

        let cluster = AssociationCluster {
            center_fact: center,
            related_facts: related,
            cluster_theme: None,
            avg_similarity: 0.85,
        };

        assert_eq!(cluster.related_facts.len(), 1);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore association::tests --no-default-features`
Expected: FAIL with "cannot find module"

**Step 3: Create association.rs**

```rust
// core/src/memory/hybrid_retrieval/association.rs

//! Dynamic Association Clustering
//!
//! Finds related facts at query time without pre-stored clusters.

use crate::memory::context::MemoryFact;
use serde::{Deserialize, Serialize};

/// Association cluster result
#[derive(Debug, Clone)]
pub struct AssociationCluster {
    /// Cluster center (most relevant fact)
    pub center_fact: MemoryFact,
    /// Related facts in the cluster
    pub related_facts: Vec<MemoryFact>,
    /// LLM-generated theme label (optional)
    pub cluster_theme: Option<String>,
    /// Average similarity within cluster
    pub avg_similarity: f32,
}

/// Association retriever configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssociationConfig {
    /// Vector space expansion radius (default: 0.4)
    pub expansion_radius: f32,
    /// Maximum associations to return (default: 5)
    pub max_associations: usize,
    /// Minimum cluster size (default: 2)
    pub min_cluster_size: usize,
    /// Whether to generate theme labels (default: false)
    pub generate_theme: bool,
}

impl Default for AssociationConfig {
    fn default() -> Self {
        Self {
            expansion_radius: 0.4,
            max_associations: 5,
            min_cluster_size: 2,
            generate_theme: false,
        }
    }
}

/// Dynamic association retriever
pub struct AssociationRetriever {
    config: AssociationConfig,
}

impl AssociationRetriever {
    /// Create a new association retriever
    pub fn new(config: AssociationConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(AssociationConfig::default())
    }

    /// Get current configuration
    pub fn config(&self) -> &AssociationConfig {
        &self.config
    }
}

impl Default for AssociationRetriever {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;

    #[test]
    fn test_association_config_default() {
        let config = AssociationConfig::default();
        assert!((config.expansion_radius - 0.4).abs() < 0.01);
        assert_eq!(config.max_associations, 5);
        assert_eq!(config.min_cluster_size, 2);
    }

    #[test]
    fn test_cluster_creation() {
        let center = MemoryFact::new(
            "User likes Rust".to_string(),
            FactType::Preference,
            vec![],
        );

        let related = vec![
            MemoryFact::new("User uses Cargo".to_string(), FactType::Learning, vec![]),
        ];

        let cluster = AssociationCluster {
            center_fact: center,
            related_facts: related,
            cluster_theme: None,
            avg_similarity: 0.85,
        };

        assert_eq!(cluster.related_facts.len(), 1);
    }
}
```

**Step 4: Update mod.rs**

```rust
// core/src/memory/hybrid_retrieval/mod.rs

pub mod association;
pub mod hybrid;
pub mod strategy;

pub use association::{AssociationCluster, AssociationConfig, AssociationRetriever};
pub use hybrid::{HybridRetrieval, HybridSearchConfig};
pub use strategy::RetrievalStrategy;
```

**Step 5: Run test to verify it passes**

Run: `cargo test -p aethecore association::tests --no-default-features`
Expected: PASS (2 tests)

**Step 6: Commit**

```bash
git add core/src/memory/hybrid_retrieval/association.rs core/src/memory/hybrid_retrieval/mod.rs
git commit -m "feat(memory): add dynamic association clustering module

Adds AssociationRetriever for query-time clustering:
- AssociationCluster: center fact + related facts
- AssociationConfig: expansion_radius, max_associations, min_cluster_size
- Zero storage overhead (computed at query time)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase P4: Memory Decay

### Task P4.1: Create Decay Module

**Files:**
- Create: `core/src/memory/decay.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Write the failing test**

```rust
// core/src/memory/decay.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decay_config_default() {
        let config = DecayConfig::default();
        assert!((config.half_life_days - 30.0).abs() < 0.01);
        assert!((config.access_boost - 0.2).abs() < 0.01);
        assert!((config.min_strength - 0.1).abs() < 0.01);
    }

    #[test]
    fn test_strength_calculation_no_decay() {
        let config = DecayConfig::default();
        let now = 1000000;

        let strength = MemoryStrength {
            access_count: 0,
            last_accessed: now,
            creation_time: now,
        };

        let score = strength.calculate_strength(&config, now);
        assert!((score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_strength_calculation_with_decay() {
        let config = DecayConfig::default();
        let now = 1000000;
        let thirty_days_ago = now - (30 * 86400);

        let strength = MemoryStrength {
            access_count: 0,
            last_accessed: thirty_days_ago,
            creation_time: thirty_days_ago,
        };

        let score = strength.calculate_strength(&config, now);
        // After one half-life, score should be ~0.5
        assert!((score - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_strength_with_access_boost() {
        let config = DecayConfig::default();
        let now = 1000000;
        let thirty_days_ago = now - (30 * 86400);

        let strength = MemoryStrength {
            access_count: 5, // 5 accesses = 1.0 boost
            last_accessed: thirty_days_ago,
            creation_time: thirty_days_ago,
        };

        let score = strength.calculate_strength(&config, now);
        // 0.5 * (1 + 1.0) = 1.0
        assert!((score - 1.0).abs() < 0.1);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore decay::tests --no-default-features`
Expected: FAIL with "cannot find module"

**Step 3: Create decay.rs**

```rust
// core/src/memory/decay.rs

//! Memory Decay Mechanism
//!
//! Implements Ebbinghaus forgetting curve for memory lifecycle management.

use crate::memory::context::FactType;
use serde::{Deserialize, Serialize};

/// Memory strength tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStrength {
    /// Number of times retrieved/hit
    pub access_count: u32,
    /// Last access timestamp (Unix seconds)
    pub last_accessed: i64,
    /// Creation timestamp (Unix seconds)
    pub creation_time: i64,
}

impl MemoryStrength {
    /// Create new strength record
    pub fn new(creation_time: i64) -> Self {
        Self {
            access_count: 0,
            last_accessed: creation_time,
            creation_time,
        }
    }

    /// Calculate current strength (Ebbinghaus curve simplified)
    pub fn calculate_strength(&self, config: &DecayConfig, now: i64) -> f32 {
        let days_since_access = (now - self.last_accessed) as f32 / 86400.0;

        // Base decay: exponential decay curve
        // strength = 0.5 ^ (days / half_life)
        let base_decay = 0.5_f32.powf(days_since_access / config.half_life_days);

        // Access boost: each access adds boost, capped at 2.0
        let access_boost = (self.access_count as f32 * config.access_boost).min(2.0);

        // Final strength = base_decay * (1 + access_boost), capped at 1.0
        (base_decay * (1.0 + access_boost)).min(1.0)
    }

    /// Record an access (increment count, update timestamp)
    pub fn record_access(&mut self, now: i64) {
        self.access_count += 1;
        self.last_accessed = now;
    }
}

/// Decay configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayConfig {
    /// Half-life in days (default: 30)
    pub half_life_days: f32,
    /// Strength boost per access (default: 0.2)
    pub access_boost: f32,
    /// Minimum strength before cleanup (default: 0.1)
    pub min_strength: f32,
    /// Fact types that never decay
    pub protected_types: Vec<FactType>,
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            half_life_days: 30.0,
            access_boost: 0.2,
            min_strength: 0.1,
            protected_types: vec![FactType::Personal],
        }
    }
}

impl DecayConfig {
    /// Get effective half-life for a fact type
    pub fn effective_half_life(&self, fact_type: &FactType) -> f32 {
        match fact_type {
            FactType::Preference => self.half_life_days * 2.0, // More durable
            FactType::Personal => f32::INFINITY,               // Never decay
            _ => self.half_life_days,
        }
    }

    /// Check if a fact type is protected from decay
    pub fn is_protected(&self, fact_type: &FactType) -> bool {
        self.protected_types.contains(fact_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decay_config_default() {
        let config = DecayConfig::default();
        assert!((config.half_life_days - 30.0).abs() < 0.01);
        assert!((config.access_boost - 0.2).abs() < 0.01);
        assert!((config.min_strength - 0.1).abs() < 0.01);
    }

    #[test]
    fn test_strength_calculation_no_decay() {
        let config = DecayConfig::default();
        let now = 1000000;

        let strength = MemoryStrength {
            access_count: 0,
            last_accessed: now,
            creation_time: now,
        };

        let score = strength.calculate_strength(&config, now);
        assert!((score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_strength_calculation_with_decay() {
        let config = DecayConfig::default();
        let now = 1000000;
        let thirty_days_ago = now - (30 * 86400);

        let strength = MemoryStrength {
            access_count: 0,
            last_accessed: thirty_days_ago,
            creation_time: thirty_days_ago,
        };

        let score = strength.calculate_strength(&config, now);
        assert!((score - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_strength_with_access_boost() {
        let config = DecayConfig::default();
        let now = 1000000;
        let thirty_days_ago = now - (30 * 86400);

        let strength = MemoryStrength {
            access_count: 5,
            last_accessed: thirty_days_ago,
            creation_time: thirty_days_ago,
        };

        let score = strength.calculate_strength(&config, now);
        assert!((score - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_preference_has_longer_half_life() {
        let config = DecayConfig::default();
        let half_life = config.effective_half_life(&FactType::Preference);
        assert!((half_life - 60.0).abs() < 0.01);
    }

    #[test]
    fn test_personal_never_decays() {
        let config = DecayConfig::default();
        assert!(config.is_protected(&FactType::Personal));
    }
}
```

**Step 4: Update mod.rs**

```rust
// core/src/memory/mod.rs - add
pub mod decay;
pub use decay::{DecayConfig, MemoryStrength};
```

**Step 5: Run test to verify it passes**

Run: `cargo test -p aethecore decay::tests --no-default-features`
Expected: PASS (6 tests)

**Step 6: Commit**

```bash
git add core/src/memory/decay.rs core/src/memory/mod.rs
git commit -m "feat(memory): add Ebbinghaus-curve memory decay mechanism

Implements memory decay with:
- MemoryStrength: access_count, last_accessed, strength calculation
- DecayConfig: half_life_days, access_boost, min_strength
- Type-based protection (Personal never decays, Preference 2x half-life)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Summary: All Tasks

| Task | Description | Status |
|------|-------------|--------|
| P0.1 | Signal types and keywords | Pending |
| P0.2 | Integrate signal detector into service | Pending |
| P0.3 | Context switch detection | Pending |
| P1.1 | FTS5 tables schema | Pending |
| P1.2 | Hybrid retrieval module | Pending |
| P1.3 | Hybrid search SQL | Pending |
| P2.1 | FactSpecificity + TemporalScope | Pending |
| P2.2 | Three-way conflict resolution | Pending |
| P3.1 | Association clustering module | Pending |
| P4.1 | Decay mechanism | Pending |

---

## Test Commands Summary

```bash
# Run all memory tests
cargo test -p aethecore --lib memory:: --no-default-features

# Run specific phase tests
cargo test -p aethecore signal_detector --no-default-features     # P0
cargo test -p aethecore hybrid --no-default-features              # P1
cargo test -p aethecore association --no-default-features         # P3
cargo test -p aethecore decay --no-default-features               # P4

# Run with all features
cargo test -p aethecore --all-features
```

---

## Notes for Implementer

1. **TDD Discipline**: Always write the failing test first, then implement
2. **Small Commits**: One task = one commit, clear message
3. **Backward Compatibility**: Existing `retrieval.rs` and `MemoryRetrieval` must keep working
4. **Database Migration**: For existing databases, schema changes need migration logic
5. **Feature Flags**: Consider gating new features behind cargo features for gradual rollout
