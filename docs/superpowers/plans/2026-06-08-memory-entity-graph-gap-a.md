# Memory Entity Graph (Gap A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the existing compound ingestor (one LLM call already made at compression time) additionally emit `entity/` notes and typed relationship edges, encoded in note frontmatter (source of truth) and mirrored into a new nullable `notes_links.relation` index column, with edge labels surfaced in `memory_explore`.

**Architecture:** Aleph-native — entities are ordinary markdown notes under a new `entity/` category; relationships are a `relations:` frontmatter list (`{to, type, confidence}`) parsed into the rebuildable `notes_links.relation` column. No new LLM calls, no resurrected triple-store tables. Everything additive and `#[serde(default)]`-gated → fully backward-compatible.

**Tech Stack:** Rust, rusqlite (SQLite + sqlite-vec), serde / serde_yaml, the existing `CompoundIngestor` + `NoteIndexer` + `RippleTask`.

---

## ⚠️ PROJECT PROTOCOL (OVERRIDES TDD "run the test" STEPS)

- **DO NOT run `cargo check` / `cargo test` / `cargo build` at any point.** The compiler is unavailable by mandate. Each task's verification is a **grep caller-verification guard** (the compiler substitute) plus reading the diff. Commit directly after the grep guard passes.
- **Worktree isolation:** all work happens in the worktree branch (created off this plan's commit on main), NEVER the main checkout.
- **Append-only main / explicit-path staging:** `git add <exact paths>` only — never `git add -A`/`.`. No `reset`/`amend`/`rebase`.
- **Backward-compatible / additive only.** Every new struct field is `#[serde(default)]`; every new frontmatter/JSON field is optional; the migration is idempotent.
- **Entropy reduction** only where you already touch code — do not delete pre-existing dead code.
- Unit tests are still written (they document intent and run later in CI), but you verify them by **reading**, not by executing `cargo`.

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/memory/notes/note/relation.rs` (NEW) | `Relation` struct + `default_relation_confidence` + `clamped()` | 1 |
| `src/memory/notes/note/mod.rs` | `mod relation`; `KnowledgeNote.relations` field; parse in `from_markdown`; render in `to_markdown` | 1 |
| `src/memory/notes/note/parsing.rs` | `Frontmatter.relations` field | 1 |
| `src/memory/notes/note/tests.rs` | round-trip + legacy + clamp tests | 1 |
| `src/memory/notes/ingest/plan.rs` | `PageOp::Create.relations` + `PageOp::Append.new_relations` | 2 |
| `src/memory/store/sqlite/schema/ddl.rs` | `relation TEXT` column in `NOTES_LINKS_DDL` | 3 |
| `src/memory/store/sqlite/schema/migrations.rs` | `migrate_notes_links_relation` (idempotent ADD COLUMN) | 3 |
| `src/memory/store/sqlite/schema/mod.rs` | call + re-export the migration | 3 |
| `src/memory/store/sqlite/notes.rs` | set-diff upsert carries `relation` | 3 |
| `src/memory/store/sqlite/schema/tests.rs` | migration idempotency test | 3 |
| `src/memory/notes/ingest/prompts.rs` | entity/relationship guidance in `PROMPT_COMPOUND_PLAN` | 4 |
| `src/memory/notes/ingest/snapshots/…compound_plan_base_prompt.snap` | mirror the prompt edit | 4 |
| `src/memory/notes/ingest/apply.rs` | `Create` writes `relations`; `Append` merges `new_relations` | 5 |
| `src/memory/notes/store.rs` | `NoteStore::get_typed_relations` trait method | 6 |
| `src/memory/store/sqlite/notes.rs` | impl `get_typed_relations` | 6 |
| `src/builtin_tools/memory_explore.rs` | `ExploredFact.relations` populated from typed edges | 6 |

**Task ordering & inter-task compile state:** Tasks are ordered 1→6. Because `cargo` is forbidden, the tree may be intentionally non-compiling *between* tasks (e.g. after Task 2 adds `PageOp` fields, `apply.rs`'s match arms are fixed in Task 5). This is acceptable. Each task's grep guard confirms its own surface is complete.

---

## Task 1: `Relation` type + `KnowledgeNote.relations` field (parse + render)

**Files:**
- Create: `src/memory/notes/note/relation.rs`
- Modify: `src/memory/notes/note/mod.rs` (add `mod relation`, `pub use`, struct field, Default, `from_markdown`, `to_markdown`)
- Modify: `src/memory/notes/note/parsing.rs` (`Frontmatter.relations`)
- Test: `src/memory/notes/note/tests.rs`

- [ ] **Step 1: Create the `Relation` type**

Create `src/memory/notes/note/relation.rs`:

```rust
//! Typed relation edges for entity-graph notes (Gap A).
//!
//! Encoded in note frontmatter under `relations:`; mirrored into the
//! rebuildable `notes_links.relation` index column. Markdown is the source of
//! truth — these structs are reconstructed from the `.md` file on every parse.

use serde::{Deserialize, Serialize};

/// A typed, directed edge from the containing note to `to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    /// Target note path ("entity/bob") or raw wikilink target.
    pub to: String,
    /// Free-form snake_case relationship verb chosen by the LLM (no fixed
    /// taxonomy — R7 LLM sovereignty). E.g. "works_at", "colleague".
    #[serde(rename = "type")]
    pub rel_type: String,
    /// LLM-judged edge confidence in [0,1]; defaults to 1.0 when absent.
    #[serde(default = "default_relation_confidence")]
    pub confidence: f32,
}

pub(crate) fn default_relation_confidence() -> f32 {
    1.0
}

