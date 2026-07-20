# Note Relationship Network Wiring — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop flattening Aleph's note relationship network — carry typed/directed/weighted edges from storage through the graph algorithms to retrieval, surface today's "computed-then-discarded" signals (backlinks, bridge/surprising insights, structural relations) to the LLM, and add a MinHash/LSH zero-embedding similarity-edge source.

**Architecture:** Four modules. (1) Un-flatten: persist `confidence` on `notes_links`, load `relation`+`confidence` into `GraphSnapshot`, add a directed+weighted layer to `GraphIndex` alongside the existing undirected one, and make the 4-signal direct-link term confidence-weighted. (2) Retrieval: structural-strong relations (`supersedes`/`superseded_by`/`contradicts`) of surfaced notes are force-injected + annotated; each surfaced note gets a backlink count. (3) Orientation: one graph-health line in `index.md` read from already-materialized `notes_graph_insights`. (4) MinHash/LSH similarity edges fed into `notes_graph_related`, auto-consumed by the existing `graph_expand`.

**Tech Stack:** Rust, tokio, rusqlite, serde_json. No new dependencies (R3). Concurrency via `std::thread::scope` (not rayon), matching `relevance::all_related`.

## Global Constraints

- **NO `cargo` runs** (user hard constraint). Do NOT run `cargo check`/`cargo test`/`cargo build`/`cargo clippy`. Verify each change with rust-analyzer/LSP diagnostics on the touched files only. Write unit tests (they document intent and run later) but do not execute them.
- **No new dependencies** (R3). No `rayon`, no graph crate. Concurrency via `std::thread::scope`.
- **Do not touch `src/harness/`** (R10) or core scheduling.
- **Worktree isolation**: all work in a dedicated worktree branch off `main`; never edit `main` directly. When writing files, use the worktree's path — never the `main` absolute path (known footgun).
- **Immutability / surgical edits**: keep changes scoped to the listed files; match surrounding style.
- **Markdown is source of truth**: SQLite is a rebuildable index. `confidence` is reconstructable from frontmatter; default `1.0` when absent.
- Commit messages: `<scope>: <description>`, English.
- Keep the existing undirected `GraphIndex.adj` intact — community detection and Adamic-Adar depend on its symmetric semantics.

---

## Task 1: Persist `confidence` on `notes_links` (schema + migration + write)

Storage half of Module 1. After this task, every `notes_links` row carries a `confidence REAL` (default 1.0); typed relations persist their LLM confidence, plain wikilinks persist 1.0.

**Files:**
- Modify: `src/memory/store/sqlite/schema/ddl.rs:104-112` (add column to `NOTES_LINKS_DDL`)
- Modify: `src/memory/store/sqlite/schema/migrations.rs` (add `migrate_notes_links_confidence`)
- Modify: `src/memory/store/sqlite/schema/mod.rs:86` (register migration after `migrate_notes_links_relation`)
- Modify: `src/memory/store/sqlite/notes/store_impl.rs:111-174` (carry confidence through `desired` map + INSERT)
- Test: `src/memory/store/sqlite/schema/tests.rs` (migration idempotency)

**Interfaces:**
- Produces: `notes_links` now has columns `(id, agent_id, from_note, to_note, to_raw, relation, confidence)`. `confidence REAL NOT NULL DEFAULT 1.0`.
- Consumes: `Relation { to, rel_type, confidence }` from `src/memory/notes/note/relation.rs` (already has `confidence: f32`).

- [ ] **Step 1: Add the column to fresh-DB DDL**

In `src/memory/store/sqlite/schema/ddl.rs`, change the `notes_links` table body (currently lines 104-112) to add `confidence`:

```rust
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    to_raw      TEXT NOT NULL,
    relation    TEXT,
    confidence  REAL NOT NULL DEFAULT 1.0,
    UNIQUE(agent_id, from_note, to_note)
);
```

- [ ] **Step 2: Write the migration (idempotent ADD COLUMN)**

Append to `src/memory/store/sqlite/schema/migrations.rs`, mirroring `migrate_recall_signals_note_path`'s `PRAGMA table_info` existence check:

```rust
/// Add the `confidence` column to `notes_links` for existing databases.
/// Safe to call multiple times (checks column existence first).
pub fn migrate_notes_links_confidence(conn: &Connection) -> Result<(), AlephError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(notes_links)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info notes_links: {e}")))?;
    let has_confidence = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.is_ok_and(|n| n == "confidence"));
    drop(stmt);

    if !has_confidence {
        conn.execute_batch(
            "ALTER TABLE notes_links ADD COLUMN confidence REAL NOT NULL DEFAULT 1.0",
        )
        .map_err(|e| {
            AlephError::config(format!("Failed to add notes_links.confidence: {e}"))
        })?;
    }
    Ok(())
}
```

- [ ] **Step 3: Register the migration**

In `src/memory/store/sqlite/schema/mod.rs`, immediately after the line `migrations::migrate_notes_links_relation(conn)` (line 86), add:

```rust
    migrations::migrate_notes_links_confidence(conn)
        .map_err(|e| AlephError::config(format!("migrate notes_links confidence: {e}")))?;
```

(Match the exact `?`/`.map_err` form of the adjacent migration calls — verify the surrounding lines and copy their style.)

- [ ] **Step 4: Carry confidence through the write path**

In `src/memory/store/sqlite/notes/store_impl.rs`, the `desired` map (line 111) is `HashMap<String, (String, Option<String>)>` = `to_note -> (to_raw, relation)`. Widen it to carry confidence:

```rust
        // to_note -> (to_raw, relation, confidence)
        let mut desired: HashMap<String, (String, Option<String>, f32)> = HashMap::new();
        for raw_target in &note.links {
            let resolved = resolve_target(raw_target)?;
            desired
                .entry(resolved)
                .or_insert_with(|| (raw_target.clone(), None, 1.0));
        }
        for rel in &note.relations {
            let resolved = resolve_target(&rel.to)?;
            // Typed relation overrides a plain wikilink to the same target.
            desired.insert(
                resolved,
                (rel.to.clone(), Some(rel.rel_type.clone()), rel.confidence.clamp(0.0, 1.0)),
            );
        }
```

Update the existing-edges scan (lines 124-144) to also select confidence so the unchanged-skip still works:

```rust
        // Existing edges: to_note -> (to_raw, relation, confidence).
        let existing: HashMap<String, (String, Option<String>, f32)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT to_note, to_raw, relation, confidence FROM notes_links \
                     WHERE agent_id = ?1 AND from_note = ?2",
                )
                .map_err(|e| AlephError::config(format!("index_note links scan prep: {e}")))?;
            let rows = stmt
                .query_map(params![agent_id, path], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, f32>(3)?,
                    ))
                })
                .map_err(|e| AlephError::config(format!("index_note links scan: {e}")))?;
            rows.filter_map(|r| r.ok())
                .map(|(to_note, to_raw, relation, conf)| (to_note, (to_raw, relation, conf)))
                .collect()
        };
```

Update the unchanged-check + UPSERT (lines 159-174):

```rust
        for (to_note, (to_raw, relation, confidence)) in &desired {
            let unchanged = existing.get(to_note).is_some_and(|(er, erel, econf)| {
                er == to_raw && erel == relation && (econf - confidence).abs() < f32::EPSILON
            });
            if unchanged {
                continue;
            }
            conn.execute(
                "INSERT INTO notes_links (agent_id, from_note, to_note, to_raw, relation, confidence) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(agent_id, from_note, to_note) \
                 DO UPDATE SET to_raw = excluded.to_raw, relation = excluded.relation, \
                               confidence = excluded.confidence",
                params![agent_id, path, to_note, to_raw, relation, confidence],
            )
            .map_err(|e| AlephError::config(format!("index_note links upsert: {e}")))?;
        }
```

> Check `store_impl.rs:464` for a second `INSERT INTO notes_links` (a different method). If it does not set `confidence`, the column default (1.0) applies — acceptable. Only update it if it writes typed relations with a known confidence.

- [ ] **Step 5: Write the migration idempotency test**

Add to `src/memory/store/sqlite/schema/tests.rs`:

```rust
#[test]
fn notes_links_confidence_migration_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Simulate a legacy table without confidence.
    conn.execute_batch(
        "CREATE TABLE notes_links (id INTEGER PRIMARY KEY AUTOINCREMENT, \
         agent_id TEXT NOT NULL DEFAULT 'default', from_note TEXT NOT NULL, \
         to_note TEXT NOT NULL, to_raw TEXT NOT NULL, relation TEXT, \
         UNIQUE(agent_id, from_note, to_note))",
    )
    .unwrap();
    super::migrations::migrate_notes_links_confidence(&conn).unwrap();
    super::migrations::migrate_notes_links_confidence(&conn).unwrap(); // twice = no-op
    let has: bool = conn
        .prepare("PRAGMA table_info(notes_links)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .any(|n| n.is_ok_and(|n| n == "confidence"));
    assert!(has);
}
```

- [ ] **Step 6: Static-verify (no cargo)**

Run rust-analyzer/LSP diagnostics on `ddl.rs`, `migrations.rs`, `mod.rs`, `store_impl.rs`, `tests.rs`. Expect zero errors. Do NOT run cargo.

- [ ] **Step 7: Commit**

```bash
git add src/memory/store/sqlite/schema/ddl.rs src/memory/store/sqlite/schema/migrations.rs \
        src/memory/store/sqlite/schema/mod.rs src/memory/store/sqlite/notes/store_impl.rs \
        src/memory/store/sqlite/schema/tests.rs
git commit -m "store: persist edge confidence on notes_links (schema + migration + write)"
```

---

## Task 2: Load typed+weighted edges into `GraphSnapshot` / `GraphIndex`

Graph half of Module 1. After this task, the snapshot carries `rel_type`+`confidence` per edge and `GraphIndex` exposes a directed+weighted layer; the undirected `adj` is unchanged.

**Files:**
- Modify: `src/memory/notes/graph/mod.rs:24-30` (`GraphSnapshot.edges` type), `:33-84` (`GraphIndex` directed layer)
- Modify: `src/memory/store/sqlite/notes/store_impl.rs:1096-1116` (`load_graph_snapshot` edge query)
- Modify: `src/memory/dreaming/stages/graph_recompute.rs:170-173` (test constructor) + `src/memory/notes/graph/tests.rs` (any `GraphSnapshot { edges: ... }` literals)
- Test: `src/memory/notes/graph/tests.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct GraphEdge { pub from: String, pub to: String, pub rel_type: Option<String>, pub confidence: f32 }
  // GraphSnapshot.edges: Vec<GraphEdge>
  // GraphIndex new methods:
  pub fn edge_confidence(&self, a: usize, b: usize) -> f32  // max over a->b and b->a directed edges, 0.0 if none
  pub fn out_edges(&self, i: usize) -> &HashMap<usize, EdgeMeta>
  // EdgeMeta { rel_type: Option<String>, confidence: f32 }
  ```
- Consumes: `notes_links(from_note, to_note, relation, confidence)` from Task 1.

- [ ] **Step 1: Write failing tests for the directed/weighted layer**

