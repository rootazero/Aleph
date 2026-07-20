# Aleph Note Layer — Phase C2: Governance / Anti-Feedback / Supersession Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a unified `governance::gate` that all three raw → note write paths (compound ingest, feedback distill, note_manage) traverse. Introduce paragraph-level provenance, frontmatter supersession lifecycle, async LLM review queue, contradiction category, and recall-signal-driven confidence decay — all without introducing new MemoryEvent variants.

**Architecture:** New module `src/memory/notes/governance/{gate.rs, supersession.rs}`; new dream stage `dreaming/stages/note_review.rs`; extended `note_decay.rs`; three new SQLite tables (`notes_provenance`, `notes_review_queue`, `notes_review_archive`); additive frontmatter fields (`status`, `supersedes`, `superseded_by`); inline HTML provenance comments parsed by a new `extract_provenance_markers` function. Phase C2 introduces no new MemoryEvent variants (a hard constraint that lets R2 ship after C2).

**Tech Stack:** Rust 2021, rusqlite, tokio, async_trait, serde_json (candidate serialization), regex (provenance comment matcher).

**Spec:** `docs/superpowers/specs/2026-05-03-aleph-note-layer-llm-wiki-optimization-design.md` §4 (Phase C2). Sub-section IDs C2.1–C2.10 map to task headings.

**Verification gate:** All test items in §4 C2.10 pass; full `cargo test -p alephcore --lib memory::notes memory::dreaming memory::store::sqlite` green; manual smoke against the author's note corpus produces zero errors; gate fail-closed verified.

---

## Task 1 (C2.1): Frontmatter `status` / `supersedes` / `superseded_by`

**Files:**
- Modify: `src/memory/notes/note.rs:30-45` (`Frontmatter`)
- Modify: `src/memory/notes/note.rs:55-99` (`KnowledgeNote`)
- Modify: `src/memory/notes/note.rs:133-180` (`to_markdown`)

- [ ] **Step 1: Write failing tests**

Add to `mod tests` in `src/memory/notes/note.rs`:

```rust
#[test]
fn note_status_default_active_for_legacy() {
    let md = "---\ncategory: skill\ntags: []\ncreated: \"2026-04-29\"\nupdated: \"2026-04-29\"\n---\n\n- f\n";
    let n = KnowledgeNote::from_markdown("legacy", md).unwrap();
    assert_eq!(n.status, NoteStatus::Active);
    assert!(n.supersedes.is_empty());
    assert!(n.superseded_by.is_empty());
}

#[test]
fn note_status_round_trip_contradicted() {
    let n = KnowledgeNote {
        title: "x".into(),
        category: "preference".into(),
        facts: vec!["body".into()],
        status: NoteStatus::Contradicted,
        supersedes: vec!["preference/old".into()],
        superseded_by: vec!["preference/new".into()],
        ..Default::default()
    };
    let md = n.to_markdown();
    assert!(md.contains("status: contradicted"));
    assert!(md.contains("supersedes: [preference/old]"));
    assert!(md.contains("superseded_by: [preference/new]"));

    let parsed = KnowledgeNote::from_markdown("x", &md).unwrap();
    assert_eq!(parsed.status, NoteStatus::Contradicted);
    assert_eq!(parsed.supersedes, vec!["preference/old".to_string()]);
    assert_eq!(parsed.superseded_by, vec!["preference/new".to_string()]);
}
```

- [ ] **Step 2: Run tests — should fail**

```bash
cargo test -p alephcore --lib memory::notes::note::tests::note_status_default_active_for_legacy memory::notes::note::tests::note_status_round_trip_contradicted
```
Expected: fail to compile (`NoteStatus`, `status`, `supersedes`, `superseded_by` missing).

- [ ] **Step 3: Add `NoteStatus` enum and frontmatter fields**

Near `Severity`, add:

```rust
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[schemars(rename_all = "lowercase")]
pub enum NoteStatus {
    #[default]
    Active,
    Deprecated,
    Contradicted,
}
```

Extend `Frontmatter`:

```rust
#[derive(Debug, Deserialize, Serialize)]
struct Frontmatter {
    #[serde(default)] category: String,
    #[serde(default)] tags: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_optional_date_string")] created: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_date_string")] updated: Option<String>,
    #[serde(default = "default_confidence")] confidence: f32,
    #[serde(default)] severity: Severity,
    #[serde(default)] source_facts: Vec<String>,
    #[serde(default)] status: NoteStatus,
    #[serde(default)] supersedes: Vec<String>,
    #[serde(default)] superseded_by: Vec<String>,
}
```

Extend `KnowledgeNote`:

```rust
pub struct KnowledgeNote {
    // ...existing fields...
    pub status: NoteStatus,
    pub supersedes: Vec<String>,
    pub superseded_by: Vec<String>,
    pub fact_provenance: Vec<FactProvenance>, // populated in Task 2; default empty
}
```

Update `Default for KnowledgeNote`:

```rust
status: NoteStatus::default(),
supersedes: Vec::new(),
superseded_by: Vec::new(),
fact_provenance: Vec::new(),
```

Update `from_markdown` to read the new fields:

```rust
status: frontmatter.status,
supersedes: frontmatter.supersedes,
superseded_by: frontmatter.superseded_by,
fact_provenance: Vec::new(),
```

Update `to_markdown` to write the new fields (after `source_facts`):

```rust
let status_str = match self.status {
    NoteStatus::Active => "active",
    NoteStatus::Deprecated => "deprecated",
    NoteStatus::Contradicted => "contradicted",
};
out.push_str(&format!("status: {status_str}\n"));
out.push_str(&format!("supersedes: {}\n", yaml_inline_array(&self.supersedes)));
out.push_str(&format!("superseded_by: {}\n", yaml_inline_array(&self.superseded_by)));
```

- [ ] **Step 4: Run tests — should pass**

```bash
cargo test -p alephcore --lib memory::notes::note
```
Expected: all green; legacy tests still pass thanks to `#[serde(default)]`.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/note.rs
git commit -m "feat(notes): add status / supersedes / superseded_by frontmatter fields"
```

---

## Task 2 (C2.2): Paragraph-level provenance — types, parser, writer

**Files:**
- Modify: `src/memory/notes/note.rs` (add `FactProvenance`, `ProvenanceOrigin`, `extract_provenance_markers`)

- [ ] **Step 1: Write failing tests**

Add to `mod tests`:

```rust
#[test]
fn extract_provenance_markers_handles_all_origins() {
    let body = "- a <!-- src: raw/abc, origin: raw_source, inferred: false -->
- b <!-- origin: inferred, inferred: true -->
- c <!-- src: note/x, origin: prior_note, inferred: false -->
- legacy fact with no marker
";
    let provs = extract_provenance_markers(body, &extract_facts(body));
    assert_eq!(provs.len(), 4);
    assert_eq!(provs[0].origin, ProvenanceOrigin::RawSource);
    assert_eq!(provs[0].source_id.as_deref(), Some("raw/abc"));
    assert_eq!(provs[1].origin, ProvenanceOrigin::Inferred);
    assert!(provs[1].inferred);
    assert_eq!(provs[2].origin, ProvenanceOrigin::PriorNote);
    assert_eq!(provs[3].origin, ProvenanceOrigin::Legacy);
}

