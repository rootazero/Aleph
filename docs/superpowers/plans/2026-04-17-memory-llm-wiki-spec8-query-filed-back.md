# Spec 8 — Query Filed-back Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically archive high-value `memory_reflect` answers as persistent `query/` category notes, so valuable Q&A compounds into the knowledge base instead of vanishing into chat history.

**Architecture:** New `src/memory/notes/query_filer/` module with three files: `types.rs` (data model), `prompts.rs` (LLM gate prompt), `filer.rs` (`QueryFiler` trait + `DefaultQueryFiler`). A `query_filed` SQLite table deduplicates by `query_hash`. The filer hooks into `memory_reflect` tool impl via fire-and-forget `tokio::spawn`. `query/` is added to `CATEGORY_DIRS`. A `query_file_note` tool allows explicit LLM override.

**Tech Stack:** Rust + tokio + async_trait + serde + sha2 + chrono + `insta`. Reuses `NoteIndexer::write_note`, `NoteOrientation::record_query`, `Synthesis` / `NoteRef` from reflector.

**Spec reference:** `docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md` §5.

**Pre-flight:**
- `Synthesis { text, sources: Vec<NoteRef> }` at `src/memory/reflector/types.rs`.
- `NoteRef { path, title, relevance }` same file.
- `CATEGORY_DIRS` at `src/memory/notes/indexer.rs:18`.
- `NoteIndexer::write_note(agent_id, category, &KnowledgeNote)` available.
- `NoteOrientation::record_query(agent_id, LogEntry)` from Spec 5.
- `memory_reflect` tool at `src/builtin_tools/memory_reflect.rs`.
- Dream stages reference `CATEGORY_DIRS` — adding `"query"` is safe (NoteDecay/NoteSynthesis check category names).
- `NoteSynthesis` excludes `category == "synthesis"` — must also exclude `"query"`.

---

## File Map

### Create
- `src/memory/notes/query_filer/mod.rs`
- `src/memory/notes/query_filer/types.rs` — `FileOutcome`, `CheapGateReason`, `QueryFiledRow`
- `src/memory/notes/query_filer/prompts.rs` — `PROMPT_QUERY_FILE_CHECK`
- `src/memory/notes/query_filer/filer.rs` — `QueryFiler` trait + `DefaultQueryFiler`
- `src/builtin_tools/query_file_note.rs` — explicit override tool

### Modify
- `src/memory/notes/mod.rs` — `pub mod query_filer;`
- `src/memory/notes/indexer.rs` — add `"query"` to `CATEGORY_DIRS`
- `src/memory/store/sqlite/schema.rs` — add `query_filed` table DDL + init
- `src/memory/dreaming/stages/note_synthesis.rs` — exclude `"query"` from synthesis
- `src/builtin_tools/memory_reflect.rs` — fire-and-forget `QueryFiler::maybe_file` after reflect
- `src/builtin_tools/mod.rs` — register new modules
- `src/config/types/memory.rs` — `QueryFilerConfig`

---

## Task 1: Add `query` to CATEGORY_DIRS + exclude from synthesis

**Files:**
- Modify: `src/memory/notes/indexer.rs`
- Modify: `src/memory/dreaming/stages/note_synthesis.rs`

- [ ] **Step 1: Add "query" to CATEGORY_DIRS**

In `src/memory/notes/indexer.rs`, find the `CATEGORY_DIRS` array. Add `"query"` after `"other"` (or alphabetically — match existing style):

```rust
pub const CATEGORY_DIRS: &[&str] = &[
    "preference",
    "plan",
    "learning",
    "project",
    "personal",
    "tool",
    "lesson",
    "skill",
    "wiki",
    "transcript",
    "subagent-run",
    "subagent-session",
    "subagent-checkpoint",
    "subagent-transcript",
    "other",
    "query",  // Spec 8: filed-back query answers
];
```

- [ ] **Step 2: Exclude "query" from NoteSynthesis**

In `src/memory/dreaming/stages/note_synthesis.rs`, find the line that excludes `"synthesis"` category. It should look like `if category == "synthesis" { continue; }` or similar filter. Add `"query"` to the exclusion:

```rust
if category == "synthesis" || category == "query" {
    continue;
}
```

- [ ] **Step 3: Verify**

```bash
cargo check -p alephcore 2>&1 | tail -5
cargo test -p alephcore --lib memory::dreaming 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/indexer.rs src/memory/dreaming/stages/note_synthesis.rs
git commit -m "feat(query-filer): add query to CATEGORY_DIRS, exclude from synthesis"
```

---