Add to `src/memory/notes/graph/tests.rs` (adjust imports to the module's existing style):

```rust
#[test]
fn edge_confidence_is_max_over_directions() {
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode { path: "g/a".into(), category: "x".into(), sources: vec![] },
            GraphNode { path: "g/b".into(), category: "x".into(), sources: vec![] },
        ],
        edges: vec![
            GraphEdge { from: "g/a".into(), to: "g/b".into(), rel_type: Some("cites".into()), confidence: 0.4 },
            GraphEdge { from: "g/b".into(), to: "g/a".into(), rel_type: None, confidence: 0.9 },
        ],
    };
    let g = GraphIndex::build(&snap);
    let (a, b) = (g.index_of("g/a").unwrap(), g.index_of("g/b").unwrap());
    assert!((g.edge_confidence(a, b) - 0.9).abs() < 1e-6);
    // Undirected adjacency still symmetric (community/AA depend on it).
    assert!(g.adj[a].contains(&b) && g.adj[b].contains(&a));
}

#[test]
fn missing_edge_confidence_is_zero() {
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode { path: "g/a".into(), category: "x".into(), sources: vec![] },
            GraphNode { path: "g/b".into(), category: "x".into(), sources: vec![] },
        ],
        edges: vec![],
    };
    let g = GraphIndex::build(&snap);
    let (a, b) = (g.index_of("g/a").unwrap(), g.index_of("g/b").unwrap());
    assert_eq!(g.edge_confidence(a, b), 0.0);
}
```

- [ ] **Step 2: Static-verify the tests fail to compile**

LSP on `graph/tests.rs`: expect "no field/variant `GraphEdge`", "no method `edge_confidence`". Confirms RED. Do NOT run cargo.

- [ ] **Step 3: Define `GraphEdge` + `EdgeMeta` and widen the snapshot**

In `src/memory/notes/graph/mod.rs`, replace the `edges: Vec<(String, String)>` field (line 29) and add the structs:

```rust
/// A directed, typed, weighted edge in the note graph.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    /// LLM-chosen relation verb; `None` for a plain wikilink.
    pub rel_type: Option<String>,
    /// Edge confidence in [0,1]; wikilinks default to 1.0.
    pub confidence: f32,
}

/// Per-target edge metadata in the directed adjacency.
#[derive(Debug, Clone)]
pub struct EdgeMeta {
    pub rel_type: Option<String>,
    pub confidence: f32,
}
```

Change `GraphSnapshot`:

```rust
#[derive(Debug, Clone, Default)]
pub struct GraphSnapshot {
    pub nodes: Vec<GraphNode>,
    /// Directed, typed, weighted resolved edges (`category/filename` pairs).
    pub edges: Vec<GraphEdge>,
}
```

- [ ] **Step 4: Build the directed+weighted layer in `GraphIndex`**

In `GraphIndex` (mod.rs:33-84), keep the undirected `adj` as-is, add directed fields and populate them in `build`. Add to the struct:

```rust
    /// Directed out-edges by node index, with per-target metadata.
    pub out: Vec<std::collections::HashMap<usize, EdgeMeta>>,
    /// Directed in-edges by node index (backlink traversal).
    pub inc: Vec<std::collections::HashMap<usize, EdgeMeta>>,
```

In `build`, where edges are walked (lines 55-63), populate all three (keep the undirected insert exactly as it is):

```rust
        let mut adj = vec![HashSet::new(); snap.nodes.len()];
        let mut out: Vec<HashMap<usize, EdgeMeta>> = vec![HashMap::new(); snap.nodes.len()];
        let mut inc: Vec<HashMap<usize, EdgeMeta>> = vec![HashMap::new(); snap.nodes.len()];
        for e in &snap.edges {
            if let (Some(&a), Some(&b)) =
                (idx_of.get(e.from.as_str()), idx_of.get(e.to.as_str()))
            {
                if a != b {
                    adj[a].insert(b);
                    adj[b].insert(a);
                    let meta = EdgeMeta { rel_type: e.rel_type.clone(), confidence: e.confidence };
                    // Keep the strongest if a pair appears twice.
                    out[a].entry(b)
                        .and_modify(|m| if meta.confidence > m.confidence { *m = meta.clone() })
                        .or_insert_with(|| meta.clone());
                    inc[b].entry(a)
                        .and_modify(|m| if meta.confidence > m.confidence { *m = meta.clone() })
                        .or_insert(meta);
                }
            }
        }
```

Add `out`/`inc` to the returned `Self { ... }`. Add the accessor methods:

```rust
    /// Confidence of the strongest edge between `a` and `b` in either
    /// direction; 0.0 if unconnected. Used to weight the direct-link signal.
    #[must_use]
    pub fn edge_confidence(&self, a: usize, b: usize) -> f32 {
        let f = self.out[a].get(&b).map_or(0.0, |m| m.confidence);
        let r = self.out[b].get(&a).map_or(0.0, |m| m.confidence);
        f.max(r)
    }

    #[must_use]
    pub fn out_edges(&self, i: usize) -> &std::collections::HashMap<usize, EdgeMeta> {
        &self.out[i]
    }
```

(Add `use std::collections::HashMap;` to the existing `use` if not already imported — it is, at line 13.)

- [ ] **Step 5: Fix `load_graph_snapshot` to read relation + confidence**

In `src/memory/store/sqlite/notes/store_impl.rs:1096-1113`, replace the edge load:

```rust
        // Resolved edges only (skip unresolved bare-filename links).
        let mut edges = Vec::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT from_note, to_note, relation, confidence FROM notes_links \
                     WHERE agent_id = ?1 AND to_note <> '' AND instr(to_note, '/') > 0",
                )
                .map_err(|e| AlephError::config(format!("load_graph_snapshot edges prep: {e}")))?;
            let rows = stmt
                .query_map(params![agent_id], |r| {
                    Ok(crate::memory::notes::graph::GraphEdge {
                        from: r.get::<_, String>(0)?,
                        to: r.get::<_, String>(1)?,
                        rel_type: r.get::<_, Option<String>>(2)?,
                        confidence: r.get::<_, f32>(3)?,
                    })
                })
                .map_err(|e| AlephError::config(format!("load_graph_snapshot edges query: {e}")))?;
            for row in rows {
                edges.push(row.map_err(|e| {
                    AlephError::config(format!("load_graph_snapshot edge row: {e}"))
                })?);
            }
        }
```

Ensure `GraphEdge` is in the existing import at line 15 (`use crate::memory::notes::graph::{GraphEdge, GraphNode, GraphSnapshot};`).

- [ ] **Step 6: Make the direct-link signal confidence-weighted**

In `src/memory/notes/graph/relevance.rs:42`, replace the boolean direct-link term:

```rust
    let conf = g.edge_confidence(a, b);
    if conf > 0.0 {
        s += w.direct_link * conf;
    }
```

Add a relevance test (in `relevance`'s tests or `graph/tests.rs`) asserting a 0.5-confidence edge yields half the direct-link contribution of a 1.0 edge, holding other signals equal (two isolated same-category nodes A,B: with conf 1.0 → `direct_link + type_affinity`; with 0.5 → `0.5*direct_link + type_affinity`).

- [ ] **Step 7: Fix all `GraphSnapshot { edges: ... }` constructors**

Update tuple-literal edges to `GraphEdge`:
- `src/memory/dreaming/stages/graph_recompute.rs:172` → `edges: vec![GraphEdge { from: "g/a".into(), to: "g/b".into(), rel_type: None, confidence: 1.0 }]` (add `GraphEdge` to the test `use` at line 152).
- `src/memory/notes/graph/tests.rs` — every `edges: vec![(...)]` literal → `GraphEdge { .. }`.
- Grep the worktree for other constructors: `grep -rn "GraphSnapshot {" src/` and fix each.

- [ ] **Step 8: Static-verify (no cargo)**

LSP on `graph/mod.rs`, `relevance.rs`, `store_impl.rs`, `graph_recompute.rs`, `graph/tests.rs`. Zero errors. The Task-2 RED tests now resolve.

- [ ] **Step 9: Commit**

```bash
git add src/memory/notes/graph/mod.rs src/memory/notes/graph/relevance.rs \
        src/memory/notes/graph/tests.rs src/memory/store/sqlite/notes/store_impl.rs \
        src/memory/dreaming/stages/graph_recompute.rs
git commit -m "graph: un-flatten edges — directed, typed, confidence-weighted GraphIndex"
```

---

## Task 3: Structural-strong must-surface + backlink annotation (retrieval)

Module 2 + Module 3 retrieval half. After this task: when a surfaced note has a `supersedes`/`superseded_by`/`contradicts` out-relation, the target is force-injected into results with an annotation, regardless of score; every surfaced note's content gets a compact backlink/relation footer.

**Files:**
- Modify: `src/memory/notes/note/relation.rs` (add `STRUCTURAL_STRONG` constant + helper)
- Create: `src/memory/note_retrieval/relation_surface.rs`
- Modify: `src/memory/note_retrieval/mod.rs` (declare module; call after `truncate` at line 397-407)
- Test: in `relation_surface.rs` (pure filter) + `relation.rs` (constant)

**Interfaces:**
- Consumes: `NoteStore::get_typed_relations(path, agent_id) -> Vec<(String, String)>` (to, rel_type) [store.rs:130]; `get_incoming_links_any(path, filename, agent_id) -> Vec<String>` [store.rs:120]; `get_notes_with_content(agent_id, &[String]) -> Vec<NoteSearchResult>`; `ScoredFact { fact: MemoryFact, score: f32 }` [store/types.rs:303]; `MemoryFact.content`, `MemoryFact.path` (`note://category/filename`).
- Produces:
  ```rust
  pub const STRUCTURAL_STRONG: &[&str] = &["supersedes", "superseded_by", "contradicts"];
  pub fn is_structural_strong(rel_type: &str) -> bool;
  // in relation_surface.rs:
  pub fn structural_targets(relations: &[(String, String)], already: &HashSet<String>) -> Vec<(String, String)>; // (target_path, rel_type), strong-only, not already present
  pub fn backlink_footer(rel_type_outs: &[(String,String)], backlink_count: usize) -> Option<String>; // compact "[关系] …" line or None
  ```

- [ ] **Step 1: Add the structural-strong vocabulary**

In `src/memory/notes/note/relation.rs`, append:

```rust
/// The only relation verbs the system treats specially: their targets are
/// force-surfaced at retrieval regardless of score (missing a superseded or
/// contradicting note is a correctness bug). All other rel_types stay
/// LLM-chosen and untyped to the system (R7).
pub const STRUCTURAL_STRONG: &[&str] = &["supersedes", "superseded_by", "contradicts"];

#[must_use]
pub fn is_structural_strong(rel_type: &str) -> bool {
    STRUCTURAL_STRONG.contains(&rel_type)
}
```

Add a test:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structural_strong_membership() {
        assert!(is_structural_strong("contradicts"));
        assert!(is_structural_strong("superseded_by"));
        assert!(!is_structural_strong("works_at"));
        assert!(!is_structural_strong("CONTRADICTS")); // case-sensitive, snake_case only
    }
}
```

- [ ] **Step 2: Write the pure surfacing helpers (failing test first)**

Create `src/memory/note_retrieval/relation_surface.rs`:

```rust
//! Pure helpers for surfacing note relationships at retrieval time:
//! force-surface structural-strong targets and render a compact backlink
//! footer. No IO — the caller fetches relations/backlinks and applies these.

use std::collections::HashSet;

use crate::memory::notes::note::relation::is_structural_strong;

/// Structural-strong targets (target_path, rel_type) of one note, excluding any
/// path already present in the result set. Order preserved, deduped by path.
#[must_use]
pub fn structural_targets(
    relations: &[(String, String)],
    already: &HashSet<String>,
) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    relations
        .iter()
        .filter(|(to, rel)| is_structural_strong(rel) && !already.contains(to))
        .filter(|(to, _)| seen.insert(to.clone()))
        .cloned()
        .collect()
}