#[test]
fn fts_body_strips_provenance_comments() {
    let n = KnowledgeNote::from_markdown("t",
        "---\ncategory: preference\ntags: []\n---\n\n- a <!-- src: raw/x, origin: raw_source, inferred: false -->\n- b\n",
    ).unwrap();
    let fts = n.body_text_for_fts();
    assert!(!fts.contains("<!--"));
    assert!(fts.contains("a"));
    assert!(fts.contains("b"));
}
```

- [ ] **Step 2: Run tests — should fail**

```bash
cargo test -p alephcore --lib memory::notes::note::tests::extract_provenance_markers_handles_all_origins memory::notes::note::tests::fts_body_strips_provenance_comments
```
Expected: fail (types missing).

- [ ] **Step 3: Define types and parser**

Add to `src/memory/notes/note.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvenanceOrigin {
    RawSource,
    PriorNote,
    Inferred,
    Legacy,
}

#[derive(Debug, Clone)]
pub struct FactProvenance {
    pub origin: ProvenanceOrigin,
    pub source_id: Option<String>,
    pub inferred: bool,
}

impl Default for FactProvenance {
    fn default() -> Self {
        Self { origin: ProvenanceOrigin::Legacy, source_id: None, inferred: false }
    }
}

static PROVENANCE_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"<!--\s*(?:src:\s*([^,]+?),\s*)?origin:\s*(raw_source|prior_note|inferred|legacy)\s*,\s*inferred:\s*(true|false)\s*-->",
    ).unwrap()
});