## Task 2: `query_filed` SQLite table

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`

- [ ] **Step 1: Add DDL constant**

Near the other `CREATE TABLE` constants, add:

```rust
pub(crate) const CREATE_QUERY_FILED: &str = r#"
CREATE TABLE IF NOT EXISTS query_filed (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    query_hash  TEXT NOT NULL,
    note_path   TEXT NOT NULL,
    session_id  TEXT,
    filed_at    INTEGER NOT NULL,
    UNIQUE(agent_id, query_hash)
);
CREATE INDEX IF NOT EXISTS idx_query_filed_agent ON query_filed(agent_id);
"#;
```

- [ ] **Step 2: Wire into init_schema**

In `init_schema`, add the execution after existing table creations:

```rust
conn.execute_batch(CREATE_QUERY_FILED)
    .map_err(|e| AlephError::other(format!("init query_filed: {e}")))?;
```

- [ ] **Step 3: Add idempotency test**

```rust
#[test]
fn create_query_filed_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(CREATE_QUERY_FILED).unwrap();
    conn.execute_batch(CREATE_QUERY_FILED).unwrap(); // idempotent
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM query_filed", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
```

- [ ] **Step 4: Verify**

```bash
cargo test -p alephcore --lib memory::store::sqlite::schema
```

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/store/sqlite/schema.rs
git commit -m "feat(query-filer): query_filed SQLite table"
```

---

## Task 3: Scaffold query_filer module + types

**Files:**
- Create: `src/memory/notes/query_filer/mod.rs`
- Create: `src/memory/notes/query_filer/types.rs`
- Modify: `src/memory/notes/mod.rs`

- [ ] **Step 1: Create types.rs**

```rust
//! Data model for the query filed-back system.

use serde::{Deserialize, Serialize};

/// Why the cheap gate rejected a query.
#[derive(Debug, Clone, Serialize)]
pub enum CheapGateReason {
    TooFewSources { count: usize },
    AnswerTooShort { chars: usize },
    AlreadyFiled { note_path: String },
}

/// Result of a filing attempt.
#[derive(Debug, Clone, Serialize)]
pub enum FileOutcome {
    SkippedCheapGate { reason: CheapGateReason },
    SkippedLlmGate { reason: String },
    Filed { note_path: String, created_at: i64 },
    AlreadyFiled { note_path: String },
}

/// Row shape for the `query_filed` dedup table.
#[derive(Debug, Clone)]
pub struct QueryFiledRow {
    pub id: String,
    pub agent_id: String,
    pub query_hash: String,
    pub note_path: String,
    pub session_id: Option<String>,
    pub filed_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cheap_gate_reason_serializes() {
        let r = CheapGateReason::TooFewSources { count: 2 };
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains("TooFewSources"));
    }

    #[test]
    fn file_outcome_filed_serializes() {
        let o = FileOutcome::Filed {
            note_path: "query/test".into(),
            created_at: 1234,
        };
        let j = serde_json::to_string(&o).unwrap();
        assert!(j.contains("query/test"));
    }
}
```

- [ ] **Step 2: Create mod.rs + wire**

```rust
//! Query filed-back: archive valuable memory_reflect answers as query/ notes.

pub mod types;

pub use types::{CheapGateReason, FileOutcome, QueryFiledRow};
```

In `src/memory/notes/mod.rs`:

```rust
pub mod query_filer;
```

- [ ] **Step 3: Verify**

```bash
cargo test -p alephcore --lib memory::notes::query_filer
```

Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/query_filer/ src/memory/notes/mod.rs
git commit -m "feat(query-filer): scaffold module and types"
```

---

## Task 4: LLM gate prompt

**Files:**
- Create: `src/memory/notes/query_filer/prompts.rs`
- Modify: `src/memory/notes/query_filer/mod.rs`

- [ ] **Step 1: Create prompts.rs**

```rust
//! LLM gate prompt for deciding whether to file a query answer.

pub const PROMPT_QUERY_FILE_CHECK: &str = r#"You decide whether a synthesised answer from memory search is worth archiving as a permanent note.

A query answer should be FILED if it is a NOVEL SYNTHESIS — it connects multiple sources in a way that would be painful to re-derive. It should NOT be filed if it merely restates facts already present in existing notes.

Input: the original query and the synthesised answer with its source notes.

Output valid JSON only:
{
  "file": true or false,
  "reason": "one sentence explaining why or why not",
  "proposed_title": "short kebab-case title for the note (only if file=true)",
  "tags": ["tag1", "tag2"],
  "links": ["category/filename"]
}

