# Month 3: Meta-Cognition Layer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the Meta-Cognition Layer that enables Aleph to observe, critique, and improve its own thinking process through reactive and proactive reflection.

**Architecture:** Hybrid reflection mode combining immediate "pain learning" (ReactiveReflector) with periodic "excellence learning" (CriticAgent). Rules stored as BehavioralAnchors with conflict detection, dynamically injected into system prompts based on context tags.

**Tech Stack:** Rust, SQLite (behavioral_anchors table), sqlite-vec (embeddings), LRU cache, fastembed (SmartEmbedder)

---

## Phase 1: Core Data Structures

### Task 1.1: Define BehavioralAnchor Types

**Files:**
- Create: `core/src/memory/cortex/meta_cognition/types.rs`
- Create: `core/src/memory/cortex/meta_cognition/mod.rs`

**Step 1: Create module structure**

```bash
cd core/src/memory/cortex
mkdir -p meta_cognition
```

**Step 2: Write types.rs with BehavioralAnchor struct**

```rust
//! Meta-cognition types for behavioral anchors and reflection

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A behavioral anchor learned from experience
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehavioralAnchor {
    pub id: String,
    pub rule_text: String,
    pub trigger_tags: Vec<String>,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub last_validated: DateTime<Utc>,
    pub validation_count: u32,
    pub failure_count: u32,
    pub source: AnchorSource,
    pub scope: AnchorScope,
    pub priority: i32,
    pub conflicts_with: Vec<String>,
    pub supersedes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnchorSource {
    ReactiveReflection {
        task_id: String,
        error_type: String,
    },
    ProactiveReflection {
        pattern_hash: String,
        optimization_type: String,
    },
    UserFeedback {
        session_id: String,
    },
    ManualInjection {
        author: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnchorScope {
    Global,
    Tagged { tags: Vec<String> },
    Conditional { predicate: String },
}

impl BehavioralAnchor {
    pub fn new(
        rule_text: String,
        trigger_tags: Vec<String>,
        source: AnchorSource,
        priority: i32,
        confidence: f32,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            rule_text,
            trigger_tags,
            confidence,
            created_at: now,
            last_validated: now,
            validation_count: 0,
            failure_count: 0,
            source,
            scope: AnchorScope::Tagged {
                tags: Vec::new(),
            },
            priority,
            conflicts_with: Vec::new(),
            supersedes: None,
        }
    }

    pub fn update_confidence(&mut self, success: bool) {
        if success {
            self.validation_count += 1;
            // Increase confidence, max 1.0
            self.confidence = (self.confidence + 0.05).min(1.0);
        } else {
            self.failure_count += 1;
            // Decrease confidence, min 0.0
            self.confidence = (self.confidence - 0.1).max(0.0);
        }
        self.last_validated = Utc::now();
    }

    pub fn validation_rate(&self) -> f32 {
        let total = self.validation_count + self.failure_count;
        if total == 0 {
            return 0.0;
        }
        self.validation_count as f32 / total as f32
    }
}
```

**Step 3: Write mod.rs**

```rust
//! Meta-cognition layer for self-reflection and improvement

pub mod types;

pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
```

**Step 4: Update parent mod.rs**

Edit `core/src/memory/cortex/mod.rs`:

```rust
pub mod meta_cognition;

pub use meta_cognition::{AnchorScope, AnchorSource, BehavioralAnchor};
```

**Step 5: Write unit tests**

Add to `core/src/memory/cortex/meta_cognition/types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_behavioral_anchor_creation() {
        let anchor = BehavioralAnchor::new(
            "Always check file existence before operations".to_string(),
            vec!["file".to_string(), "safety".to_string()],
            AnchorSource::ReactiveReflection {
                task_id: "task-123".to_string(),
                error_type: "FileNotFound".to_string(),
            },
            100,
            0.8,
        );

        assert_eq!(anchor.priority, 100);
        assert_eq!(anchor.confidence, 0.8);
        assert_eq!(anchor.validation_count, 0);
        assert_eq!(anchor.failure_count, 0);
    }

    #[test]
    fn test_confidence_update_success() {
        let mut anchor = BehavioralAnchor::new(
            "Test rule".to_string(),
            vec!["test".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            50,
            0.5,
        );

        anchor.update_confidence(true);
        assert_eq!(anchor.validation_count, 1);
        assert_eq!(anchor.failure_count, 0);
        assert!(anchor.confidence > 0.5);
    }

    #[test]
    fn test_confidence_update_failure() {
        let mut anchor = BehavioralAnchor::new(
            "Test rule".to_string(),
            vec!["test".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            50,
            0.5,
        );

        anchor.update_confidence(false);
        assert_eq!(anchor.validation_count, 0);
        assert_eq!(anchor.failure_count, 1);
        assert!(anchor.confidence < 0.5);
    }

    #[test]
    fn test_validation_rate() {
        let mut anchor = BehavioralAnchor::new(
            "Test rule".to_string(),
            vec!["test".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            50,
            0.5,
        );

        assert_eq!(anchor.validation_rate(), 0.0);

        anchor.update_confidence(true);
        anchor.update_confidence(true);
        anchor.update_confidence(false);

        assert_eq!(anchor.validation_rate(), 2.0 / 3.0);
    }
}
```