impl Relation {
    /// Clamp `confidence` into `[0,1]` (P7 boundary hardening). Applied when a
    /// relation enters the system from markdown or from an ingest op.
    pub fn clamped(mut self) -> Self {
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self
    }
}
```

- [ ] **Step 2: Wire the module + field into `KnowledgeNote`**

In `src/memory/notes/note/mod.rs`:

After the existing `mod parsing;` line (~line 11) add:
```rust
mod relation;
```
After the existing `pub use types::{...};` (~line 17) add:
```rust
pub use relation::Relation;
```

In `struct KnowledgeNote` (~line 47, right after the `pub links: Vec<String>,` field) add:
```rust
    /// Typed relation edges from frontmatter `relations:` (Gap A entity graph).
    /// Empty for notes without a `relations:` block (legacy + non-entity notes).
    pub relations: Vec<Relation>,
```

In `impl Default for KnowledgeNote` (~line 90, right after `links: Vec::new(),`) add:
```rust
            relations: Vec::new(),
```

In `from_markdown` (~line 123, in the `Ok(Self { ... })` literal, right after `links,`) add:
```rust
            relations: frontmatter
                .relations
                .into_iter()
                .map(Relation::clamped)
                .collect(),
```

- [ ] **Step 3: Render `relations:` in `to_markdown`**

In `to_markdown` (`mod.rs`), immediately AFTER the `superseded_by:` block (after line ~192, before the `if self.permanent {` block) insert:

```rust
        // Only emit `relations:` when non-empty, so notes without typed edges
        // serialize byte-for-byte as before this field existed (legacy parity).
        if !self.relations.is_empty() {
            out.push_str("relations:\n");
            for r in &self.relations {
                out.push_str(&format!("  - to: {}\n", r.to));
                out.push_str(&format!("    type: {}\n", r.rel_type));
                out.push_str(&format!("    confidence: {:.4}\n", r.confidence));
            }
        }
```

- [ ] **Step 4: Parse `relations:` in `Frontmatter`**

In `src/memory/notes/note/parsing.rs`, inside `struct Frontmatter` (after the `permanent` field, ~line 80) add:

```rust
    /// Typed relation edges (Gap A). Absent in legacy notes → empty.
    #[serde(default)]
    pub(super) relations: Vec<super::relation::Relation>,
```

- [ ] **Step 5: Write the unit tests**

In `src/memory/notes/note/tests.rs`, add (use the module's existing `use super::*;` — add `use super::super::Relation;` if `Relation` is not already in scope; check the file's existing imports first):

```rust
#[test]
fn relations_roundtrip_through_markdown() {
    let note = KnowledgeNote {
        title: "alice".to_string(),
        category: "entity".to_string(),
        relations: vec![
            Relation { to: "entity/acme-corp".to_string(), rel_type: "works_at".to_string(), confidence: 0.9 },
            Relation { to: "entity/bob".to_string(), rel_type: "colleague".to_string(), confidence: 0.7 },
        ],
        ..Default::default()
    };
    let md = note.to_markdown();
    assert!(md.contains("relations:"));
    assert!(md.contains("type: works_at"));
    let parsed = KnowledgeNote::from_markdown("alice", &md).unwrap();
    assert_eq!(parsed.relations, note.relations);
}

#[test]
fn legacy_note_without_relations_omits_block() {
    let note = KnowledgeNote {
        title: "x".to_string(),
        category: "learning".to_string(),
        ..Default::default()
    };
    let md = note.to_markdown();
    assert!(!md.contains("relations:"), "no relations block when empty");
    let parsed = KnowledgeNote::from_markdown("x", &md).unwrap();
    assert!(parsed.relations.is_empty());
}

#[test]
fn relation_confidence_is_clamped_on_parse() {
    let md = "---\ncategory: entity\nrelations:\n  - to: entity/bob\n    type: knows\n    confidence: 1.5\n---\n\n- hi\n";
    let parsed = KnowledgeNote::from_markdown("a", md).unwrap();
    assert_eq!(parsed.relations.len(), 1);
    assert_eq!(parsed.relations[0].confidence, 1.0);
    assert_eq!(parsed.relations[0].rel_type, "knows");
}

#[test]
fn relation_confidence_defaults_to_one_when_absent() {
    let md = "---\ncategory: entity\nrelations:\n  - to: entity/bob\n    type: knows\n---\n\n- hi\n";
    let parsed = KnowledgeNote::from_markdown("a", md).unwrap();
    assert_eq!(parsed.relations[0].confidence, 1.0);
}
```

- [ ] **Step 6: Grep guard (compiler substitute) — verify no `KnowledgeNote` literal breaks**

Adding a field breaks any `KnowledgeNote { … }` literal that does NOT end with `..Default::default()`. List every literal and confirm each spreads Default:

Run:
```bash
grep -rn "KnowledgeNote {" src/ | grep -v "pub struct\|struct KnowledgeNote"
```
For EACH hit, open the file and confirm the literal contains `..Default::default()` (the existing ones at `apply.rs:113`, `apply.rs:226` do). If any literal does NOT spread Default, add `relations: Vec::new(),` to it.

Also confirm the new module is wired:
```bash
grep -n "mod relation;" src/memory/notes/note/mod.rs        # expect 1
grep -n "pub use relation::Relation;" src/memory/notes/note/mod.rs  # expect 1
grep -rn "relations" src/memory/notes/note/parsing.rs       # expect the Frontmatter field
```
Expected: every `KnowledgeNote {` literal spreads Default; module + re-export present.

- [ ] **Step 7: Commit**

```bash
git add src/memory/notes/note/relation.rs src/memory/notes/note/mod.rs src/memory/notes/note/parsing.rs src/memory/notes/note/tests.rs
git commit -m "feat(memory): Relation type + KnowledgeNote.relations frontmatter parse/render (Gap A)"
```

---

## Task 2: `PageOp::Create.relations` + `PageOp::Append.new_relations`

**Files:**
- Modify: `src/memory/notes/ingest/plan.rs` (enum fields)
- Modify (mechanical, grep-driven): every `PageOp::Create { … }` / `PageOp::Append { … }` **construction literal** and every **full destructure** (those without a trailing `..`).
- Test: `src/memory/notes/ingest/plan.rs` `#[cfg(test)]`

- [ ] **Step 1: Add the enum fields**

In `src/memory/notes/ingest/plan.rs`, at the top of the file add to the imports:
```rust
use crate::memory::notes::note::Relation;
```
In `enum PageOp` (~line 19), add to the `Create` variant (after `tags`):
```rust
        #[serde(default)]
        relations: Vec<Relation>,
```
Add to the `Append` variant (after `new_links`):
```rust
        #[serde(default)]
        new_relations: Vec<Relation>,
```

- [ ] **Step 2: Grep guard — enumerate every construction + full-destructure site**

Run:
```bash
grep -rn "PageOp::Create {\|PageOp::Append {" src/
```
Classify each hit:
- **Destructure ending in `, .. }`** → SAFE, no change (e.g. `plan.rs:65`, `ingestor.rs:582,695,815`, `apply.rs:627`, `ref_table.rs:151`).
- **Construction literal** (`PageOp::Create { note_path: …, … }` building a value, typically inside `vec![ … ]` or `tx.stage(&PageOp::Create { … })`) → add `relations: vec![],` (Create) / `new_relations: vec![],` (Append).
- **Full destructure WITHOUT `..`** (binds every field in a `match`/`if let`) → append `, ..` so it tolerates the new field, OR (only in `apply.rs`, handled in Task 5) bind the field.

Known construction-literal sites to update (verify against the live grep — line numbers drift):
`ref_table.rs:241, 263, 292, 302, 319, 334`; `plan.rs:125, 133, 174`; `ingestor.rs:556, 614, 626, 1065, 1663, 1674, 1719, 1770`; `apply.rs:472, 496, 529, 550, 563, 587, 596`.

> NOTE: `apply.rs:101` (Create) and `apply.rs:133` (Append) are the main `stage()` match arms — **leave their bodies for Task 5**, but in THIS task append `, ..` to their patterns so the file is at least pattern-complete. Task 5 replaces the `..` with real bindings.

For each construction literal, the mechanical edit is e.g.:
```rust
// before
PageOp::Create { note_path: "...".into(), title: "...".into(), summary: "...".into(), facts: vec![...], links: vec![...], tags: vec![...] }
// after — add one field
PageOp::Create { note_path: "...".into(), title: "...".into(), summary: "...".into(), facts: vec![...], links: vec![...], tags: vec![...], relations: vec![] }
```

- [ ] **Step 3: Add a serde round-trip test**

In `plan.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn create_op_parses_relations_and_defaults_when_absent() {
    // With relations
    let j = r#"{"kind":"create","note_path":"entity/alice","title":"Alice","summary":"","facts":[],"links":[],"tags":[],"relations":[{"to":"entity/acme","type":"works_at","confidence":0.9}]}"#;
    let op: PageOp = serde_json::from_str(j).unwrap();
    match op {
        PageOp::Create { relations, .. } => {
            assert_eq!(relations.len(), 1);
            assert_eq!(relations[0].rel_type, "works_at");
        }
        _ => panic!("expected create"),
    }
    // Without relations → default empty (backward compat)
    let j2 = r#"{"kind":"create","note_path":"learning/x","title":"X","summary":"","facts":[],"links":[],"tags":[]}"#;
    let op2: PageOp = serde_json::from_str(j2).unwrap();
    match op2 {
        PageOp::Create { relations, .. } => assert!(relations.is_empty()),
        _ => panic!("expected create"),
    }
}

#[test]
fn append_op_parses_new_relations() {
    let j = r#"{"kind":"append","note_path":"entity/alice","new_facts":[],"new_links":[],"new_relations":[{"to":"entity/bob","type":"colleague"}]}"#;
    let op: PageOp = serde_json::from_str(j).unwrap();
    match op {
        PageOp::Append { new_relations, .. } => {
            assert_eq!(new_relations.len(), 1);
            assert_eq!(new_relations[0].confidence, 1.0); // serde default
        }
        _ => panic!("expected append"),
    }
}
```

- [ ] **Step 4: Grep guard**

Run:
```bash
grep -rn "PageOp::Create {\|PageOp::Append {" src/ | grep -v ", \.\. }" | grep -v "relations"
```
Expected: the only remaining hits are `apply.rs:101`/`:133` (their patterns now end in `, ..` from Step 2; bodies handled in Task 5) and the `plan.rs` enum definition itself. Every construction literal now contains `relations`/`new_relations`. If a construction literal still lacks it, fix it.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/ingest/plan.rs src/memory/notes/ingest/ref_table.rs src/memory/notes/ingest/ingestor.rs src/memory/notes/ingest/apply.rs
git commit -m "feat(memory): PageOp Create.relations + Append.new_relations (serde default, Gap A)"
```

---

## Task 3: `notes_links.relation` column + idempotent migration + set-diff upsert

**Files:**
- Modify: `src/memory/store/sqlite/schema/ddl.rs` (DDL)
- Modify: `src/memory/store/sqlite/schema/migrations.rs` (migration)
- Modify: `src/memory/store/sqlite/schema/mod.rs` (call + re-export)
- Modify: `src/memory/store/sqlite/notes.rs` (upsert)
- Test: `src/memory/store/sqlite/schema/tests.rs`

- [ ] **Step 1: Add the column to fresh-DB DDL**

In `src/memory/store/sqlite/schema/ddl.rs`, change `NOTES_LINKS_DDL` (line 102) — add a nullable `relation` column after `to_raw`:

```rust
pub const NOTES_LINKS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    to_raw      TEXT NOT NULL,
    relation    TEXT,
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(agent_id, to_note);
"#;
```

- [ ] **Step 2: Add the idempotent migration**

In `src/memory/store/sqlite/schema/migrations.rs`, after `migrate_notes_links_to_raw` (ends line 138) add — mirroring that function exactly:

```rust
/// Add the nullable `relation` column to existing `notes_links` rows.
///
/// Pre-existing edges keep `relation = NULL` (untyped body wikilinks). Typed
/// frontmatter relations populate it on the next reindex. Idempotent:
/// re-running on a migrated table is a no-op (checks column existence first).
pub fn migrate_notes_links_relation(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let has_col: bool = conn
        .prepare("PRAGMA table_info(notes_links)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "relation");
    if has_col {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE notes_links ADD COLUMN relation TEXT;")?;
    Ok(())
}
```

- [ ] **Step 3: Call + re-export the migration**

In `src/memory/store/sqlite/schema/mod.rs`:
- After the `migrations::migrate_notes_links_to_raw(conn)` call (line 82) add a new call. Read lines 80-85 first to match the surrounding `?`/return style; the existing line is `migrations::migrate_notes_links_to_raw(conn)` possibly as the trailing expression. Insert BEFORE it so both run, e.g.:
```rust
    migrations::migrate_notes_links_to_raw(conn)?;
    migrations::migrate_notes_links_relation(conn)
        .map_err(|e| AlephError::config(format!("migrate notes_links.relation: {e}")))?;
```
  (If `migrate_notes_links_to_raw(conn)` was the final expression without `?`, convert it to a `?` statement and make the new migration the final expression, or add `Ok(())` — preserve the function's return type. Read the function body before editing.)
- At line 140, extend the re-export:
```rust
pub use migrations::{drop_obsolete_facts_tables, migrate_notes_links_relation, migrate_notes_links_to_raw};
```

- [ ] **Step 4: Rework the set-diff upsert to carry `relation`**

In `src/memory/store/sqlite/notes.rs`, replace the block from line 131 (`// Build the new (to_raw, to_note) pair set…`) through line 198 (end of the INSERT loop) with the version below. It (a) factors resolution into a closure, (b) builds a `to_note → (to_raw, relation)` map from body wikilinks (relation `None`) **and** frontmatter relations (relation `Some(type)`, typed wins), (c) deletes vanished targets, (d) upserts new/changed targets via `ON CONFLICT … DO UPDATE`, preserving the unchanged-row write-skip.

```rust
        // Desired outgoing edges: to_note -> (to_raw, relation).
        // Body wikilinks contribute relation = None; frontmatter `relations:`
        // contribute relation = Some(type). When both target the same to_note,
        // the typed relation wins. Keyed by to_note to match the table's
        // UNIQUE(agent_id, from_note, to_note).
        let resolve_target = |raw_target: &str| -> Result<String, AlephError> {
            if raw_target.contains('/') {
                return Ok(raw_target.to_string());
            }
            let mut stmt = conn
                .prepare(
                    "SELECT path FROM notes_index WHERE agent_id = ?1 AND filename = ?2 LIMIT 2",
                )
                .map_err(|e| AlephError::config(format!("resolve filename prep: {e}")))?;
            let paths: Vec<String> = stmt
                .query_map(params![agent_id, raw_target], |r| r.get::<_, String>(0))
                .map_err(|e| AlephError::config(format!("resolve filename query: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(if paths.len() == 1 {
                paths[0].clone()
            } else {
                raw_target.to_string()
            })
        };

        let mut desired: HashMap<String, (String, Option<String>)> = HashMap::new();
        for raw_target in &note.links {
            let resolved = resolve_target(raw_target)?;
            desired
                .entry(resolved)
                .or_insert_with(|| (raw_target.clone(), None));
        }
        for rel in &note.relations {
            let resolved = resolve_target(&rel.to)?;
            // Typed relation overrides a plain wikilink to the same target.
            desired.insert(resolved, (rel.to.clone(), Some(rel.rel_type.clone())));
        }

        // Existing edges for this from_note: to_note -> (to_raw, relation).
        let existing: HashMap<String, (String, Option<String>)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT to_note, to_raw, relation FROM notes_links \
                     WHERE agent_id = ?1 AND from_note = ?2",
                )
                .map_err(|e| AlephError::config(format!("index_note links scan prep: {e}")))?;
            let rows = stmt
                .query_map(params![agent_id, path], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|e| AlephError::config(format!("index_note links scan: {e}")))?;
            rows.filter_map(|r| r.ok())
                .map(|(to_note, to_raw, relation)| (to_note, (to_raw, relation)))
                .collect()
        };

        // DELETE targets no longer desired.
        for to_note in existing.keys() {
            if !desired.contains_key(to_note) {
                conn.execute(
                    "DELETE FROM notes_links \
                     WHERE agent_id = ?1 AND from_note = ?2 AND to_note = ?3",
                    params![agent_id, path, to_note],
                )
                .map_err(|e| AlephError::config(format!("index_note links delete: {e}")))?;
            }
        }

        // UPSERT new or changed targets; skip unchanged rows (no write storm).
        for (to_note, (to_raw, relation)) in &desired {
            let unchanged = existing
                .get(to_note)
                .map(|(er, erel)| er == to_raw && erel == relation)
                .unwrap_or(false);
            if unchanged {
                continue;
            }
            conn.execute(
                "INSERT INTO notes_links (agent_id, from_note, to_note, to_raw, relation) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(agent_id, from_note, to_note) \
                 DO UPDATE SET to_raw = excluded.to_raw, relation = excluded.relation",
                params![agent_id, path, to_note, to_raw, relation],
            )
            .map_err(|e| AlephError::config(format!("index_note links upsert: {e}")))?;
        }
```

> The surrounding code already has `use std::collections::HashSet;` — add `use std::collections::HashMap;` to the file's imports if not already present (grep first). `HashSet` may now be unused; if so, remove the `HashSet` import (entropy reduction — only because this edit orphaned it).

- [ ] **Step 5: Migration idempotency test**

In `src/memory/store/sqlite/schema/tests.rs`, mirror the existing `migrate_notes_links_to_raw` test (~line 402). Add:

```rust
#[test]
fn migrate_notes_links_relation_is_idempotent() {
    use super::migrate_notes_links_relation;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Create the pre-relation table shape (no relation column).
    conn.execute_batch(
        "CREATE TABLE notes_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL DEFAULT 'default',
            from_note TEXT NOT NULL,
            to_note TEXT NOT NULL,
            to_raw TEXT NOT NULL,
            UNIQUE(agent_id, from_note, to_note)
        );",
    )
    .unwrap();
    migrate_notes_links_relation(&conn).expect("first migration");
    migrate_notes_links_relation(&conn).expect("second migration is a no-op");
    // Column now exists exactly once.
    let count: i64 = conn
        .prepare("PRAGMA table_info(notes_links)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .filter(|n| n == "relation")
        .count() as i64;
    assert_eq!(count, 1);
}
```

Also add `migrate_notes_links_relation` to the test module's `use super::{…}` import line (the existing one at `tests.rs:4`).

- [ ] **Step 6: Upsert behaviour test**

In `src/memory/store/sqlite/notes.rs` `#[cfg(test)]` (mirror `stores_and_queries_links` at line 384), add a test that indexes a note with one body wikilink + one frontmatter relation and asserts the `relation` column:

```rust
#[tokio::test]
async fn index_note_writes_typed_relation_column() {
    use crate::memory::notes::note::{KnowledgeNote, Relation};
    const AGENT: &str = "main";
    let backend = test_backend().await; // use whatever helper the sibling tests use
    // Seed the target so resolution succeeds.
    let target = KnowledgeNote { title: "bob".into(), category: "entity".into(), ..Default::default() };
    backend.index_note("entity/bob", "bob", AGENT, &target).await.unwrap();

    let mut alice = KnowledgeNote { title: "alice".into(), category: "entity".into(), ..Default::default() };
    alice.links = vec!["entity/bob".into()];                       // plain wikilink
    alice.relations = vec![Relation { to: "entity/bob".into(), rel_type: "colleague".into(), confidence: 0.7 }];
    backend.index_note("entity/alice", "alice", AGENT, &alice).await.unwrap();

    // The typed relation wins for the (alice -> bob) edge.
    let rel: Option<String> = backend
        .with_conn(|c| {
            c.query_row(
                "SELECT relation FROM notes_links WHERE agent_id=?1 AND from_note=?2 AND to_note=?3",
                rusqlite::params![AGENT, "entity/alice", "entity/bob"],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
        })
        .await
        .unwrap();
    assert_eq!(rel.as_deref(), Some("colleague"));
}
```

> Match the exact signatures of `index_note` and the test-backend/connection helper used by the neighbouring tests in this file (read `stores_and_queries_links` and its setup first; `index_note`'s real argument order/types must be copied verbatim). If `with_conn`/`optional` helpers differ, use whatever the sibling tests use to read a row.

- [ ] **Step 7: Grep guard**

```bash
grep -n "relation" src/memory/store/sqlite/schema/ddl.rs            # column in DDL
grep -n "migrate_notes_links_relation" src/memory/store/sqlite/schema/migrations.rs src/memory/store/sqlite/schema/mod.rs   # defined + called + re-exported (>=3 hits)
grep -n "ON CONFLICT(agent_id, from_note, to_note)" src/memory/store/sqlite/notes.rs   # upsert present
grep -n "HashMap" src/memory/store/sqlite/notes.rs                  # import present
```
Expected: DDL has `relation TEXT`; migration defined, called in `mod.rs`, re-exported; upsert uses `ON CONFLICT … DO UPDATE`; `HashMap` imported. Confirm no remaining reference to the deleted `new_pairs`/`HashSet`-based block.

- [ ] **Step 8: Commit**

```bash
git add src/memory/store/sqlite/schema/ddl.rs src/memory/store/sqlite/schema/migrations.rs src/memory/store/sqlite/schema/mod.rs src/memory/store/sqlite/notes.rs src/memory/store/sqlite/schema/tests.rs
git commit -m "feat(memory): notes_links.relation column + idempotent migration + typed set-diff upsert (Gap A)"
```

---

## Task 4: Ingestor prompt — entity & relationship guidance

**Files:**
- Modify: `src/memory/notes/ingest/prompts.rs` (`PROMPT_COMPOUND_PLAN`)
- Modify: `src/memory/notes/ingest/snapshots/alephcore__memory__notes__ingest__prompts__tests__compound_plan_base_prompt.snap` (mirror the edit)

- [ ] **Step 1: Add the guidance block to the base prompt**

In `src/memory/notes/ingest/prompts.rs`, inside the `PROMPT_COMPOUND_PLAN` raw-string const, insert a new section **immediately before the `## Output` heading** (the `## Output` line near the tail, ~line 67). Insert:

```text
## Entities & relationships

When the source names durable entities (people, organisations, projects,
concepts), create or append `entity/<slug>` notes for them (category `entity`).
Express relationships BETWEEN entities with the op's `relations` field — a list
of `{ "to": "<entity path or [P<n>] token>", "type": "<snake_case verb>",
"confidence": <0..1> }`. Choose a concise `type` yourself (e.g. `works_at`,
`depends_on`, `colleague`, `part_of`); there is no fixed vocabulary. Reuse
existing entity notes shown in "Related existing pages" — never duplicate an
entity that already exists; append new relations to it instead.

```

(Keep one blank line after the block so it reads cleanly before `## Output`.)

- [ ] **Step 2: Mirror the edit into the insta snapshot**

The const is snapshot-tested (`prompts.rs:118`, `insta::assert_snapshot!("compound_plan_base_prompt", PROMPT_COMPOUND_PLAN)`). Because `cargo`/`cargo insta` cannot run, update the snapshot file by hand so it stays in sync:

Open `src/memory/notes/ingest/snapshots/alephcore__memory__notes__ingest__prompts__tests__compound_plan_base_prompt.snap` and insert the **identical** "## Entities & relationships" block at the **same position** (immediately before `## Output`) as in the const. The snapshot body is a verbatim copy of the const string, so the inserted text must match byte-for-byte (same wording, same blank lines).

> The other two prompt tests are unaffected: `prompts.rs:132` checks each op-`kind` token still appears (we added none); `prompts.rs:149` asserts `build_compound_system_prompt(<no-source>) == PROMPT_COMPOUND_PLAN`, which stays true because the guidance lives in the const itself, not in the builder.

- [ ] **Step 3: Grep guard**

```bash
grep -n "## Entities & relationships" src/memory/notes/ingest/prompts.rs   # expect 1 (in the const)
grep -n "## Entities & relationships" src/memory/notes/ingest/snapshots/alephcore__memory__notes__ingest__prompts__tests__compound_plan_base_prompt.snap  # expect 1
grep -c "## Output" src/memory/notes/ingest/prompts.rs                     # unchanged structure
```
Expected: the block exists once in both the const and the snapshot, before `## Output`.

- [ ] **Step 4: Commit**

```bash
git add src/memory/notes/ingest/prompts.rs src/memory/notes/ingest/snapshots/alephcore__memory__notes__ingest__prompts__tests__compound_plan_base_prompt.snap
git commit -m "feat(memory): ingestor prompt — entity notes + typed relations guidance (Gap A)"
```

---

## Task 5: Apply ops — write entity relations into the note

**Files:**
- Modify: `src/memory/notes/ingest/apply.rs` (`Create` + `Append` arms in `stage()`)
- Test: `src/memory/notes/ingest/apply.rs` `#[cfg(test)]`

- [ ] **Step 1: Bind + write `relations` in the `Create` arm**

In `apply.rs` `stage()`, the `PageOp::Create` arm (line ~101). In Task 2 you appended `, ..` to its pattern; now bind `relations` instead and write it into the `KnowledgeNote`.

Change the pattern to add `relations` (keep all existing bindings):
```rust
            PageOp::Create {
                note_path,
                title,
                summary,
                facts,
                links,
                tags,
                relations,
            } => {
```
In the `KnowledgeNote { … }` literal it builds (~line 113), add after `links: links.clone(),`:
```rust
                    relations: relations.iter().cloned().map(Relation::clamped).collect(),
```
Add the import at the top of `apply.rs` if not present:
```rust
use crate::memory::notes::note::Relation;
```
(`KnowledgeNote` is already imported in this file.)

- [ ] **Step 2: Merge `new_relations` in the `Append` arm**

In the `PageOp::Append` arm (line ~133), change the pattern to bind `new_relations` (replacing the `, ..` added in Task 2):
```rust
            PageOp::Append {
                note_path,
                new_facts,
                new_links,
                new_relations,
            } => {
```
After the existing `for l in new_links { … }` loop (~line 148-152), add a merge loop that upserts by `to` (a re-stated relation updates type/confidence; a new one is appended):
```rust
                for r in new_relations {
                    let r = r.clone().clamped();
                    if let Some(existing) = merged.relations.iter_mut().find(|e| e.to == r.to) {
                        *existing = r;
                    } else {
                        merged.relations.push(r);
                    }
                }
```

- [ ] **Step 3: Tests**

In `apply.rs` `#[cfg(test)]`, mirror the existing `create_op_writes_file_and_indexes` test (line ~469). Add:

```rust
#[tokio::test]
async fn create_op_persists_frontmatter_relations() {
    let (mut tx, _dir) = new_test_tx().await; // use the file's existing tx/dir setup helper
    tx.stage(&PageOp::Create {
        note_path: "entity/alice".to_string(),
        title: "Alice".to_string(),
        summary: String::new(),
        facts: vec![],
        links: vec![],
        tags: vec!["person".to_string()],
        relations: vec![Relation { to: "entity/acme".to_string(), rel_type: "works_at".to_string(), confidence: 0.9 }],
    })
    .await
    .unwrap();
    // The staged note carries the relation; its markdown contains the block.
    let staged = tx.staged_markdown("entity", "alice"); // use the file's staging accessor
    assert!(staged.contains("relations:"));
    assert!(staged.contains("type: works_at"));
}

#[tokio::test]
async fn append_op_merges_new_relations_by_target() {
    let (mut tx, _dir) = new_test_tx().await;
    // First create with one relation.
    tx.stage(&PageOp::Create {
        note_path: "entity/alice".to_string(),
        title: "Alice".to_string(),
        summary: String::new(),
        facts: vec![],
        links: vec![],
        tags: vec![],
        relations: vec![Relation { to: "entity/bob".to_string(), rel_type: "knows".to_string(), confidence: 0.5 }],
    }).await.unwrap();
    // Append updates the same target's type and adds a new one.
    tx.stage(&PageOp::Append {
        note_path: "entity/alice".to_string(),
        new_facts: vec![],
        new_links: vec![],
        new_relations: vec![
            Relation { to: "entity/bob".to_string(), rel_type: "colleague".to_string(), confidence: 0.8 },
            Relation { to: "entity/acme".to_string(), rel_type: "works_at".to_string(), confidence: 0.9 },
        ],
    }).await.unwrap();
    let staged = tx.staged_markdown("entity", "alice");
    assert!(staged.contains("type: colleague")); // bob upgraded
    assert!(!staged.contains("type: knows"));     // old type replaced
    assert!(staged.contains("to: entity/acme"));  // new edge added
}
```

> Match the real test scaffolding in this file: read `create_op_writes_file_and_indexes` (line ~469) to copy its transaction/dir setup and the accessor it uses to read staged note content. Replace `new_test_tx()` / `staged_markdown(...)` with the actual helpers (e.g. it may read the file from `_dir` on disk after a flush, or inspect `tx.staged`). Keep the assertions.

- [ ] **Step 4: Grep guard**

```bash
grep -n "relations" src/memory/notes/ingest/apply.rs   # Create binds+writes relations; Append merges new_relations
grep -n "Relation::clamped" src/memory/notes/ingest/apply.rs   # clamp applied at the op boundary (>=1)
```
Confirm the `Create`/`Append` arms no longer end in bare `, ..` (they now bind the new field) and that `relations` flows into the staged `KnowledgeNote`.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/ingest/apply.rs
git commit -m "feat(memory): apply Create.relations + Append.new_relations into entity notes (Gap A)"
```

---

## Task 6: `memory_explore` edge labels

**Files:**
- Modify: `src/memory/notes/store.rs` (`NoteStore::get_typed_relations` trait method)
- Modify: `src/memory/store/sqlite/notes.rs` (impl)
- Modify: `src/builtin_tools/memory_explore.rs` (`ExploredFact.relations` + populate)
- Test: `src/builtin_tools/memory_explore.rs` `#[cfg(test)]`

- [ ] **Step 1: Add the trait method**

In `src/memory/notes/store.rs`, after `get_outgoing_links` (the trait declaration at ~line 87) add:

```rust
    /// Outgoing **typed** edges for a note: `(to_note, relation_type)` for every
    /// row whose `relation` column is non-NULL. Untyped body wikilinks are
    /// excluded. Used to surface entity-graph edge labels (Gap A).
    async fn get_typed_relations(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<(String, String)>, AlephError>;
```

- [ ] **Step 2: Implement it**

In `src/memory/store/sqlite/notes.rs`, after the `get_outgoing_links` impl (~line 401) add (mirroring its shape):

```rust
    async fn get_typed_relations(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<(String, String)>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT to_note, relation FROM notes_links \
                 WHERE from_note = ?1 AND agent_id = ?2 AND relation IS NOT NULL",
            )
            .map_err(|e| AlephError::config(format!("get_typed_relations prepare: {e}")))?;
        let rows = stmt
            .query_map(params![path, agent_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| AlephError::config(format!("get_typed_relations query: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AlephError::config(format!("get_typed_relations row: {e}")))?);
        }
        Ok(out)
    }
```

> Grep guard before moving on: `grep -rn "impl NoteStore for\|impl.*NoteStore" src/` — if there is more than one `NoteStore` implementer (e.g. an in-memory test double), add `get_typed_relations` to each, returning `Ok(vec![])` for non-SQLite stubs. A trait method without a default breaks every impl.

- [ ] **Step 3: Add `relations` to `ExploredFact` and populate it**

In `src/builtin_tools/memory_explore.rs`:

Add to `struct ExploredFact` (line ~51), after `relevance_score`:
```rust
    /// Typed outgoing entity-graph edges, formatted "type→to_note" (Gap A).
    /// Empty for notes with no typed relations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<String>,
```

Add a `NoteStore` import near the top (already present: `use crate::memory::notes::store::NoteStore;`).

Add a helper on `MemoryExploreTool` (after `call_impl`):
```rust
    /// Look up typed outgoing edges for a note path and format them as
    /// "type→to_note" labels. Best-effort: a lookup error yields no labels.
    async fn edge_labels(&self, note_path: &str) -> Vec<String> {
        match self.database.get_typed_relations(note_path, &self.agent_id).await {
            Ok(edges) => edges.into_iter().map(|(to, ty)| format!("{ty}→{to}")).collect(),
            Err(e) => {
                debug!(note_path = %note_path, error = %e, "edge_labels lookup failed");
                Vec::new()
            }
        }
    }
```

In `call_impl`, when building `seed_output` (line ~173) and `expanded_output` (line ~198), the `.map(|f| ExploredFact { … })` closures cannot be `async`. Replace each with a sequential loop that awaits `edge_labels`. For seeds:
```rust
        let mut seed_output: Vec<ExploredFact> = Vec::with_capacity(seed_facts.len());
        for f in &seed_facts {
            let relations = self.edge_labels(&f.id).await;
            seed_output.push(ExploredFact {
                id: f.id.clone(),
                content: f.content.clone(),
                path: f.path.clone(),
                relevance_score: f.similarity_score.unwrap_or(0.0),
                relations,
            });
        }
```
For expanded (after `ripple.explore`):
```rust
        let mut expanded_output: Vec<ExploredFact> = Vec::with_capacity(result.expanded_facts.len());
        for f in &result.expanded_facts {
            let relations = self.edge_labels(&f.id).await;
            expanded_output.push(ExploredFact {
                id: f.id.clone(),
                content: f.content.clone(),
                path: f.path.clone(),
                relevance_score: f.similarity_score.unwrap_or(0.0),
                relations,
            });
        }
```
> For note-backed facts, `f.id` is the note path (the seeds come from `vector_search_notes_with_content` → `to_memory_fact`, whose `id` is the note path used by `notes_links.from_note`). If a fact's `id` is not a note path, `get_typed_relations` simply returns empty — safe.

Update the existing `ExploredFact { … }` literals in this file's `#[cfg(test)]` (`test_explored_fact_serialization`, line ~293) to add `relations: vec![]`.

- [ ] **Step 4: Test**

Add to the `#[cfg(test)] mod tests` in `memory_explore.rs`:

```rust
#[test]
fn explored_fact_relations_default_empty_and_skipped_in_json() {
    let fact = ExploredFact {
        id: "entity/alice".to_string(),
        content: "Alice".to_string(),
        path: "entity/alice".to_string(),
        relevance_score: 0.9,
        relations: vec![],
    };
    let json = serde_json::to_string(&fact).unwrap();
    // Empty relations are skipped (clean output for the LLM).
    assert!(!json.contains("relations"));

    let fact2 = ExploredFact {
        id: "entity/alice".to_string(),
        content: "Alice".to_string(),
        path: "entity/alice".to_string(),
        relevance_score: 0.9,
        relations: vec!["works_at→entity/acme".to_string()],
    };
    let json2 = serde_json::to_string(&fact2).unwrap();
    assert!(json2.contains("works_at→entity/acme"));
}
```

- [ ] **Step 5: Grep guard**

```bash
grep -rn "fn get_typed_relations" src/                      # trait decl + every impl (>=2)
grep -n "edge_labels" src/builtin_tools/memory_explore.rs   # helper + 2 call sites (>=3)
grep -n "ExploredFact {" src/builtin_tools/memory_explore.rs # every literal has `relations`
```
Expected: `get_typed_relations` declared once and implemented in every `NoteStore` impl; `edge_labels` defined and used for seeds + expanded; every `ExploredFact` literal includes `relations`.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/store.rs src/memory/store/sqlite/notes.rs src/builtin_tools/memory_explore.rs
git commit -m "feat(memory): surface typed entity-graph edge labels in memory_explore (Gap A)"
```

---

## Final verification (whole branch, no cargo)

After all six tasks, run the cross-cutting grep guards:

```bash
# 1. No PageOp construction literal missing the new fields:
grep -rn "PageOp::Create {\|PageOp::Append {" src/ | grep -v ", \.\. }" | grep -vE "relations|enum PageOp"
#    → expect ZERO lines (every literal carries relations/new_relations).

# 2. Relation type reachable from every consumer:
grep -rn "use crate::memory::notes::note::Relation" src/   # plan.rs, apply.rs at least

# 3. notes_links.relation end-to-end:
grep -n "relation TEXT" src/memory/store/sqlite/schema/ddl.rs
grep -n "migrate_notes_links_relation" src/memory/store/sqlite/schema/mod.rs
grep -n "ON CONFLICT(agent_id, from_note, to_note)" src/memory/store/sqlite/notes.rs
grep -n "get_typed_relations" src/memory/notes/store.rs

# 4. Prompt + snapshot in sync:
grep -c "## Entities & relationships" src/memory/notes/ingest/prompts.rs src/memory/notes/ingest/snapshots/*compound_plan_base_prompt.snap
#    → 1 in each.
```

Then hand off to **superpowers:finishing-a-development-branch** (merge `--no-ff` to main per project protocol; the worktree cleanup happens in a fresh session per the CLAUDE.md `git worktree remove` hazard).

---

## Self-Review (plan author, against the spec)

**Spec coverage:**
- §3 data model (Relation + frontmatter) → Task 1 ✓
- §4 extraction (PageOp fields + prompt) → Tasks 2, 4 ✓
- §5 storage (KnowledgeNote field, migration, upsert) → Tasks 1, 3 ✓
- §6 retrieval (entity notes auto-retrievable [no code], explore edge labels) → Task 6 ✓
- §7 error handling (clamp, warn-not-fail, unresolved fallback) → clamp in Tasks 1/5; unresolved fallback inherited from existing resolver in Task 3 ✓
- §8 testing → tests in Tasks 1, 2, 3, 5, 6 ✓
- §9/§10 backward-compat (serde default, nullable column, omit-when-empty render) → Tasks 1, 2, 3 ✓

**Type consistency:** `Relation { to, rel_type (serde "type"), confidence }` used identically in Tasks 1, 2, 5; `get_typed_relations(path, agent_id) -> Vec<(String,String)>` consistent between Task 6 trait + impl + caller; `notes_links.relation` nullable `TEXT` consistent across DDL, migration, upsert, query.

**Placeholder scan:** No "TBD"/"implement later". The two spots that say "match the real test scaffolding / signatures" (Task 3 Step 6, Task 5 Step 3) are deliberate — the surrounding test helpers vary and the implementer must copy the verbatim local pattern; the assertions and the production code are fully specified.
