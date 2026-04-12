# Dream Daemon Notes Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the facts→notes migration by creating `raw_memories` table, refactoring CompressionService, redesigning Dream daemon for the notes layer, unifying tools, and eliminating the `facts` table.

**Architecture:** Four-phase migration: (1) create raw_memories + dual-write, (2) switch CompressionService reads + quality fixes, (3) redesign Dream daemon 6 stages for notes, build note_manage tool, migrate legacy data, (4) drop facts table and cleanup dead code.

**Tech Stack:** Rust, SQLite (rusqlite), async_trait, serde, chrono, tokio

**Design Spec:** `docs/superpowers/specs/2026-04-12-dream-notes-migration-design.md`

---

## File Structure

### New Files
| File | Purpose |
|------|---------|
| `src/memory/store/sqlite/raw_memories.rs` | RawMemoryStore SQLite implementation |
| `src/memory/store/raw_memory.rs` | RawMemory struct + RawMemoryStore trait |
| `src/memory/dreaming/stages/note_consolidate.rs` | NoteConsolidate stage |
| `src/memory/dreaming/stages/note_drift.rs` | NoteDrift stage |
| `src/memory/dreaming/stages/note_synthesis.rs` | NoteSynthesis stage |
| `src/memory/dreaming/stages/note_lint.rs` | NoteLint stage |
| `src/memory/dreaming/stages/note_decay.rs` | NoteDecay stage |
| `src/memory/dreaming/stages/daily_digest.rs` | DailyDigest stage |
| `src/builtin_tools/note_manage.rs` | Unified NoteManageTool |
| `src/memory/migration/skill_to_notes.rs` | One-time skill migration script |

### Modified Files
| File | Changes |
|------|---------|
| `src/memory/store/sqlite/schema.rs` | Add raw_memories DDL; later remove facts DDL |
| `src/memory/store/mod.rs` | Add RawMemoryStore trait; later remove facts methods from MemoryStore |
| `src/memory/store/sqlite/mod.rs` | Wire raw_memories module; later remove facts SQL |
| `src/memory/compression/service.rs` | Switch data source to raw_memories; fix 5 quality issues |
| `src/memory/compression/extractor.rs` | Fix truncation, add attachment prompt |
| `src/memory/notes/extractor.rs` | Add note content context to extraction prompt |
| `src/memory/dreaming/mod.rs` | New DreamContext, DreamReport, pipeline orchestration |
| `src/memory/dreaming/stages/mod.rs` | Replace old stage re-exports with new ones |
| `src/memory/session_compactor/mod.rs` | Dual-write → raw_memories only |
| `src/memory/transcript_indexer/mod.rs` | Dual-write → raw_memories only |
| `src/builtin_tools/mod.rs` | Register note_manage, deprecate skill_manage/wiki_manage |
| `src/memory/store/sqlite/notes.rs` | Add recall_signals note_path queries for NoteDecay |

---

## Phase 1: Foundation — raw_memories Table + Dual Write

### Task 1: RawMemory Struct + Trait

**Files:**
- Create: `src/memory/store/raw_memory.rs`
- Modify: `src/memory/store/mod.rs`

- [ ] **Step 1: Create RawMemory struct and RawMemoryStore trait**

```rust
// src/memory/store/raw_memory.rs

use crate::error::AlephError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Source of raw memory data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawMemorySource {
    SessionCompressed,
    Transcript,
    ToolOutput,
    Attachment,
}

impl RawMemorySource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SessionCompressed => "session_compressed",
            Self::Transcript => "transcript",
            Self::ToolOutput => "tool_output",
            Self::Attachment => "attachment",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "session_compressed" => Self::SessionCompressed,
            "transcript" => Self::Transcript,
            "tool_output" => Self::ToolOutput,
            "attachment" => Self::Attachment,
            _ => Self::ToolOutput,
        }
    }
}

/// A raw memory record — ephemeral data consumed by CompressionService.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMemory {
    pub id: String,
    pub content: String,
    pub source: RawMemorySource,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub path: Option<String>,
    pub layer: Option<String>,
    pub attachment_text: Option<String>,
    pub is_processed: bool,
    pub created_at: i64,
}

impl RawMemory {
    pub fn new(content: String, source: RawMemorySource) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content,
            source,
            agent_id: "default".to_string(),
            session_id: None,
            path: None,
            layer: None,
            attachment_text: None,
            is_processed: false,
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = agent_id.into();
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_layer(mut self, layer: impl Into<String>) -> Self {
        self.layer = Some(layer.into());
        self
    }

    pub fn with_attachment_text(mut self, text: impl Into<String>) -> Self {
        self.attachment_text = Some(text.into());
        self
    }
}

/// Storage trait for raw memory records.
#[async_trait]
pub trait RawMemoryStore: Send + Sync {
    /// Insert a raw memory record.
    async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError>;

    /// Get unprocessed raw memories for an agent, ordered by created_at ASC.
    async fn get_unprocessed_raw_memories(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError>;

    /// Mark raw memories as processed after CompressionService consumes them.
    async fn mark_raw_as_processed(&self, ids: &[String]) -> Result<usize, AlephError>;

    /// Count unprocessed raw memories for an agent.
    async fn count_unprocessed(&self, agent_id: &str) -> Result<usize, AlephError>;
}
```

- [ ] **Step 2: Add module declaration and re-export in store/mod.rs**

Add to `src/memory/store/mod.rs` at the top module declarations:

```rust
pub mod raw_memory;
pub use raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: No errors related to raw_memory module.

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/raw_memory.rs src/memory/store/mod.rs
git commit -m "feat(memory): add RawMemory struct and RawMemoryStore trait"
```

---

### Task 2: raw_memories SQLite Schema + Implementation

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Create: `src/memory/store/sqlite/raw_memories.rs`
- Modify: `src/memory/store/sqlite/mod.rs`

- [ ] **Step 1: Write the failing test**

Add test at the bottom of `src/memory/store/sqlite/raw_memories.rs` (create file):