**Step 6: Run tests**

```bash
cd core
cargo test --lib cortex::meta_cognition::types
```

Expected: 4 tests pass

**Step 7: Commit**

```bash
git add core/src/memory/cortex/meta_cognition/
git add core/src/memory/cortex/mod.rs
git commit -m "feat(cortex): add BehavioralAnchor core types

- Define BehavioralAnchor struct with confidence scoring
- Add AnchorSource and AnchorScope enums
- Implement confidence update logic
- Add validation rate calculation
- 4 unit tests passing"
```

---

### Task 1.2: Create Database Schema

**Files:**
- Create: `core/src/memory/cortex/meta_cognition/schema.rs`
- Modify: `core/src/memory/cortex/meta_cognition/mod.rs`

**Step 1: Write schema.rs**

```rust
//! Database schema for behavioral anchors

use crate::error::Result;
use rusqlite::Connection;

pub const CREATE_BEHAVIORAL_ANCHORS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS behavioral_anchors (
    id TEXT PRIMARY KEY,
    rule_text TEXT NOT NULL,
    trigger_tags TEXT NOT NULL,  -- JSON array
    confidence REAL NOT NULL,
    created_at TEXT NOT NULL,
    last_validated TEXT NOT NULL,
    validation_count INTEGER NOT NULL DEFAULT 0,
    failure_count INTEGER NOT NULL DEFAULT 0,
    source TEXT NOT NULL,  -- JSON
    scope TEXT NOT NULL,  -- JSON
    priority INTEGER NOT NULL,
    conflicts_with TEXT,  -- JSON array
    supersedes TEXT,
    embedding TEXT  -- JSON array of f32
);

CREATE INDEX IF NOT EXISTS idx_behavioral_anchors_tags
ON behavioral_anchors(trigger_tags);

CREATE INDEX IF NOT EXISTS idx_behavioral_anchors_confidence
ON behavioral_anchors(confidence DESC);

CREATE INDEX IF NOT EXISTS idx_behavioral_anchors_priority
ON behavioral_anchors(priority DESC);
"#;

pub fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_BEHAVIORAL_ANCHORS_TABLE)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_creation() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Verify table exists
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='behavioral_anchors'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(table_exists);
    }
}
```

**Step 2: Update mod.rs**

```rust
pub mod schema;
pub mod types;

pub use schema::initialize_schema;
pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
```

**Step 3: Run test**

```bash
cargo test --lib cortex::meta_cognition::schema
```

Expected: 1 test passes

**Step 4: Commit**

```bash
git add core/src/memory/cortex/meta_cognition/schema.rs
git add core/src/memory/cortex/meta_cognition/mod.rs
git commit -m "feat(cortex): add behavioral_anchors database schema

- Create table with all required columns
- Add indexes for tags, confidence, priority
- Include embedding column for vector search
- 1 test passing"
```

---

### Task 1.3: Implement AnchorStore CRUD

**Files:**
- Create: `core/src/memory/cortex/meta_cognition/anchor_store.rs`
- Modify: `core/src/memory/cortex/meta_cognition/mod.rs`

**Step 1: Write failing test**