/// Compact one-line relationship footer for a surfaced note, or None when there
/// is nothing to add. `strong_outs` is (target, rel_type) of this note's
/// structural-strong out-edges; `backlink_count` is how many notes link to it.
#[must_use]
pub fn backlink_footer(strong_outs: &[(String, String)], backlink_count: usize) -> Option<String> {
    if strong_outs.is_empty() && backlink_count == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if backlink_count > 0 {
        parts.push(format!("← {backlink_count} backlinks"));
    }
    for (to, rel) in strong_outs {
        parts.push(format!("⚠ {rel} → {to}"));
    }
    Some(format!("[relations] {}", parts.join(" · ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_targets_filters_to_strong_and_unseen() {
        let rels = vec![
            ("plan/old".to_string(), "superseded_by".to_string()),
            ("entity/acme".to_string(), "works_at".to_string()),
            ("plan/dup".to_string(), "contradicts".to_string()),
        ];
        let already: HashSet<String> = ["plan/dup".to_string()].into_iter().collect();
        let got = structural_targets(&rels, &already);
        assert_eq!(got, vec![("plan/old".to_string(), "superseded_by".to_string())]);
    }

    #[test]
    fn footer_none_when_nothing() {
        assert!(backlink_footer(&[], 0).is_none());
    }

    #[test]
    fn footer_renders_backlinks_and_strong() {
        let outs = vec![("plan/old".to_string(), "supersedes".to_string())];
        let f = backlink_footer(&outs, 3).unwrap();
        assert!(f.contains("← 3 backlinks"));
        assert!(f.contains("⚠ supersedes → plan/old"));
    }
}
```

- [ ] **Step 3: Static-verify RED**

LSP on `relation_surface.rs`: errors only about the module not yet declared in `mod.rs`. Add the declaration in Step 4, then re-check.

- [ ] **Step 4: Declare the module and wire into `retrieve_inner`**

In `src/memory/note_retrieval/mod.rs`, add near the other `mod`/`use` declarations:

```rust
mod relation_surface;
```

After `ranked.truncate(limit);` and `record_recall` (around lines 397-405), and before `Ok(ranked)` (line 406), add a surfacing step. Add this private method to the same `impl` block:

```rust
    /// Annotate surfaced notes with backlink counts + structural-strong
    /// relations, and force-inject the targets of structural-strong relations
    /// that the score-based ranking dropped. Scoped to already-surfaced notes.
    /// Non-fatal: store errors are logged and skipped.
    async fn surface_relations(&self, agent_id: &str, ranked: &mut Vec<ScoredFact>) {
        use std::collections::HashSet;
        let store = self.indexer.store();
        // path form in ScoredFact is "note://category/filename"; strip the scheme.
        let strip = |p: &str| p.strip_prefix("note://").unwrap_or(p).to_string();
        let mut present: HashSet<String> = ranked.iter().map(|f| strip(&f.fact.path)).collect();

        let mut inject: Vec<(String, String, String)> = Vec::new(); // (target_path, rel, source_path)
        for f in ranked.iter_mut() {
            let path = strip(&f.fact.path);
            let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
            let relations = store
                .get_typed_relations(&path, agent_id)
                .await
                .unwrap_or_default();
            let backlinks = store
                .get_incoming_links_any(&path, &filename, agent_id)
                .await
                .unwrap_or_default();
            let strong_outs: Vec<(String, String)> = relations
                .iter()
                .filter(|(_, rel)| crate::memory::notes::note::relation::is_structural_strong(rel))
                .cloned()
                .collect();
            if let Some(footer) =
                relation_surface::backlink_footer(&strong_outs, backlinks.len())
            {
                f.fact.content.push('\n');
                f.fact.content.push_str(&footer);
            }
            for (to, rel) in relation_surface::structural_targets(&relations, &present) {
                if present.insert(to.clone()) {
                    inject.push((to, rel, path.clone()));
                }
            }
        }

        if inject.is_empty() {
            return;
        }
        let paths: Vec<String> = inject.iter().map(|(t, _, _)| t.clone()).collect();
        let hydrated = match store.get_notes_with_content(agent_id, &paths).await {
            Ok(h) => h,
            Err(e) => {
                tracing::debug!(error = %e, "surface_relations: hydrate failed (non-fatal)");
                return;
            }
        };
        for r in hydrated {
            let meta = inject.iter().find(|(t, _, _)| *t == r.path);
            let mut fact = r.to_scored_fact(agent_id);
            if let Some((_, rel, src)) = meta {
                fact.fact.content.push('\n');
                fact.fact
                    .content
                    .push_str(&format!("[relations] ⚠ {rel} ← {src} (force-surfaced)"));
            }
            // Sentinel score below any real hit; presence is the point.
            fact.score = 0.0;
            ranked.push(fact);
        }
    }
```

Then call it just before `Ok(ranked)`:

```rust
        self.surface_relations(agent_id, &mut ranked).await;
        Ok(ranked)
```

> Verify `ScoredFact.fact.content` is the right field name by reading `MemoryFact` in `src/memory/store/types.rs`; if the body field is named differently (e.g. `text`), use that. Verify `NoteSearchResult.to_scored_fact` exists [search_result.rs:44]. These are existing symbols, not placeholders.

- [ ] **Step 5: Static-verify (no cargo)**

LSP on `relation.rs`, `relation_surface.rs`, `note_retrieval/mod.rs`. Zero errors.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/note/relation.rs src/memory/note_retrieval/relation_surface.rs \
        src/memory/note_retrieval/mod.rs
git commit -m "retrieval: force-surface structural-strong relations + backlink annotation"
```

---

## Task 4: Graph-health line in `index.md` (orientation)

Module 3 orientation half. After this task, `index.md` carries one line summarizing the already-materialized `notes_graph_insights` (isolated / bridge / surprising counts), so the LLM can proactively weave the network.

**Files:**
- Modify: `src/memory/notes/orientation/index_md.rs` (add optional health line to `render`/`write`)
- Modify: `src/memory/notes/orientation/fs_orientation.rs:179-182` (`rebuild_index` reads insights, passes counts)
- Modify: `src/memory/notes/store.rs` consumer — use existing `read_graph_insights(agent_id, kind)` [store.rs:273]
- Test: `index_md.rs`

**Interfaces:**
- Consumes: `NoteStore::read_graph_insights(agent_id, Option<&str>) -> Vec<(String, String)>` (kind, json_payload) [store.rs:273]. The JSON arrays' lengths are the counts.
- Produces: `IndexMdGenerator::render(entries, health)` where `health: Option<GraphHealth>`; `GraphHealth { isolated: usize, bridges: usize, surprising: usize }`.

- [ ] **Step 1: Failing test for the health line**

Add to `src/memory/notes/orientation/index_md.rs` tests:

```rust
#[tokio::test]
async fn health_line_rendered_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let g = IndexMdGenerator::new(dir.path());
    let entries = vec![entry("learning", "rust", 1_700_000_000)];
    let health = Some(GraphHealth { isolated: 4, bridges: 1, surprising: 2 });
    let s = g.render(&entries, health).await.unwrap();
    assert!(s.contains("graph health"));
    assert!(s.contains("isolated 4"));
    assert!(s.contains("bridges 1"));
    assert!(s.contains("surprising 2"));
}

#[tokio::test]
async fn no_health_line_when_none() {
    let dir = tempfile::tempdir().unwrap();
    let g = IndexMdGenerator::new(dir.path());
    let s = g.render(&[], None).await.unwrap();
    assert!(!s.contains("graph health"));
}
```

Update the existing tests that call `g.render(&entries)` / `g.render(&[])` to pass `None` as the second arg (and `g.write(&entries)` → `g.write(&entries, None)`).

- [ ] **Step 2: Static-verify RED**

LSP on `index_md.rs`: errors about `GraphHealth` undefined and `render` arity. Confirms RED.

- [ ] **Step 3: Add `GraphHealth` and thread it through `render`/`write`**

In `src/memory/notes/orientation/index_md.rs`:

```rust
/// Compact graph-health counts surfaced in index.md's header.
#[derive(Debug, Clone, Copy, Default)]
pub struct GraphHealth {
    pub isolated: usize,
    pub bridges: usize,
    pub surprising: usize,
}
```

Change `write` and `render` signatures to take `health: Option<GraphHealth>`:

```rust
    pub async fn write(&self, entries: &[NoteIndexEntry], health: Option<GraphHealth>)
        -> Result<IndexStats, AlephError>
    {
        let text = self.render(entries, health).await?;
        // ... unchanged ...
    }

    pub async fn render(&self, entries: &[NoteIndexEntry], health: Option<GraphHealth>)
        -> Result<String, AlephError>
    {
        // ... after the "<!-- total ... -->\n\n# Index\n\n" header block (line ~71):
```

Insert the health line right after the `# Index` header push (after line 71):

```rust
        if let Some(h) = health {
            if h.isolated + h.bridges + h.surprising > 0 {
                out.push_str(&format!(
                    "> graph health: isolated {} · bridges {} · surprising {} — consider weaving isolated notes.\n\n",
                    h.isolated, h.bridges, h.surprising
                ));
            }
        }
```

- [ ] **Step 4: Read insights in `rebuild_index` and pass counts**

In `src/memory/notes/orientation/fs_orientation.rs:179-182`, the `rebuild_index` impl currently does `gen.write(&entries).await`. Change it to read insights and pass the health. The orientation struct must have store access — check how it gets `entries` (it already queries notes for `entries`); use the same store handle. Sketch:

```rust
    async fn rebuild_index(&self, agent_id: &str) -> Result<IndexStats, AlephError> {
        let entries = /* existing: list_notes(...) */;
        let gen = IndexMdGenerator::new(self.agent_dir(agent_id));
        let health = self.graph_health(agent_id).await; // Option<GraphHealth>, non-fatal
        let stats = gen.write(&entries, health).await?;
        // ... unchanged ...
    }
```

Add the helper (reads the three insight kinds; JSON array length = count; swallow errors → `None`):

```rust
    async fn graph_health(&self, agent_id: &str)
        -> Option<crate::memory::notes::orientation::index_md::GraphHealth>
    {
        use crate::memory::notes::orientation::index_md::GraphHealth;
        let store = /* the store handle this struct already holds */;
        let count = |rows: &[(String, String)], kind: &str| -> usize {
            rows.iter()
                .find(|(k, _)| k == kind)
                .and_then(|(_, json)| serde_json::from_str::<serde_json::Value>(json).ok())
                .and_then(|v| v.as_array().map(Vec::len))
                .unwrap_or(0)
        };
        let rows = store.read_graph_insights(agent_id, None).await.ok()?;
        Some(GraphHealth {
            isolated: count(&rows, "isolated"),
            bridges: count(&rows, "bridge"),
            surprising: count(&rows, "surprising"),
        })
    }
```

> Confirm the field/handle name for the store inside `FsNoteOrientation` (read the struct def near the top of `fs_orientation.rs`) and the exact `read_graph_insights` signature [store.rs:273] — pass `None` for "all kinds". Also update the in-file tests at `fs_orientation.rs:322` (`rebuild_index`) — they should still pass since `graph_health` degrades to `None` on an empty store.

- [ ] **Step 5: Static-verify (no cargo)**

LSP on `index_md.rs`, `fs_orientation.rs`. Zero errors.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/orientation/index_md.rs src/memory/notes/orientation/fs_orientation.rs
git commit -m "orientation: surface graph-health line from materialized insights"
```

---

## Task 5: MinHash/LSH similarity-edge module (pure)

Module 4 half 1. A standalone, dependency-free MinHash + LSH module computing near-duplicate / high-similarity edges between note bodies.

**Files:**
- Create: `src/memory/notes/graph/minhash.rs`
- Modify: `src/memory/notes/graph/mod.rs:7-9` (add `pub mod minhash;`)
- Test: in `minhash.rs`

**Interfaces:**
- Produces:
  ```rust
  pub const K: usize = 64;
  pub fn shingles(body: &str) -> std::collections::HashSet<u64>; // 3-word shingle hashes (whole-token set if <3 words)
  pub fn signature(shingles: &std::collections::HashSet<u64>) -> [u64; K];
  pub fn jaccard_estimate(a: &[u64; K], b: &[u64; K]) -> f32;
  /// Similarity edges among docs. `docs`: (path, body). Returns (from, to, score)
  /// where score = jaccard * SIMILARITY_EDGE_WEIGHT, jaccard ≥ threshold,
  /// capped at `cap` per node, threads via std::thread::scope.
  pub fn similarity_edges(docs: &[(String, String)], threshold: f32, cap: usize, threads: usize)
      -> Vec<(String, String, f32)>;
  pub const SIMILARITY_EDGE_WEIGHT: f32 = 3.0;
  ```

- [ ] **Step 1: Failing tests**

Create `src/memory/notes/graph/minhash.rs` with tests first (and the API stubs `todo!()`-free — write the real impl in Step 3; here, write the test module that references the API):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_bodies_estimate_near_one() {
        let a = signature(&shingles("the quick brown fox jumps over the lazy dog"));
        let b = signature(&shingles("the quick brown fox jumps over the lazy dog"));
        assert!((jaccard_estimate(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn disjoint_bodies_estimate_near_zero() {
        let a = signature(&shingles("alpha beta gamma delta epsilon zeta"));
        let b = signature(&shingles("one two three four five six seven"));
        assert!(jaccard_estimate(&a, &b) < 0.2);
    }

    #[test]
    fn similarity_edges_link_near_duplicates_only() {
        let docs = vec![
            ("g/a".to_string(), "rust ownership borrowing lifetimes prevent data races".to_string()),
            ("g/b".to_string(), "rust ownership borrowing lifetimes prevent data races today".to_string()),
            ("g/c".to_string(), "completely unrelated text about cooking pasta sauce".to_string()),
        ];
        let edges = similarity_edges(&docs, 0.5, 8, 1);
        assert!(edges.iter().any(|(f, t, _)| (f == "g/a" && t == "g/b") || (f == "g/b" && t == "g/a")));
        assert!(!edges.iter().any(|(f, t, _)| *f == "g/c" || *t == "g/c"));
        assert!(edges.iter().all(|(_, _, s)| *s > 0.0));
    }

    #[test]
    fn short_body_falls_back_to_token_set() {
        // <3 words: shingle set = the individual token hashes, still comparable.
        let a = shingles("hello world");
        assert!(!a.is_empty());
    }
}
```

- [ ] **Step 2: Static-verify RED**

LSP: `shingles`/`signature`/`jaccard_estimate`/`similarity_edges` undefined. Confirms RED.

- [ ] **Step 3: Implement the module**

Prepend to `src/memory/notes/graph/minhash.rs` (above the tests). Use a deterministic per-seed hash (xorshift-mixed) — no external crate:

```rust
//! MinHash + LSH similarity edges over note bodies. Zero-embedding, zero new
//! deps (R3). Word-level 3-shingles, K=64 MinHash, LSH banding for O(n)
//! candidate generation, exact Jaccard estimate gating. Concurrency via
//! std::thread::scope (matching relevance::all_related). Deterministic.

use std::collections::{HashMap, HashSet};

pub const K: usize = 64;
/// Scale jaccard (≤1) into the 4-signal magnitude range so similarity edges are
/// competitive with a single direct link when merged into notes_graph_related.
pub const SIMILARITY_EDGE_WEIGHT: f32 = 3.0;
const BANDS: usize = 32;
const ROWS: usize = K / BANDS; // 2

/// FNV-1a 64-bit of a string.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Mix a base hash with a seed (splitmix64-style) for K independent hashes.
fn mix(x: u64, seed: u64) -> u64 {
    let mut z = x.wrapping_add(seed).wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// 3-word shingle hashes of a lowercased body. Whole-token set if < 3 words.
#[must_use]
pub fn shingles(body: &str) -> HashSet<u64> {
    let toks: Vec<&str> = body.split_whitespace().collect();
    let mut out = HashSet::new();
    if toks.len() < 3 {
        for t in &toks {
            out.insert(fnv1a(&t.to_lowercase()));
        }
        return out;
    }
    for w in toks.windows(3) {
        let s = format!("{} {} {}", w[0].to_lowercase(), w[1].to_lowercase(), w[2].to_lowercase());
        out.insert(fnv1a(&s));
    }
    out
}

/// K-length MinHash signature. Empty input → all u64::MAX (estimates 0 vs any).
#[must_use]
pub fn signature(shingles: &HashSet<u64>) -> [u64; K] {
    let mut sig = [u64::MAX; K];
    for &sh in shingles {
        for (k, slot) in sig.iter_mut().enumerate() {
            let h = mix(sh, k as u64);
            if h < *slot {
                *slot = h;
            }
        }
    }
    sig
}

#[must_use]
pub fn jaccard_estimate(a: &[u64; K], b: &[u64; K]) -> f32 {
    let agree = a.iter().zip(b.iter()).filter(|(x, y)| x == y).count();
    agree as f32 / K as f32
}

/// Similarity edges among docs (path, body). LSH candidate generation, exact
/// Jaccard gating ≥ threshold, ≤ cap edges per node. Deterministic output
/// (sorted). Edge score = jaccard * SIMILARITY_EDGE_WEIGHT.
#[must_use]
pub fn similarity_edges(
    docs: &[(String, String)],
    threshold: f32,
    cap: usize,
    threads: usize,
) -> Vec<(String, String, f32)> {
    let n = docs.len();
    if n < 2 {
        return Vec::new();
    }
    // Signatures in parallel (CPU-bound; std::thread::scope, no rayon).
    let threads = threads.clamp(1, n);
    let chunk = n.div_ceil(threads);
    let mut sigs: Vec<[u64; K]> = vec![[u64::MAX; K]; n];
    {
        let docs_ref = &docs;
        let slices: Vec<&mut [[u64; K]]> = sigs.chunks_mut(chunk).collect();
        std::thread::scope(|scope| {
            for (t, slice) in slices.into_iter().enumerate() {
                let start = t * chunk;
                scope.spawn(move || {
                    for (j, slot) in slice.iter_mut().enumerate() {
                        *slot = signature(&shingles(&docs_ref[start + j].1));
                    }
                });
            }
        });
    }

    // LSH: bucket by band; candidate pairs collide in ≥1 band.
    let mut buckets: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
    for (i, sig) in sigs.iter().enumerate() {
        for band in 0..BANDS {
            let mut h: u64 = 0xcbf29ce484222325;
            for r in 0..ROWS {
                h ^= sig[band * ROWS + r];
                h = h.wrapping_mul(0x100000001b3);
            }
            buckets.entry((band, h)).or_default().push(i);
        }
    }
    let mut cand: HashSet<(usize, usize)> = HashSet::new();
    for members in buckets.values() {
        for a in 0..members.len() {
            for b in (a + 1)..members.len() {
                let (x, y) = (members[a], members[b]);
                cand.insert(if x < y { (x, y) } else { (y, x) });
            }
        }
    }

    // Exact-estimate gate + per-node cap.
    let mut per_node: HashMap<usize, Vec<(usize, f32)>> = HashMap::new();
    for (x, y) in cand {
        let j = jaccard_estimate(&sigs[x], &sigs[y]);
        if j >= threshold {
            per_node.entry(x).or_default().push((y, j));
            per_node.entry(y).or_default().push((x, j));
        }
    }
    let mut edges: Vec<(String, String, f32)> = Vec::new();
    let mut keys: Vec<usize> = per_node.keys().copied().collect();
    keys.sort_unstable();
    for k in keys {
        let mut peers = per_node.remove(&k).unwrap();
        peers.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        peers.truncate(cap);
        for (peer, j) in peers {
            edges.push((docs[k].0.clone(), docs[peer].0.clone(), j * SIMILARITY_EDGE_WEIGHT));
        }
    }
    edges
}
```

Add `pub mod minhash;` to `src/memory/notes/graph/mod.rs` after line 9 (`pub mod relevance;`).

- [ ] **Step 4: Static-verify (no cargo)**

LSP on `minhash.rs`, `graph/mod.rs`. Zero errors. RED tests now resolve.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/graph/minhash.rs src/memory/notes/graph/mod.rs
git commit -m "graph: add MinHash/LSH zero-embedding similarity module"
```

---

## Task 6: Wire MinHash edges into the dream graph recompute

Module 4 half 2. After this task, each graph recompute also computes MinHash similarity edges over note bodies and merges them (max-by-pair) into `notes_graph_related`, so `graph_expand` surfaces lexically-similar notes automatically.

**Files:**
- Modify: `src/memory/dreaming/stages/graph_recompute.rs` (load bodies, compute minhash edges, merge)
- Test: `graph_recompute.rs`

**Interfaces:**
- Consumes: `minhash::similarity_edges` [Task 5]; `NoteStore::list_notes`, `NoteStore::get_notes_with_content` (returns `NoteSearchResult { path, content, .. }`).
- Produces: merged `related` rows in `notes_graph_related` (existing consumer `graph_expand` unchanged).

- [ ] **Step 1: Failing test for the merge helper**

Add to `graph_recompute.rs` tests — a pure merge that dedups `(seed, peer)` keeping the max score:

```rust
#[test]
fn merge_related_keeps_max_per_pair() {
    let four_signal = vec![("a".to_string(), "b".to_string(), 4.0)];
    let minhash = vec![
        ("a".to_string(), "b".to_string(), 2.5), // same pair, lower → dropped
        ("a".to_string(), "c".to_string(), 2.5), // new pair → kept
    ];
    let merged = merge_related(four_signal, minhash);
    let ab = merged.iter().find(|(s, p, _)| s == "a" && p == "b").unwrap();
    assert!((ab.2 - 4.0).abs() < 1e-6, "max score wins");
    assert!(merged.iter().any(|(s, p, _)| s == "a" && p == "c"));
}
```

- [ ] **Step 2: Static-verify RED**

LSP: `merge_related` undefined.

- [ ] **Step 3: Implement merge + load bodies + compute**

Add the pure merge fn to `graph_recompute.rs`:

```rust
/// Merge two `(seed, peer, score)` edge lists, deduped by `(seed, peer)` keeping
/// the max score (explicit/4-signal edges beat lexical-similarity edges on ties).
fn merge_related(
    a: Vec<(String, String, f32)>,
    b: Vec<(String, String, f32)>,
) -> Vec<(String, String, f32)> {
    use std::collections::HashMap;
    let mut best: HashMap<(String, String), f32> = HashMap::new();
    for (s, p, sc) in a.into_iter().chain(b) {
        best.entry((s, p))
            .and_modify(|e| { if sc > *e { *e = sc; } })
            .or_insert(sc);
    }
    let mut out: Vec<(String, String, f32)> =
        best.into_iter().map(|((s, p), sc)| (s, p, sc)).collect();
    out.sort_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.cmp(&y.1)));
    out
}
```

In `execute` (lines 30-51), after the `spawn_blocking(compute)` produces `computed`, load bodies and compute minhash edges, then merge before `replace_graph_related`:

```rust
        // MinHash similarity edges (content-based; the structural snapshot has
        // no bodies). Non-fatal: failure → skip, keep 4-signal edges.
        let docs: Vec<(String, String)> = match async {
            let entries = store.list_notes(&agent_id).await?;
            let paths: Vec<String> = entries.into_iter().map(|e| e.path).collect();
            let hydrated = store.get_notes_with_content(&agent_id, &paths).await?;
            Ok::<_, AlephError>(hydrated.into_iter().map(|r| (r.path, r.content)).collect())
        }
        .await
        {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(error = %e, "graph recompute: body load failed, skipping minhash");
                Vec::new()
            }
        };

        let related = if docs.len() >= 2 {
            let mh = tokio::task::spawn_blocking(move || {
                let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
                crate::memory::notes::graph::minhash::similarity_edges(&docs, 0.82, 8, threads)
            })
            .await
            .map_err(|e| AlephError::other(format!("minhash join: {e}")))?;
            merge_related(computed.related, mh)
        } else {
            computed.related
        };

        store.replace_graph_cache(&agent_id, &computed.cache).await?;
        store.replace_graph_insights(&agent_id, &computed.insights).await?;
        store.replace_graph_related(&agent_id, &related).await?;