If file is false, proposed_title/tags/links can be empty or omitted.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_snapshot() {
        insta::assert_snapshot!("query_file_check_prompt", PROMPT_QUERY_FILE_CHECK);
    }

    #[test]
    fn prompt_mentions_json_output() {
        assert!(PROMPT_QUERY_FILE_CHECK.contains("JSON"));
        assert!(PROMPT_QUERY_FILE_CHECK.contains("file"));
        assert!(PROMPT_QUERY_FILE_CHECK.contains("proposed_title"));
    }
}
```

- [ ] **Step 2: Wire + accept snapshot**

```rust
pub mod prompts;
pub use prompts::PROMPT_QUERY_FILE_CHECK;
```

```bash
INSTA_UPDATE=always cargo test -p alephcore --lib memory::notes::query_filer::prompts
```

- [ ] **Step 3: Verify**

```bash
cargo test -p alephcore --lib memory::notes::query_filer
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/query_filer/
git commit -m "feat(query-filer): LLM gate prompt with insta snapshot"
```

---

## Task 5: `QueryFilerConfig`

**Files:**
- Modify: `src/config/types/memory.rs`

- [ ] **Step 1: Add config**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryFilerConfig {
    #[serde(default = "default_qf_enabled")]
    pub enabled: bool,
    #[serde(default = "default_qf_min_sources")]
    pub min_sources: usize,
    #[serde(default = "default_qf_min_answer_chars")]
    pub min_answer_chars: usize,
    #[serde(default = "default_qf_llm_gate_enabled")]
    pub llm_gate_enabled: bool,
}

impl Default for QueryFilerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_sources: 3,
            min_answer_chars: 200,
            llm_gate_enabled: true,
        }
    }
}

fn default_qf_enabled() -> bool { true }
fn default_qf_min_sources() -> usize { 3 }
fn default_qf_min_answer_chars() -> usize { 200 }
fn default_qf_llm_gate_enabled() -> bool { true }
```

Add to `MemoryConfig`:

```rust
#[serde(default)]
pub query_filer: QueryFilerConfig,
```

- [ ] **Step 2: Verify**

```bash
cargo check -p alephcore 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
cargo fmt -p alephcore
git add src/config/types/memory.rs
git commit -m "feat(query-filer): QueryFilerConfig in MemoryConfig"
```

---

## Task 6: `QueryFiler` trait + `DefaultQueryFiler`

**Files:**
- Create: `src/memory/notes/query_filer/filer.rs`
- Modify: `src/memory/notes/query_filer/mod.rs`

This is the core: cheap gate → LLM gate → write note → dedup table → log.

- [ ] **Step 1: Create filer.rs**

The file should contain:

**Trait:**
```rust
#[async_trait]
pub trait QueryFiler: Send + Sync {
    async fn maybe_file(
        &self,
        agent_id: &str,
        query: &str,
        synthesis: &Synthesis,
        session_id: Option<&str>,
    ) -> Result<FileOutcome, AlephError>;
}
```

**`DefaultQueryFiler` struct:**
- Fields: `store: Arc<S>` (NoteStore), `indexer: Arc<NoteIndexer<S>>`, `provider: Arc<dyn AiProvider>`, `orientation: Option<Arc<dyn NoteOrientation>>`, `memory_dir: PathBuf`, `config: QueryFilerConfig`
- `query_hash(query: &str) -> String` — sha256 of `query.trim().to_lowercase()`

**`maybe_file` implementation:**

1. **Cheap gate:**
   - `synthesis.sources.len() < config.min_sources` → `SkippedCheapGate(TooFewSources)`
   - `synthesis.text.chars().count() < config.min_answer_chars` → `SkippedCheapGate(AnswerTooShort)`
   - Check `query_filed` table: if `(agent_id, query_hash)` exists → `AlreadyFiled { note_path }`

2. **LLM gate** (if `config.llm_gate_enabled`):
   - System: `PROMPT_QUERY_FILE_CHECK`
   - User: query + synthesis.text + source paths
   - Parse JSON `{ file, reason, proposed_title, tags, links }`
   - If `file == false` → `SkippedLlmGate { reason }`

3. **Write note:**
   - Category: `"query"`
   - Filename: `sanitize_title(proposed_title)` or `sanitize_title(query_hash[..12])`
   - Frontmatter: category, title, tags, created, updated, query_hash, session_id, sources, summary
   - Body: `## Question\n> {query}\n\n## Answer\n{synthesis.text}\n\n## Sources\n` + wikilinks
   - Write via `NoteIndexer::write_note`

4. **Dedup table insert:** `INSERT INTO query_filed (id, agent_id, query_hash, note_path, session_id, filed_at)`

5. **Log:** orientation `record_query` if available.

6. Return `Filed { note_path, created_at }`.

**Tests (4):**
- `cheap_gate_rejects_few_sources`
- `cheap_gate_rejects_short_answer`
- `llm_gate_rejects_when_not_novel`
- `files_when_both_gates_pass`

- [ ] **Step 2: Wire module**

```rust
pub mod filer;
pub use filer::{DefaultQueryFiler, QueryFiler};
```

- [ ] **Step 3: Verify**

```bash
cargo test -p alephcore --lib memory::notes::query_filer
```