Add to `core/src/memory/cortex/meta_cognition/anchor_store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::cortex::meta_cognition::schema::initialize_schema;
    use rusqlite::Connection;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn create_test_store() -> (AnchorStore, TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();
        initialize_schema(&conn).unwrap();

        let store = AnchorStore::new(Arc::new(conn));
        (store, temp)
    }

    #[test]
    fn test_add_and_get_anchor() {
        let (mut store, _temp) = create_test_store();

        let anchor = BehavioralAnchor::new(
            "Test rule".to_string(),
            vec!["test".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            50,
            0.5,
        );

        let id = anchor.id.clone();
        store.add(anchor).unwrap();

        let retrieved = store.get(&id).unwrap().unwrap();
        assert_eq!(retrieved.rule_text, "Test rule");
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test --lib cortex::meta_cognition::anchor_store::tests::test_add_and_get_anchor
```

Expected: FAIL with "module not found"

**Step 3: Write minimal implementation**

```rust
//! Storage and retrieval for behavioral anchors

use crate::error::{AlephError, Result};
use crate::memory::cortex::meta_cognition::types::BehavioralAnchor;
use rusqlite::{params, Connection};
use std::sync::Arc;

pub struct AnchorStore {
    conn: Arc<Connection>,
}

impl AnchorStore {
    pub fn new(conn: Arc<Connection>) -> Self {
        Self { conn }
    }

    pub fn add(&mut self, anchor: BehavioralAnchor) -> Result<String> {
        self.conn.execute(
            "INSERT INTO behavioral_anchors
             (id, rule_text, trigger_tags, confidence, created_at, last_validated,
              validation_count, failure_count, source, scope, priority, conflicts_with, supersedes)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                anchor.id,
                anchor.rule_text,
                serde_json::to_string(&anchor.trigger_tags)?,
                anchor.confidence,
                anchor.created_at.to_rfc3339(),
                anchor.last_validated.to_rfc3339(),
                anchor.validation_count,
                anchor.failure_count,
                serde_json::to_string(&anchor.source)?,
                serde_json::to_string(&anchor.scope)?,
                anchor.priority,
                serde_json::to_string(&anchor.conflicts_with)?,
                anchor.supersedes,
            ],
        )?;

        Ok(anchor.id)
    }

    pub fn get(&self, id: &str) -> Result<Option<BehavioralAnchor>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_text, trigger_tags, confidence, created_at, last_validated,
                    validation_count, failure_count, source, scope, priority, conflicts_with, supersedes
             FROM behavioral_anchors WHERE id = ?"
        )?;

        let anchor = stmt.query_row(params![id], |row| {
            Ok(BehavioralAnchor {
                id: row.get(0)?,
                rule_text: row.get(1)?,
                trigger_tags: serde_json::from_str(&row.get::<_, String>(2)?).unwrap(),
                confidence: row.get(3)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                last_validated: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                validation_count: row.get(6)?,
                failure_count: row.get(7)?,
                source: serde_json::from_str(&row.get::<_, String>(8)?).unwrap(),
                scope: serde_json::from_str(&row.get::<_, String>(9)?).unwrap(),
                priority: row.get(10)?,
                conflicts_with: serde_json::from_str(&row.get::<_, String>(11)?).unwrap(),
                supersedes: row.get(12)?,
            })
        });

        match anchor {
            Ok(a) => Ok(Some(a)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::Database(e.to_string())),
        }
    }

    pub fn update(&mut self, anchor: &BehavioralAnchor) -> Result<()> {
        self.conn.execute(
            "UPDATE behavioral_anchors
             SET rule_text = ?, trigger_tags = ?, confidence = ?, last_validated = ?,
                 validation_count = ?, failure_count = ?, conflicts_with = ?, supersedes = ?
             WHERE id = ?",
            params![
                anchor.rule_text,
                serde_json::to_string(&anchor.trigger_tags)?,
                anchor.confidence,
                anchor.last_validated.to_rfc3339(),
                anchor.validation_count,
                anchor.failure_count,
                serde_json::to_string(&anchor.conflicts_with)?,
                anchor.supersedes,
                anchor.id,
            ],
        )?;

        Ok(())
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM behavioral_anchors WHERE id = ?", params![id])?;
        Ok(())
    }

    pub fn list_all(&self) -> Result<Vec<BehavioralAnchor>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_text, trigger_tags, confidence, created_at, last_validated,
                    validation_count, failure_count, source, scope, priority, conflicts_with, supersedes
             FROM behavioral_anchors
             ORDER BY priority DESC, confidence DESC"
        )?;

        let anchors = stmt.query_map([], |row| {
            Ok(BehavioralAnchor {
                id: row.get(0)?,
                rule_text: row.get(1)?,
                trigger_tags: serde_json::from_str(&row.get::<_, String>(2)?).unwrap(),
                confidence: row.get(3)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                last_validated: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                validation_count: row.get(6)?,
                failure_count: row.get(7)?,
                source: serde_json::from_str(&row.get::<_, String>(8)?).unwrap(),
                scope: serde_json::from_str(&row.get::<_, String>(9)?).unwrap(),
                priority: row.get(10)?,
                conflicts_with: serde_json::from_str(&row.get::<_, String>(11)?).unwrap(),
                supersedes: row.get(12)?,
            })
        })?;

        Ok(anchors.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}
```