```

> Remove the original `replace_graph_related(&agent_id, &computed.related)` call (lines 46-48) — it's superseded by the merged `related`. Keep `cache`/`insights` calls. Confirm `list_notes` and `get_notes_with_content` signatures in `store.rs`; `NoteSearchResult.content` is the body [search_result.rs].

- [ ] **Step 4: Static-verify (no cargo)**

LSP on `graph_recompute.rs`. Zero errors. Confirm the existing `compute_*` tests still compile (they don't touch `merge_related`).

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/graph_recompute.rs
git commit -m "dream: feed MinHash similarity edges into notes_graph_related"
```

---

## Task 7: Optional config knob for the MinHash threshold

YAGNI-guarded: only do this if `memory.graph` config already exists. Makes the 0.82 threshold tunable.

**Files:**
- Modify: `src/config/types/memory.rs` (add `minhash_threshold: f32` to the graph config, default 0.82) — only if a graph config struct exists.
- Modify: `src/memory/dreaming/stages/graph_recompute.rs` (read from config instead of literal 0.82).

- [ ] **Step 1: Check for an existing graph config**

```bash
grep -rn "ExpansionConfig\|memory.graph\|GraphConfig\|struct.*Graph.*Config" src/config/ 2>/dev/null
```

If a graph config struct exists, add `minhash_threshold` (default 0.82) and thread it into the `similarity_edges` call. If none exists, **skip this task** — keep the literal 0.82 with a `// TODO(config)`-free named const `const MINHASH_THRESHOLD: f32 = 0.82;` at the top of `graph_recompute.rs` instead, and note in the commit that config wiring is deferred (no consumer yet — YAGNI).