Expected: 8 tests pass (2 types + 2 prompts + 4 filer).

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/query_filer/
git commit -m "feat(query-filer): QueryFiler trait + DefaultQueryFiler with two-tier gating"
```

---

## Task 7: Hook into `memory_reflect` tool

**Files:**
- Modify: `src/builtin_tools/memory_reflect.rs`

- [ ] **Step 1: Add optional QueryFiler field**

Read the file first. Find the tool struct. Add:

```rust
pub query_filer: Option<Arc<dyn QueryFiler>>,
```

Initialize to `None` in constructors. Add builder:

```rust
pub fn with_query_filer(mut self, qf: Arc<dyn QueryFiler>) -> Self {
    self.query_filer = Some(qf);
    self
}
```

- [ ] **Step 2: Fire after successful reflect**

After `MemoryReflector::reflect` returns a successful `Synthesis`, add:

```rust
if let Some(qf) = self.query_filer.clone() {
    let agent = agent_id.to_string();
    let q = query.to_string();
    let synth = synthesis.clone();
    let sid = session_id.map(|s| s.to_string());
    tokio::spawn(async move {
        if let Err(e) = qf.maybe_file(&agent, &q, &synth, sid.as_deref()).await {
            tracing::warn!("query filer failed: {e}");
        }
    });
}
```

- [ ] **Step 3: Verify**

```bash
cargo check -p alephcore 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add src/builtin_tools/memory_reflect.rs
git commit -m "feat(query-filer): fire-and-forget QueryFiler hook in memory_reflect"
```

---

## Task 8: App startup wiring

**Files:**
- Modify: startup builder (grep `with_profile_synthesizer\|with_compound_ingestor`)

- [ ] **Step 1: Construct DefaultQueryFiler + inject**

At the startup site, construct:

```rust
let query_filer: Arc<dyn QueryFiler> = Arc::new(DefaultQueryFiler {
    store: backend.clone(),
    indexer: indexer_arc.clone(),
    provider: provider.clone(),
    orientation: Some(orientation.clone()),
    memory_dir: note_memory_dir.clone(),
    config: app_config.memory.query_filer.clone(),
});
```

Inject into `memory_reflect` tool: `.with_query_filer(query_filer.clone())`.

- [ ] **Step 2: Verify**

```bash
cargo check -p alephcore 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
cargo fmt -p alephcore
git add -A
git commit -m "feat(query-filer): wire DefaultQueryFiler into startup + memory_reflect"
```

---

## Task 9: Final sanity pass + docs

- [ ] **Step 1: Run validation**

```bash
cargo fmt --check -p alephcore
cargo test -p alephcore --lib memory::notes::query_filer
cargo test -p alephcore --lib memory::dreaming
cargo test -p alephcore --lib memory::store::sqlite::schema
```

Scoped clippy:

```bash
cargo clippy -p alephcore --lib -- -D warnings 2>&1 \
  | grep -E "memory/notes/query_filer|builtin_tools/memory_reflect|builtin_tools/query_file" \
  | head -20
```

- [ ] **Step 2: Update docs**

Append to `docs/reference/MEMORY_SYSTEM.md`:

```markdown
## Query filed-back (Spec 8, shipped 2026-04-17)

High-value `memory_reflect` answers are automatically archived as
`query/` category notes. A two-tier gate (cheap: ≥3 sources + ≥200 chars;
LLM: novel synthesis check) decides filing. The `query_filed` SQLite
table deduplicates by `sha256(query)`. `NoteSynthesis` weekly stage
excludes `query/` to prevent recursion. See
[docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md §5](../superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md).
```

- [ ] **Step 3: Commit**

```bash
git add docs/reference/MEMORY_SYSTEM.md
git commit -m "docs(memory): point to query filed-back (Spec 8) from MEMORY_SYSTEM.md"
```

---

## Self-Review

**Spec coverage (§5):**

| Spec §5 sub-section | Task(s) |
|---|---|
| §5.1 Two-tier gating | 6 (cheap + LLM gate) |
| §5.2 query/ category | 1 (CATEGORY_DIRS) |
| §5.3 Note format | 6 (write_note with frontmatter + body) |
| §5.4 QueryFiler trait | 6 |
| §5.5 Trigger hook | 7 (memory_reflect fire-and-forget) |
| §5.6 Dedup table | 2 (schema) + 6 (insert) |
| §5.7 Dream interaction | 1 (synthesis exclusion) |
| §5.8 Config | 5 |

**Type consistency:** `FileOutcome`, `CheapGateReason`, `QueryFiledRow` defined in T3, used in T6. `QueryFiler` trait defined in T6, consumed in T7-T8. `Synthesis` / `NoteRef` from existing `src/memory/reflector/types.rs`.

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-04-17-memory-llm-wiki-spec8-query-filed-back.md`.** Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task.
2. **Inline Execution** — batch with checkpoints.

**Which approach?**