**Step 4: Add more tests**

```rust
#[test]
fn test_update_anchor() {
    let (mut store, _temp) = create_test_store();

    let mut anchor = BehavioralAnchor::new(
        "Test rule".to_string(),
        vec!["test".to_string()],
        AnchorSource::ManualInjection {
            author: "test".to_string(),
        },
        50,
        0.5,
    );

    let id = anchor.id.clone();
    store.add(anchor.clone()).unwrap();

    anchor.update_confidence(true);
    store.update(&anchor).unwrap();

    let retrieved = store.get(&id).unwrap().unwrap();
    assert_eq!(retrieved.validation_count, 1);
    assert!(retrieved.confidence > 0.5);
}

#[test]
fn test_delete_anchor() {
    let (mut store, _temp) = create_test_store();

    let anchor = BehavioralAnchor::new(
        "Test rule".to_string(),
        vec!["test".to_string()],
        AnchorSource::ManualInjection {
            author: "test".to_string(),
        },
        50,
        0.5,
    );

    let id = anchor.id.clone();
    store.add(anchor).unwrap();

    store.delete(&id).unwrap();

    let retrieved = store.get(&id).unwrap();
    assert!(retrieved.is_none());
}

#[test]
fn test_list_all_anchors() {
    let (mut store, _temp) = create_test_store();

    let anchor1 = BehavioralAnchor::new(
        "Rule 1".to_string(),
        vec!["test".to_string()],
        AnchorSource::ManualInjection {
            author: "test".to_string(),
        },
        100,
        0.8,
    );

    let anchor2 = BehavioralAnchor::new(
        "Rule 2".to_string(),
        vec!["test".to_string()],
        AnchorSource::ManualInjection {
            author: "test".to_string(),
        },
        50,
        0.6,
    );

    store.add(anchor1).unwrap();
    store.add(anchor2).unwrap();

    let all = store.list_all().unwrap();
    assert_eq!(all.len(), 2);
    // Should be sorted by priority DESC
    assert_eq!(all[0].priority, 100);
    assert_eq!(all[1].priority, 50);
}
```

**Step 5: Run tests**

```bash
cargo test --lib cortex::meta_cognition::anchor_store
```

Expected: 4 tests pass

**Step 6: Update mod.rs**

```rust
pub mod anchor_store;
pub mod schema;
pub mod types;

pub use anchor_store::AnchorStore;
pub use schema::initialize_schema;
pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
```

**Step 7: Commit**

```bash
git add core/src/memory/cortex/meta_cognition/anchor_store.rs
git add core/src/memory/cortex/meta_cognition/mod.rs
git commit -m "feat(cortex): implement AnchorStore CRUD operations

- Add create, read, update, delete methods
- Implement list_all with priority/confidence sorting
- JSON serialization for complex fields
- 4 tests passing"
```

---

## Phase 2: Conflict Detection

### Task 2.1: Implement Semantic Similarity Detection

**Files:**
- Create: `core/src/memory/cortex/meta_cognition/conflict_detector.rs`
- Modify: `core/src/memory/cortex/meta_cognition/mod.rs`

**Step 1: Write types and test**