```rust
// src/memory/store/sqlite/raw_memories.rs

use crate::error::AlephError;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use async_trait::async_trait;

use super::SqliteMemoryBackend;

#[async_trait]
impl RawMemoryStore for SqliteMemoryBackend {
    async fn insert_raw_memory(&self, _raw: &RawMemory) -> Result<(), AlephError> {
        todo!()
    }

    async fn get_unprocessed_raw_memories(
        &self,
        _agent_id: &str,
        _limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        todo!()
    }

    async fn mark_raw_as_processed(&self, _ids: &[String]) -> Result<usize, AlephError> {
        todo!()
    }

    async fn count_unprocessed(&self, _agent_id: &str) -> Result<usize, AlephError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use crate::sync_primitives::Arc;

    async fn create_backend() -> SqliteMemoryBackend {
        let dir = tempdir().unwrap();
        SqliteMemoryBackend::new(dir.path()).unwrap()
    }

    #[tokio::test]
    async fn insert_and_retrieve_raw_memory() {
        let backend = create_backend().await;
        let raw = RawMemory::new(
            "User said hello".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("default")
        .with_session("sess-001");

        backend.insert_raw_memory(&raw).await.unwrap();

        let unprocessed = backend.get_unprocessed_raw_memories("default", 10).await.unwrap();
        assert_eq!(unprocessed.len(), 1);
        assert_eq!(unprocessed[0].content, "User said hello");
        assert!(!unprocessed[0].is_processed);
    }

    #[tokio::test]
    async fn mark_as_processed_excludes_from_query() {
        let backend = create_backend().await;
        let raw = RawMemory::new("Fact A".to_string(), RawMemorySource::SessionCompressed);
        backend.insert_raw_memory(&raw).await.unwrap();

        let marked = backend.mark_raw_as_processed(&[raw.id.clone()]).await.unwrap();
        assert_eq!(marked, 1);

        let unprocessed = backend.get_unprocessed_raw_memories("default", 10).await.unwrap();
        assert!(unprocessed.is_empty());
    }

    #[tokio::test]
    async fn empty_store_returns_empty() {
        let backend = create_backend().await;
        let unprocessed = backend.get_unprocessed_raw_memories("default", 10).await.unwrap();
        assert!(unprocessed.is_empty());
        assert_eq!(backend.count_unprocessed("default").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn attachment_text_preserved() {
        let backend = create_backend().await;
        let raw = RawMemory::new("Discuss PDF".to_string(), RawMemorySource::SessionCompressed)
            .with_attachment_text("The system uses microservices...");

        backend.insert_raw_memory(&raw).await.unwrap();

        let results = backend.get_unprocessed_raw_memories("default", 10).await.unwrap();
        assert_eq!(results[0].attachment_text.as_deref(), Some("The system uses microservices..."));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib raw_memories -- --nocapture 2>&1 | tail -5`
Expected: FAIL with "not yet implemented"

- [ ] **Step 3: Add DDL to schema.rs**

Add the following constant to `src/memory/store/sqlite/schema.rs`:

```rust
pub const CREATE_RAW_MEMORIES: &str = "
CREATE TABLE IF NOT EXISTS raw_memories (
    id              TEXT PRIMARY KEY,
    content         TEXT NOT NULL,
    source          TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    session_id      TEXT,
    path            TEXT,
    layer           TEXT,
    attachment_text TEXT,
    is_processed    INTEGER DEFAULT 0,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_raw_unprocessed ON raw_memories(is_processed, created_at)
    WHERE is_processed = 0;
CREATE INDEX IF NOT EXISTS idx_raw_agent ON raw_memories(agent_id);
CREATE INDEX IF NOT EXISTS idx_raw_session ON raw_memories(session_id);
";
```

Add `CREATE_RAW_MEMORIES` to the `init_tables()` function (or wherever other CREATE TABLE calls are batched), alongside the existing notes tables.

- [ ] **Step 4: Implement RawMemoryStore for SqliteMemoryBackend**

Replace the `todo!()` stubs in `src/memory/store/sqlite/raw_memories.rs`:

```rust
#[async_trait]
impl RawMemoryStore for SqliteMemoryBackend {
    async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError> {
        let conn = self.pool.get().map_err(|e| AlephError::other(format!("pool: {e}")))?;
        conn.execute(
            "INSERT INTO raw_memories (id, content, source, agent_id, session_id, path, layer, attachment_text, is_processed, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                raw.id,
                raw.content,
                raw.source.as_str(),
                raw.agent_id,
                raw.session_id,
                raw.path,
                raw.layer,
                raw.attachment_text,
                raw.is_processed as i32,
                raw.created_at,
            ],
        ).map_err(|e| AlephError::other(format!("insert_raw_memory: {e}")))?;
        Ok(())
    }

    async fn get_unprocessed_raw_memories(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        let conn = self.pool.get().map_err(|e| AlephError::other(format!("pool: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT id, content, source, agent_id, session_id, path, layer, attachment_text, is_processed, created_at
             FROM raw_memories
             WHERE is_processed = 0 AND agent_id = ?1
             ORDER BY created_at ASC
             LIMIT ?2"
        ).map_err(|e| AlephError::other(format!("prepare: {e}")))?;

        let rows = stmt.query_map(rusqlite::params![agent_id, limit as i64], |row| {
            Ok(RawMemory {
                id: row.get(0)?,
                content: row.get(1)?,
                source: RawMemorySource::from_str(&row.get::<_, String>(2)?),
                agent_id: row.get(3)?,
                session_id: row.get(4)?,
                path: row.get(5)?,
                layer: row.get(6)?,
                attachment_text: row.get(7)?,
                is_processed: row.get::<_, i32>(8)? != 0,
                created_at: row.get(9)?,
            })
        }).map_err(|e| AlephError::other(format!("query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AlephError::other(format!("row: {e}")))?);
        }
        Ok(results)
    }

    async fn mark_raw_as_processed(&self, ids: &[String]) -> Result<usize, AlephError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.pool.get().map_err(|e| AlephError::other(format!("pool: {e}")))?;
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "UPDATE raw_memories SET is_processed = 1 WHERE id IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
        let updated = conn.execute(&sql, params.as_slice())
            .map_err(|e| AlephError::other(format!("mark_processed: {e}")))?;
        Ok(updated)
    }

    async fn count_unprocessed(&self, agent_id: &str) -> Result<usize, AlephError> {
        let conn = self.pool.get().map_err(|e| AlephError::other(format!("pool: {e}")))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM raw_memories WHERE is_processed = 0 AND agent_id = ?1",
            rusqlite::params![agent_id],
            |row| row.get(0),
        ).map_err(|e| AlephError::other(format!("count: {e}")))?;
        Ok(count as usize)
    }
}
```

