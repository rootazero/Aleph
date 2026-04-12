# Memory Palace Evolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enhance Aleph's long-term memory with structural navigation (Palace Topology), temporal validity (valid_from/valid_to), and automatic cross-domain association (Tunnel Discovery).

**Architecture:** Three additive phases on the existing SQLite+sqlite-vec memory backend. Phase 1 adds derived domain/topic columns and progressive search. Phase 2 adds temporal validity fields to MemoryFact and integrates with DriftDetectStage. Phase 3 adds tunnel candidate flagging at write-time and a TunnelDiscoveryStage in DreamDaemon.

**Tech Stack:** Rust, SQLite (generated columns, partial indexes), sqlite-vec, async_trait, serde, tokio

**Spec:** `docs/superpowers/specs/2026-04-09-memory-palace-evolution-design.md`

---

## File Map

### Phase 1 — Palace Topology
| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `src/memory/store/sqlite/schema.rs` | Add domain/topic generated columns + index |
| Modify | `src/memory/context/paths.rs` | Add `parse_domain_topic()` utility |
| Modify | `src/memory/store/types.rs` | Add `domain`/`topic` fields to SearchFilter |
| Create | `src/memory/hybrid_retrieval/progressive.rs` | Progressive search scope logic |
| Modify | `src/memory/hybrid_retrieval/mod.rs` | Re-export progressive module |
| Modify | `src/memory/hybrid_retrieval/hybrid.rs` | Add `search_facts_progressive()` method |

### Phase 2 — Temporal Validity
| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `src/memory/context/fact.rs` | Add `valid_from`/`valid_to` fields + builders |
| Modify | `src/memory/store/sqlite/schema.rs` | Add validity columns + partial index |
| Modify | `src/memory/store/sqlite/facts.rs` | Read/write validity fields in row mapping |
| Modify | `src/memory/store/types.rs` | Add `as_of`/`include_historical` to SearchFilter |
| Modify | `src/memory/dreaming/stages/drift.rs` | Supersede closes validity window instead of invalidating |

### Phase 3 — Associative Ripple
| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `src/memory/store/sqlite/schema.rs` | Add `tunnel_pending` column |
| Modify | `src/memory/store/mod.rs` | Add tunnel query methods to MemoryStore trait |
| Modify | `src/memory/store/sqlite/facts.rs` | Implement tunnel query methods |
| Create | `src/memory/dreaming/stages/tunnel.rs` | TunnelDiscoveryStage implementation |
| Modify | `src/memory/dreaming/stages/mod.rs` | Register tunnel stage |
| Modify | `src/memory/dreaming/mod.rs` | Add TunnelDiscoveryStage to daily pipeline |
| Modify | `src/memory/ripple/config.rs` | Add tunnel traversal config fields |
| Modify | `src/memory/ripple/task.rs` | Tunnel-aware BFS traversal |

---

## Phase 1 — Palace Topology

### Task 1: VFS Path Parser Utility

**Files:**
- Modify: `src/memory/context/paths.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/memory/context/paths.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_domain_topic_standard_path() {
        let (domain, topic) = parse_domain_topic("aleph://user/preferences/coding");
        assert_eq!(domain, "user");
        assert_eq!(topic, "preferences");
    }

    #[test]
    fn parse_domain_topic_with_trailing_slash() {
        let (domain, topic) = parse_domain_topic("aleph://knowledge/projects/");
        assert_eq!(domain, "knowledge");
        assert_eq!(topic, "projects");
    }

    #[test]
    fn parse_domain_topic_domain_only() {
        let (domain, topic) = parse_domain_topic("aleph://user/");
        assert_eq!(domain, "user");
        assert_eq!(topic, "");
    }

    #[test]
    fn parse_domain_topic_empty_path() {
        let (domain, topic) = parse_domain_topic("");
        assert_eq!(domain, "");
        assert_eq!(topic, "");
    }

    #[test]
    fn parse_domain_topic_no_prefix() {
        let (domain, topic) = parse_domain_topic("random/path");
        assert_eq!(domain, "");
        assert_eq!(topic, "");
    }

    #[test]
    fn parse_domain_topic_agent_tools() {
        let (domain, topic) = parse_domain_topic("aleph://agent/tools/shell");
        assert_eq!(domain, "agent");
        assert_eq!(topic, "tools");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- context::paths::tests --nocapture`
Expected: FAIL — `parse_domain_topic` not defined

- [ ] **Step 3: Implement parse_domain_topic**

Add to `src/memory/context/paths.rs` before the existing `compute_parent_path` function:

```rust
/// Parse domain and topic from an `aleph://` VFS path.
///
/// Given `aleph://user/preferences/coding`, returns `("user", "preferences")`.
/// Returns `("", "")` for empty or non-conforming paths.
pub fn parse_domain_topic(path: &str) -> (&str, &str) {
    const PREFIX: &str = "aleph://";

    let rest = match path.strip_prefix(PREFIX) {
        Some(r) => r,
        None => return ("", ""),
    };

    // Split: "user/preferences/coding" → ["user", "preferences", "coding"]
    let mut segments = rest.split('/').filter(|s| !s.is_empty());

    let domain = segments.next().unwrap_or("");
    let topic = segments.next().unwrap_or("");

    (domain, topic)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- context::paths::tests --nocapture`
Expected: All 6 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/context/paths.rs
git commit -m "feat(memory): add parse_domain_topic utility for VFS path navigation"
```

---

### Task 2: Schema Migration — Generated Columns + Index

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`

- [ ] **Step 1: Add migration DDL constant**

Add after the existing `DREAM_REPORTS_DDL` constant in `src/memory/store/sqlite/schema.rs`:

```rust
// ---------------------------------------------------------------------------
// Palace Topology: derived domain/topic columns
// ---------------------------------------------------------------------------

const PALACE_TOPOLOGY_DDL: &str = r#"
ALTER TABLE facts ADD COLUMN domain TEXT
  GENERATED ALWAYS AS (
    CASE WHEN path LIKE 'aleph://%/%'
    THEN substr(path, 9, instr(substr(path, 9), '/') - 1)
    ELSE '' END
  ) STORED;

ALTER TABLE facts ADD COLUMN topic TEXT
  GENERATED ALWAYS AS (
    CASE WHEN path LIKE 'aleph://%/%/%'
    THEN substr(path, 9 + instr(substr(path, 9), '/'),
         instr(substr(path, 9 + instr(substr(path, 9), '/')), '/') - 1)
    ELSE '' END
  ) STORED;
"#;

const PALACE_TOPOLOGY_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_facts_domain_topic ON facts(domain, topic);
"#;
```

- [ ] **Step 2: Add migration function**

Add a new public function in the same file:

```rust
/// Apply Palace Topology migration: domain/topic generated columns + index.
///
/// Safe to call multiple times — checks for column existence before altering.
pub fn migrate_palace_topology(conn: &Connection) -> Result<(), AlephError> {
    // Check if columns already exist
    let has_domain: bool = conn
        .prepare("SELECT domain FROM facts LIMIT 0")
        .is_ok();

    if !has_domain {
        // Execute ALTER TABLE statements one at a time (SQLite requirement)
        for stmt in PALACE_TOPOLOGY_DDL.split(';').filter(|s| !s.trim().is_empty()) {
            conn.execute_batch(stmt).map_err(|e| {
                AlephError::config(format!("Failed to add palace topology column: {e}"))
            })?;
        }
    }

    conn.execute_batch(PALACE_TOPOLOGY_INDEX_DDL).map_err(|e| {
        AlephError::config(format!("Failed to create palace topology index: {e}"))
    })?;

    Ok(())
}
```

- [ ] **Step 3: Wire migration into init_schema**

In the `init_schema` function, add at the end before `Ok(())`:

```rust
    migrate_palace_topology(conn)?;
```

- [ ] **Step 4: Run compile check**

Run: `cargo check -p alephcore`
Expected: Compiles without errors

- [ ] **Step 5: Write integration test**

Add at the end of `src/memory/store/sqlite/schema.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn palace_topology_columns_auto_populated() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        init_vec_tables(&conn).unwrap();

        // Insert a fact with a known path
        conn.execute(
            "INSERT INTO facts (id, content, fact_type, fact_source, created_at, updated_at, path)
             VALUES ('t1', 'test', 'preference', 'extracted', 0, 0, 'aleph://user/preferences/coding')",
            [],
        ).unwrap();

        let (domain, topic): (String, String) = conn
            .query_row(
                "SELECT domain, topic FROM facts WHERE id = 't1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(domain, "user");
        assert_eq!(topic, "preferences");
    }

    #[test]
    fn palace_topology_empty_path_yields_empty_strings() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        init_vec_tables(&conn).unwrap();

        conn.execute(
            "INSERT INTO facts (id, content, fact_type, fact_source, created_at, updated_at, path)
             VALUES ('t2', 'test', 'other', 'extracted', 0, 0, '')",
            [],
        ).unwrap();

        let (domain, topic): (String, String) = conn
            .query_row(
                "SELECT domain, topic FROM facts WHERE id = 't2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(domain, "");
        assert_eq!(topic, "");
    }

    #[test]
    fn migrate_palace_topology_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        // Call again — should not error
        assert!(migrate_palace_topology(&conn).is_ok());
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib -- store::sqlite::schema::tests --nocapture`
Expected: All 3 tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/memory/store/sqlite/schema.rs
git commit -m "feat(memory): add palace topology generated columns (domain/topic) to facts schema"
```

---

### Task 3: SearchFilter — Domain/Topic Fields

**Files:**
- Modify: `src/memory/store/types.rs`

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in `src/memory/store/types.rs`:

```rust
    #[test]
    fn search_filter_domain_and_topic() {
        let filter = SearchFilter::new()
            .with_domain("user")
            .with_topic("preferences");
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("domain = 'user'"));
        assert!(sql.contains("topic = 'preferences'"));
    }

    #[test]
    fn search_filter_domain_only() {
        let filter = SearchFilter::new().with_domain("knowledge");
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("domain = 'knowledge'"));
        assert!(!sql.contains("topic"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- store::types::tests::search_filter_domain --nocapture`
Expected: FAIL — `with_domain` method not found

- [ ] **Step 3: Add domain/topic fields and builders to SearchFilter**

In `src/memory/store/types.rs`, add to the `SearchFilter` struct fields (after `scope_stack_clause`):

```rust
    /// Restrict to a specific domain (derived from VFS path first segment).
    pub domain: Option<String>,
    /// Restrict to a specific topic (derived from VFS path second segment).
    pub topic: Option<String>,
```

Add builder methods (after `with_persona_id`):

```rust
    /// Set domain filter.
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Set topic filter.
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }
```

Add to `to_lance_filter()` method, before the final `if clauses.is_empty()` check:

```rust
        if let Some(ref domain) = self.domain {
            clauses.push(format!("domain = '{}'", escape_sql_string(domain)));
        }

        if let Some(ref topic) = self.topic {
            clauses.push(format!("topic = '{}'", escape_sql_string(topic)));
        }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- store::types::tests --nocapture`
Expected: All tests PASS (existing + 2 new)

- [ ] **Step 5: Commit**

```bash
git add src/memory/store/types.rs
git commit -m "feat(memory): add domain/topic filter fields to SearchFilter"
```

---

### Task 4: Progressive Search Strategy

**Files:**
- Create: `src/memory/hybrid_retrieval/progressive.rs`
- Modify: `src/memory/hybrid_retrieval/mod.rs`

- [ ] **Step 1: Create progressive.rs with tests**

Create `src/memory/hybrid_retrieval/progressive.rs`:

```rust
//! Progressive search scope narrowing.
//!
//! Implements three-level scope: TopicLocal → DomainWide → Global.
//! The search starts narrow and expands only if insufficient results are found.

use serde::{Deserialize, Serialize};

/// Search scope level for progressive narrowing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchScope {
    /// Same topic within same domain.
    TopicLocal { domain: String, topic: String },
    /// Same domain, all topics.
    DomainWide { domain: String },
    /// Full corpus, no domain/topic filter.
    Global,
}

/// Configuration for progressive search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressiveSearchConfig {
    /// Enable progressive scope narrowing (default: true).
    pub enabled: bool,
    /// Minimum results before scope expansion (default: 3).
    pub min_results: usize,
    /// Score bonus for same-topic results (default: 0.1).
    pub topic_boost: f32,
    /// Score bonus for same-domain results (default: 0.05).
    pub domain_boost: f32,
}

impl Default for ProgressiveSearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_results: 3,
            topic_boost: 0.1,
            domain_boost: 0.05,
        }
    }
}

/// Build the sequence of search scopes to try, from narrowest to widest.
///
/// Returns an empty vec if domain/topic are both empty (skip straight to Global
/// which is the caller's default).
pub fn build_scope_sequence(domain: &str, topic: &str) -> Vec<SearchScope> {
    let mut scopes = Vec::new();

    if !domain.is_empty() && !topic.is_empty() {
        scopes.push(SearchScope::TopicLocal {
            domain: domain.to_string(),
            topic: topic.to_string(),
        });
    }

    if !domain.is_empty() {
        scopes.push(SearchScope::DomainWide {
            domain: domain.to_string(),
        });
    }

    // Global is always the final fallback (handled by caller)
    scopes
}

/// Infer the most likely domain/topic from a set of recent fact paths.
///
/// Counts frequency of (domain, topic) pairs and returns the most common.
/// Returns `("", "")` if no facts have valid paths.
pub fn infer_scope_from_facts(paths: &[&str]) -> (String, String) {
    use std::collections::HashMap;
    use crate::memory::context::paths::parse_domain_topic;

    let mut counts: HashMap<(&str, &str), usize> = HashMap::new();

    for path in paths {
        let (domain, topic) = parse_domain_topic(path);
        if !domain.is_empty() {
            *counts.entry((domain, topic)).or_insert(0) += 1;
        }
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|((d, t), _)| (d.to_string(), t.to_string()))
        .unwrap_or_else(|| (String::new(), String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_scope_sequence_full_path() {
        let scopes = build_scope_sequence("user", "preferences");
        assert_eq!(scopes.len(), 2);
        assert_eq!(
            scopes[0],
            SearchScope::TopicLocal {
                domain: "user".to_string(),
                topic: "preferences".to_string(),
            }
        );
        assert_eq!(
            scopes[1],
            SearchScope::DomainWide {
                domain: "user".to_string(),
            }
        );
    }

    #[test]
    fn build_scope_sequence_domain_only() {
        let scopes = build_scope_sequence("knowledge", "");
        assert_eq!(scopes.len(), 1);
        assert_eq!(
            scopes[0],
            SearchScope::DomainWide {
                domain: "knowledge".to_string(),
            }
        );
    }

    #[test]
    fn build_scope_sequence_empty() {
        let scopes = build_scope_sequence("", "");
        assert!(scopes.is_empty());
    }

    #[test]
    fn infer_scope_from_facts_picks_most_frequent() {
        let paths = [
            "aleph://user/preferences/coding",
            "aleph://user/preferences/editor",
            "aleph://knowledge/projects/aleph",
        ];
        let (domain, topic) = infer_scope_from_facts(&paths);
        assert_eq!(domain, "user");
        assert_eq!(topic, "preferences");
    }

    #[test]
    fn infer_scope_from_facts_empty_input() {
        let paths: [&str; 0] = [];
        let (domain, topic) = infer_scope_from_facts(&paths);
        assert_eq!(domain, "");
        assert_eq!(topic, "");
    }

    #[test]
    fn infer_scope_from_facts_invalid_paths_ignored() {
        let paths = ["not-a-vfs-path", "random/thing"];
        let (domain, topic) = infer_scope_from_facts(&paths);
        assert_eq!(domain, "");
        assert_eq!(topic, "");
    }
}
```

- [ ] **Step 2: Add module to mod.rs**

In `src/memory/hybrid_retrieval/mod.rs`, add:

```rust
pub mod progressive;

pub use progressive::{
    build_scope_sequence, infer_scope_from_facts, ProgressiveSearchConfig, SearchScope,
};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib -- hybrid_retrieval::progressive::tests --nocapture`
Expected: All 6 tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/hybrid_retrieval/progressive.rs src/memory/hybrid_retrieval/mod.rs
git commit -m "feat(memory): add progressive search scope narrowing module"
```

---

### Task 5: Integrate Progressive Search into HybridRetrieval

**Files:**
- Modify: `src/memory/hybrid_retrieval/hybrid.rs`

- [ ] **Step 1: Add progressive search method**

Add to the `impl HybridRetrieval` block in `src/memory/hybrid_retrieval/hybrid.rs`:

```rust
    /// Search facts with progressive scope narrowing.
    ///
    /// Starts with the narrowest scope inferred from `context_paths`, then
    /// expands to wider scopes until `min_results` are found or Global is reached.
    ///
    /// # Arguments
    /// * `query_embedding` - Vector embedding of the query
    /// * `query_text` - Natural language query text
    /// * `context_paths` - Recent fact paths for scope inference
    /// * `progressive_config` - Progressive search configuration
    pub async fn search_facts_progressive(
        &self,
        query_embedding: &[f32],
        query_text: &str,
        context_paths: &[&str],
        progressive_config: &super::progressive::ProgressiveSearchConfig,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        use super::progressive::{build_scope_sequence, infer_scope_from_facts, SearchScope};

        if !progressive_config.enabled {
            return self.search_facts(query_embedding, query_text).await;
        }

        let (domain, topic) = infer_scope_from_facts(context_paths);
        let scopes = build_scope_sequence(&domain, &topic);

        let dim_hint = query_embedding.len() as u32;

        // Try each scope from narrowest to widest
        for scope in &scopes {
            let filter = match scope {
                SearchScope::TopicLocal { domain, topic } => SearchFilter::valid_only(None)
                    .with_domain(domain)
                    .with_topic(topic),
                SearchScope::DomainWide { domain } => {
                    SearchFilter::valid_only(None).with_domain(domain)
                }
                SearchScope::Global => SearchFilter::valid_only(None),
            };

            let scored = self
                .database
                .hybrid_search(&crate::memory::store::HybridSearchParams {
                    embedding: query_embedding,
                    dim_hint,
                    query_text,
                    vector_weight: self.config.vector_weight,
                    text_weight: self.config.text_weight,
                    filter: &filter,
                    limit: self.config.max_results,
                })
                .await?;

            let scored = self.apply_pipeline(scored, query_embedding, query_text);
            let results = Self::apply_min_score(scored, self.config.min_score);

            if results.len() >= progressive_config.min_results {
                return Ok(results);
            }
        }

        // Final fallback: Global scope (no domain/topic filter)
        self.search_facts(query_embedding, query_text).await
    }
```

- [ ] **Step 2: Run compile check**

Run: `cargo check -p alephcore`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add src/memory/hybrid_retrieval/hybrid.rs
git commit -m "feat(memory): integrate progressive search into HybridRetrieval"
```

---

## Phase 2 — Temporal Validity

### Task 6: MemoryFact — Add Validity Fields

**Files:**
- Modify: `src/memory/context/fact.rs`

- [ ] **Step 1: Add fields to MemoryFact struct**

In `src/memory/context/fact.rs`, add after the `last_accessed_at` field:

```rust
    /// When this fact became true (Unix seconds). None = since creation.
    #[serde(default)]
    pub valid_from: Option<i64>,
    /// When this fact stopped being true (Unix seconds). None = still valid.
    #[serde(default)]
    pub valid_to: Option<i64>,
```

- [ ] **Step 2: Update constructors**

In the `MemoryFact::new()` method, add before the closing brace:

```rust
            valid_from: None,
            valid_to: None,
```

In the `MemoryFact::with_id()` method, add before the closing brace:

```rust
            valid_from: None,
            valid_to: None,
```

- [ ] **Step 3: Add builder methods**

Add to the `impl MemoryFact` block:

```rust
    /// Set valid_from timestamp
    pub fn with_valid_from(mut self, ts: i64) -> Self {
        self.valid_from = Some(ts);
        self
    }

    /// Set valid_to timestamp
    pub fn with_valid_to(mut self, ts: i64) -> Self {
        self.valid_to = Some(ts);
        self
    }

    /// Close the validity window — mark this fact as historical.
    pub fn close_validity(mut self) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        self.valid_to = Some(now);
        self
    }

    /// Check if this fact is currently valid (no valid_to set).
    pub fn is_currently_valid(&self) -> bool {
        self.valid_to.is_none()
    }
```

- [ ] **Step 4: Write tests**

Add a test module at the end of `src/memory/context/fact.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;

    #[test]
    fn new_fact_has_no_validity_bounds() {
        let fact = MemoryFact::new("test".into(), FactType::Other, vec![]);
        assert!(fact.valid_from.is_none());
        assert!(fact.valid_to.is_none());
        assert!(fact.is_currently_valid());
    }

    #[test]
    fn close_validity_sets_valid_to() {
        let fact = MemoryFact::new("test".into(), FactType::Other, vec![]).close_validity();
        assert!(fact.valid_to.is_some());
        assert!(!fact.is_currently_valid());
    }

    #[test]
    fn with_valid_from_sets_timestamp() {
        let fact =
            MemoryFact::new("test".into(), FactType::Other, vec![]).with_valid_from(1000);
        assert_eq!(fact.valid_from, Some(1000));
        assert!(fact.is_currently_valid()); // valid_to still None
    }

    #[test]
    fn with_valid_to_sets_timestamp() {
        let fact = MemoryFact::new("test".into(), FactType::Other, vec![]).with_valid_to(2000);
        assert_eq!(fact.valid_to, Some(2000));
        assert!(!fact.is_currently_valid());
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- context::fact::tests --nocapture`
Expected: All 4 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/context/fact.rs
git commit -m "feat(memory): add valid_from/valid_to temporal validity fields to MemoryFact"
```

---

### Task 7: Schema + Row Mapping for Validity Fields

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/sqlite/facts.rs`

- [ ] **Step 1: Add validity columns to schema migration**

In `src/memory/store/sqlite/schema.rs`, add a new constant after `PALACE_TOPOLOGY_INDEX_DDL`:

```rust
// ---------------------------------------------------------------------------
// Temporal Validity: valid_from / valid_to columns
// ---------------------------------------------------------------------------

const TEMPORAL_VALIDITY_DDL: &str = r#"
ALTER TABLE facts ADD COLUMN valid_from INTEGER DEFAULT NULL;
ALTER TABLE facts ADD COLUMN valid_to INTEGER DEFAULT NULL;
"#;

const TEMPORAL_VALIDITY_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_facts_validity ON facts(valid_to) WHERE valid_to IS NULL;
"#;
```

Add migration function:

```rust
/// Apply Temporal Validity migration: valid_from/valid_to columns + partial index.
///
/// Safe to call multiple times — checks for column existence.
pub fn migrate_temporal_validity(conn: &Connection) -> Result<(), AlephError> {
    let has_valid_from: bool = conn
        .prepare("SELECT valid_from FROM facts LIMIT 0")
        .is_ok();

    if !has_valid_from {
        for stmt in TEMPORAL_VALIDITY_DDL.split(';').filter(|s| !s.trim().is_empty()) {
            conn.execute_batch(stmt).map_err(|e| {
                AlephError::config(format!("Failed to add temporal validity column: {e}"))
            })?;
        }
    }

    conn.execute_batch(TEMPORAL_VALIDITY_INDEX_DDL).map_err(|e| {
        AlephError::config(format!("Failed to create temporal validity index: {e}"))
    })?;

    Ok(())
}
```

Wire into `init_schema` after `migrate_palace_topology(conn)?;`:

```rust
    migrate_temporal_validity(conn)?;
```

- [ ] **Step 2: Update row_to_fact_pub in facts.rs**

In `src/memory/store/sqlite/facts.rs`, in the `row_to_fact_pub` function, add after
the `last_accessed_at` field in the returned `MemoryFact` struct:

```rust
        valid_from: row.get("valid_from")?,
        valid_to: row.get("valid_to")?,
```

- [ ] **Step 3: Update INSERT in insert_fact**

Find the `insert_fact` method in `src/memory/store/sqlite/facts.rs`. Add `valid_from`
and `valid_to` to the INSERT column list and parameter bindings:

In the column list add: `valid_from, valid_to`
In the VALUES placeholders add two more `?N` params.
In the params array add: `&fact.valid_from`, `&fact.valid_to`

- [ ] **Step 4: Run compile check + existing tests**

Run: `cargo test -p alephcore --lib -- store::sqlite --nocapture`
Expected: All existing tests PASS, compile succeeds

- [ ] **Step 5: Add schema test**

Add to the existing `tests` module in `src/memory/store/sqlite/schema.rs`:

```rust
    #[test]
    fn temporal_validity_columns_default_null() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        init_vec_tables(&conn).unwrap();

        conn.execute(
            "INSERT INTO facts (id, content, fact_type, fact_source, created_at, updated_at, path)
             VALUES ('tv1', 'test', 'other', 'extracted', 0, 0, '')",
            [],
        ).unwrap();

        let (vf, vt): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT valid_from, valid_to FROM facts WHERE id = 'tv1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert!(vf.is_none());
        assert!(vt.is_none());
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib -- store::sqlite::schema::tests --nocapture`
Expected: All tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/memory/store/sqlite/schema.rs src/memory/store/sqlite/facts.rs
git commit -m "feat(memory): add temporal validity schema columns and row mapping"
```

---

### Task 8: SearchFilter — Temporal Validity Filtering

**Files:**
- Modify: `src/memory/store/types.rs`

- [ ] **Step 1: Write the failing tests**

Add to the existing test module in `src/memory/store/types.rs`:

```rust
    #[test]
    fn search_filter_default_excludes_historical() {
        // Default behavior: only currently-valid facts
        let filter = SearchFilter::new().with_valid_only();
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("valid_to IS NULL"));
    }

    #[test]
    fn search_filter_as_of_generates_temporal_range() {
        let filter = SearchFilter::new().with_as_of(1700000000);
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("valid_from IS NULL OR valid_from <= 1700000000"));
        assert!(sql.contains("valid_to IS NULL OR valid_to >= 1700000000"));
    }

    #[test]
    fn search_filter_include_historical_skips_validity() {
        let filter = SearchFilter::new()
            .with_valid_only()
            .with_include_historical();
        let sql = filter.to_lance_filter().unwrap();
        assert!(!sql.contains("valid_to"));
        assert!(sql.contains("is_valid = true"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- store::types::tests::search_filter_default_excludes --nocapture`
Expected: FAIL — methods not found

- [ ] **Step 3: Add fields and builders**

Add to `SearchFilter` struct fields:

```rust
    /// Query facts valid at this point in time (Unix seconds).
    /// When set, returns facts where valid_from <= as_of AND (valid_to IS NULL OR valid_to >= as_of).
    pub as_of: Option<i64>,
    /// Include historically-valid facts (valid_to IS NOT NULL).
    /// Default: false — only currently-valid facts are returned.
    pub include_historical: bool,
```

Add builder methods:

```rust
    /// Set temporal point-in-time query.
    pub fn with_as_of(mut self, ts: i64) -> Self {
        self.as_of = Some(ts);
        self
    }

    /// Include historical facts (don't filter by valid_to).
    pub fn with_include_historical(mut self) -> Self {
        self.include_historical = true;
        self
    }
```

Add to `to_lance_filter()`, before the domain/topic section:

```rust
        // Temporal validity filtering
        if !self.include_historical {
            if let Some(ts) = self.as_of {
                clauses.push(format!(
                    "(valid_from IS NULL OR valid_from <= {ts}) AND (valid_to IS NULL OR valid_to >= {ts})"
                ));
            } else if self.is_valid.is_some() {
                // Default: only currently-valid facts (valid_to IS NULL)
                clauses.push("valid_to IS NULL".to_string());
            }
        }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- store::types::tests --nocapture`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/store/types.rs
git commit -m "feat(memory): add temporal validity filtering to SearchFilter (as_of, include_historical)"
```

---

### Task 9: DriftDetectStage — Close Validity Window on Supersede

**Files:**
- Modify: `src/memory/dreaming/stages/drift.rs`
- Modify: `src/memory/store/mod.rs` (MemoryStore trait)
- Modify: `src/memory/store/sqlite/facts.rs`

- [ ] **Step 1: Add `close_fact_validity` to MemoryStore trait**

In `src/memory/store/mod.rs`, add to the `MemoryStore` trait (after `invalidate_fact`):

```rust
    /// Close a fact's validity window by setting `valid_to` to the given timestamp.
    /// The fact remains valid (`is_valid = true`) but becomes historical.
    async fn close_fact_validity(&self, id: &str, valid_to: i64) -> Result<(), AlephError>;

    /// Set `valid_from` on a fact.
    async fn set_fact_valid_from(&self, id: &str, valid_from: i64) -> Result<(), AlephError>;
```

- [ ] **Step 2: Implement in SQLite backend**

In `src/memory/store/sqlite/facts.rs`, add implementations:

```rust
    async fn close_fact_validity(&self, id: &str, valid_to: i64) -> Result<(), AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::other(format!("Failed to lock connection: {e}"))
        })?;
        conn.execute(
            "UPDATE facts SET valid_to = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![valid_to, valid_to, id],
        )
        .map_err(|e| AlephError::other(format!("Failed to close fact validity: {e}")))?;
        Ok(())
    }

    async fn set_fact_valid_from(&self, id: &str, valid_from: i64) -> Result<(), AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::other(format!("Failed to lock connection: {e}"))
        })?;
        conn.execute(
            "UPDATE facts SET valid_from = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![valid_from, valid_from, id],
        )
        .map_err(|e| AlephError::other(format!("Failed to set fact valid_from: {e}")))?;
        Ok(())
    }
```

- [ ] **Step 3: Update DriftDetectStage Supersede handling**

In `src/memory/dreaming/stages/drift.rs`, replace the `DriftAction::Supersede` arm in the resolution application loop (around line 215-218):

```rust
                DriftAction::Supersede { old_id, new_id } => {
                    // Close the old fact's validity window instead of invalidating
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    ctx.database.close_fact_validity(old_id, now).await?;
                    ctx.database.set_fact_valid_from(new_id, now).await?;
                }
```

- [ ] **Step 4: Run compile check**

Run: `cargo check -p alephcore`
Expected: Compiles without errors

- [ ] **Step 5: Run existing drift tests**

Run: `cargo test -p alephcore --lib -- dreaming::stages::drift::tests --nocapture`
Expected: All existing tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/store/mod.rs src/memory/store/sqlite/facts.rs src/memory/dreaming/stages/drift.rs
git commit -m "feat(memory): DriftDetect closes validity window on supersede instead of invalidating"
```

---

## Phase 3 — Associative Ripple

### Task 10: Schema + Store Methods for Tunnel Candidates

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/mod.rs`
- Modify: `src/memory/store/sqlite/facts.rs`

- [ ] **Step 1: Add tunnel_pending column**

In `src/memory/store/sqlite/schema.rs`, add constant:

```rust
// ---------------------------------------------------------------------------
// Tunnel Discovery: pending candidate flag
// ---------------------------------------------------------------------------

const TUNNEL_PENDING_DDL: &str = r#"
ALTER TABLE facts ADD COLUMN tunnel_pending INTEGER DEFAULT 0;
"#;

const TUNNEL_PENDING_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_facts_tunnel_pending ON facts(tunnel_pending) WHERE tunnel_pending = 1;
"#;
```

Add migration function:

```rust
/// Apply Tunnel Discovery migration: tunnel_pending column + partial index.
pub fn migrate_tunnel_pending(conn: &Connection) -> Result<(), AlephError> {
    let has_col: bool = conn
        .prepare("SELECT tunnel_pending FROM facts LIMIT 0")
        .is_ok();

    if !has_col {
        for stmt in TUNNEL_PENDING_DDL.split(';').filter(|s| !s.trim().is_empty()) {
            conn.execute_batch(stmt).map_err(|e| {
                AlephError::config(format!("Failed to add tunnel_pending column: {e}"))
            })?;
        }
    }

    conn.execute_batch(TUNNEL_PENDING_INDEX_DDL).map_err(|e| {
        AlephError::config(format!("Failed to create tunnel_pending index: {e}"))
    })?;

    Ok(())
}
```

Wire into `init_schema` after `migrate_temporal_validity(conn)?;`:

```rust
    migrate_tunnel_pending(conn)?;
```

- [ ] **Step 2: Add tunnel methods to MemoryStore trait**

In `src/memory/store/mod.rs`, add to the `MemoryStore` trait:

```rust
    /// Count facts in a given topic that belong to a domain other than `exclude_domain`.
    async fn count_facts_by_topic_excluding_domain(
        &self,
        topic: &str,
        exclude_domain: &str,
    ) -> Result<u64, AlephError>;

    /// Mark a fact as a tunnel candidate.
    async fn set_tunnel_pending(&self, id: &str, pending: bool) -> Result<(), AlephError>;

    /// Check if any tunnel candidates exist.
    async fn has_tunnel_pending(&self) -> Result<bool, AlephError>;

    /// Get tunnel candidate facts, up to `limit`.
    async fn get_tunnel_candidates(&self, limit: usize) -> Result<Vec<MemoryFact>, AlephError>;

    /// Clear tunnel_pending flag for all facts with the given topic.
    async fn clear_tunnel_pending_by_topic(&self, topic: &str) -> Result<(), AlephError>;
```

- [ ] **Step 3: Implement in SQLite backend**

In `src/memory/store/sqlite/facts.rs`, add implementations:

```rust
    async fn count_facts_by_topic_excluding_domain(
        &self,
        topic: &str,
        exclude_domain: &str,
    ) -> Result<u64, AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::other(format!("Failed to lock connection: {e}"))
        })?;
        let count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE topic = ?1 AND domain != ?2 AND is_valid = 1",
                rusqlite::params![topic, exclude_domain],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::other(format!("count_facts_by_topic query failed: {e}")))?;
        Ok(count)
    }

    async fn set_tunnel_pending(&self, id: &str, pending: bool) -> Result<(), AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::other(format!("Failed to lock connection: {e}"))
        })?;
        conn.execute(
            "UPDATE facts SET tunnel_pending = ?1 WHERE id = ?2",
            rusqlite::params![pending as i32, id],
        )
        .map_err(|e| AlephError::other(format!("set_tunnel_pending failed: {e}")))?;
        Ok(())
    }

    async fn has_tunnel_pending(&self) -> Result<bool, AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::other(format!("Failed to lock connection: {e}"))
        })?;
        let count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE tunnel_pending = 1 LIMIT 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::other(format!("has_tunnel_pending query failed: {e}")))?;
        Ok(count > 0)
    }

    async fn get_tunnel_candidates(&self, limit: usize) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::other(format!("Failed to lock connection: {e}"))
        })?;
        let mut stmt = conn
            .prepare("SELECT * FROM facts WHERE tunnel_pending = 1 AND is_valid = 1 LIMIT ?1")
            .map_err(|e| AlephError::other(format!("get_tunnel_candidates prepare failed: {e}")))?;

        let facts = stmt
            .query_map(rusqlite::params![limit as u32], row_to_fact)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(facts)
    }

    async fn clear_tunnel_pending_by_topic(&self, topic: &str) -> Result<(), AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::other(format!("Failed to lock connection: {e}"))
        })?;
        conn.execute(
            "UPDATE facts SET tunnel_pending = 0 WHERE topic = ?1 AND tunnel_pending = 1",
            rusqlite::params![topic],
        )
        .map_err(|e| AlephError::other(format!("clear_tunnel_pending_by_topic failed: {e}")))?;
        Ok(())
    }
```

- [ ] **Step 4: Run compile check**

Run: `cargo check -p alephcore`
Expected: Compiles without errors

- [ ] **Step 5: Commit**

```bash
git add src/memory/store/sqlite/schema.rs src/memory/store/mod.rs src/memory/store/sqlite/facts.rs
git commit -m "feat(memory): add tunnel_pending schema column and MemoryStore query methods"
```

---

### Task 11: TunnelDiscoveryStage

**Files:**
- Create: `src/memory/dreaming/stages/tunnel.rs`
- Modify: `src/memory/dreaming/stages/mod.rs`
- Modify: `src/memory/dreaming/mod.rs`

- [ ] **Step 1: Create tunnel.rs**

Create `src/memory/dreaming/stages/tunnel.rs`:

```rust
//! TunnelDiscoveryStage: discovers cross-domain associations.
//!
//! When facts about the same topic exist in different domains, this stage
//! creates "tunnel" edges in the knowledge graph, enabling Ripple to
//! traverse across domain boundaries.

use async_trait::async_trait;
use std::collections::HashMap;
use tracing::{debug, info};

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::paths::parse_domain_topic;
use crate::memory::context::MemoryFact;
use crate::memory::store::MemoryStore;

/// Default similarity threshold for creating tunnel edges.
const DEFAULT_TUNNEL_SIMILARITY: f32 = 0.6;

/// Default max tunnel candidates per DreamDaemon run.
const DEFAULT_TUNNEL_BATCH_SIZE: usize = 100;

/// Default max tunnel edges per topic.
const DEFAULT_MAX_TUNNELS_PER_TOPIC: usize = 5;

/// Discovers cross-domain tunnel associations.
pub struct TunnelDiscoveryStage;

#[async_trait]
impl DreamStage for TunnelDiscoveryStage {
    fn name(&self) -> &'static str {
        "tunnel_discovery"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.database.has_tunnel_pending().unwrap_or(false)
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let batch_size = DEFAULT_TUNNEL_BATCH_SIZE;
        let candidates = ctx.database.get_tunnel_candidates(batch_size).await?;

        if candidates.is_empty() {
            return Ok(ctx);
        }

        info!(count = candidates.len(), "Processing tunnel candidates");

        // Group by topic
        let by_topic = group_by_topic(&candidates);

        let mut tunnels_created = 0u32;

        for (topic, facts) in &by_topic {
            let domain_groups = group_by_domain(facts);

            // Need facts in at least 2 domains to create a tunnel
            if domain_groups.len() < 2 {
                // Clear pending flag — single-domain topic, no tunnel needed
                ctx.database.clear_tunnel_pending_by_topic(topic).await?;
                continue;
            }

            // Pick representative fact per domain (highest strength)
            let reps: Vec<&MemoryFact> = domain_groups
                .values()
                .filter_map(|fs| {
                    fs.iter()
                        .max_by(|a, b| a.strength.partial_cmp(&b.strength).unwrap_or(std::cmp::Ordering::Equal))
                })
                .collect();

            // Pairwise embedding similarity check
            let mut topic_tunnels = 0usize;
            for i in 0..reps.len() {
                if topic_tunnels >= DEFAULT_MAX_TUNNELS_PER_TOPIC {
                    break;
                }
                for j in (i + 1)..reps.len() {
                    if topic_tunnels >= DEFAULT_MAX_TUNNELS_PER_TOPIC {
                        break;
                    }

                    let sim = embedding_similarity(
                        reps[i].embedding.as_deref(),
                        reps[j].embedding.as_deref(),
                    );

                    if sim >= DEFAULT_TUNNEL_SIMILARITY {
                        // Create tunnel edge via graph store
                        let from_node = ensure_fact_node(&ctx, reps[i]).await?;
                        let to_node = ensure_fact_node(&ctx, reps[j]).await?;

                        ctx.graph_store.upsert_edge_simple(
                            &from_node,
                            &to_node,
                            "tunnel",
                            sim,
                            topic,
                        ).await?;

                        topic_tunnels += 1;
                        tunnels_created += 1;

                        debug!(
                            from = %reps[i].id,
                            to = %reps[j].id,
                            similarity = sim,
                            topic = topic,
                            "Created tunnel edge"
                        );
                    }
                }
            }

            // Clear pending flag for this topic
            ctx.database.clear_tunnel_pending_by_topic(topic).await?;
        }

        info!(tunnels_created, "Tunnel discovery complete");
        Ok(ctx)
    }
}

/// Group facts by their topic (derived from VFS path).
fn group_by_topic(facts: &[MemoryFact]) -> HashMap<String, Vec<&MemoryFact>> {
    let mut groups: HashMap<String, Vec<&MemoryFact>> = HashMap::new();
    for fact in facts {
        let (_, topic) = parse_domain_topic(&fact.path);
        if !topic.is_empty() {
            groups.entry(topic.to_string()).or_default().push(fact);
        }
    }
    groups
}

/// Group facts by their domain (derived from VFS path).
fn group_by_domain<'a>(facts: &[&'a MemoryFact]) -> HashMap<String, Vec<&'a MemoryFact>> {
    let mut groups: HashMap<String, Vec<&'a MemoryFact>> = HashMap::new();
    for fact in facts {
        let (domain, _) = parse_domain_topic(&fact.path);
        if !domain.is_empty() {
            groups.entry(domain.to_string()).or_default().push(fact);
        }
    }
    groups
}

/// Compute cosine similarity between two optional embeddings.
fn embedding_similarity(a: Option<&[f32]>, b: Option<&[f32]>) -> f32 {
    let (Some(a), Some(b)) = (a, b) else {
        return 0.0;
    };
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// Ensure a graph node exists for a fact (by fact ID as node ID).
async fn ensure_fact_node(ctx: &DreamContext, fact: &MemoryFact) -> Result<String, AlephError> {
    let (domain, topic) = parse_domain_topic(&fact.path);
    let kind = format!("fact:{}", fact.fact_type);

    // Try to find existing node, create if not found
    let node_id = format!("fact:{}", fact.id);
    ctx.graph_store
        .upsert_node_simple(&node_id, &fact.content, &kind, &format!("{}/{}", domain, topic))
        .await?;
    Ok(node_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;

    fn make_fact(id: &str, path: &str, strength: f32, embedding: Option<Vec<f32>>) -> MemoryFact {
        let mut fact = MemoryFact::with_id(id.into(), "test content".into(), FactType::Other);
        fact.path = path.into();
        fact.strength = strength;
        fact.embedding = embedding;
        fact
    }

    #[test]
    fn group_by_topic_groups_correctly() {
        let facts = vec![
            make_fact("1", "aleph://user/preferences/a", 1.0, None),
            make_fact("2", "aleph://knowledge/preferences/b", 1.0, None),
            make_fact("3", "aleph://user/projects/c", 1.0, None),
        ];

        let groups = group_by_topic(&facts);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups["preferences"].len(), 2);
        assert_eq!(groups["projects"].len(), 1);
    }

    #[test]
    fn group_by_domain_groups_correctly() {
        let f1 = make_fact("1", "aleph://user/preferences/a", 1.0, None);
        let f2 = make_fact("2", "aleph://knowledge/preferences/b", 1.0, None);
        let f3 = make_fact("3", "aleph://user/projects/c", 1.0, None);
        let refs: Vec<&MemoryFact> = vec![&f1, &f2, &f3];

        let groups = group_by_domain(&refs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups["user"].len(), 2);
        assert_eq!(groups["knowledge"].len(), 1);
    }

    #[test]
    fn embedding_similarity_identical_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = embedding_similarity(Some(&a), Some(&b));
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn embedding_similarity_orthogonal_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = embedding_similarity(Some(&a), Some(&b));
        assert!(sim.abs() < 0.001);
    }

    #[test]
    fn embedding_similarity_none_returns_zero() {
        assert_eq!(embedding_similarity(None, Some(&[1.0])), 0.0);
        assert_eq!(embedding_similarity(Some(&[1.0]), None), 0.0);
        assert_eq!(embedding_similarity(None, None), 0.0);
    }
}
```

- [ ] **Step 2: Register in stages/mod.rs**

In `src/memory/dreaming/stages/mod.rs`, add:

```rust
pub mod tunnel;
```

And in the re-exports section:

```rust
pub use tunnel::TunnelDiscoveryStage;
```

- [ ] **Step 3: Add to daily pipeline**

In `src/memory/dreaming/mod.rs`, update the `daily()` method:

```rust
    pub fn daily() -> Self {
        Self::new()
            .stage(CollectStage)
            .stage(ClusterStage)
            .stage(SummarizeStage)
            .stage(DriftDetectStage)
            .stage(ConsolidateStage)
            .stage(TunnelDiscoveryStage)
            .stage(DecayStage)
    }
```

Also add to the `use stages::` import at line 36:

```rust
pub use stages::TunnelDiscoveryStage;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- dreaming::stages::tunnel::tests --nocapture`
Expected: All 5 tests PASS

- [ ] **Step 5: Run compile check for full pipeline**

Run: `cargo check -p alephcore`
Expected: Compiles without errors

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/stages/tunnel.rs src/memory/dreaming/stages/mod.rs src/memory/dreaming/mod.rs
git commit -m "feat(memory): add TunnelDiscoveryStage to DreamDaemon pipeline"
```

---

### Task 12: Ripple — Tunnel-Aware Traversal

**Files:**
- Modify: `src/memory/ripple/config.rs`
- Modify: `src/memory/ripple/task.rs`

- [ ] **Step 1: Add tunnel config fields**

In `src/memory/ripple/config.rs`, add to `RippleConfig`:

```rust
    /// Enable cross-domain traversal via tunnel edges (default: true).
    pub enable_tunnels: bool,

    /// Maximum tunnel hops per ripple (default: 1).
    pub max_tunnel_hops: usize,
```

Update the `Default` impl:

```rust
impl Default for RippleConfig {
    fn default() -> Self {
        Self {
            max_hops: 2,
            max_facts_per_hop: 5,
            similarity_threshold: 0.7,
            enable_tunnels: true,
            max_tunnel_hops: 1,
        }
    }
}
```

- [ ] **Step 2: Add tunnel hop to RippleResult**

In `src/memory/ripple/config.rs`, add to `RippleResult`:

```rust
    /// Facts discovered via tunnel edges (cross-domain).
    pub tunnel_facts: Vec<MemoryFact>,
```

- [ ] **Step 3: Add tunnel traversal to RippleTask**

In `src/memory/ripple/task.rs`, add a new method to `impl RippleTask`:

```rust
    /// Explore cross-domain facts via tunnel edges in the knowledge graph.
    ///
    /// For each seed fact, queries the graph for "tunnel" edges and retrieves
    /// facts from the connected domains.
    pub async fn explore_tunnels(
        &self,
        seed_facts: &[MemoryFact],
        graph_store: &crate::memory::graph::GraphStore,
        visited: &mut HashSet<String>,
    ) -> Result<Vec<MemoryFact>, crate::error::AlephError> {
        if !self.config.enable_tunnels {
            return Ok(Vec::new());
        }

        let mut tunnel_facts = Vec::new();

        for fact in seed_facts {
            let node_id = format!("fact:{}", fact.id);

            // Query graph for tunnel edges from this node
            let edges = graph_store.get_edges_by_relation(&node_id, "tunnel").await?;

            for edge in edges {
                // Resolve the target node to a fact ID
                let target_node = if edge.from_id == node_id {
                    &edge.to_id
                } else {
                    &edge.from_id
                };

                // Extract fact ID from node ID ("fact:uuid" → "uuid")
                let target_fact_id = target_node.strip_prefix("fact:").unwrap_or(target_node);

                if visited.contains(target_fact_id) {
                    continue;
                }

                // Retrieve the target fact
                if let Some(target_fact) = self.database.get_fact_by_id(target_fact_id).await? {
                    if target_fact.is_valid && target_fact.is_currently_valid() {
                        visited.insert(target_fact_id.to_string());
                        tunnel_facts.push(target_fact);
                    }
                }
            }

            // Respect max_tunnel_hops (for now, 1 hop = direct tunnel neighbors)
            if tunnel_facts.len() >= self.config.max_facts_per_hop * self.config.max_tunnel_hops {
                break;
            }
        }

        Ok(tunnel_facts)
    }
```

- [ ] **Step 4: Write tests**

Add to the existing test module in `src/memory/ripple/task.rs`:

```rust
    #[test]
    fn default_config_enables_tunnels() {
        let config = super::super::config::RippleConfig::default();
        assert!(config.enable_tunnels);
        assert_eq!(config.max_tunnel_hops, 1);
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- ripple --nocapture`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/ripple/config.rs src/memory/ripple/task.rs
git commit -m "feat(memory): add tunnel-aware traversal to Ripple module"
```

---

## Final Verification

### Task 13: Full Build + Test Suite

- [ ] **Step 1: Run full compile check**

Run: `cargo check -p alephcore`
Expected: Zero errors

- [ ] **Step 2: Run all memory tests**

Run: `cargo test -p alephcore --lib -- memory --nocapture`
Expected: All tests PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: Zero warnings

- [ ] **Step 4: Commit any clippy fixes**

If clippy found issues, fix them and commit:

```bash
git add -u
git commit -m "fix(memory): address clippy warnings from palace evolution changes"
```