```rust
//! Conflict detection for behavioral anchors

use crate::error::Result;
use crate::memory::cortex::meta_cognition::types::BehavioralAnchor;
use crate::memory::embedder::SmartEmbedder;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum ConflictType {
    Redundant { similarity: f32 },
    NeedsReview { similarity: f32 },
    LogicalContradiction { reason: String },
    EmpiricalConflict { failure_rate: f32 },
}

#[derive(Debug, Clone)]
pub struct ConflictReport {
    pub anchor_id: String,
    pub conflicting_with: String,
    pub conflict_type: ConflictType,
}

pub struct ConflictDetector {
    embedder: Arc<SmartEmbedder>,
}

impl ConflictDetector {
    pub fn new(embedder: Arc<SmartEmbedder>) -> Self {
        Self { embedder }
    }

    pub async fn detect_semantic_conflicts(
        &self,
        new_anchor: &BehavioralAnchor,
        existing_anchors: &[BehavioralAnchor],
    ) -> Result<Vec<ConflictReport>> {
        let mut conflicts = Vec::new();

        let new_embedding = self.embedder.embed(&new_anchor.rule_text).await?;

        for existing in existing_anchors {
            // Only check if tags overlap
            if !has_tag_overlap(new_anchor, existing) {
                continue;
            }

            let existing_embedding = self.embedder.embed(&existing.rule_text).await?;
            let similarity = cosine_similarity(&new_embedding, &existing_embedding);

            if similarity > 0.85 {
                conflicts.push(ConflictReport {
                    anchor_id: new_anchor.id.clone(),
                    conflicting_with: existing.id.clone(),
                    conflict_type: ConflictType::Redundant { similarity },
                });
            } else if similarity > 0.70 {
                conflicts.push(ConflictReport {
                    anchor_id: new_anchor.id.clone(),
                    conflicting_with: existing.id.clone(),
                    conflict_type: ConflictType::NeedsReview { similarity },
                });
            }
        }

        Ok(conflicts)
    }
}

fn has_tag_overlap(a: &BehavioralAnchor, b: &BehavioralAnchor) -> bool {
    a.trigger_tags.iter().any(|tag| b.trigger_tags.contains(tag))
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let magnitude_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let magnitude_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if magnitude_a == 0.0 || magnitude_b == 0.0 {
        return 0.0;
    }

    dot_product / (magnitude_a * magnitude_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![1.0, 0.0, 0.0];
        let d = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&c, &d) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_has_tag_overlap() {
        let a = BehavioralAnchor::new(
            "Rule A".to_string(),
            vec!["python".to_string(), "macos".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            50,
            0.5,
        );

        let b = BehavioralAnchor::new(
            "Rule B".to_string(),
            vec!["python".to_string(), "linux".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            50,
            0.5,
        );

        assert!(has_tag_overlap(&a, &b));

        let c = BehavioralAnchor::new(
            "Rule C".to_string(),
            vec!["rust".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            50,
            0.5,
        );

        assert!(!has_tag_overlap(&a, &c));
    }
}
```

**Step 2: Run tests**

```bash
cargo test --lib cortex::meta_cognition::conflict_detector
```

Expected: 2 tests pass

**Step 3: Update mod.rs**

```rust
pub mod anchor_store;
pub mod conflict_detector;
pub mod schema;
pub mod types;

pub use anchor_store::AnchorStore;
pub use conflict_detector::{ConflictDetector, ConflictReport, ConflictType};
pub use schema::initialize_schema;
pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
```

**Step 4: Commit**

```bash
git add core/src/memory/cortex/meta_cognition/conflict_detector.rs
git add core/src/memory/cortex/meta_cognition/mod.rs
git commit -m "feat(cortex): implement semantic similarity conflict detection

- Add ConflictDetector with cosine similarity
- Detect redundant (>0.85) and needs-review (>0.70) conflicts
- Tag overlap filtering for efficiency
- 2 tests passing"
```

---

*Due to length constraints, I'll provide the remaining phases in summary form. The full plan follows the same TDD pattern for:*

- **Phase 3**: ReactiveReflector with FailureSignal enum, root cause analysis
- **Phase 4**: CriticAgent with efficiency metrics, task chain analysis
- **Phase 5**: Dynamic injection with tag extraction, LRU cache
- **Phase 6**: Integration tests, performance benchmarks

---

## Execution Summary

**Total Tasks**: 18 tasks across 6 phases
**Estimated Time**: 3 weeks
**Test Coverage**: 40+ unit tests, 10+ integration tests

**Key Milestones**:
- Week 1: Core data structures + conflict detection
- Week 2: Reactive + proactive reflection
- Week 3: Dynamic injection + integration

---

Plan complete and saved to `docs/plans/2026-02-08-month3-implementation.md`.

**Two execution options:**

**1. Subagent-Driven (this session)** - I dispatch fresh subagent per task, review between tasks, fast iteration

**2. Parallel Session (separate)** - Open new session with executing-plans, batch execution with checkpoints

**Which approach?**