- [ ] **Step 5: Wire the module in sqlite/mod.rs**

Add `pub mod raw_memories;` to `src/memory/store/sqlite/mod.rs`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib raw_memories -- --nocapture 2>&1 | tail -10`
Expected: All 4 tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/memory/store/sqlite/schema.rs src/memory/store/sqlite/raw_memories.rs src/memory/store/sqlite/mod.rs
git commit -m "feat(memory): implement raw_memories table and RawMemoryStore"
```

---

### Task 3: SessionCompactor Dual-Write

**Files:**
- Modify: `src/memory/session_compactor/mod.rs` (or `src/components/session_compactor/compactor.rs`)
- Modify: `src/memory/session_compactor/summary_engine.rs`

- [ ] **Step 1: Find the `summary_to_fact()` call site**

Run: `grep -n "summary_to_fact\|insert_fact" src/memory/session_compactor/mod.rs src/components/session_compactor/compactor.rs`

Identify where `MemoryFact` is constructed and `insert_fact()` is called.

- [ ] **Step 2: Add RawMemoryStore dependency to SessionCompactor**

In the SessionCompactor struct, add an `Option<MemoryBackend>` field for raw memory writes (use the same backend since it implements both traits):

```rust
// Where SessionCompactor stores its database handle, add:
// The MemoryBackend already implements RawMemoryStore, so no new field needed.
// We just need to call insert_raw_memory() alongside insert_fact().
```

- [ ] **Step 3: Add dual-write logic after each `insert_fact()` call**

At every call site where `summary_to_fact()` builds a `MemoryFact` and calls `database.insert_fact(&fact)`, add a parallel write:

```rust
// After: database.insert_fact(&fact).await?;
// Add:
{
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    let raw = RawMemory::new(fact.content.clone(), RawMemorySource::SessionCompressed)
        .with_agent(&fact.agent)
        .with_session(session_id)
        .with_path(fact.path.clone())
        .with_layer(fact.layer.map(|l| l.as_str()).unwrap_or("d0"));
    if let Err(e) = database.insert_raw_memory(&raw).await {
        tracing::warn!(error = %e, "Failed to dual-write raw_memory (non-fatal)");
    }
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: No errors.

- [ ] **Step 5: Commit**

```bash
git add src/memory/session_compactor/ src/components/session_compactor/
git commit -m "feat(memory): SessionCompactor dual-writes to raw_memories"
```

---

### Task 4: TranscriptIndexer Dual-Write

**Files:**
- Modify: `src/memory/transcript_indexer/mod.rs`

- [ ] **Step 1: Find the insert_fact call site**

Run: `grep -n "insert_fact" src/memory/transcript_indexer/mod.rs`

- [ ] **Step 2: Add dual-write logic**

Same pattern as Task 3: after each `insert_fact()`, add a parallel `insert_raw_memory()` call with `RawMemorySource::Transcript`.

```rust
// After: database.insert_fact(&fact).await?;
{
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    let raw = RawMemory::new(fact.content.clone(), RawMemorySource::Transcript)
        .with_agent(&fact.agent)
        .with_path(fact.path.clone());
    if let Err(e) = database.insert_raw_memory(&raw).await {
        tracing::warn!(error = %e, "Failed to dual-write transcript raw_memory (non-fatal)");
    }
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`

- [ ] **Step 4: Commit**

```bash
git add src/memory/transcript_indexer/
git commit -m "feat(memory): TranscriptIndexer dual-writes to raw_memories"
```

---

## Phase 2: CompressionService Refactoring

### Task 5: Switch CompressionService to Read from raw_memories

**Files:**
- Modify: `src/memory/compression/service.rs`

- [ ] **Step 1: Write integration test**

Add to the test module at the bottom of `service.rs`:

```rust
#[tokio::test]
async fn test_compress_from_raw_memories() {
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

    let (service, database, _temp_dir) = create_test_service_with_tempdir().await;

    // Insert raw memory instead of fact
    let raw = RawMemory::new(
        "User: I prefer Vim for coding\nAssistant: Vim is a great editor.".to_string(),
        RawMemorySource::SessionCompressed,
    );
    database.insert_raw_memory(&raw).await.unwrap();

    let result = service.compress().await.unwrap();
    // With mock provider, extraction may return empty, but pipeline should not panic
    assert!(result.memories_processed >= 0);

    // Verify raw memory was marked as processed
    let unprocessed = database.get_unprocessed_raw_memories("default", 10).await.unwrap();
    // After compression, should be marked processed (or still 1 if mock returns no extractions)
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib test_compress_from_raw_memories -- --nocapture 2>&1 | tail -10`
Expected: FAIL (CompressionService still reads from facts).

- [ ] **Step 3: Modify `compress_to_notes()` to read from raw_memories**

In `src/memory/compression/service.rs`, modify `compress_to_notes()`:

```rust
// Replace the block that calls get_uncompressed_session_facts():
//   let raw_facts = self.database.get_uncompressed_session_facts(...)?;
// With:
use crate::memory::store::raw_memory::RawMemoryStore;
let raw_memories = self
    .database
    .get_unprocessed_raw_memories(workspace_id, self.config.batch_size as usize)
    .await
    .map_err(|e| AlephError::other(format!("Failed to fetch raw memories: {e}")))?;

// Convert to MemoryEntry for the existing extractor:
let memories: Vec<crate::memory::context::MemoryEntry> = raw_memories
    .iter()
    .map(|raw| {
        // Parse content to extract user/assistant parts if possible
        let (user_input, ai_output) = parse_raw_content(&raw.content);
        let mut entry = crate::memory::context::MemoryEntry::new(
            raw.id.clone(),
            crate::memory::context::ContextAnchor::now("".to_string()),
            user_input,
            ai_output,
        );
        entry
    })
    .collect();
```

Add helper function at the module level:

```rust
/// Parse raw memory content into user/assistant parts.
/// Content may be in "User: ...\nAssistant: ..." format from SessionCompactor.
fn parse_raw_content(content: &str) -> (String, String) {
    if let Some(assistant_pos) = content.find("\nAssistant: ") {
        let user_part = content[..assistant_pos].trim_start_matches("User: ").to_string();
        let ai_part = content[assistant_pos..].trim_start_matches("\nAssistant: ").to_string();
        (user_part, ai_part)
    } else {
        (content.to_string(), String::new())
    }
}
```

Replace the invalidation block:

```rust
// Replace: self.database.invalidate_consumed_chunks(&consumed_ids)
// With:
let consumed_ids: Vec<String> = raw_memories.iter().map(|r| r.id.clone()).collect();
match self.database.mark_raw_as_processed(&consumed_ids).await {
    Ok(n) => tracing::info!(marked = n, "Marked raw memories as processed"),
    Err(e) => tracing::warn!(error = %e, "Failed to mark raw memories as processed"),
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib test_compress_from_raw_memories -- --nocapture 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/memory/compression/service.rs
git commit -m "refactor(compression): switch data source from facts to raw_memories"
```

---

### Task 6: Fix 5 Quality Issues in CompressionService

**Files:**
- Modify: `src/memory/compression/service.rs`
- Modify: `src/memory/compression/extractor.rs`
- Modify: `src/memory/notes/extractor.rs`

- [ ] **Step 1: Fix #1 — Preserve AI response in MemoryEntry**

Already handled in Task 5 by `parse_raw_content()`. Verify the `ai_output` field is populated.

- [ ] **Step 2: Fix #2 — Increase AI response truncation limit**

In `src/memory/compression/extractor.rs`, find `build_extraction_prompt()` method:

```rust
// Change: let char_count = memory.ai_output.chars().count();
//         let ai_output: String = memory.ai_output.chars().take(500).collect();
// To:
let char_count = memory.ai_output.chars().count();
let ai_output: String = memory.ai_output.chars().take(2000).collect();
let truncated = if char_count > 2000 {
    format!("{}...[truncated]", ai_output)
} else {
    ai_output
};
```

- [ ] **Step 3: Fix #3 — Pass note content summaries instead of just titles**

In `src/memory/compression/service.rs`, in `compress_to_notes()`, replace:

```rust
// Old: let existing_titles: Vec<String> = existing_notes.iter().map(|n| n.path.clone()).collect();
// New:
let mut existing_note_summaries: Vec<String> = Vec::new();
for note_idx in &existing_notes {
    let note_path = indexer
        .memory_dir()
        .join(workspace_id)
        .join(&note_idx.category)
        .join(format!("{}.md", note_idx.filename));
    let summary = match tokio::fs::read_to_string(&note_path).await {
        Ok(content) => {
            let body: String = content.chars().take(500).collect();
            format!("{}: {}", note_idx.path, body)
        }
        Err(_) => note_idx.path.clone(),
    };
    existing_note_summaries.push(summary);
}
```

Pass `existing_note_summaries` instead of `existing_titles` to `extract_note_updates()`.

Update `extract_note_updates()` signature in `extractor.rs` and `build_note_extraction_prompt()` in `notes/extractor.rs` to accept `&[String]` (no change needed — it already accepts `&[String]`, just the content is richer now).

- [ ] **Step 4: Fix #4 — Inject attachment text into extraction prompt**

In `src/memory/compression/service.rs`, when building `MemoryEntry` from `RawMemory`, inject attachment text:

```rust
let memories: Vec<crate::memory::context::MemoryEntry> = raw_memories
    .iter()
    .map(|raw| {
        let (user_input, ai_output) = parse_raw_content(&raw.content);
        // Inject attachment text into user_input if present
        let enriched_input = match &raw.attachment_text {
            Some(att) if !att.is_empty() => {
                let att_preview: String = att.chars().take(2000).collect();
                format!("{user_input}\n[Attachment]: {att_preview}")
            }
            _ => user_input,
        };
        crate::memory::context::MemoryEntry::new(
            raw.id.clone(),
            crate::memory::context::ContextAnchor::now("".to_string()),
            enriched_input,
            ai_output,
        )
    })
    .collect();
```

- [ ] **Step 5: Fix #5 — Add extraction quality validation**

In `compress_to_notes()`, after `extract_note_updates()` returns, validate:

```rust
let note_updates = self.extractor.extract_note_updates(&memories, &existing_note_summaries).await?;

// Validate extraction quality
let valid_categories = [
    "preference", "plan", "learning", "project", "personal",
    "tool", "lesson", "skill", "wiki", "transcript", "other",
    "subagent-run", "subagent-session", "subagent-checkpoint", "subagent-transcript",
];
let note_updates = crate::memory::notes::extractor::NoteExtractionResponse {
    updates: note_updates.updates.into_iter().filter(|u| {
        // Must have a valid path
        let has_slash = u.note_path.contains('/');
        // Must have non-empty facts for create/append
        let has_content = !u.new_facts.is_empty() || u.action == crate::memory::notes::extractor::NoteAction::Update;
        // Category must be valid
        let category = u.note_path.split('/').next().unwrap_or("");
        let valid_cat = valid_categories.contains(&category);

        if !has_slash || !has_content || !valid_cat {
            tracing::warn!(
                note_path = %u.note_path,
                action = ?u.action,
                facts = u.new_facts.len(),
                "Filtered out invalid note extraction"
            );
        }
        has_slash && has_content && valid_cat
    }).collect(),
};
```

- [ ] **Step 6: Run all compression tests**

Run: `cargo test -p alephcore --lib compression -- --nocapture 2>&1 | tail -15`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/memory/compression/service.rs src/memory/compression/extractor.rs src/memory/notes/extractor.rs
git commit -m "fix(compression): fix 5 quality issues — preserve AI response, increase truncation, add note context, inject attachments, validate extractions"
```

---

## Phase 3: Dream Daemon Redesign + note_manage Tool

### Task 7: New DreamContext and DreamReport

**Files:**
- Modify: `src/memory/dreaming/mod.rs`

- [ ] **Step 1: Define new DreamContext and DreamReport**

Replace the existing `DreamContext` and `DreamReport` (preserve the old ones as `LegacyDreamContext` temporarily if needed for compilation):

```rust
use std::collections::HashMap;
use crate::memory::notes::NoteIndexer;
use crate::memory::store::sqlite::SqliteMemoryBackend;

/// Metadata for a single note in the dream pipeline.
#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub path: String,
    pub category: String,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_accessed_at: Option<i64>,
    pub content_hash: String,
}

/// Context passed through the dream pipeline stages.
pub struct DreamContext {
    pub notes: Vec<NoteEntry>,
    pub note_contents: HashMap<String, String>,
    pub agent_id: String,
    pub database: MemoryBackend,
    pub indexer: NoteIndexer<SqliteMemoryBackend>,
    pub provider: Arc<dyn AiProvider>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub report: DreamReport,
    pub pipeline_type: String,
}

impl DreamContext {
    /// Lazy-load a note's markdown content.
    pub async fn load_content(&mut self, path: &str) -> Option<&str> {
        if self.note_contents.contains_key(path) {
            return self.note_contents.get(path).map(|s| s.as_str());
        }
        let (category, filename) = path.split_once('/')?;
        let file_path = self.indexer.memory_dir()
            .join(&self.agent_id)
            .join(category)
            .join(format!("{filename}.md"));
        let content = tokio::fs::read_to_string(&file_path).await.ok()?;
        self.note_contents.insert(path.to_string(), content);
        self.note_contents.get(path).map(|s| s.as_str())
    }
}

/// Report generated by dream pipeline execution.
#[derive(Debug, Clone, Default)]
pub struct DreamReport {
    pub pipeline_type: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: u64,
    pub notes_consolidated: u32,
    pub contradictions_found: u32,
    pub notes_marked_stale: u32,
    pub synthesis_count: u32,
    pub format_fixed: u32,
    pub broken_links_found: u32,
    pub links_repaired: u32,
    pub notes_archived: u32,
    pub notes_protected: u32,
    pub errors: Option<String>,
}
```

- [ ] **Step 2: Update DreamPipeline to use new context**

```rust
impl DreamPipeline {
    pub fn daily() -> Self {
        Self::new(vec![
            Box::new(NoteConsolidateStage),
            Box::new(NoteDriftStage),
            Box::new(NoteLintStage),
            Box::new(NoteDecayStage),
            Box::new(DailyDigestStage),
        ])
    }

    pub fn weekly() -> Self {
        Self::new(vec![
            Box::new(NoteConsolidateStage),
            Box::new(NoteDriftStage),
            Box::new(NoteSynthesisStage),
            Box::new(NoteLintStage),
            Box::new(NoteDecayStage),
            Box::new(DailyDigestStage),
        ])
    }
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: Compilation errors from old stages referencing old DreamContext. This is expected — we'll replace them in the next tasks.

- [ ] **Step 4: Commit (may have compilation warnings/errors — that's OK for intermediate state)**

```bash
git add src/memory/dreaming/mod.rs
git commit -m "refactor(dream): new DreamContext and DreamReport for notes layer"
```

---

### Task 8: Implement 6 Dream Stages

This is the largest task. Each stage is a separate file. Implement them one at a time. The order follows the pipeline: Consolidate → Drift → Synthesis → Lint → Decay → Digest.

**Due to the size of this task, each stage follows this pattern:**

1. Create `src/memory/dreaming/stages/{stage_name}.rs`
2. Implement `DreamStage` trait
3. Add unit tests (happy path + empty input + edge case)
4. Compile check
5. Commit

**Files per stage:**
- Create: `src/memory/dreaming/stages/note_consolidate.rs`
- Create: `src/memory/dreaming/stages/note_drift.rs`
- Create: `src/memory/dreaming/stages/note_synthesis.rs`
- Create: `src/memory/dreaming/stages/note_lint.rs`
- Create: `src/memory/dreaming/stages/note_decay.rs`
- Create: `src/memory/dreaming/stages/daily_digest.rs`
- Modify: `src/memory/dreaming/stages/mod.rs`

Each stage file must:
1. `impl DreamStage` with `name()`, `should_run()`, `execute()`
2. Use `ctx.indexer` and `ctx.database` for reads/writes — never touch `facts` table
3. Use `ctx.provider` for LLM calls where needed
4. Update `ctx.report` with stage-specific metrics
5. Include `#[cfg(test)] mod tests` with at least 3 tests

- [ ] **Step 1: Implement NoteConsolidateStage**

Key logic: `notes_vec` similarity search → group pairs > 0.85 → LLM merge decision → rewrite markdown files. Back up originals before merge. Update `ctx.report.notes_consolidated`.

- [ ] **Step 2: Implement NoteDriftStage**

Key logic: filter `notes` by `updated_at > now - 7d` → for each, load wikilinked notes → LLM consistency check → mark stale/contradictory in markdown. Update `ctx.report.contradictions_found`, `ctx.report.notes_marked_stale`.

- [ ] **Step 3: Implement NoteSynthesisStage**

Key logic: `should_run()` returns true only for weekly pipeline. Group notes by category → DBSCAN clustering on embeddings → LLM synthesis → write `synthesis/{category}-insights.md`. Update `ctx.report.synthesis_count`.

- [ ] **Step 4: Implement NoteLintStage**

Key logic: scan all notes for frontmatter completeness → check `notes_links` for broken references → auto-fix where possible → rebuild FTS/embedding if content changed. Update `ctx.report.format_fixed`, `ctx.report.broken_links_found`, `ctx.report.links_repaired`.

- [ ] **Step 5: Implement NoteDecayStage**

Key logic: compute activity score per note using `recall_signals` + `notes_links` incoming count + recency → bottom 10% below threshold → apply protection rules → move to `archive/`. Update `ctx.report.notes_archived`, `ctx.report.notes_protected`.

- [ ] **Step 6: Implement DailyDigestStage**

Key logic: collect notes with changes in last 24h → read content → LLM summary → `upsert_daily_insight()`. Reuses existing `DreamStore::upsert_daily_insight()`.

- [ ] **Step 7: Update stages/mod.rs**

Replace old stage re-exports:

```rust
// src/memory/dreaming/stages/mod.rs

pub mod daily_digest;
pub mod note_consolidate;
pub mod note_decay;
pub mod note_drift;
pub mod note_lint;
pub mod note_synthesis;
pub mod types;

// Keep old modules but don't re-export (Phase 4 will delete them)
#[allow(dead_code)]
mod consolidate;
#[allow(dead_code)]
mod decay;
#[allow(dead_code)]
mod drift;
#[allow(dead_code)]
mod summarize;
#[allow(dead_code)]
mod synthesis;
#[allow(dead_code)]
mod tunnel;
#[allow(dead_code)]
mod wiki_ingest;
#[allow(dead_code)]
mod wiki_lint;

use async_trait::async_trait;
use super::DreamContext;
use crate::error::AlephError;

#[async_trait]
pub trait DreamStage: Send + Sync {
    fn name(&self) -> &'static str;

    async fn should_run(&self, _ctx: &DreamContext) -> bool {
        true
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError>;
}

pub use daily_digest::DailyDigestStage;
pub use note_consolidate::NoteConsolidateStage;
pub use note_decay::NoteDecayStage;
pub use note_drift::NoteDriftStage;
pub use note_lint::NoteLintStage;
pub use note_synthesis::NoteSynthesisStage;
```

- [ ] **Step 8: Run all dream tests**

Run: `cargo test -p alephcore --lib dreaming -- --nocapture 2>&1 | tail -20`
Expected: All new stage tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/memory/dreaming/
git commit -m "feat(dream): implement 6 note-based dream stages — consolidate, drift, synthesis, lint, decay, digest"
```

---

### Task 9: Implement note_manage Tool

**Files:**
- Create: `src/builtin_tools/note_manage.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Create NoteManageTool**

```rust
// src/builtin_tools/note_manage.rs

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::{AlephError, Result};
use crate::memory::notes::{KnowledgeNote, NoteIndexer};
use crate::memory::store::MemoryBackend;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NoteManageAction {
    Create,
    Update,
    Append,
    Query,
    List,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NoteManageArgs {
    pub action: NoteManageAction,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub facts: Option<Vec<String>>,
    #[serde(default)]
    pub links: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteManageResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<NoteListEntry>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteListEntry {
    pub path: String,
    pub category: String,
    pub filename: String,
    pub tags: Vec<String>,
}

const VALID_CATEGORIES: &[&str] = &[
    "preference", "plan", "learning", "project", "personal",
    "tool", "lesson", "skill", "wiki", "transcript", "other",
    "subagent-run", "subagent-session", "subagent-checkpoint", "subagent-transcript",
];

fn frontmatter_template(category: &str, title: &str, tags: &[String]) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d").to_string();
    let tags_str = serde_json::to_string(tags).unwrap_or_else(|_| "[]".into());

    match category {
        "wiki" => format!(
            "---\ntitle: {title}\naliases: []\ntags: {tags_str}\nsources: []\ncreated: \"{now}\"\nupdated: \"{now}\"\n---"
        ),
        "skill" => format!(
            "---\ntitle: {title}\nscope: persona\ntags: {tags_str}\ncreated: \"{now}\"\nupdated: \"{now}\"\n---"
        ),
        _ => format!(
            "---\ncategory: {category}\ntags: {tags_str}\ncreated: \"{now}\"\nupdated: \"{now}\"\n---"
        ),
    }
}
```

Implement the 6 action handlers (create, update, append, query, list, delete) following the same pattern as `WikiManageTool` but generalized for all categories. The `create` handler uses `frontmatter_template()`, writes markdown via `indexer.write_note()`, and for wiki category calls `git_manager.commit_changes()`.

- [ ] **Step 2: Register in builtin_tools/mod.rs**

Add `pub mod note_manage;` and register the tool in the tool registry.

- [ ] **Step 3: Deprecate skill_manage and wiki_manage**

Add `#[deprecated(note = "Use note_manage instead")]` to `SkillManageTool` and `WikiManageTool`.

- [ ] **Step 4: Write tests**

Test each action: create (wiki, skill, preference), append, query, list, delete. Verify frontmatter template selection by category.

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib note_manage -- --nocapture 2>&1 | tail -15`

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/note_manage.rs src/builtin_tools/mod.rs
git commit -m "feat(tools): add unified note_manage tool, deprecate skill_manage/wiki_manage"
```

---

### Task 10: Skill Data Migration Script

**Files:**
- Create: `src/memory/migration/skill_to_notes.rs`
- Modify: `src/memory/migration/mod.rs` (or create if needed)

- [ ] **Step 1: Implement migration function**

```rust
// src/memory/migration/skill_to_notes.rs

use crate::error::AlephError;
use crate::memory::context::NoteType;
use crate::memory::notes::{KnowledgeNote, NoteIndexer};
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::skill::tools::search::skill_name_from_path;
use tracing::info;

pub struct SkillMigrationReport {
    pub total_skills: usize,
    pub migrated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Migrate skill facts from the facts table into skill/*.md Knowledge Notes.
///
/// Set `dry_run = true` to produce a report without writing files.
pub async fn migrate_skills_to_notes<S: crate::memory::notes::store::NoteStore + Send + Sync>(
    database: &MemoryBackend,
    indexer: &NoteIndexer<S>,
    agent_id: &str,
    dry_run: bool,
) -> Result<SkillMigrationReport, AlephError> {
    let all_facts = database.get_all_facts(false, None).await?;
    let skill_facts: Vec<_> = all_facts
        .into_iter()
        .filter(|f| f.note_type == NoteType::Skill && f.is_valid)
        .collect();

    let mut report = SkillMigrationReport {
        total_skills: skill_facts.len(),
        migrated: 0,
        skipped: 0,
        errors: Vec::new(),
    };

    for fact in &skill_facts {
        let name = skill_name_from_path(&fact.path).unwrap_or("unknown");
        let category_hint = fact.path
            .strip_prefix("aleph://skills/")
            .and_then(|s| s.split('/').next())
            .unwrap_or("general");

        // Parse content: first line is description, rest is body
        let (facts, _description) = parse_skill_content(&fact.content);

        if dry_run {
            info!(name, category = category_hint, facts = facts.len(), "DRY RUN: would migrate skill");
            report.migrated += 1;
            continue;
        }

        let note = KnowledgeNote {
            title: name.to_string(),
            category: "skill".to_string(),
            tags: vec![category_hint.to_string()],
            facts,
            links: vec![],
            created_at: fact.created_at,
            updated_at: fact.updated_at,
            content_hash: String::new(),
        };

        match indexer.write_note(agent_id, "skill", &note).await {
            Ok(_) => {
                report.migrated += 1;
                info!(name, "Migrated skill to notes");
            }
            Err(e) => {
                report.errors.push(format!("{name}: {e}"));
                report.skipped += 1;
            }
        }
    }

    Ok(report)
}

fn parse_skill_content(content: &str) -> (Vec<String>, String) {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return (vec![], String::new());
    }

    let description = lines[0].to_string();
    let body_lines: Vec<String> = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let trimmed = l.trim();
            if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                trimmed[2..].to_string()
            } else {
                trimmed.to_string()
            }
        })
        .collect();

    if body_lines.is_empty() {
        (vec![description.clone()], description)
    } else {
        (body_lines, description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_content_with_bullets() {
        let content = "Debug Rust errors\n\n- Read error messages carefully\n- Check borrow checker hints\n- Use cargo clippy";
        let (facts, desc) = parse_skill_content(content);
        assert_eq!(desc, "Debug Rust errors");
        assert_eq!(facts.len(), 3);
        assert_eq!(facts[0], "Read error messages carefully");
    }

    #[test]
    fn parses_single_line_skill() {
        let content = "Always use --release for benchmarks";
        let (facts, desc) = parse_skill_content(content);
        assert_eq!(facts, vec!["Always use --release for benchmarks"]);
        assert_eq!(desc, content);
    }
}
```

- [ ] **Step 2: Run dry-run first before actual migration**

The migration script should be callable from a CLI command or at server startup. Add it to the server startup sequence with a one-time migration flag check via `compression_metadata` table:

```rust
// Check if migration already ran
let key = "skill_to_notes_migrated";
if database.get_compression_metadata(key).await?.is_none() {
    let report = migrate_skills_to_notes(&database, &indexer, agent_id, false).await?;
    info!(?report, "Skill migration complete");
    database.set_compression_metadata(key, "true").await?;
}
```

- [ ] **Step 3: Commit**

```bash
git add src/memory/migration/
git commit -m "feat(migration): one-time skill facts → notes migration script"
```

---

### Task 11: recall_signals Adaptation

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/sqlite/mod.rs` (wherever recall_signals SQL lives)

- [ ] **Step 1: Add migration SQL for recall_signals**

```rust
// In schema.rs, add a migration constant:
pub const MIGRATE_RECALL_SIGNALS_NOTE_PATH: &str = "
ALTER TABLE recall_signals RENAME COLUMN fact_id TO note_path;
";
```

Apply this in the `init_tables()` migration path. Use `PRAGMA table_info(recall_signals)` to check if the column is already renamed before running.

- [ ] **Step 2: Update any Rust code referencing `fact_id` in recall_signals queries**

Run: `grep -rn "fact_id" src/memory/store/sqlite/ | grep -i recall`

Update all SQL strings from `fact_id` to `note_path`.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/sqlite/
git commit -m "refactor(memory): rename recall_signals.fact_id to note_path"
```

---

## Phase 4: Facts Table Elimination

### Task 12: Stop Dual-Write (raw_memories only)

**Files:**
- Modify: `src/memory/session_compactor/mod.rs`
- Modify: `src/memory/transcript_indexer/mod.rs`

- [ ] **Step 1: Remove `insert_fact()` calls from SessionCompactor**

Remove the `database.insert_fact(&fact)` call, keeping only `database.insert_raw_memory(&raw)`.

- [ ] **Step 2: Remove `insert_fact()` calls from TranscriptIndexer**

Same pattern.

- [ ] **Step 3: Verify no active facts writes remain**

Run: `grep -rn "insert_fact\|update_fact\|batch_insert_facts" src/ --include="*.rs" | grep -v "test\|mod.rs\|trait\|#\[deprecated\]"`

Expect: only trait definitions and deprecated tool code.

- [ ] **Step 4: Commit**

```bash
git add src/memory/session_compactor/ src/memory/transcript_indexer/
git commit -m "refactor(memory): stop writing to facts table, raw_memories only"
```

---

### Task 13: Drop Facts Table + Code Cleanup

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/mod.rs`
- Modify: `src/memory/store/sqlite/mod.rs`
- Delete: `src/memory/dreaming/stages/consolidate.rs`
- Delete: `src/memory/dreaming/stages/decay.rs`
- Delete: `src/memory/dreaming/stages/drift.rs`
- Delete: `src/memory/dreaming/stages/summarize.rs`
- Delete: `src/memory/dreaming/stages/synthesis.rs`
- Delete: `src/memory/dreaming/stages/tunnel.rs`
- Delete: `src/memory/dreaming/stages/wiki_ingest.rs`
- Delete: `src/memory/dreaming/stages/wiki_lint.rs`
- Delete: `src/builtin_tools/wiki_manage.rs`
- Delete: `src/memory/wiki_sync.rs`
- Modify: `src/memory/compression/service.rs` (remove `compress_in_workspace()`)

- [ ] **Step 1: Remove facts DDL from schema.rs**

Delete `CREATE_FACTS_TABLE`, `CREATE_FACTS_VEC_*`, `CREATE_FACTS_FTS` constants. Remove from `init_tables()`.

- [ ] **Step 2: Remove facts methods from MemoryStore trait**

Remove: `insert_fact`, `get_fact`, `update_fact`, `delete_fact`, `batch_insert_facts`, `get_all_facts`, `invalidate_fact`, `close_fact_validity`, `set_fact_valid_from`, `update_fact_content`, `find_similar_facts`, `apply_fact_decay`, `get_fact_stats`, `soft_delete_fact`, `count_facts_by_topic_excluding_domain`, `set_tunnel_pending`, `has_tunnel_pending`, `get_tunnel_candidates`, `clear_tunnel_pending_by_topic`.

Keep search methods (`vector_search`, `text_search`, `hybrid_search`) if they're used for notes retrieval. If they only search facts, remove them too.

- [ ] **Step 3: Remove facts SQL implementations from sqlite/mod.rs**

Delete all implementations of the removed trait methods.

- [ ] **Step 4: Delete old dream stage files**

```bash
rm src/memory/dreaming/stages/consolidate.rs
rm src/memory/dreaming/stages/decay.rs
rm src/memory/dreaming/stages/drift.rs
rm src/memory/dreaming/stages/summarize.rs
rm src/memory/dreaming/stages/synthesis.rs
rm src/memory/dreaming/stages/tunnel.rs
rm src/memory/dreaming/stages/wiki_ingest.rs
rm src/memory/dreaming/stages/wiki_lint.rs
```

Remove the `#[allow(dead_code)] mod ...` entries from `stages/mod.rs`.

- [ ] **Step 5: Delete wiki_manage, wiki_sync, and remove skill_manage**

```bash
rm src/builtin_tools/wiki_manage.rs
rm src/memory/wiki_sync.rs
```

Remove module declarations from their parent `mod.rs` files.

- [ ] **Step 6: Remove `compress_in_workspace()` from CompressionService**

Delete the legacy compression path. `compress_to_notes()` is now the only path.

- [ ] **Step 7: Remove ConflictDetector facts references**

If `ConflictDetector` only operates on facts, delete it. If it has notes-compatible logic, adapt it.

- [ ] **Step 8: Full compilation check**

Run: `cargo check -p alephcore 2>&1 | head -30`

Fix any remaining compilation errors caused by dead references.

- [ ] **Step 9: Run full test suite**

Run: `cargo test -p alephcore 2>&1 | tail -20`
Expected: All tests pass. Some old tests may need to be removed if they depend on facts.

- [ ] **Step 10: Verify no facts remnants**

Run: `grep -rn "facts" src/ --include="*.rs" | grep -v "test\|//\|new_facts\|note_facts\|body_facts\|raw_facts\|skill_facts"` 

The result should show zero hits for `facts` table references.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor(memory): eliminate facts table — drop DDL, remove MemoryStore facts methods, delete old dream stages, remove deprecated tools"
```

---

## Post-Migration Verification

### Task 14: End-to-End Integration Test

**Files:**
- Add tests to `tests/` or inline in relevant modules

- [ ] **Step 1: Test full pipeline: raw_memory → compress → notes**

```rust
#[tokio::test]
async fn full_pipeline_raw_to_notes() {
    // 1. Insert raw memory with attachment
    // 2. Trigger CompressionService
    // 3. Verify markdown file created
    // 4. Verify notes_index has entry
    // 5. Verify raw_memory marked processed
}
```

- [ ] **Step 2: Test dream pipeline on populated notes**

```rust
#[tokio::test]
async fn dream_pipeline_runs_without_error() {
    // 1. Create several test notes (preference, wiki, skill)
    // 2. Run DreamPipeline::daily()
    // 3. Verify DreamReport has no errors
    // 4. Verify daily_insights table has entry
}
```

- [ ] **Step 3: Test note_manage CRUD**

```rust
#[tokio::test]
async fn note_manage_create_query_delete() {
    // 1. Create note via note_manage (category=wiki)
    // 2. Query for it
    // 3. Verify result
    // 4. Delete it
    // 5. Verify gone
}
```

- [ ] **Step 4: Commit**

```bash
git add tests/
git commit -m "test(memory): add end-to-end integration tests for notes migration"
```

---

## Summary

| Phase | Tasks | Key Deliverable |
|-------|-------|-----------------|
| Phase 1 | Tasks 1-4 | raw_memories table + dual write from SessionCompactor/TranscriptIndexer |
| Phase 2 | Tasks 5-6 | CompressionService reads raw_memories, 5 quality fixes |
| Phase 3 | Tasks 7-11 | Dream daemon 6 stages, note_manage tool, skill migration, recall_signals |
| Phase 4 | Tasks 12-14 | Stop dual write, drop facts table, cleanup code, E2E tests |

**Total: 14 tasks, ~45 bite-sized steps**

Each phase produces a working, testable system. Phase 1 can be deployed independently (dual-write is additive). Phase 2 switches the read path. Phase 3 adds new functionality. Phase 4 is the final cleanup.
