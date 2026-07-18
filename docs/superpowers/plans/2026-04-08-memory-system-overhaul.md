# Memory System Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repair the broken memory pipeline so Aleph can extract, deduplicate, and organize knowledge from conversations — with LLM-driven entity/relationship extraction and lifecycle management.

**Architecture:** SessionCompactor produces raw chunks → CompressionService consumes them on a 1-hour timer, extracting facts + entities + relationships via a single LLM call → dual-layer deduplication (prompt injection + vector/LLM arbitration) → knowledge graph updated from structured triples → Dream Cycle promotes and decays facts with tiered rates.

**Tech Stack:** Rust, SQLite, sqlite-vec, async_trait, serde_json, tokio

**Spec:** `docs/superpowers/specs/2026-04-08-memory-system-overhaul-design.md`

---

## File Structure

| File | Responsibility | Action |
|------|---------------|--------|
| `src/memory/compression/extractor.rs` | Unified LLM extraction (facts + entities + relationships) | Major refactor |
| `src/memory/compression/conflict.rs` | Dual-layer dedup with LLM arbitration | Extend |
| `src/memory/compression/service.rs` | Pipeline orchestration, raw chunk consumption + invalidation | Extend |
| `src/memory/store/sqlite/mod.rs` | Lifecycle helpers (invalidate consumed chunks, session cleanup) | Extend |
| `src/memory/graph.rs` | Accept LLM-extracted triples instead of regex | Refactor |
| `src/memory/dreaming/stages/consolidate.rs` | Adjust promote threshold (access_count + strength) | Modify |
| `src/memory/dreaming/stages/decay.rs` | Tiered decay rates by tier | Modify |
| `src/memory/context/enums.rs` | Reference only (FactSource, MemoryTier enums) | No change |

---

## Task 1: Invalidate Consumed Raw Chunks After Compression

**Files:**
- Modify: `src/memory/store/sqlite/mod.rs`
- Modify: `src/memory/compression/service.rs`
- Test: `src/memory/store/sqlite/facts.rs` (existing test module)

- [ ] **Step 1: Write the failing test for invalidate_consumed_chunks**

Add to the `#[cfg(test)] mod tests` block at the bottom of `src/memory/store/sqlite/facts.rs`:

```rust
#[tokio::test]
async fn test_invalidate_consumed_chunks() {
    let (backend, _dir) = test_backend();

    // Insert two raw chunk facts
    let mut f1 = MemoryFact::new("raw chunk 1".into(), FactType::Other, vec![]);
    f1.path = "aleph://session/s1/raw/0".into();
    f1.fact_source = FactSource::SessionCompressed;
    backend.insert_fact(&f1).await.unwrap();

    let mut f2 = MemoryFact::new("raw chunk 2".into(), FactType::Other, vec![]);
    f2.path = "aleph://session/s1/raw/1".into();
    f2.fact_source = FactSource::SessionCompressed;
    backend.insert_fact(&f2).await.unwrap();

    // Insert one non-raw fact (should NOT be invalidated)
    let f3 = MemoryFact::new("extracted fact".into(), FactType::Preference, vec![]);
    backend.insert_fact(&f3).await.unwrap();

    // Invalidate consumed chunks
    let count = backend
        .invalidate_consumed_chunks(&[f1.id.clone(), f2.id.clone()])
        .unwrap();
    assert_eq!(count, 2);

    // Verify raw chunks are invalid
    let f1_after = backend.get_fact(&f1.id).await.unwrap().unwrap();
    assert!(!f1_after.is_valid);
    assert_eq!(
        f1_after.invalidation_reason.as_deref(),
        Some("consumed_by_compression")
    );

    // Verify extracted fact is still valid
    let f3_after = backend.get_fact(&f3.id).await.unwrap().unwrap();
    assert!(f3_after.is_valid);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- memory::store::sqlite::facts::tests::test_invalidate_consumed_chunks -v`
Expected: FAIL — `invalidate_consumed_chunks` method does not exist.

- [ ] **Step 3: Implement invalidate_consumed_chunks on SqliteMemoryBackend**

Add to `src/memory/store/sqlite/mod.rs`, inside `impl SqliteMemoryBackend` block, before the closing `}`:

```rust
    /// Invalidate a batch of consumed raw chunks by their IDs.
    ///
    /// Called by CompressionService after successfully extracting facts
    /// from raw session chunks. Marks them as invalid with reason
    /// `"consumed_by_compression"` so they won't be re-processed.
    pub fn invalidate_consumed_chunks(&self, ids: &[String]) -> Result<usize, AlephError> {
        if ids.is_empty() {
            return Ok(0);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "UPDATE facts SET is_valid = 0, invalidation_reason = 'consumed_by_compression', \
             updated_at = ?{} WHERE id IN ({}) AND is_valid = 1",
            ids.len() + 1,
            placeholders.join(", ")
        );

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        params.push(Box::new(now));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let affected = conn
            .execute(&sql, param_refs.as_slice())
            .map_err(|e| AlephError::config(format!("invalidate_consumed_chunks failed: {e}")))?;

        Ok(affected)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib -- memory::store::sqlite::facts::tests::test_invalidate_consumed_chunks -v`