- [ ] **Step 2: Static-verify + Commit (only if Step 1 applied)**

```bash
git add src/config/types/memory.rs src/memory/dreaming/stages/graph_recompute.rs
git commit -m "config: expose memory.graph.minhash_threshold (default 0.82)"
```

---

## Self-Review

**Spec coverage:**
- Module 1 (un-flatten) → Task 1 (storage) + Task 2 (graph). ✓
- Module 2 (structural-strong must-out) → Task 3. ✓
- Module 3 (backlinks + insights surface) → Task 3 (backlinks/retrieval) + Task 4 (orientation health). ✓
- Module 4 (MinHash) → Task 5 (module) + Task 6 (wiring). ✓
- Schema migration (D4) → Task 1. ✓
- D2 (weight + structural-strong) → Task 2 Step 6 (weight) + Task 3 (structural-strong). ✓
- D3 (retrieval-first + one-line health) → Task 3 + Task 4. ✓
- Entropy reduction → Task 6 removes the superseded `replace_graph_related(computed.related)` call; dead data (`relation`/`confidence`, bridge/surprising insights) becomes live. ✓
- Concurrency via std::thread::scope, no rayon → Task 5/6. ✓

**Placeholder scan:** No "TBD"/"implement later". Task 7 is explicitly YAGNI-gated with a concrete fallback (named const). The few "verify field name X by reading file Y" notes point at existing symbols (not placeholders) and are guards against drift in code the plan can't fully quote without reading mid-execution.

**Type consistency:** `GraphEdge`/`EdgeMeta` defined in Task 2, consumed in Task 2 Steps 5-7. `STRUCTURAL_STRONG`/`is_structural_strong` defined Task 3 Step 1, used Steps 2/4. `GraphHealth` defined Task 4 Step 3, used Steps 1/4. `similarity_edges`/`SIMILARITY_EDGE_WEIGHT` defined Task 5, used Task 6. `merge_related` defined + used Task 6. Edge-tuple shape `(String, String, f32)` consistent across `replace_graph_related`, `all_related`, `similarity_edges`, `merge_related`. ✓

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-06-26-note-relationship-network-wiring.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