/// Parse a fact-line list and return one FactProvenance per fact, defaulting
/// to `Legacy` when no marker is present.
pub fn extract_provenance_markers(body: &str, facts: &[String]) -> Vec<FactProvenance> {
    // Walk the body line-by-line in parallel with the parsed facts. Each
    // top-level "- " line corresponds to facts[i]; trailing comment on the
    // same physical line is the marker.
    let mut out: Vec<FactProvenance> = Vec::with_capacity(facts.len());
    let mut idx = 0;
    for raw_line in body.lines() {
        if idx >= facts.len() { break; }
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("- ") {
            let prov = PROVENANCE_RE
                .captures(raw_line)
                .map(|c| FactProvenance {
                    origin: match &c[2] {
                        "raw_source" => ProvenanceOrigin::RawSource,
                        "prior_note" => ProvenanceOrigin::PriorNote,
                        "inferred"   => ProvenanceOrigin::Inferred,
                        _            => ProvenanceOrigin::Legacy,
                    },
                    source_id: c.get(1).map(|m| m.as_str().trim().to_string()),
                    inferred: &c[3] == "true",
                })
                .unwrap_or_default();
            out.push(prov);
            idx += 1;
        }
    }
    while out.len() < facts.len() {
        out.push(FactProvenance::default());
    }
    out
}
```

In `KnowledgeNote::from_markdown`, populate after `extract_facts`:

```rust
let fact_provenance = extract_provenance_markers(&body, &facts);
```

Add a `body_text_for_fts` method (strips comments):

```rust
impl KnowledgeNote {
    pub fn body_text_for_fts(&self) -> String {
        // Re-emit body without HTML comments. Use the parsed facts list which
        // already holds the structural content; do not re-parse markdown.
        self.facts.iter()
            .map(|f| PROVENANCE_RE.replace_all(f, "").trim().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
```

Update the FTS write path in `src/memory/store/sqlite/notes.rs` (where it calls `note.body_text()`) to use `note.body_text_for_fts()` instead.

- [ ] **Step 4: Run tests — should pass**

```bash
cargo test -p alephcore --lib memory::notes::note memory::store::sqlite::notes
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/note.rs src/memory/store/sqlite/notes.rs
git commit -m "feat(notes): paragraph-level provenance types + parser; FTS strips comments"
```

---

## Task 3 (C2.9.1): SQLite tables for governance

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs` (add three table DDLs)

- [ ] **Step 1: Write a presence test**

Add to `src/memory/store/sqlite/schema.rs` `mod tests`:

```rust
#[test]
fn governance_tables_present_after_init() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();
    for table in ["notes_provenance", "notes_review_queue", "notes_review_archive"] {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![table],
                |_| Ok(true),
            )
            .optional()
            .unwrap()
            .unwrap_or(false);
        assert!(exists, "{table} missing");
    }
}
```

- [ ] **Step 2: Run test — should fail**

```bash
cargo test -p alephcore --lib memory::store::sqlite::schema::tests::governance_tables_present_after_init
```
Expected: fail.

- [ ] **Step 3: Add DDL**

In `src/memory/store/sqlite/schema.rs`, add the three blocks and wire them into `init_schema`:

```rust
const NOTES_PROVENANCE_DDL: &str = "
CREATE TABLE IF NOT EXISTS notes_provenance (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL,
    note_path   TEXT NOT NULL,
    fact_idx    INTEGER NOT NULL,
    origin      TEXT NOT NULL,
    source_kind TEXT,
    source_id   TEXT,
    inferred    INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_prov_path ON notes_provenance(agent_id, note_path);
CREATE INDEX IF NOT EXISTS idx_prov_source ON notes_provenance(source_kind, source_id);
";

const NOTES_REVIEW_QUEUE_DDL: &str = "
CREATE TABLE IF NOT EXISTS notes_review_queue (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL,
    candidate_json  TEXT NOT NULL,
    severity        TEXT NOT NULL,
    confidence      REAL NOT NULL,
    reason          TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending',
    retry_count     INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    decided_at      INTEGER,
    decision_actor  TEXT
);
CREATE INDEX IF NOT EXISTS idx_review_pending
    ON notes_review_queue(agent_id, status, created_at);
";

const NOTES_REVIEW_ARCHIVE_DDL: &str = "
CREATE TABLE IF NOT EXISTS notes_review_archive (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL,
    candidate_json  TEXT NOT NULL,
    final_status    TEXT NOT NULL,
    reason          TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    archived_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_archive_age
    ON notes_review_archive(archived_at);
";
```

In `init_schema`, after the existing notes_* tables:

```rust
conn.execute_batch(NOTES_PROVENANCE_DDL)
    .map_err(|e| AlephError::config(format!("Failed to create notes_provenance: {e}")))?;
conn.execute_batch(NOTES_REVIEW_QUEUE_DDL)
    .map_err(|e| AlephError::config(format!("Failed to create notes_review_queue: {e}")))?;
conn.execute_batch(NOTES_REVIEW_ARCHIVE_DDL)
    .map_err(|e| AlephError::config(format!("Failed to create notes_review_archive: {e}")))?;
```

- [ ] **Step 4: Run test — should pass**

```bash
cargo test -p alephcore --lib memory::store::sqlite::schema
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/store/sqlite/schema.rs
git commit -m "feat(notes): governance schema (notes_provenance, review_queue, review_archive)"
```

---

## Task 4 (C2.9.2): NoteStore methods for provenance + review

**Files:**
- Modify: `src/memory/notes/store.rs` (trait additions)
- Modify: `src/memory/store/sqlite/notes.rs` (impls)

- [ ] **Step 1: Write tests**

Add to `src/memory/store/sqlite/notes.rs` `mod tests`:

```rust
#[tokio::test]
async fn upsert_provenance_writes_one_row_per_fact() {
    use crate::memory::notes::{FactProvenance, ProvenanceOrigin};
    let db = SqliteMemoryBackend::new(&std::env::temp_dir().join(
        format!("aleph_prov_{}", uuid::Uuid::new_v4())
    )).unwrap();

    let provs = vec![
        FactProvenance { origin: ProvenanceOrigin::RawSource, source_id: Some("raw/x".into()), inferred: false },
        FactProvenance { origin: ProvenanceOrigin::Inferred,  source_id: None, inferred: true },
    ];
    db.upsert_provenance("default", "preference/p", &provs).await.unwrap();

    let rows = db.get_provenance("default", "preference/p").await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].source_id.as_deref(), Some("raw/x"));
    assert!(rows[1].inferred);
}

#[tokio::test]
async fn enqueue_and_list_review_pending() {
    let db = SqliteMemoryBackend::new(&std::env::temp_dir().join(
        format!("aleph_q_{}", uuid::Uuid::new_v4())
    )).unwrap();
    let id = db.enqueue_review("default", r#"{"any":"json"}"#, "high", 0.4, "low confidence").await.unwrap();
    let pending = db.list_pending_review("default", 1714000000).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, id);
}
```

- [ ] **Step 2: Run tests — should fail**

```bash
cargo test -p alephcore --lib memory::store::sqlite::notes::tests::upsert_provenance_writes_one_row_per_fact memory::store::sqlite::notes::tests::enqueue_and_list_review_pending
```
Expected: fail (methods missing).

- [ ] **Step 3: Add trait methods + impls**

In `src/memory/notes/store.rs`, append to `NoteStore`:

```rust
async fn upsert_provenance(
    &self,
    agent_id: &str,
    note_path: &str,
    provs: &[FactProvenance],
) -> Result<(), AlephError>;

async fn get_provenance(
    &self,
    agent_id: &str,
    note_path: &str,
) -> Result<Vec<FactProvenance>, AlephError>;

async fn enqueue_review(
    &self,
    agent_id: &str,
    candidate_json: &str,
    severity: &str,
    confidence: f32,
    reason: &str,
) -> Result<String, AlephError>;

async fn list_pending_review(
    &self,
    agent_id: &str,
    earlier_than: i64,
) -> Result<Vec<ReviewQueueRow>, AlephError>;

async fn mark_review_decided(
    &self,
    queue_id: &str,
    new_status: &str,
    decision_actor: &str,
) -> Result<(), AlephError>;

async fn archive_review(
    &self,
    queue_id: &str,
    final_status: &str,
) -> Result<(), AlephError>;
```

Define the row struct in `src/memory/notes/store.rs`:

```rust
pub struct ReviewQueueRow {
    pub id: String,
    pub agent_id: String,
    pub candidate_json: String,
    pub severity: String,
    pub confidence: f32,
    pub reason: String,
    pub status: String,
    pub retry_count: i64,
    pub created_at: i64,
}
```

Implement in `src/memory/store/sqlite/notes.rs`:

```rust
async fn upsert_provenance(&self, agent_id: &str, note_path: &str, provs: &[FactProvenance]) -> Result<(), AlephError> {
    let conn = lock_conn!(self)?;
    conn.execute(
        "DELETE FROM notes_provenance WHERE agent_id = ?1 AND note_path = ?2",
        params![agent_id, note_path],
    ).map_err(|e| AlephError::config(format!("prov delete: {e}")))?;
    let now = chrono::Utc::now().timestamp();
    for (idx, p) in provs.iter().enumerate() {
        let origin_str = match p.origin {
            ProvenanceOrigin::RawSource => "raw_source",
            ProvenanceOrigin::PriorNote => "prior_note",
            ProvenanceOrigin::Inferred  => "inferred",
            ProvenanceOrigin::Legacy    => "legacy",
        };
        let source_kind = match p.origin {
            ProvenanceOrigin::RawSource => Some("raw"),
            ProvenanceOrigin::PriorNote => Some("note"),
            _ => None,
        };
        conn.execute(
            "INSERT INTO notes_provenance (agent_id, note_path, fact_idx, origin, source_kind, source_id, inferred, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![agent_id, note_path, idx as i64, origin_str, source_kind, p.source_id, p.inferred as i64, now],
        ).map_err(|e| AlephError::config(format!("prov insert: {e}")))?;
    }
    Ok(())
}

async fn get_provenance(&self, agent_id: &str, note_path: &str) -> Result<Vec<FactProvenance>, AlephError> {
    let conn = lock_conn!(self)?;
    let mut stmt = conn.prepare(
        "SELECT origin, source_id, inferred FROM notes_provenance
         WHERE agent_id = ?1 AND note_path = ?2 ORDER BY fact_idx",
    ).map_err(|e| AlephError::config(format!("prov get prep: {e}")))?;
    let rows: Vec<FactProvenance> = stmt
        .query_map(params![agent_id, note_path], |r| {
            let origin_s: String = r.get(0)?;
            let source_id: Option<String> = r.get(1)?;
            let inferred: i64 = r.get(2)?;
            let origin = match origin_s.as_str() {
                "raw_source" => ProvenanceOrigin::RawSource,
                "prior_note" => ProvenanceOrigin::PriorNote,
                "inferred"   => ProvenanceOrigin::Inferred,
                _            => ProvenanceOrigin::Legacy,
            };
            Ok(FactProvenance { origin, source_id, inferred: inferred != 0 })
        })
        .map_err(|e| AlephError::config(format!("prov get exec: {e}")))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

async fn enqueue_review(&self, agent_id: &str, candidate_json: &str, severity: &str, confidence: f32, reason: &str) -> Result<String, AlephError> {
    let conn = lock_conn!(self)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO notes_review_queue (id, agent_id, candidate_json, severity, confidence, reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, agent_id, candidate_json, severity, confidence, reason, now],
    ).map_err(|e| AlephError::config(format!("enqueue review: {e}")))?;
    Ok(id)
}

async fn list_pending_review(&self, agent_id: &str, earlier_than: i64) -> Result<Vec<ReviewQueueRow>, AlephError> {
    let conn = lock_conn!(self)?;
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, candidate_json, severity, confidence, reason, status, retry_count, created_at
         FROM notes_review_queue
         WHERE agent_id = ?1 AND status = 'pending' AND created_at <= ?2
         ORDER BY created_at",
    ).map_err(|e| AlephError::config(format!("review list prep: {e}")))?;
    let rows: Vec<ReviewQueueRow> = stmt
        .query_map(params![agent_id, earlier_than], |r| {
            Ok(ReviewQueueRow {
                id: r.get(0)?,
                agent_id: r.get(1)?,
                candidate_json: r.get(2)?,
                severity: r.get(3)?,
                confidence: r.get::<_, f64>(4)? as f32,
                reason: r.get(5)?,
                status: r.get(6)?,
                retry_count: r.get(7)?,
                created_at: r.get(8)?,
            })
        })
        .map_err(|e| AlephError::config(format!("review list exec: {e}")))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

async fn mark_review_decided(&self, queue_id: &str, new_status: &str, decision_actor: &str) -> Result<(), AlephError> {
    let conn = lock_conn!(self)?;
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE notes_review_queue SET status = ?1, decided_at = ?2, decision_actor = ?3 WHERE id = ?4",
        params![new_status, now, decision_actor, queue_id],
    ).map_err(|e| AlephError::config(format!("review decide: {e}")))?;
    Ok(())
}

async fn archive_review(&self, queue_id: &str, final_status: &str) -> Result<(), AlephError> {
    let conn = lock_conn!(self)?;
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO notes_review_archive (id, agent_id, candidate_json, final_status, reason, created_at, archived_at)
         SELECT id, agent_id, candidate_json, ?1, reason, created_at, ?2 FROM notes_review_queue WHERE id = ?3;
         DELETE FROM notes_review_queue WHERE id = ?3;",
        params![final_status, now, queue_id],
    ).map_err(|e| AlephError::config(format!("review archive: {e}")))?;
    Ok(())
}
```

- [ ] **Step 4: Wire `upsert_provenance` into `index_note` write path**

In `src/memory/store/sqlite/notes.rs::index_note`, after the existing FTS write block, add:

```rust
self.upsert_provenance(agent_id, path, &note.fact_provenance).await?;
```

(The path is the local `&str` already in scope. If `index_note` is `&self`, calling `self.upsert_provenance` requires releasing the `lock_conn!` guard first; restructure to a two-step approach or use the connection directly inline.)

- [ ] **Step 5: Run tests**

```bash
cargo test -p alephcore --lib memory::store::sqlite::notes memory::notes
```
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/store.rs src/memory/store/sqlite/notes.rs
git commit -m "feat(notes): NoteStore methods for provenance and review queue"
```

---

## Task 5 (C2.3): `notes/governance/gate.rs`

**Files:**
- Create: `src/memory/notes/governance/mod.rs`
- Create: `src/memory/notes/governance/gate.rs`
- Modify: `src/memory/notes/mod.rs` (`pub mod governance;`)

- [ ] **Step 1: Write failing tests**

Create `src/memory/notes/governance/gate.rs` with the test stub first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;
    use std::sync::Arc;

    fn make_candidate(severity: crate::memory::notes::Severity, confidence: f32) -> CandidateNote {
        CandidateNote {
            agent_id: "default".into(),
            category: "preference".into(),
            note: KnowledgeNote {
                title: "x".into(),
                category: "preference".into(),
                facts: vec!["body".into()],
                severity,
                confidence,
                ..Default::default()
            },
            source_path: None,
            fact_provenance: vec![],
            action: NoteWriteAction::Create,
            bypass_review: false,
            contradicts_existing: false,
        }
    }

    fn make_store() -> Arc<crate::memory::store::SqliteMemoryBackend> {
        Arc::new(crate::memory::store::SqliteMemoryBackend::new(
            &std::env::temp_dir().join(format!("aleph_gate_{}", uuid::Uuid::new_v4())),
        ).unwrap())
    }

    #[tokio::test]
    async fn defers_low_confidence() {
        let store = make_store();
        let gate = DefaultNoteWriteGate::new(store.clone(), Default::default());
        let cand = make_candidate(crate::memory::notes::Severity::Low, 0.4);
        match gate.evaluate(&cand).await.unwrap() {
            GateOutcome::Defer { .. } => {},
            other => panic!("expected Defer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn admits_high_confidence_low_severity() {
        let store = make_store();
        let gate = DefaultNoteWriteGate::new(store.clone(), Default::default());
        let cand = make_candidate(crate::memory::notes::Severity::Low, 0.9);
        match gate.evaluate(&cand).await.unwrap() {
            GateOutcome::Accept(_) => {},
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn defers_high_severity_medium_confidence() {
        let store = make_store();
        let gate = DefaultNoteWriteGate::new(store.clone(), Default::default());
        let cand = make_candidate(crate::memory::notes::Severity::High, 0.7);
        assert!(matches!(gate.evaluate(&cand).await.unwrap(), GateOutcome::Defer { .. }));
    }

    #[tokio::test]
    async fn bypass_review_admits_unconditionally() {
        let store = make_store();
        let gate = DefaultNoteWriteGate::new(store.clone(), Default::default());
        let mut cand = make_candidate(crate::memory::notes::Severity::Critical, 0.1);
        cand.bypass_review = true;
        assert!(matches!(gate.evaluate(&cand).await.unwrap(), GateOutcome::Accept(_)));
    }

    #[tokio::test]
    async fn delete_critical_defers() {
        let store = make_store();
        let gate = DefaultNoteWriteGate::new(store.clone(), Default::default());
        let mut cand = make_candidate(crate::memory::notes::Severity::Critical, 0.95);
        cand.action = NoteWriteAction::Delete;
        assert!(matches!(gate.evaluate(&cand).await.unwrap(), GateOutcome::Defer { .. }));
    }
}
```

- [ ] **Step 2: Implement gate**

Replace the rest of `src/memory/notes/governance/gate.rs`:

```rust
//! Unified raw → note write gate. Concentrates Accept/Defer/Reject routing
//! plus the side effects of writing review queue / archive rows.

use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::notes::{FactProvenance, KnowledgeNote, Severity};
use crate::memory::notes::store::NoteStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteWriteAction { Create, Update, Append, Delete }

#[derive(Debug, Clone)]
pub struct CandidateNote {
    pub agent_id: String,
    pub category: String,
    pub note: KnowledgeNote,
    pub source_path: Option<String>,
    pub fact_provenance: Vec<FactProvenance>,
    pub action: NoteWriteAction,
    pub bypass_review: bool,
    pub contradicts_existing: bool,
}

#[derive(Debug)]
pub enum GateOutcome {
    Accept(CandidateNote),
    Defer { queue_id: String, reason: String },
    Reject { archive_id: String, reason: String },
}

#[derive(Debug, Clone)]
pub struct GateThresholds {
    pub min_confidence: f32,
    pub high_severity_min_confidence: f32,
    pub critical_severity_floor: f32,
}

impl Default for GateThresholds {
    fn default() -> Self {
        Self { min_confidence: 0.5, high_severity_min_confidence: 0.8, critical_severity_floor: 0.85 }
    }
}

#[async_trait]
pub trait NoteWriteGate: Send + Sync {
    async fn evaluate(&self, candidate: &CandidateNote) -> Result<GateOutcome, AlephError>;
}

pub struct DefaultNoteWriteGate {
    store: Arc<dyn NoteStore + Send + Sync>,
    thresholds: GateThresholds,
}

impl DefaultNoteWriteGate {
    pub fn new(store: Arc<dyn NoteStore + Send + Sync>, thresholds: GateThresholds) -> Self {
        Self { store, thresholds }
    }
}

#[async_trait]
impl NoteWriteGate for DefaultNoteWriteGate {
    async fn evaluate(&self, candidate: &CandidateNote) -> Result<GateOutcome, AlephError> {
        if candidate.bypass_review {
            return Ok(GateOutcome::Accept(candidate.clone()));
        }

        // Delete of a Critical-severity note: defer.
        if matches!(candidate.action, NoteWriteAction::Delete)
            && candidate.note.severity == Severity::Critical
        {
            return self.defer(candidate, "delete of critical note requires review").await;
        }

        if candidate.note.confidence < self.thresholds.min_confidence {
            return self.defer(candidate, "confidence below minimum").await;
        }
        if candidate.note.severity >= Severity::High
            && candidate.note.confidence < self.thresholds.high_severity_min_confidence
        {
            return self.defer(candidate, "high severity needs higher confidence").await;
        }
        if candidate.contradicts_existing {
            return self.defer(candidate, "contradicts existing note").await;
        }

        Ok(GateOutcome::Accept(candidate.clone()))
    }
}

impl DefaultNoteWriteGate {
    async fn defer(&self, candidate: &CandidateNote, reason: &str) -> Result<GateOutcome, AlephError> {
        let json = serde_json::to_string(candidate).map_err(|e| AlephError::config(format!("candidate serialize: {e}")))?;
        let severity_str = format!("{:?}", candidate.note.severity).to_lowercase();
        let queue_id = self.store
            .enqueue_review(&candidate.agent_id, &json, &severity_str, candidate.note.confidence, reason)
            .await?;
        Ok(GateOutcome::Defer { queue_id, reason: reason.to_string() })
    }
}
```

You will also need `Severity` to derive `PartialOrd, Ord` (with explicit ordering Low < Med < High < Critical) to support the `>=` comparison. Modify `Severity` accordingly:

```rust
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
```

The variant order in source already matches Low, Med, High, Critical so derived ordering is correct.

You will also need `CandidateNote: Serialize` and `Deserialize`. Add the derive on `CandidateNote`, `NoteWriteAction`, and import `serde::{Serialize, Deserialize}`.

- [ ] **Step 3: Wire module visibility**

Add to `src/memory/notes/mod.rs`:

```rust
pub mod governance;
pub use governance::gate::{
    CandidateNote, DefaultNoteWriteGate, GateOutcome, GateThresholds, NoteWriteAction, NoteWriteGate,
};
```

Create `src/memory/notes/governance/mod.rs`:

```rust
pub mod gate;
pub mod supersession;
```

(`supersession` is added in Task 7; the mod declaration can pre-empt with an empty file for now.)

- [ ] **Step 4: Run tests — should pass**

```bash
cargo test -p alephcore --lib memory::notes::governance::gate
```
Expected: all five tests green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/governance/ src/memory/notes/mod.rs src/memory/notes/note.rs
git commit -m "feat(notes): governance::gate with default thresholds + side-effecting Defer"
```

---

## Task 6 (C2.3.2): Mount the gate in three write paths

**Files:**
- Modify: `src/memory/notes/ingest/apply.rs`
- Modify: `src/memory/dreaming/stages/feedback_distill.rs`
- Modify: `src/builtin_tools/note_manage.rs`

- [ ] **Step 1: Write integration test**

Create `tests/note_governance_gate.rs`:

```rust
use std::sync::Arc;
use alephcore::memory::notes::{KnowledgeNote, Severity};
use alephcore::memory::notes::governance::gate::{DefaultNoteWriteGate, GateOutcome, GateThresholds, CandidateNote, NoteWriteAction};
use alephcore::memory::store::SqliteMemoryBackend;

#[tokio::test]
async fn ingest_low_confidence_lands_in_queue_not_markdown() {
    let store = Arc::new(SqliteMemoryBackend::new(
        &std::env::temp_dir().join(format!("aleph_int_{}", uuid::Uuid::new_v4())),
    ).unwrap());
    let gate = DefaultNoteWriteGate::new(store.clone(), GateThresholds::default());

    let cand = CandidateNote {
        agent_id: "default".into(),
        category: "preference".into(),
        note: KnowledgeNote {
            title: "untrusted".into(),
            category: "preference".into(),
            facts: vec!["x".into()],
            confidence: 0.3,
            severity: Severity::Low,
            ..Default::default()
        },
        source_path: None,
        fact_provenance: vec![],
        action: NoteWriteAction::Create,
        bypass_review: false,
        contradicts_existing: false,
    };

    let out = gate.evaluate(&cand).await.unwrap();
    let queue_id = match out {
        GateOutcome::Defer { queue_id, .. } => queue_id,
        _ => panic!("expected Defer"),
    };

    let pending = store.list_pending_review("default", chrono::Utc::now().timestamp() + 1).await.unwrap();
    assert!(pending.iter().any(|p| p.id == queue_id));
}
```

- [ ] **Step 2: Run test — should pass with already-built gate**

```bash
cargo test -p alephcore --test note_governance_gate
```
Expected: green.

- [ ] **Step 3: Insert gate into `apply.rs::write_note`**

In `src/memory/notes/ingest/apply.rs`, locate every site that ultimately calls `NoteIndexer::write_note` (or `index_file`). Wrap with:

```rust
use crate::memory::notes::governance::gate::{
    CandidateNote, GateOutcome, NoteWriteAction, NoteWriteGate,
};

let candidate = CandidateNote {
    agent_id: agent_id.to_string(),
    category: category.to_string(),
    note: note.clone(),
    source_path: Some(source.to_string()),
    fact_provenance: note.fact_provenance.clone(),
    action: NoteWriteAction::Create, // or Update / Append, depending on the call site
    bypass_review: false,
    contradicts_existing: plan_flagged_contradiction,
};

match self.gate.evaluate(&candidate).await? {
    GateOutcome::Accept(c) => self.indexer.write_note(&c.agent_id, &c.category, &c.note).await?,
    GateOutcome::Defer { queue_id, reason } => {
        tracing::info!(queue_id, reason, "ingest deferred to review queue");
        return Ok(());
    }
    GateOutcome::Reject { archive_id, reason } => {
        tracing::warn!(archive_id, reason, "ingest rejected at gate");
        return Ok(());
    }
}
```

`self.gate` is a new `Arc<dyn NoteWriteGate>` field on `DefaultCompoundIngestor`. Add it to the struct, the constructor, and the builder wiring (`src/bin/aleph-server/commands/start/builder/agent_init.rs:1239-1273`).

- [ ] **Step 4: Insert gate into `feedback_distill.rs::execute`**

Same pattern, in `src/memory/dreaming/stages/feedback_distill.rs`. The stage already holds a `DreamContext`; add an `Option<Arc<dyn NoteWriteGate>>` to `DreamContext` (or to the stage struct), set it during dream pipeline construction, and gate every `index_file` call.

- [ ] **Step 5: Insert gate into `note_manage` Create/Update/Append/Delete**

In `src/builtin_tools/note_manage.rs`, each action handler already constructs a `KnowledgeNote`. Wrap the indexer call with a `gate.evaluate` invocation. Action mapping:
- `NoteManageAction::Create` → `NoteWriteAction::Create`
- `NoteManageAction::Update` → `NoteWriteAction::Update`
- `NoteManageAction::Append` → `NoteWriteAction::Append`
- `NoteManageAction::Delete` → `NoteWriteAction::Delete`

- [ ] **Step 6: Run full test set**

```bash
cargo test -p alephcore --lib
cargo test -p alephcore --test note_governance_gate
```
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add src/memory/notes/ingest/apply.rs src/memory/dreaming/stages/feedback_distill.rs src/builtin_tools/note_manage.rs src/bin/aleph-server/commands/start/builder/agent_init.rs src/memory/dreaming/mod.rs
git commit -m "feat(notes): mount governance gate in ingest, feedback_distill, and note_manage paths"
```

---

## Task 7 (C2.4): `governance::supersession` — frontmatter ↔ body sync

**Files:**
- Create: `src/memory/notes/governance/supersession.rs`

- [ ] **Step 1: Write failing tests**

```rust
// in src/memory/notes/governance/supersession.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::{KnowledgeNote, NoteStatus};

    #[test]
    fn body_section_promotes_to_frontmatter() {
        let md = "---\ncategory: preference\ntags: []\n---\n\n- claim\n\n## Superseded by [[preference/new]]\n";
        let mut n = KnowledgeNote::from_markdown("old", md).unwrap();
        sync_body_to_frontmatter(&mut n, md);
        assert_eq!(n.superseded_by, vec!["preference/new".to_string()]);
    }

    #[test]
    fn frontmatter_emits_body_section_on_write() {
        let n = KnowledgeNote {
            title: "old".into(), category: "preference".into(),
            facts: vec!["claim".into()],
            superseded_by: vec!["preference/new".into()],
            ..Default::default()
        };
        let md = n.to_markdown();
        let with_section = ensure_supersession_section(&md, &n);
        assert!(with_section.contains("## Superseded by [[preference/new]]"));
    }

    #[test]
    fn idempotent_when_already_synced() {
        let md = "---\ncategory: preference\ntags: []\nsuperseded_by: [preference/new]\n---\n\n- x\n\n## Superseded by [[preference/new]]\n";
        let n = KnowledgeNote::from_markdown("old", md).unwrap();
        let again = ensure_supersession_section(&md, &n);
        let count = again.matches("## Superseded by [[preference/new]]").count();
        assert_eq!(count, 1);
    }
}
```

- [ ] **Step 2: Implement**

```rust
//! Frontmatter ↔ body bidirectional supersession sync.

use crate::memory::notes::KnowledgeNote;

static SUPERSEDED_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?m)^## Superseded by \[\[([^\]]+)\]\]\s*$").unwrap()
});

/// Read body for `## Superseded by [[X]]` headings and merge targets into
/// `note.superseded_by` (union semantics).
pub fn sync_body_to_frontmatter(note: &mut KnowledgeNote, full_markdown: &str) {
    let mut existing: std::collections::HashSet<String> = note.superseded_by.iter().cloned().collect();
    for cap in SUPERSEDED_RE.captures_iter(full_markdown) {
        existing.insert(cap[1].to_string());
    }
    note.superseded_by = existing.into_iter().collect();
    note.superseded_by.sort();
}

/// Append `## Superseded by [[X]]` sections to the markdown for any
/// `superseded_by` entry not already present. Idempotent.
pub fn ensure_supersession_section(markdown: &str, note: &KnowledgeNote) -> String {
    let mut out = markdown.to_string();
    for target in &note.superseded_by {
        let line = format!("## Superseded by [[{target}]]");
        if !markdown.contains(&line) {
            if !out.ends_with('\n') { out.push('\n'); }
            out.push('\n');
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}
```

- [ ] **Step 3: Wire into indexer**

In `src/memory/notes/indexer.rs`, locate the function that calls `KnowledgeNote::from_markdown` after reading a file (e.g. `index_file`). After the `let mut note = KnowledgeNote::from_markdown(...)` call:

```rust
crate::memory::notes::governance::supersession::sync_body_to_frontmatter(&mut note, &content);
```

In the function that calls `to_markdown` for write (`write_note`, `append_to_note`), wrap the result:

```rust
let md = note.to_markdown();
let md = crate::memory::notes::governance::supersession::ensure_supersession_section(&md, &note);
tokio::fs::write(&path, &md).await?;
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore --lib memory::notes::governance::supersession memory::notes::indexer
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/governance/ src/memory/notes/indexer.rs
git commit -m "feat(notes): governance::supersession syncs frontmatter and body section"
```

---

## Task 8 (C2.5): `dreaming/stages/note_review.rs`

**Files:**
- Create: `src/memory/dreaming/stages/note_review.rs`
- Modify: `src/memory/dreaming/stages/mod.rs` and `dreaming/strategy.rs`

- [ ] **Step 1: Write failing test**

Add a test inside the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn approves_pending_then_applies() {
        // Build dream context with a stub LLM client that returns
        // {"verdict":"approve","reason":"ok"} and a store with one pending row.
        // Run stage, assert: queue row marked approved + apply.rs wrote the note + bypass_review was true.
        // (Implement against existing test fixtures.)
    }

    #[tokio::test]
    async fn rejects_archives() { /* similar */ }

    #[tokio::test]
    async fn rewrite_applies_substituted_content() { /* similar */ }

    #[tokio::test]
    async fn three_failures_archive_as_timeout() { /* simulate retry_count >= 3 */ }
}
```

- [ ] **Step 2: Implement stage**

```rust
//! NoteReview stage — consumes notes_review_queue and routes via LLM verdict.

use std::sync::Arc;
use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::governance::gate::{CandidateNote, NoteWriteGate, NoteWriteAction, GateOutcome};
use crate::memory::notes::store::NoteStore;
use crate::providers::adapter::RequestPayload;

use super::DreamStage;

pub struct NoteReviewStage {
    pub dwell_seconds: i64,
    pub max_retries: i64,
}

impl Default for NoteReviewStage {
    fn default() -> Self { Self { dwell_seconds: 300, max_retries: 3 } }
}

#[async_trait]
impl DreamStage for NoteReviewStage {
    fn name(&self) -> &'static str { "note_review" }

    async fn should_run(&self, _ctx: &DreamContext) -> bool { true }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let now = chrono::Utc::now().timestamp();
        let earlier = now - self.dwell_seconds;
        let pending = ctx.store.list_pending_review(&ctx.agent_id, earlier).await?;

        for row in pending {
            let candidate: CandidateNote = match serde_json::from_str(&row.candidate_json) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, queue_id = %row.id, "candidate json parse failed");
                    if row.retry_count + 1 >= self.max_retries {
                        let _ = ctx.store.archive_review(&row.id, "timeout").await;
                    }
                    continue;
                }
            };

            // Build LLM prompt with candidate + nearest 3-5 notes in same category as comparison.
            let nearest = ctx.store
                .get_notes_by_category(&ctx.agent_id, &candidate.category, 5)
                .await
                .unwrap_or_default();

            let verdict = match call_review_llm(&ctx, &candidate, &nearest).await {
                Ok(v) => v,
                Err(_) => {
                    // bump retry counter via decided update with status=pending then retry_count+1
                    if row.retry_count + 1 >= self.max_retries {
                        ctx.store.archive_review(&row.id, "timeout").await?;
                    }
                    continue;
                }
            };

            match verdict {
                ReviewVerdict::Approve => {
                    let mut admitted = candidate.clone();
                    admitted.bypass_review = true;
                    apply_admitted(&ctx, &admitted).await?;
                    ctx.store.mark_review_decided(&row.id, "approved", "llm_review").await?;
                    ctx.store.archive_review(&row.id, "approved").await?;
                }
                ReviewVerdict::Reject(reason) => {
                    ctx.store.mark_review_decided(&row.id, "rejected", "llm_review").await?;
                    ctx.store.archive_review(&row.id, "rejected").await?;
                    let _ = reason;
                }
                ReviewVerdict::Rewrite(new_content) => {
                    let mut admitted = candidate.clone();
                    admitted.note.facts = new_content;
                    admitted.bypass_review = true;
                    apply_admitted(&ctx, &admitted).await?;
                    ctx.store.mark_review_decided(&row.id, "rewritten", "llm_review").await?;
                    ctx.store.archive_review(&row.id, "rewritten").await?;
                }
            }
        }
        Ok(ctx)
    }
}

enum ReviewVerdict {
    Approve,
    Reject(String),
    Rewrite(Vec<String>),
}

async fn call_review_llm(
    ctx: &DreamContext,
    candidate: &CandidateNote,
    nearest: &[crate::memory::notes::store::NoteIndexEntry],
) -> Result<ReviewVerdict, AlephError> {
    // Fill in: build messages, call ctx.llm, parse JSON {verdict, reason, rewritten_content}.
    // Return one of the three variants. (Implement against existing provider fixtures.)
    let _ = (ctx, candidate, nearest);
    Ok(ReviewVerdict::Approve)
}

async fn apply_admitted(ctx: &DreamContext, admitted: &CandidateNote) -> Result<(), AlephError> {
    // Reuse the same write path as ingest. The simplest approach is to call
    // ctx.indexer.write_note (or index_file) directly with bypass_review semantics.
    let indexer = ctx.indexer.as_ref().ok_or_else(|| AlephError::config("dream ctx missing indexer"))?;
    indexer.write_note(&admitted.agent_id, &admitted.category, &admitted.note).await?;
    Ok(())
}
```

- [ ] **Step 3: Add to dream strategy**

In `src/memory/dreaming/strategy.rs`, insert `"note_review"` immediately after `"note_lint"` in every strategy vector.

In `src/memory/dreaming/stages/mod.rs`:

```rust
pub mod note_review;
pub use note_review::NoteReviewStage;
```

In `src/memory/dreaming/mod.rs`, where the stage list is dispatched, register:

```rust
"note_review" => Box::new(NoteReviewStage::default()),
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore --lib memory::dreaming::stages::note_review memory::dreaming::strategy
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/note_review.rs src/memory/dreaming/stages/mod.rs src/memory/dreaming/strategy.rs src/memory/dreaming/mod.rs
git commit -m "feat(dreaming): note_review stage consumes review queue, routes by LLM verdict"
```

---

## Task 9 (C2.6): `contradiction` category

**Files:**
- Modify: `src/memory/notes/indexer.rs:CATEGORY_DIRS`
- Modify: `src/builtin_tools/note_manage.rs:22-38` (category list)

- [ ] **Step 1: Write failing test**

Add to `src/builtin_tools/note_manage.rs` `mod tests`:

```rust
#[test]
fn validate_category_accepts_contradiction() {
    assert!(validate_category("contradiction").is_ok());
}
```

- [ ] **Step 2: Run test — should fail**

- [ ] **Step 3: Add `"contradiction"` to both lists**

In `src/memory/notes/indexer.rs:20`:

```rust
pub const CATEGORY_DIRS: &[&str] = &[
    "preference", "plan", "learning", "project", "personal",
    "tool", "lesson", "skill", "reference", "transcript",
    "subagent-run", "subagent-session", "subagent-checkpoint", "subagent-transcript",
    "contradiction",
    "other",
];
```

Mirror the change in `src/builtin_tools/note_manage.rs:22-38` (the tool-side category list). Also confirm `frontmatter_template` falls through to the default branch for `contradiction` (no special template needed).

- [ ] **Step 4: Run test**

```bash
cargo test -p alephcore --lib builtin_tools::note_manage
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/indexer.rs src/builtin_tools/note_manage.rs
git commit -m "feat(notes): add contradiction category for note_drift conflict pages"
```

---

## Task 10 (C2.7): Recall-driven confidence decay

**Files:**
- Modify: `src/memory/dreaming/stages/note_decay.rs`

- [ ] **Step 1: Write failing tests**

Add to that file's `mod tests`:

```rust
#[tokio::test]
async fn cold_low_severity_decays() {
    // Seed a note last-hit 365 days ago; severity Low; confidence 1.0.
    // Run decay; assert new confidence < 0.1.
}

#[tokio::test]
async fn high_severity_floor_holds() {
    // Same as above but severity High. Assert new confidence >= 0.7.
}

#[tokio::test]
async fn epsilon_avoids_micro_writes() {
    // Note last-hit yesterday; assert frontmatter NOT rewritten.
}
```

- [ ] **Step 2: Add helpers and update stage logic**

```rust
fn severity_floor(sev: crate::memory::notes::Severity) -> f32 {
    use crate::memory::notes::Severity::*;
    match sev { Low => 0.0, Med => 0.5, High => 0.7, Critical => 0.85 }
}

async fn days_since_last_hit(
    store: &dyn crate::memory::notes::store::NoteStore,
    note_path: &str,
) -> i64 {
    let last = store.recall_signals_last_hit(note_path).await.unwrap_or(None);
    let now = chrono::Utc::now().timestamp();
    match last {
        Some(t) => ((now - t) / 86400).max(0),
        None => 365 * 10, // very large
    }
}
```

In the stage's `execute` body:

```rust
for entry in &ctx.notes {
    let note_path = format!("{}/{}", entry.category, entry.filename);
    let days = days_since_last_hit(ctx.store.as_ref(), &note_path).await as f32;
    let old_conf = entry.confidence;
    let decayed = old_conf * (-days / 90.0).exp();
    let floor = severity_floor(entry.severity);
    let new_conf = decayed.max(floor);
    if (new_conf - old_conf).abs() > 0.02 {
        let mut full = ctx.indexer.read_note(&ctx.agent_id, &entry.category, &entry.filename).await?;
        full.confidence = new_conf;
        ctx.indexer.write_note(&ctx.agent_id, &entry.category, &full).await?;
    }
}
```

You will need a `recall_signals_last_hit` method on `NoteStore` if it does not yet exist:

```rust
async fn recall_signals_last_hit(&self, note_path: &str) -> Result<Option<i64>, AlephError> {
    let conn = lock_conn!(self)?;
    let v: Option<i64> = conn.query_row(
        "SELECT MAX(created_at) FROM recall_signals WHERE note_path = ?1",
        params![note_path],
        |r| r.get(0),
    ).optional().map_err(|e| AlephError::config(format!("recall last hit: {e}")))?;
    Ok(v)
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib memory::dreaming::stages::note_decay
```
Expected: green.

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/stages/note_decay.rs src/memory/notes/store.rs src/memory/store/sqlite/notes.rs
git commit -m "feat(decay): recall-signal-driven confidence decay with severity floor"
```

---

## Task 11 (C2.8): Ingest-time origin tagging

**Files:**
- Modify: `src/memory/notes/ingest/retrieve.rs` (annotate context blocks)
- Modify: `src/memory/notes/ingest/prompts.rs` (origin instruction)
- Modify: `src/memory/notes/ingest/apply.rs` (post-process patch)

- [ ] **Step 1: Add origin instruction to prompt**

In `src/memory/notes/ingest/prompts.rs`, append to the relevant prompt builder:

```rust
const ORIGIN_INSTRUCTION: &str = r#"
For every fact bullet, append an inline HTML comment exactly in this form:
<!-- src: <id>, origin: raw_source|prior_note|inferred, inferred: true|false -->

Selection rule:
- If the fact came entirely from a [RAW src=...] block, use origin: raw_source and inferred: false.
- If the fact came entirely from a [PRIOR_NOTE src=...] block, use origin: prior_note and inferred: false.
- If the fact synthesizes across multiple sources, use origin: inferred and inferred: true (omit src).
"#;
```

Use `ORIGIN_INSTRUCTION` in the prompt assembly.

- [ ] **Step 2: Annotate context blocks**

In `src/memory/notes/ingest/retrieve.rs`, prefix each retrieved excerpt:
- raw_memories rows → `[RAW src=raw/{id}] ...`
- prior notes → `[PRIOR_NOTE src=note/{path}] ...`

- [ ] **Step 3: Post-process patch**

In `src/memory/notes/ingest/apply.rs`, before `gate.evaluate`:

```rust
fn ensure_origin_marker(line: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"<!--\s*(?:src:[^,]+,\s*)?origin:\s*(?:raw_source|prior_note|inferred|legacy)\s*,\s*inferred:\s*(?:true|false)\s*-->").unwrap()
    });
    if RE.is_match(line) {
        line.to_string()
    } else {
        format!("{line} <!-- origin: inferred, inferred: true -->")
    }
}
```

For each fact line emitted by the LLM, run `ensure_origin_marker` and store the result back into `note.facts[i]`. Then re-parse provenance via `extract_provenance_markers` to populate `note.fact_provenance`.

- [ ] **Step 4: Test**

```rust
// in apply.rs mod tests
#[test]
fn ensure_origin_marker_idempotent_when_present() {
    let s = "- claim <!-- src: raw/x, origin: raw_source, inferred: false -->";
    assert_eq!(ensure_origin_marker(s), s);
}

#[test]
fn ensure_origin_marker_patches_missing_to_inferred() {
    let s = "- bare claim";
    assert_eq!(ensure_origin_marker(s), "- bare claim <!-- origin: inferred, inferred: true -->");
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p alephcore --lib memory::notes::ingest
```
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/ingest/retrieve.rs src/memory/notes/ingest/prompts.rs src/memory/notes/ingest/apply.rs
git commit -m "feat(ingest): origin tagging in prompts + lenient post-process patching"
```

---

## Task 12 (C2.8.2): Strict-mode origin filter in retrieval

**Files:**
- Modify: `src/memory/note_retrieval/hybrid.rs` (config field + filter)

- [ ] **Step 1: Write failing test**

```rust
#[tokio::test]
async fn strict_filter_excludes_prior_note_origin() {
    // Seed two notes: A with one prior_note fact, B with one raw_source fact.
    // Run hybrid_search with strict_origin_filter=true; expect only B in results.
}
```

- [ ] **Step 2: Add config field + filter SQL**

Locate the hybrid search config struct in `src/memory/note_retrieval/hybrid.rs`. Add:

```rust
pub struct HybridSearchConfig {
    // ...existing fields...
    pub strict_origin_filter: bool,
}
```

In the `hybrid_search_notes` query body, when `strict_origin_filter == true`, JOIN `notes_provenance` and filter:

```sql
LEFT JOIN notes_provenance prov ON prov.agent_id = ? AND prov.note_path = notes_index.path
WHERE NOT EXISTS (
    SELECT 1 FROM notes_provenance prov2
    WHERE prov2.agent_id = ? AND prov2.note_path = notes_index.path AND prov2.origin = 'prior_note'
)
```

(Adjust to the existing query shape — the actual JOIN may need to be in the score-aggregation stage.)

- [ ] **Step 3: Test, commit**

```bash
cargo test -p alephcore --lib memory::note_retrieval::hybrid
git add src/memory/note_retrieval/hybrid.rs
git commit -m "feat(retrieval): strict_origin_filter excludes prior_note facts when enabled"
```

---

## Task 13 (Phase C2 verification gate)

**Files:** none (verification only)

- [ ] **Step 1: Run all C2 unit tests**

```bash
cargo test -p alephcore --lib memory::notes::governance memory::dreaming::stages::note_review memory::dreaming::stages::note_decay memory::notes::note memory::notes::ingest memory::store::sqlite::schema memory::store::sqlite::notes memory::note_retrieval::hybrid
```
Expected: green.

- [ ] **Step 2: Run integration tests**

```bash
cargo test -p alephcore --test note_governance_gate
```
Expected: green.

- [ ] **Step 3: notes_provenance rebuild test**

```bash
cargo test -p alephcore --lib memory::notes::indexer::tests::full_rebuild_indexes_all_notes
```
Plus add an explicit test that `DELETE FROM notes_provenance` followed by `full_rebuild` recovers all rows (verify via `db.get_provenance` after rebuild).

- [ ] **Step 4: A and B regression check**

```bash
cargo test -p alephcore --lib memory::notes memory::dreaming
```
Expected: green; Phase A and Phase B tests still pass.

- [ ] **Step 5: Tag the phase**

```bash
git tag note-layer-phase-c2-complete
```

Phase C2 done. Phase R2 (rename `fact` → `note` in event sourcing layer) is the last phase.