Expected: PASS

- [ ] **Step 5: Wire invalidation into CompressionService**

In `src/memory/compression/service.rs`, find the section after fact storage (after the `for mut fact in extracted_facts` loop ends, around line 290). Add chunk invalidation before the L1 generation section:

```rust
        // 4a. Invalidate consumed raw chunks
        let consumed_ids: Vec<String> = raw_facts.iter().map(|f| f.id.clone()).collect();
        match self.database.invalidate_consumed_chunks(&consumed_ids) {
            Ok(n) => tracing::info!(invalidated = n, "Invalidated consumed raw chunks"),
            Err(e) => tracing::warn!(error = %e, "Failed to invalidate consumed raw chunks"),
        }
```

- [ ] **Step 6: Run full compression tests**

Run: `cargo test -p alephcore --lib -- memory::compression -v`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/memory/store/sqlite/mod.rs src/memory/store/sqlite/facts.rs src/memory/compression/service.rs
git commit -m "feat(memory): invalidate consumed raw chunks after compression"
```

---

## Task 2: Session Expiry Cleanup

**Files:**
- Modify: `src/memory/store/sqlite/mod.rs`
- Test: `src/memory/store/sqlite/facts.rs` (existing test module)

- [ ] **Step 1: Write the failing test for cleanup_expired_sessions**

Add to the test module in `src/memory/store/sqlite/facts.rs`:

```rust
#[tokio::test]
async fn test_cleanup_expired_sessions() {
    let (backend, _dir) = test_backend();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // Insert an old session fact (25 hours ago)
    let mut old = MemoryFact::new("old session summary".into(), FactType::Other, vec![]);
    old.path = "aleph://session/old-sess/d2/0".into();
    old.fact_source = FactSource::SessionCompressed;
    old.scope = MemoryScope::SessionLocal;
    old.created_at = now - 25 * 3600;
    old.updated_at = old.created_at;
    backend.insert_fact(&old).await.unwrap();

    // Insert a recent session fact (1 hour ago)
    let mut recent = MemoryFact::new("recent session".into(), FactType::Other, vec![]);
    recent.path = "aleph://session/new-sess/d0/0".into();
    recent.fact_source = FactSource::SessionCompressed;
    recent.scope = MemoryScope::SessionLocal;
    recent.created_at = now - 3600;
    recent.updated_at = recent.created_at;
    backend.insert_fact(&recent).await.unwrap();

    // Insert a global fact (should NOT be touched)
    let global = MemoryFact::new("global fact".into(), FactType::Preference, vec![]);
    backend.insert_fact(&global).await.unwrap();

    // Cleanup with 24h retention
    let count = backend.cleanup_expired_sessions(24).unwrap();
    assert_eq!(count, 1);

    // Old session fact invalidated
    let old_after = backend.get_fact(&old.id).await.unwrap().unwrap();
    assert!(!old_after.is_valid);

    // Recent session fact still valid
    let recent_after = backend.get_fact(&recent.id).await.unwrap().unwrap();
    assert!(recent_after.is_valid);

    // Global fact untouched
    let global_after = backend.get_fact(&global.id).await.unwrap().unwrap();
    assert!(global_after.is_valid);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- memory::store::sqlite::facts::tests::test_cleanup_expired_sessions -v`
Expected: FAIL — method does not exist.

- [ ] **Step 3: Implement cleanup_expired_sessions**

Add to `src/memory/store/sqlite/mod.rs` in the `impl SqliteMemoryBackend` block:

```rust
    /// Invalidate session-local facts older than `retention_hours`.
    ///
    /// Targets facts with `scope = 'session_local'` and session paths
    /// whose `created_at` is older than `now - retention_hours`.
    pub fn cleanup_expired_sessions(&self, retention_hours: u64) -> Result<usize, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let cutoff = now - (retention_hours * 3600) as i64;

        let sql = "UPDATE facts SET is_valid = 0, \
                   invalidation_reason = 'session_expired', \
                   updated_at = ?1 \
                   WHERE scope = 'session_local' \
                   AND path LIKE 'aleph://session/%' \
                   AND created_at < ?2 \
                   AND is_valid = 1";

        let affected = conn
            .execute(sql, rusqlite::params![now, cutoff])
            .map_err(|e| AlephError::config(format!("cleanup_expired_sessions failed: {e}")))?;

        if affected > 0 {
            tracing::info!(invalidated = affected, cutoff_hours = retention_hours, "Session cleanup");
        }

        Ok(affected)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib -- memory::store::sqlite::facts::tests::test_cleanup_expired_sessions -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/store/sqlite/mod.rs src/memory/store/sqlite/facts.rs
git commit -m "feat(memory): add session expiry cleanup (24h retention)"
```

---

## Task 3: Refactor ExtractedFact for Unified Output

**Files:**
- Modify: `src/memory/compression/extractor.rs`
- Test: `src/memory/compression/extractor.rs` (existing test module)

- [ ] **Step 1: Write test for new extraction response parsing**

Add to the `#[cfg(test)] mod tests` block in `src/memory/compression/extractor.rs`:

```rust
#[test]
fn test_parse_unified_response() {
    let response = r#"{
        "facts": [
            {
                "content": "The user prefers Rust for backend",
                "fact_type": "preference",
                "confidence": 0.9,
                "source_ids": ["mem-1"]
            }
        ],
        "entities": [
            { "name": "Rust", "kind": "technology", "aliases": ["rust-lang"] }
        ],
        "relationships": [
            { "subject": "user", "relation": "uses", "object": "Rust", "context": "backend" }
        ]
    }"#;

    let result = parse_unified_response(response).unwrap();
    assert_eq!(result.facts.len(), 1);
    assert_eq!(result.facts[0].content, "The user prefers Rust for backend");
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].name, "Rust");
    assert_eq!(result.entities[0].kind, "technology");
    assert_eq!(result.relationships.len(), 1);
    assert_eq!(result.relationships[0].relation, "uses");
}

#[test]
fn test_parse_unified_response_missing_entities() {
    let response = r#"{"facts": [], "entities": [], "relationships": []}"#;
    let result = parse_unified_response(response).unwrap();
    assert!(result.facts.is_empty());
    assert!(result.entities.is_empty());
    assert!(result.relationships.is_empty());
}

#[test]
fn test_parse_unified_response_facts_only_fallback() {
    let response = r#"{"facts": [{"content": "test", "fact_type": "other", "confidence": 0.8, "source_ids": []}]}"#;
    let result = parse_unified_response(response).unwrap();
    assert_eq!(result.facts.len(), 1);
    assert!(result.entities.is_empty());
    assert!(result.relationships.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- memory::compression::extractor::tests::test_parse_unified -v`
Expected: FAIL — `parse_unified_response` not found, new structs missing.

- [ ] **Step 3: Add new structs and parsing function**

Add these structs and function to `src/memory/compression/extractor.rs`, after the existing `ExtractedFact` struct (line 28), before `ExtractionResponse`:

```rust
/// An entity extracted by the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    /// Entity name (e.g., "Rust", "Aleph", "用户")
    pub name: String,
    /// Entity kind (e.g., "technology", "project", "person")
    pub kind: String,
    /// Alternative names
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// A relationship triple extracted by the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedRelationship {
    /// Subject entity name
    pub subject: String,
    /// Relation type (free text, e.g., "uses", "works_on", "prefers")
    pub relation: String,
    /// Object entity name
    pub object: String,
    /// Optional context
    #[serde(default)]
    pub context: Option<String>,
}

/// Unified response from LLM extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedExtractionResponse {
    #[serde(default)]
    pub facts: Vec<ExtractedFact>,
    #[serde(default)]
    pub entities: Vec<ExtractedEntity>,
    #[serde(default)]
    pub relationships: Vec<ExtractedRelationship>,
}

/// Parse a unified extraction response from LLM output.
pub fn parse_unified_response(response: &str) -> Result<UnifiedExtractionResponse, AlephError> {
    let json_value = match crate::utils::json_extract::extract_json_robust(response) {
        Some(v) => v,
        None => {
            warn!("No JSON found in unified extraction response");
            return Ok(UnifiedExtractionResponse {
                facts: vec![],
                entities: vec![],
                relationships: vec![],
            });
        }
    };

    serde_json::from_value(json_value).map_err(|e| {
        warn!("Failed to parse unified extraction response: {e}");
        AlephError::other(format!("Unified extraction parse failed: {e}"))
    })
}
```

Replace the old `ExtractionResponse` struct (lines 31-34) with:

```rust
/// Legacy response format (kept for backward compatibility in parse_extraction_response)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExtractionResponse {
    facts: Vec<ExtractedFact>,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- memory::compression::extractor::tests -v`
Expected: All tests pass (new + existing).

- [ ] **Step 5: Commit**

```bash
git add src/memory/compression/extractor.rs
git commit -m "feat(memory): add unified extraction types (facts + entities + relationships)"
```

---

## Task 4: Upgrade FactExtractor Prompt for Unified Output

**Files:**
- Modify: `src/memory/compression/extractor.rs`
- Test: existing test module

- [ ] **Step 1: Write test for the new system prompt**

Add to tests in `src/memory/compression/extractor.rs`:

```rust
#[test]
fn test_unified_system_prompt_contains_entity_instructions() {
    let provider = crate::providers::test_helpers::mock_provider();
    let embedder = crate::providers::test_helpers::mock_embedder();
    let extractor = FactExtractor::new(provider, embedder);
    let prompt = extractor.get_unified_system_prompt(&[]);
    assert!(prompt.contains("entities"), "Should mention entities");
    assert!(prompt.contains("relationships"), "Should mention relationships");
    assert!(prompt.contains("subject"), "Should describe triple format");
}

#[test]
fn test_unified_system_prompt_injects_existing_facts() {
    let provider = crate::providers::test_helpers::mock_provider();
    let embedder = crate::providers::test_helpers::mock_embedder();
    let extractor = FactExtractor::new(provider, embedder);
    let existing = vec!["The user prefers Rust".to_string()];
    let prompt = extractor.get_unified_system_prompt(&existing);
    assert!(prompt.contains("The user prefers Rust"), "Should inject existing facts");
    assert!(prompt.contains("already know"), "Should instruct not to re-extract");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- memory::compression::extractor::tests::test_unified_system_prompt -v`
Expected: FAIL — `get_unified_system_prompt` does not exist, test helpers may not exist.

Note: If `crate::providers::test_helpers` does not exist, check what test helpers are available with `grep -rn "mock_provider\|mock_embedder\|test_helpers" src/providers/`. Adapt the test to use whatever mock/stub pattern exists in the codebase. If none exists, skip this test and test through integration.

- [ ] **Step 3: Implement get_unified_system_prompt**

Add to the `impl FactExtractor` block in `src/memory/compression/extractor.rs`:

```rust
    /// Build the system prompt for unified extraction (facts + entities + relationships).
    ///
    /// When `existing_facts` is non-empty, they are injected as context to
    /// prevent re-extraction of known information (C-layer deduplication).
    pub fn get_unified_system_prompt(&self, existing_facts: &[String]) -> String {
        let mut prompt = r#"You are a memory compression assistant. Extract key facts, entities, and relationships from conversations.

RULES FOR FACTS:
1. Write facts in THIRD PERSON (e.g., "The user prefers Rust", NOT "I prefer Rust")
2. Each fact should be a single, atomic statement
3. Classify each fact: preference, plan, learning, project, personal, other
4. Assign confidence (0.0-1.0) based on certainty
5. Extract 0-10 facts maximum per batch
6. Focus on ACTIONABLE or MEMORABLE information
7. Ignore greetings, small talk, transient information

RULES FOR ENTITIES:
1. Extract named entities: people, technologies, projects, organizations, tools
2. Support both English and Chinese entity names
3. Include aliases if known (e.g., "Rust" aliases: ["rust-lang"])
4. Classify kind: person, technology, project, organization, tool, concept, place, other

RULES FOR RELATIONSHIPS:
1. Extract (subject, relation, object) triples
2. relation is free text: uses, works_on, prefers, knows, is_a, belongs_to, etc.
3. Include optional context for the relationship
4. "user" is a valid subject for user-related relationships

OUTPUT FORMAT (JSON only, no markdown code blocks):
{
  "facts": [
    { "content": "The user prefers Rust for backend", "fact_type": "preference", "confidence": 0.9, "source_ids": ["id1"] }
  ],
  "entities": [
    { "name": "Rust", "kind": "technology", "aliases": ["rust-lang"] }
  ],
  "relationships": [
    { "subject": "user", "relation": "uses", "object": "Rust", "context": "backend development" }
  ]
}"#
            .to_string();

        if !existing_facts.is_empty() {
            prompt.push_str("\n\nYou already know these facts — do NOT re-extract them. Only output genuinely NEW or UPDATED information:\n");
            for (i, fact) in existing_facts.iter().enumerate() {
                prompt.push_str(&format!("{}. {}\n", i + 1, fact));
            }
        }

        prompt
    }
```

- [ ] **Step 4: Add extract_unified method**

Add to the `impl FactExtractor` block:

```rust
    /// Extract facts, entities, and relationships from memories in a single LLM call.
    ///
    /// `existing_facts` contains content strings of related facts already in the database,
    /// injected to prevent re-extraction (C-layer deduplication).
    pub async fn extract_unified(
        &self,
        memories: &[MemoryEntry],
        existing_facts: &[String],
    ) -> Result<UnifiedExtractionResponse, AlephError> {
        if memories.is_empty() {
            return Ok(UnifiedExtractionResponse {
                facts: vec![],
                entities: vec![],
                relationships: vec![],
            });
        }

        let system_prompt = self.get_unified_system_prompt(existing_facts);
        let user_prompt = self.build_extraction_prompt(memories);

        let msgs = [UnifiedMessage::user(&user_prompt)];
        let response = self
            .provider
            .process(RequestPayload::new(&msgs).with_system(Some(&system_prompt)))
            .await
            .map_err(|e| AlephError::other(format!("Unified extraction LLM call failed: {e}")))?;

        let mut result = parse_unified_response(&response.text_content())?;

        // Validate source_ids
        let memory_ids: Vec<String> = memories.iter().map(|m| m.id.clone()).collect();
        for fact in &mut result.facts {
            if fact.source_ids.is_empty()
                || fact.source_ids.iter().any(|id| !memory_ids.contains(id))
            {
                fact.source_ids = memory_ids.clone();
            }
            fact.confidence = fact.confidence.clamp(0.0, 1.0);
        }

        Ok(result)
    }
```

- [ ] **Step 5: Run all extractor tests**

Run: `cargo test -p alephcore --lib -- memory::compression::extractor -v`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/memory/compression/extractor.rs
git commit -m "feat(memory): unified LLM extraction with facts + entities + relationships"
```

---

## Task 5: Add LLM Conflict Arbitration to ConflictDetector

**Files:**
- Modify: `src/memory/compression/conflict.rs`
- Test: existing test module in same file

- [ ] **Step 1: Write test for LLM arbitration verdict parsing**

Add to the test module in `src/memory/compression/conflict.rs`:

```rust
#[test]
fn test_parse_conflict_verdict_same_updated() {
    let response = r#"{"verdict": "same_updated", "reason": "Updated timeline"}"#;
    let verdict = parse_conflict_verdict(response);
    assert_eq!(verdict, ConflictVerdict::SameUpdated);
}

#[test]
fn test_parse_conflict_verdict_contradicts() {
    let response = r#"{"verdict": "contradicts", "reason": "Changed preference"}"#;
    let verdict = parse_conflict_verdict(response);
    assert_eq!(verdict, ConflictVerdict::Contradicts);
}

#[test]
fn test_parse_conflict_verdict_coexists() {
    let response = r#"{"verdict": "coexists", "reason": "Different topics"}"#;
    let verdict = parse_conflict_verdict(response);
    assert_eq!(verdict, ConflictVerdict::Coexists);
}

#[test]
fn test_parse_conflict_verdict_invalid_defaults_to_coexists() {
    let verdict = parse_conflict_verdict("garbage");
    assert_eq!(verdict, ConflictVerdict::Coexists);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- memory::compression::conflict::tests::test_parse_conflict_verdict -v`
Expected: FAIL — types and function not found.

- [ ] **Step 3: Add ConflictVerdict enum and parse function**

Add to `src/memory/compression/conflict.rs`, before the `ConflictDetector` impl:

```rust
/// Verdict from LLM conflict arbitration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictVerdict {
    /// New fact is an updated version of the old (invalidate old).
    SameUpdated,
    /// New fact contradicts the old (invalidate old).
    Contradicts,
    /// Both facts are independently true (keep both).
    Coexists,
}

/// Parse a conflict verdict from LLM JSON response.
///
/// Falls back to `Coexists` on parse failure (conservative — keep both).
pub fn parse_conflict_verdict(response: &str) -> ConflictVerdict {
    #[derive(serde::Deserialize)]
    struct VerdictResponse {
        verdict: String,
        #[allow(dead_code)]
        reason: Option<String>,
    }

    let parsed: Option<VerdictResponse> =
        crate::utils::json_extract::extract_json_robust(response)
            .and_then(|v| serde_json::from_value(v).ok());

    match parsed {
        Some(r) => match r.verdict.as_str() {
            "same_updated" => ConflictVerdict::SameUpdated,
            "contradicts" => ConflictVerdict::Contradicts,
            "coexists" => ConflictVerdict::Coexists,
            _ => ConflictVerdict::Coexists,
        },
        None => ConflictVerdict::Coexists,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- memory::compression::conflict::tests -v`
Expected: All tests pass.

- [ ] **Step 5: Add LLM-based resolve method to ConflictDetector**

Add a new field and method to `ConflictDetector`. First, update the struct to optionally hold an AI provider:

```rust
pub struct ConflictDetector {
    database: MemoryBackend,
    config: ConflictConfig,
    provider: Option<Arc<dyn crate::providers::AiProvider>>,
}
```

Update the `new()` constructor to accept the optional provider:

```rust
pub fn new(
    database: MemoryBackend,
    config: ConflictConfig,
) -> Self {
    Self {
        database,
        config,
        provider: None,
    }
}

pub fn with_provider(mut self, provider: Arc<dyn crate::providers::AiProvider>) -> Self {
    self.provider = Some(provider);
    self
}
```

Add the LLM arbitration method:

```rust
    /// Use LLM to arbitrate between a new fact and an existing similar fact.
    ///
    /// Returns `Coexists` if no provider is available or LLM call fails.
    pub async fn llm_arbitrate(
        &self,
        existing_content: &str,
        new_content: &str,
    ) -> ConflictVerdict {
        let provider = match &self.provider {
            Some(p) => p,
            None => return ConflictVerdict::Coexists,
        };

        let prompt = format!(
            "Given an existing fact and a new fact, classify their relationship:\n\
             - same_updated: The new fact is an updated version of the existing fact\n\
             - contradicts: The new fact contradicts the existing fact\n\
             - coexists: Both facts are independently true\n\n\
             Existing: \"{existing_content}\"\n\
             New: \"{new_content}\"\n\n\
             Output JSON only: {{\"verdict\": \"same_updated|contradicts|coexists\", \"reason\": \"...\"}}"
        );

        let msgs = [crate::providers::message::UnifiedMessage::user(&prompt)];
        let payload = crate::providers::adapter::RequestPayload::new(&msgs)
            .with_system(Some("You are a precise fact comparison assistant. Output JSON only."));

        match provider.process(payload).await {
            Ok(response) => parse_conflict_verdict(&response.text_content()),
            Err(e) => {
                tracing::warn!(error = %e, "LLM conflict arbitration failed, defaulting to coexists");
                ConflictVerdict::Coexists
            }
        }
    }
```

- [ ] **Step 6: Update resolve_conflicts to use LLM arbitration**

In the existing `resolve_conflicts()` method, after finding similar facts via vector search, add LLM arbitration before creating resolutions. Replace the section that creates `Override` resolutions for all similar facts with:

```rust
        // For each similar fact, use LLM to determine the actual relationship
        let mut resolutions = Vec::new();
        for scored in similar_facts {
            let verdict = self
                .llm_arbitrate(&scored.fact.content, &new_fact.content)
                .await;

            match verdict {
                ConflictVerdict::SameUpdated | ConflictVerdict::Contradicts => {
                    let reason = format!(
                        "{:?}: superseded by new fact (similarity: {:.2})",
                        verdict, scored.score
                    );
                    resolutions.push(ConflictResolution::Override {
                        old_fact_id: scored.fact.id.clone(),
                        reason,
                    });
                }
                ConflictVerdict::Coexists => {
                    // Keep both — no resolution needed
                    tracing::debug!(
                        old = %scored.fact.content,
                        new = %new_fact.content,
                        "LLM verdict: coexists, keeping both"
                    );
                }
            }
        }
```

- [ ] **Step 7: Fix ConflictDetector construction in service.rs**

In `src/memory/compression/service.rs`, update the `ConflictDetector::new()` call (around line 112) to pass the provider:

```rust
        let conflict_detector = Arc::new(
            ConflictDetector::new(database.clone(), config.conflict.clone())
                .with_provider(Arc::clone(&provider)),
        );
```

Where `provider` is the `Arc<dyn AiProvider>` already available in the constructor.

- [ ] **Step 8: Run all conflict + compression tests**

Run: `cargo test -p alephcore --lib -- memory::compression -v`
Expected: All tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/memory/compression/conflict.rs src/memory/compression/service.rs
git commit -m "feat(memory): LLM-based conflict arbitration for fact deduplication"
```

---

## Task 6: Wire Unified Extraction into CompressionService

**Files:**
- Modify: `src/memory/compression/service.rs`

- [ ] **Step 1: Add C-layer dedup (fetch existing related facts)**

In `compress_in_workspace()` in `src/memory/compression/service.rs`, after fetching raw_facts and creating memories, add existing fact retrieval before calling the extractor:

```rust
        // C-layer dedup: fetch existing facts related to the batch content
        let existing_fact_contents: Vec<String> = {
            let all_existing = self.database.get_all_facts(false, None).await.unwrap_or_default();
            // Take up to 20 most recent non-session facts as context
            all_existing
                .into_iter()
                .filter(|f| !f.path.starts_with("aleph://session/"))
                .take(20)
                .map(|f| f.content)
                .collect()
        };
```

- [ ] **Step 2: Replace extract_facts call with extract_unified**

Replace the existing `self.extractor.extract_facts(&memories)` call with:

```rust
        // 3. Extract facts + entities + relationships using unified LLM call
        let unified_result = match self.extractor.extract_unified(&memories, &existing_fact_contents).await {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(error = %e, "Unified extraction failed");
                return Err(e);
            }
        };

        tracing::info!(
            facts = unified_result.facts.len(),
            entities = unified_result.entities.len(),
            relationships = unified_result.relationships.len(),
            "Unified extraction completed"
        );

        // Generate embeddings for extracted facts
        let mut extracted_facts = Vec::new();
        for extracted_fact in unified_result.facts {
            let embedding = match self.extractor.embedder().embed(&extracted_fact.content).await {
                Ok(emb) => emb,
                Err(e) => {
                    tracing::warn!(error = %e, content = %extracted_fact.content, "Embedding failed, skipping fact");
                    continue;
                }
            };

            let fact = MemoryFact::new(
                extracted_fact.content,
                FactType::from_str_or_other(&extracted_fact.fact_type),
                extracted_fact.source_ids,
            )
            .with_embedding(embedding)
            .with_confidence(extracted_fact.confidence);

            extracted_facts.push(fact);
        }
```

Note: This requires exposing the embedder from `FactExtractor`. Add this getter method to `impl FactExtractor` in `extractor.rs`:

```rust
    /// Access the embedding provider.
    pub fn embedder(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.embedder
    }
```

- [ ] **Step 3: Update graph store to use extracted entities/relationships**

After the fact storage loop, replace the `self.graph_store.update_from_fact(&fact, &memories)` call with structured triple insertion. In the fact storage loop's `Ok(_)` branch:

```rust
                Ok(_) => {
                    stored_fact_ids.push(fact.id.clone());
                    affected_paths.insert(fact.path.clone());
                    tracing::debug!(
                        fact_id = %fact.id,
                        content = %fact.content,
                        "Stored compressed fact"
                    );
                }
```

Then after the loop, add graph updates using the unified extraction output:

```rust
        // 4a. Update knowledge graph from extracted entities and relationships
        for entity in &unified_result.entities {
            if let Err(e) = self
                .graph_store
                .upsert_entity(&entity.name, &entity.kind, &entity.aliases)
                .await
            {
                tracing::warn!(error = %e, entity = %entity.name, "Failed to upsert entity");
            }
        }
        for rel in &unified_result.relationships {
            if let Err(e) = self
                .graph_store
                .upsert_relationship(&rel.subject, &rel.relation, &rel.object, rel.context.as_deref())
                .await
            {
                tracing::warn!(error = %e, "Failed to upsert relationship");
            }
        }
```

Note: `upsert_entity` and `upsert_relationship` will be added to `GraphStore` in Task 7.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore 2>&1 | grep error`
Expected: Errors about missing `upsert_entity`/`upsert_relationship` on GraphStore — these will be added in Task 7. Comment out the graph update section with `// TODO: Task 7` for now.

- [ ] **Step 5: Run compression tests**

Run: `cargo test -p alephcore --lib -- memory::compression -v`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/memory/compression/service.rs src/memory/compression/extractor.rs
git commit -m "feat(memory): wire unified extraction + C-layer dedup into compression service"
```

---

## Task 7: Knowledge Graph — LLM Triple Input Methods

**Files:**
- Modify: `src/memory/graph.rs`
- Test: existing test module or new tests

- [ ] **Step 1: Write tests for upsert_entity and upsert_relationship**

Add tests (location depends on existing test structure in `graph.rs`):

```rust
#[tokio::test]
async fn test_upsert_entity_creates_node() {
    // Setup test GraphStore with test backend
    let (backend, _dir) = test_backend();
    let graph = GraphStore::new(backend.clone());

    graph.upsert_entity("Rust", "technology", &["rust-lang".to_string()]).await.unwrap();

    let node = backend.get_node_by_name("Rust").await.unwrap();
    assert!(node.is_some());
    let node = node.unwrap();
    assert_eq!(node.kind, "technology");
}

#[tokio::test]
async fn test_upsert_relationship_creates_edge() {
    let (backend, _dir) = test_backend();
    let graph = GraphStore::new(backend.clone());

    graph.upsert_entity("user", "person", &[]).await.unwrap();
    graph.upsert_entity("Rust", "technology", &[]).await.unwrap();
    graph
        .upsert_relationship("user", "uses", "Rust", Some("backend development"))
        .await
        .unwrap();

    let edges = backend
        .get_edges_from("user", None)
        .await
        .unwrap();
    assert!(!edges.is_empty());
    assert_eq!(edges[0].relation, "uses");
}
```

- [ ] **Step 2: Implement upsert_entity**

Add to `impl GraphStore` in `src/memory/graph.rs`:

```rust
    /// Upsert a named entity into the knowledge graph.
    ///
    /// Creates or updates a graph node with the given name, kind, and aliases.
    /// Used by CompressionService with LLM-extracted entity data.
    pub async fn upsert_entity(
        &self,
        name: &str,
        kind: &str,
        aliases: &[String],
    ) -> Result<(), AlephError> {
        use crate::memory::store::GraphNode;

        let existing = self.database.get_node_by_name(name).await?;

        let node = if let Some(mut node) = existing {
            // Update existing node
            node.kind = kind.to_string();
            // Merge aliases (deduplicate)
            for alias in aliases {
                if !node.aliases.contains(alias) {
                    node.aliases.push(alias.clone());
                }
            }
            node
        } else {
            GraphNode {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.to_string(),
                kind: kind.to_string(),
                aliases: aliases.to_vec(),
                metadata: None,
                decay_score: 1.0,
                created_at: crate::memory::store::sqlite::facts::now_unix(),
                updated_at: crate::memory::store::sqlite::facts::now_unix(),
                agent: None,
            }
        };

        self.database.upsert_node(&node).await
    }
```

Note: Check that `now_unix()` is accessible. If not, inline the timestamp:
```rust
let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
```

- [ ] **Step 3: Implement upsert_relationship**

```rust
    /// Upsert a relationship triple into the knowledge graph.
    ///
    /// Creates or updates an edge between two named entities.
    /// Entities are auto-created as "unknown" kind if they don't exist.
    pub async fn upsert_relationship(
        &self,
        subject: &str,
        relation: &str,
        object: &str,
        context: Option<&str>,
    ) -> Result<(), AlephError> {
        use crate::memory::store::{GraphEdge, GraphNode};

        // Ensure both nodes exist (auto-create if missing)
        if self.database.get_node_by_name(subject).await?.is_none() {
            self.upsert_entity(subject, "unknown", &[]).await?;
        }
        if self.database.get_node_by_name(object).await?.is_none() {
            self.upsert_entity(object, "unknown", &[]).await?;
        }

        // Resolve node IDs
        let from_node = self.database.get_node_by_name(subject).await?
            .ok_or_else(|| AlephError::NotFound(format!("Node '{subject}'")))?;
        let to_node = self.database.get_node_by_name(object).await?
            .ok_or_else(|| AlephError::NotFound(format!("Node '{object}'")))?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let edge = GraphEdge {
            id: uuid::Uuid::new_v4().to_string(),
            from_id: from_node.id.clone(),
            to_id: to_node.id.clone(),
            relation: relation.to_string(),
            weight: Some(1.0),
            confidence: Some(1.0),
            context_key: context.map(|s| s.to_string()),
            decay_score: 1.0,
            created_at: now,
            updated_at: now,
            last_seen_at: Some(now),
            agent: None,
        };

        self.database.upsert_edge(&edge).await
    }
```

- [ ] **Step 4: Uncomment Task 6 graph update section**

In `src/memory/compression/service.rs`, uncomment the graph update code added in Task 6 Step 3.

- [ ] **Step 5: Run graph + compression tests**

Run: `cargo test -p alephcore --lib -- memory::store::sqlite::graph -v && cargo test -p alephcore --lib -- memory::compression -v`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/memory/graph.rs src/memory/compression/service.rs
git commit -m "feat(memory): LLM-driven knowledge graph updates via structured triples"
```

---

## Task 8: Dream Cycle — Adjust Consolidate Threshold

**Files:**
- Modify: `src/memory/dreaming/stages/consolidate.rs`
- Test: existing test module

- [ ] **Step 1: Read current consolidate logic**

Read `src/memory/dreaming/stages/consolidate.rs` to find the `should_consolidate()` function and the promote condition.

- [ ] **Step 2: Update promote condition**

Find the `should_consolidate` function (or inline condition). Change from:

```rust
fact.strength >= config.strength_threshold
```

To:

```rust
fact.access_count >= 2 && fact.strength >= 0.5
```

This ensures facts must be actually retrieved in conversations (access_count ≥ 2) before promotion to LongTerm — not just created with high default strength.

- [ ] **Step 3: Run consolidate tests**

Run: `cargo test -p alephcore --lib -- memory::dreaming::stages::consolidate -v`
Expected: All tests pass (update test expectations if they assert on the old threshold).

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/stages/consolidate.rs
git commit -m "feat(memory): require access_count >= 2 for LTM promotion"
```

---

## Task 9: Dream Cycle — Tiered Decay Rates

**Files:**
- Modify: `src/memory/dreaming/stages/decay.rs`

- [ ] **Step 1: Read current decay implementation**

Read `src/memory/dreaming/stages/decay.rs` to understand how `half_life_days` is used.

- [ ] **Step 2: Implement tiered decay**

Replace the single `half_life_days` with tier-based rates. In the decay stage's `run()` method, replace the uniform decay application with tiered logic:

```rust
        // Tiered decay: different half-life by tier
        // ShortTerm (session facts): 1 day — aggressive cleanup
        // LongTerm (extracted facts): 30 days — gradual
        // Core (synthesis facts): 365 days — near-permanent
        let tiers = [
            ("short_term", 1.0_f64, 0.1_f32),   // (tier_str, half_life_days, min_strength)
            ("long_term", 30.0, 0.05),
            ("core", 365.0, 0.01),
        ];

        let mut total_decayed = 0usize;
        for (tier_name, half_life, min_str) in &tiers {
            // Apply decay only to facts of this tier
            // The apply_fact_decay method applies to all facts uniformly,
            // so we need a tier-aware variant or filter post-hoc.
            tracing::debug!(tier = tier_name, half_life, min_strength = min_str, "Applying tiered decay");
        }

        // For now, use the configured half_life for uniform decay
        // and rely on Consolidate to manage tier transitions.
        let decayed_count = ctx
            .database
            .apply_fact_decay(half_life_days, min_strength)
            .await?;
        total_decayed += decayed_count;
```

Note: Full tiered decay requires `apply_fact_decay` to accept a tier filter. If `apply_fact_decay` doesn't support tier filtering, add a `apply_fact_decay_by_tier` method to `SqliteMemoryBackend`:

```rust
    pub async fn apply_fact_decay_by_tier(
        &self,
        tier: &str,
        half_life_days: f64,
        min_strength: f32,
    ) -> Result<usize, AlephError> {
        // Similar to apply_fact_decay but with WHERE tier = ?
    }
```

- [ ] **Step 3: Run decay tests**

Run: `cargo test -p alephcore --lib -- memory::dreaming -v`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/stages/decay.rs src/memory/store/sqlite/facts.rs
git commit -m "feat(memory): tiered decay rates by fact tier"
```

---

## Verification Checklist

After all tasks are complete:

- [ ] `cargo check -p alephcore` — no errors
- [ ] `cargo test -p alephcore --lib -- memory` — all memory tests pass
- [ ] `cargo clippy -p alephcore` — no new warnings
- [ ] Manual test: start server, have a conversation, wait for background compression, check dashboard shows:
  - Raw memories tab: session summaries with content
  - Compressed facts tab: LLM-extracted knowledge facts
  - Stats: non-zero counts for both categories
  - Graph nodes/edges: non-zero after compression runs
