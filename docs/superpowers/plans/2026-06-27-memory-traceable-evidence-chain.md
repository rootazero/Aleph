# Memory Traceable Evidence-Chain Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire Aleph's already-built (but unpopulated) memory provenance infrastructure so any high-level memory claim can be drilled down to ground-truth evidence: L3 profile section → L2 synthesis note → L1 note fact → L0 raw_memory → transcript.

**Architecture:** Connection-first. Aleph already has `source_notes` / `fact_provenance` fields, `notes_sources` / `notes_provenance` tables (with reverse indexes), and per-fact `<!-- src: -->` markers — they are simply never filled by the compression pipeline. We thread raw ids through `PageOp` → `CompoundApplyTx::stage()`, populate the note's provenance fields before indexing, extend the synthesis stage (L2) and profile synthesizer (L3), then add read APIs + a `memory_trace` builtin tool + a read-only `memory.trace` RPC to consume the chain.

**Tech Stack:** Rust (tokio + serde + rusqlite), the `alephcore` crate. Tests are `#[tokio::test]` / `#[test]` in `#[cfg(test)]` modules. insta snapshot tests for prompts.

## Global Constraints

- **Redlines:** R7 LLM sovereignty (code only threads ids; extraction/attribution stays in the LLM). R3 core-minimalism (NO new third-party deps — reuse serde / rusqlite / existing tables). R4 interfaces are I/O-only (the RPC only forwards). R10 thin harness (do NOT touch `src/harness/`; no error-recovery hooks).
- **No new physical L2 layer, no Mermaid short-term memory, no raw_memories retention sweep, no rerouting compression through the event handler.** (Out of scope per spec §9.)
- **cargo frugality (user machine constraint):** run **targeted single tests** (`cargo test -p alephcore <module>::<test> -- --exact --nocapture`), never the full suite. At most **one** `cargo check -p alephcore --lib` at the very end of the whole plan.
- **Commit style:** English, `memory: <description>` scope. No attribution trailer.
- **Branch isolation:** all code changes happen in a git worktree off `main` (created at execution time via `superpowers:using-git-worktrees`), never directly on `main`.
- **Provenance marker format (canonical, already supported by `PROVENANCE_RE`):** `<!-- src: <source_id>, origin: raw_source, inferred: false -->`. The `src:` segment is optional; unattributed facts get `<!-- origin: inferred, inferred: true -->`.
- **Spec:** `docs/superpowers/specs/2026-06-27-memory-traceable-evidence-chain-design.md`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `src/memory/notes/ingest/plan.rs` | `PageOp` data model | Add `source_ids` to `Create` + `Append` (Task 1) |
| `src/memory/notes/note/parsing.rs` | frontmatter + per-fact marker parse | Add `fact_provenance_for(fact)` single-fact helper (Task 2) |
| `src/memory/notes/note/mod.rs` | `KnowledgeNote` | Re-export `fact_provenance_for` (Task 2) |
| `src/memory/notes/ingest/apply.rs` | transactional apply | Populate `source_notes` + `fact_provenance`; batch-source fallback (Task 2) |
| `src/memory/notes/ingest/ingestor/batch.rs` | batch pipeline | Thread batch raw-ids into `try_apply`; carry `source_ids` across dedup redirect (Task 2) |
| `src/memory/notes/ingest/prompts.rs` | ingest LLM prompt | Instruct LLM to emit `source_ids` + inline per-fact `src:` markers (Task 3) |
| `src/memory/dreaming/stages/note_synthesis.rs` | L2 synthesis | Set synthesis note's `source_notes` = cluster member paths (Task 4) |
| `src/memory/notes/profile/types.rs` | `UserProfile` | Add `sources: BTreeMap<String, Vec<String>>` (Task 5) |
| `src/memory/notes/profile/store.rs` | USER.md parse/render | Parse + render `## Sources` block (Task 5) |
| `src/memory/notes/profile/synthesizer.rs` | profile update | Accumulate session_ids per modified section (Task 5) |
| `src/memory/notes/store.rs` | `NoteStore` trait | Add `sources_of`, `notes_citing` (Task 6) |
| `src/memory/store/raw_memory.rs` | `RawMemoryStore` trait | Add `get_raws_by_ids`, `get_raws_by_session` (Task 6) |
| `src/memory/store/sqlite/notes/store_impl.rs` | sqlite NoteStore impl | Implement `sources_of`, `notes_citing` (Task 6) |
| `src/memory/store/sqlite/` (raw impl) | sqlite RawMemoryStore impl | Implement raw fetch methods (Task 6) |
| `src/builtin_tools/memory_trace.rs` | NEW drill-down tool | Walk the chain (Task 7) |
| `src/builtin_tools/mod.rs` | tool module list | `pub mod memory_trace;` (Task 7) |
| `src/executor/builtin_registry/builder/constructor/mod.rs` | DI | Inject `memory_trace_db` (Task 7) |
| `src/executor/builtin_registry/registry/tool_registry_impl.rs` | dispatch | `"memory_trace" =>` arm (Task 7) |
| `src/gateway/handlers/memory.rs` | RPC handler | `handle_trace` (Task 8) |
| `src/bin/aleph-server/commands/start/builder/handlers/memory.rs` | RPC registration | register `memory.trace` (Task 8) |
| `docs/reference/memory/RAW_MEMORY.md`, `MEMORY_SYSTEM.md`, `FEATURE_LOCATOR.md` | docs | Invariant + chain doc (Task 9) |

---

## Task 1: `PageOp` carries `source_ids`

**Files:**
- Modify: `src/memory/notes/ingest/plan.rs:20-64` (enum), and the in-file test literals at `:130-205`
- Test: `src/memory/notes/ingest/plan.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `PageOp::Create { …, source_ids: Vec<String> }` and `PageOp::Append { …, source_ids: Vec<String> }`, both `#[serde(default)]` (old plan JSON without the field still parses). Consumed by Task 2 (`apply.rs`) and Task 3 (prompt).

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `plan.rs`:

```rust
#[test]
fn create_and_append_carry_source_ids_with_serde_default() {
    // New field round-trips.
    let op = PageOp::Create {
        note_path: "preference/typescript".into(),
        title: "TypeScript".into(),
        summary: "User prefers TypeScript".into(),
        facts: vec!["The user prefers TypeScript.".into()],
        links: vec![],
        tags: vec![],
        relations: vec![],
        source_ids: vec!["raw-uuid-1".into(), "raw-uuid-2".into()],
    };
    let j = serde_json::to_string(&op).unwrap();
    let back: PageOp = serde_json::from_str(&j).unwrap();
    match back {
        PageOp::Create { source_ids, .. } => assert_eq!(source_ids, vec!["raw-uuid-1", "raw-uuid-2"]),
        _ => panic!("expected create"),
    }

    // Old JSON WITHOUT source_ids still parses (serde default = empty).
    let legacy = r#"{"kind":"create","note_path":"a/b","title":"T","summary":"","facts":[],"links":[],"tags":[]}"#;
    let op2: PageOp = serde_json::from_str(legacy).unwrap();
    match op2 {
        PageOp::Create { source_ids, .. } => assert!(source_ids.is_empty()),
        _ => panic!("expected create"),
    }

    let legacy_append = r#"{"kind":"append","note_path":"a/b","new_facts":["x"],"new_links":[]}"#;
    let op3: PageOp = serde_json::from_str(legacy_append).unwrap();
    match op3 {
        PageOp::Append { source_ids, .. } => assert!(source_ids.is_empty()),
        _ => panic!("expected append"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore notes::ingest::plan::tests::create_and_append_carry_source_ids_with_serde_default -- --exact --nocapture`
Expected: FAIL to compile — `PageOp::Create` has no field `source_ids`.

- [ ] **Step 3: Add the field to the enum**

In `plan.rs`, modify `PageOp::Create` (after `relations`) and `PageOp::Append` (after `new_relations`):

```rust
    Create {
        note_path: String,
        title: String,
        summary: String,
        #[serde(default)]
        facts: Vec<String>,
        #[serde(default)]
        links: Vec<String>,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        relations: Vec<Relation>,
        /// Raw-memory ids (or prior-note paths) this page was distilled from.
        /// LLM-attributed; empty means the apply layer falls back to the batch set.
        #[serde(default)]
        source_ids: Vec<String>,
    },
    Append {
        note_path: String,
        #[serde(default)]
        new_facts: Vec<String>,
        #[serde(default)]
        new_links: Vec<String>,
        #[serde(default)]
        new_relations: Vec<Relation>,
        #[serde(default)]
        source_ids: Vec<String>,
    },
```

- [ ] **Step 4: Fix the in-file test literals**

The existing `plan.rs` tests spell out every `Create`/`Append` field. Add `source_ids: vec![]` to each:
- `page_op_roundtrip_json` (`:132` Create, `:142` Append)
- `page_op_primary_path_matches_variant` (`:183` Create)
- `create_op_parses_relations_and_defaults_when_absent` — no change (uses JSON strings)
- `append_op_parses_new_relations` — no change (JSON string)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore notes::ingest::plan::tests -- --nocapture`
Expected: PASS (all plan.rs tests).

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/ingest/plan.rs
git commit -m "memory: PageOp Create/Append carry source_ids (serde default)"
```

---

## Task 2: Populate note provenance in apply + thread batch sources

**Files:**
- Modify: `src/memory/notes/note/parsing.rs` (add `fact_provenance_for`)
- Modify: `src/memory/notes/note/mod.rs` (re-export it)
- Modify: `src/memory/notes/ingest/apply.rs` (`CompoundApplyTx` field + `with_batch_sources` + `stage()` Create/Append + test literals)
- Modify: `src/memory/notes/ingest/ingestor/batch.rs` (`try_apply` threads batch ids; dedup carries `source_ids`)
- Test: `src/memory/notes/ingest/apply.rs` tests

**Interfaces:**
- Consumes: `PageOp::*.source_ids` (Task 1).
- Produces: `CompoundApplyTx::with_batch_sources(self, ids: Vec<String>) -> Self`; `parsing::fact_provenance_for(fact: &str) -> FactProvenance` re-exported as `crate::memory::notes::note::fact_provenance_for`. After this task a created/appended note has `source_notes` (note-level, with batch fallback) and `fact_provenance` (per-fact from markers) populated → `index_note` materializes `notes_sources` + `notes_provenance`.

- [ ] **Step 1: Write the failing test (note-level source_notes + fallback)**

Add to `apply.rs` tests:

```rust
#[tokio::test]
async fn create_populates_source_notes_from_op_ids() {
    let (dir, backend, indexer) = fresh().await;
    let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
    tx.stage(&PageOp::Create {
        note_path: "preference/typescript".into(),
        title: "TypeScript".into(),
        summary: "".into(),
        facts: vec!["The user prefers TypeScript.".into()],
        links: vec![],
        tags: vec![],
        relations: vec![],
        source_ids: vec!["raw-A".into(), "raw-B".into()],
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let body = tokio::fs::read_to_string(dir.path().join("note/default/preference/typescript.md"))
        .await
        .unwrap();
    assert!(body.contains("source_notes:"), "frontmatter must carry source_notes");
    assert!(body.contains("raw-A") && body.contains("raw-B"));

    // notes_sources reverse index is populated through index_note.
    let citing = backend.notes_citing("default", "raw-A").await.unwrap();
    assert!(citing.iter().any(|p| p == "preference/typescript"));
}

#[tokio::test]
async fn create_falls_back_to_batch_sources_when_op_ids_empty() {
    let (dir, backend, indexer) = fresh().await;
    let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default")
        .with_batch_sources(vec!["raw-batch-1".into()]);
    tx.stage(&PageOp::Create {
        note_path: "learning/x".into(),
        title: "X".into(),
        summary: "".into(),
        facts: vec!["fact".into()],
        links: vec![],
        tags: vec![],
        relations: vec![],
        source_ids: vec![], // LLM omitted → fall back to batch
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let citing = backend.notes_citing("default", "raw-batch-1").await.unwrap();
    assert!(citing.iter().any(|p| p == "learning/x"));
}
```

> Note: `notes_citing` is added in Task 6. To keep Task 2 independently testable BEFORE Task 6, assert against the file body + read `source_notes` back via `KnowledgeNote::from_markdown` instead of `notes_citing`. Use this variant for Step 1 if executing strictly in order:

```rust
    // Order-independent assertion (no Task 6 dependency):
    let n = crate::memory::notes::note::KnowledgeNote::from_markdown(
        "typescript",
        &body,
    ).unwrap();
    assert_eq!(n.source_notes, vec!["raw-A".to_string(), "raw-B".to_string()]);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore notes::ingest::apply::tests::create_populates_source_notes_from_op_ids -- --exact --nocapture`
Expected: FAIL — `with_batch_sources` missing / `source_notes` empty.

- [ ] **Step 3: Add `fact_provenance_for` to parsing.rs and re-export**

In `src/memory/notes/note/parsing.rs` (uses the existing `PROVENANCE_RE`):

```rust
/// Per-fact provenance parse — single-fact counterpart of
/// `extract_provenance_markers`. Returns `Default` (Legacy) when the fact
/// carries no recognizable marker.
#[must_use]
pub fn fact_provenance_for(fact: &str) -> super::types::FactProvenance {
    use super::types::{FactProvenance, ProvenanceOrigin};
    if let Some(caps) = PROVENANCE_RE.captures(fact) {
        let source_id = caps.get(1).map(|m| m.as_str().trim().to_string());
        let origin = match caps.get(2).map(|m| m.as_str()).unwrap_or("legacy") {
            "raw_source" => ProvenanceOrigin::RawSource,
            "prior_note" => ProvenanceOrigin::PriorNote,
            "inferred" => ProvenanceOrigin::Inferred,
            _ => ProvenanceOrigin::Legacy,
        };
        let inferred = caps.get(3).map(|m| m.as_str() == "true").unwrap_or(false);
        FactProvenance { origin, source_id, inferred }
    } else {
        FactProvenance::default()
    }
}
```

In `src/memory/notes/note/mod.rs`, add near the other `pub use parsing::…` re-exports (the module already re-exports `extract_provenance_markers`):

```rust
pub use parsing::fact_provenance_for;
```

- [ ] **Step 4: Add batch-source field + builder to `CompoundApplyTx`**

In `apply.rs`, add a field to the struct (`:58-69`) and initialize it in `new` (`:71-93`):

```rust
pub struct CompoundApplyTx<'a, S: NoteStore + Send + Sync + 'static> {
    // ... existing fields ...
    batch_source_ids: Vec<String>,
    committed: bool,
}
```
In `new(...)`, add `batch_source_ids: Vec::new(),` to the struct literal. Then add the builder right after `tx_id()`:

```rust
    /// Deterministic fallback raw-ids applied to any staged note whose op
    /// carried no `source_ids` (so the L0→L1 chain is never empty).
    #[must_use]
    pub fn with_batch_sources(mut self, ids: Vec<String>) -> Self {
        self.batch_source_ids = ids;
        self
    }

    fn resolve_sources(&self, op_ids: &[String]) -> Vec<String> {
        if op_ids.is_empty() {
            self.batch_source_ids.clone()
        } else {
            op_ids.to_vec()
        }
    }
```

- [ ] **Step 5: Populate provenance in `stage()` Create + Append**

In `stage()`, the `PageOp::Create` arm — destructure `source_ids` and set both provenance fields. Replace the `KnowledgeNote { … ..Default::default() }` construction (`:115-126`) so it sets `source_notes`, and after the summary/title facts are inserted, set `fact_provenance`:

```rust
            PageOp::Create {
                note_path,
                title,
                summary,
                facts,
                links,
                tags,
                relations,
                source_ids,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename)?;
                let mut note = KnowledgeNote {
                    title: safe.clone(),
                    category: category.clone(),
                    tags: tags.clone(),
                    facts: facts.iter().map(|f| ensure_origin_marker(f)).collect(),
                    links: links.clone(),
                    relations: relations.iter().cloned().map(Relation::clamped).collect(),
                    source_notes: self.resolve_sources(source_ids),
                    created_at: chrono::Utc::now().timestamp(),
                    updated_at: chrono::Utc::now().timestamp(),
                    content_hash: String::new(),
                    ..Default::default()
                };
                let summary_trimmed: String = summary.chars().take(120).collect();
                if !summary_trimmed.is_empty() {
                    note.facts.insert(0, format!("[summary] {summary_trimmed}"));
                }
                if !title.is_empty() && title != &safe {
                    note.facts.insert(0, format!("[title] {title}"));
                }
                // Per-fact provenance: every fact now carries a marker
                // (ensure_origin_marker stamped any the LLM omitted).
                note.fact_provenance = note
                    .facts
                    .iter()
                    .map(|f| crate::memory::notes::note::fact_provenance_for(f))
                    .collect();
                self.push_staged(&category, &safe, note, "create").await?;
            }
```

In the `PageOp::Append` arm (`:136-169`), destructure `source_ids`, union into `merged.source_notes`, and recompute `fact_provenance` after facts merge:

```rust
            PageOp::Append {
                note_path,
                new_facts,
                new_links,
                new_relations,
                source_ids,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename)?;
                let existing = self.load_existing_or_default(&category, &safe).await?;
                let mut merged = existing;
                for raw in new_facts {
                    let f = ensure_origin_marker(raw);
                    if !merged.facts.contains(&f) {
                        merged.facts.push(f.clone());
                    }
                }
                for l in new_links {
                    if !merged.links.contains(l) {
                        merged.links.push(l.clone());
                    }
                }
                for r in new_relations {
                    let r = r.clone().clamped();
                    if let Some(existing) = merged.relations.iter_mut().find(|e| e.to == r.to) {
                        *existing = r;
                    } else {
                        merged.relations.push(r);
                    }
                }
                for s in self.resolve_sources(source_ids) {
                    if !merged.source_notes.contains(&s) {
                        merged.source_notes.push(s);
                    }
                }
                merged.fact_provenance = merged
                    .facts
                    .iter()
                    .map(|f| crate::memory::notes::note::fact_provenance_for(f))
                    .collect();
                merged.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, merged, "append").await?;
            }
```

- [ ] **Step 6: Fix `apply.rs` test literals**

Every `PageOp::Create`/`Append` literal in `apply.rs` tests must add `source_ids: vec![]`:
- `create_op_writes_file_and_indexes`, `update_rejects_stale_hash`, `rollback_removes_staged_files`, `append_merges_without_duplicates`, `create_persists_frontmatter_relations`, `append_merges_relations_by_target`, and the proptest `op_strategy()` (`:729` Create, `:739` Append → add `source_ids: vec![]`).

- [ ] **Step 7: Thread batch ids through `try_apply` (batch.rs)**

In `batch.rs`, change `try_apply` (`:227-242`):

```rust
    async fn try_apply(
        &self,
        agent_id: &str,
        plan: &IngestPlan,
        batch_ids: &[String],
    ) -> Result<ApplyReport, ApplyError> {
        let mut tx = CompoundApplyTx::new(
            &self.indexer,
            &self.store,
            self.memory_dir.clone(),
            agent_id,
        )
        .with_batch_sources(batch_ids.to_vec());
        for op in &plan.ops {
            tx.stage(op).await?;
        }
        tx.commit().await
    }
```

In `ingest_batch`, compute the batch ids once (after `let source = raws[0].source.clone();`, `:47`):

```rust
        let batch_ids: Vec<String> = raws.iter().map(|r| r.id.clone()).collect();
```

Update both `try_apply` call sites: `:114` → `self.try_apply(agent_id, &plan, &batch_ids).await` and `:142` → `self.try_apply(agent_id, &plan2, &batch_ids).await`.

- [ ] **Step 8: Carry `source_ids` across the dedup redirect (batch.rs)**

In `dedup_redirect_creates`, the `Create`→`Append` rewrite (`:366-387`) currently drops `source_ids` via `..`. Fix:

```rust
                match (redirect.remove(&i), op) {
                    (
                        Some(target),
                        PageOp::Create {
                            note_path,
                            facts,
                            links,
                            source_ids,
                            ..
                        },
                    ) => {
                        info!(
                            from = %note_path,
                            into = %target,
                            "ingest dedup: redirecting near-duplicate Create into Append"
                        );
                        Some(PageOp::Append {
                            note_path: target,
                            new_facts: facts,
                            new_links: links,
                            new_relations: vec![],
                            source_ids,
                        })
                    }
                    (_, op) => Some(op),
                }
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p alephcore notes::ingest::apply::tests -- --nocapture`
Expected: PASS (new provenance tests + existing apply tests).

- [ ] **Step 10: Commit**

```bash
git add src/memory/notes/note/parsing.rs src/memory/notes/note/mod.rs src/memory/notes/ingest/apply.rs src/memory/notes/ingest/ingestor/batch.rs
git commit -m "memory: populate note source_notes + fact_provenance in apply (L0->L1 chain)"
```

---

## Task 3: Ingest prompt emits `source_ids` + per-fact `src:` markers

**Files:**
- Modify: `src/memory/notes/ingest/prompts.rs` (`PROMPT_COMPOUND_PLAN`)
- Modify: insta snapshot for `base_prompt_snapshot` (regenerate)
- Test: `src/memory/notes/note/mod.rs` (round-trip a src-marked fact)

**Interfaces:**
- Consumes: the raw `id=<uuid>` already rendered by `build_user_prompt` (helpers.rs:81-86).
- Produces: planning LLM output that fills `source_ids` per create/append + optional inline `<!-- src: <uuid>, origin: raw_source, inferred: false -->` on facts. No Rust signature change — prompt text only.

- [ ] **Step 1: Write the failing test (per-fact marker → RawSource)**

Add to `src/memory/notes/note/mod.rs` tests:

```rust
#[test]
fn fact_with_src_marker_parses_to_raw_source() {
    let md = "---\ncategory: preference\n---\n\n- The user prefers TypeScript. <!-- src: raw-uuid-9, origin: raw_source, inferred: false -->\n- A bare inferred fact. <!-- origin: inferred, inferred: true -->\n";
    let n = KnowledgeNote::from_markdown("typescript", md).unwrap();
    assert_eq!(n.fact_provenance.len(), 2);
    assert_eq!(n.fact_provenance[0].origin, crate::memory::notes::note::ProvenanceOrigin::RawSource);
    assert_eq!(n.fact_provenance[0].source_id.as_deref(), Some("raw-uuid-9"));
    assert!(!n.fact_provenance[0].inferred);
    assert_eq!(n.fact_provenance[1].origin, crate::memory::notes::note::ProvenanceOrigin::Inferred);
    assert!(n.fact_provenance[1].inferred);
}
```

(If `ProvenanceOrigin` is not re-exported from `note`, add `pub use parsing::…`/`types::ProvenanceOrigin` or reference the real path `crate::memory::notes::note::types::ProvenanceOrigin`.)

- [ ] **Step 2: Run test to verify it fails or passes**

Run: `cargo test -p alephcore notes::note::tests::fact_with_src_marker_parses_to_raw_source -- --exact --nocapture`
Expected: PASS already (parsing exists) — this test LOCKS the contract the prompt depends on. If it fails, fix the path/import, not the parser.

- [ ] **Step 3: Extend the prompt**

In `prompts.rs` `PROMPT_COMPOUND_PLAN`, add a new rule after rule 12 (`:53`):

```
13. PROVENANCE. Each raw memory in the input is shown as
    `### raw-N (id=<UUID>, source=...)`. For every `create` and `append`
    op, set `source_ids` to the list of `<UUID>` values whose content the
    op was distilled from. When a SINGLE fact comes verbatim from one raw,
    you MAY also append an inline marker to that fact string:
    `<!-- src: <UUID>, origin: raw_source, inferred: false -->`.
    Facts you infer or generalize need no marker (they default to inferred).
    Never invent a UUID — copy it exactly from the input.
```

Update the `create`/`append` field docs (`:57-61`):

```
- `create` — new page. Fields: `note_path` (category/filename),
  `title`, `summary` (≤120 chars), `facts[]`, `links[]` (use `[P<n>]`
  tokens), `tags[]`, `source_ids[]` (raw UUIDs this page came from).
- `append` — add facts to an existing page. Fields: `note_path`,
  `new_facts[]`, `new_links[]`, `source_ids[]`.
```

Update the two JSON examples (`:103-104`):

```json
{"kind": "create", "note_path": "preference/typescript.md", "title": "TypeScript", "summary": "User prefers TypeScript", "facts": ["The user prefers TypeScript. <!-- src: 7f3a..., origin: raw_source, inferred: false -->"], "links": ["[P3]"], "tags": ["preference"], "source_ids": ["7f3a..."]}
{"kind": "append", "note_path": "[P1]", "new_facts": ["Comments must be in English."], "new_links": [], "source_ids": ["9b2c..."]}
```

- [ ] **Step 4: Regenerate the insta snapshot**

The `base_prompt_snapshot` test will fail (snapshot mismatch). Update the snapshot file at `src/memory/notes/ingest/snapshots/…compound_plan_base_prompt.snap` to the new prompt text (or run `cargo insta accept` if available). Verify `base_prompt_mentions_every_op_kind` still passes.

Run: `cargo test -p alephcore notes::ingest::prompts::tests -- --nocapture`
Expected: PASS after snapshot update.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/ingest/prompts.rs src/memory/notes/ingest/snapshots/ src/memory/notes/note/mod.rs
git commit -m "memory: ingest prompt emits source_ids + per-fact provenance markers"
```

---

## Task 4: L1→L2 synthesis records `source_notes`

**Files:**
- Modify: `src/memory/dreaming/stages/note_synthesis.rs:96-108`
- Test: `src/memory/dreaming/stages/note_synthesis.rs` tests

**Interfaces:**
- Produces: synthesis notes whose `source_notes` = the cluster's member L1 note paths, so a synthesis (L2) note drills down to its source L1 notes via the same `notes_sources` table.

- [ ] **Step 1: Write the failing test**

Add a focused unit test that the constructed synthesis `KnowledgeNote` carries `source_notes`. If the stage has an existing test harness, assert on the produced note; otherwise add:

```rust
#[test]
fn synthesis_note_records_source_member_paths() {
    // Mirror the construction at note_synthesis.rs: a synthesis note built
    // from member paths must expose them as source_notes (provenance), not
    // only as links.
    let member_paths = vec!["learning/tokio".to_string(), "learning/async".to_string()];
    let note = KnowledgeNote {
        title: "learning Synthesis".into(),
        category: "synthesis".into(),
        tags: vec!["learning".into(), "synthesis".into()],
        facts: vec!["Synthesized insight.".into()],
        links: member_paths.clone(),
        source_notes: member_paths.clone(),
        created_at: 0,
        updated_at: 0,
        content_hash: String::new(),
        ..Default::default()
    };
    assert_eq!(note.source_notes, member_paths);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore dreaming::stages::note_synthesis::tests::synthesis_note_records_source_member_paths -- --exact --nocapture`
Expected: FAIL if `source_notes` field path differs, else compile-checks the intent. (This test mainly guards the construction shape.)

- [ ] **Step 3: Set `source_notes` in the real construction**

In `note_synthesis.rs` (the `KnowledgeNote { … }` build around `:98-108`), add `source_notes: source_links.clone(),` alongside the existing `links: source_links,`:

```rust
            let source_links: Vec<String> = note_paths.to_vec();
            let note = KnowledgeNote {
                title: format!("{category} Synthesis"),
                category: "synthesis".to_string(),
                tags: vec![category.clone(), "synthesis".to_string()],
                facts: vec![synthesis_text.clone()],
                links: source_links.clone(),
                source_notes: source_links,
                created_at: chrono::Utc::now().timestamp(),
                updated_at: chrono::Utc::now().timestamp(),
                content_hash: String::new(),
                ..Default::default()
            };
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore dreaming::stages::note_synthesis -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/note_synthesis.rs
git commit -m "memory: synthesis notes record source_notes (L1->L2 chain)"
```

---

## Task 5: L3 USER.md structured `## Sources` block

**Files:**
- Modify: `src/memory/notes/profile/types.rs` (`UserProfile.sources`)
- Modify: `src/memory/notes/profile/store.rs` (parse + render + signature)
- Modify: `src/memory/notes/profile/synthesizer.rs` (accumulate session_ids per modified section)
- Test: `store.rs` tests

**Interfaces:**
- Produces: `UserProfile.sources: BTreeMap<String, Vec<String>>` (section heading → session ids). `render_user_md(revision, last_session, confidence, sections, sources)`. On each profile update, every section in `ProfileDiff.sections_modified` gains the current `signal.session_id`. The trace tool (Task 7) reads this and bridges section → session → raws → citing notes.

> **Faithful-realization note:** at profile-update time the merge LLM sees the session *digest*, not retrieved notes — so per-section NOTE attribution is not deterministically available. We store the deterministic unit (**session_ids**) and derive notes downstream. Still explicit回指, still beats the reference's timestamp inference.

- [ ] **Step 1: Write the failing test**

Add to `store.rs` tests:

```rust
#[test]
fn sources_block_round_trips() {
    let mut sources: BTreeMap<String, Vec<String>> = BTreeMap::new();
    sources.insert("Identity".into(), vec!["ses_a".into(), "ses_b".into()]);
    sources.insert("Current Focus".into(), vec!["ses_c".into()]);

    let mut sections: BTreeMap<String, Vec<String>> = BTreeMap::new();
    sections.insert("Identity".into(), vec!["Prefers TypeScript".into()]);

    let md = render_user_md(2, "ses_c", "high", &sections, &sources);
    assert!(md.contains("## Sources"));
    let parsed = parse_user_md(&md).unwrap();
    assert_eq!(parsed.sources.get("Identity"), Some(&vec!["ses_a".to_string(), "ses_b".to_string()]));
    assert_eq!(parsed.sources.get("Current Focus"), Some(&vec!["ses_c".to_string()]));
    // The Sources heading must NOT pollute the 6 content sections.
    assert!(!parsed.sections.contains_key("Sources"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore notes::profile::store::tests::sources_block_round_trips -- --exact --nocapture`
Expected: FAIL — `UserProfile` has no `sources`; `render_user_md` arity wrong.

- [ ] **Step 3: Add the `sources` field**

In `types.rs` `UserProfile`, add:

```rust
    pub sources: std::collections::BTreeMap<String, Vec<String>>,
```

- [ ] **Step 4: Parse the `## Sources` block**

In `store.rs` `parse_user_md`, while iterating body lines, route the `Sources` heading into a separate map and exclude it from `sections`. Replace the body-section loop (`:131-145`):

```rust
    let mut sections: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut sources: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current_heading: Option<String> = None;
    let mut in_sources = false;

    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let h = heading.trim().to_string();
            in_sources = h == "Sources";
            current_heading = Some(h);
        } else if let Some(bullet) = line.strip_prefix("- ") {
            let bullet = bullet.trim();
            if in_sources {
                // Format: "<Section>: sid1, sid2"
                if let Some((sec, ids)) = bullet.split_once(": ") {
                    let list: Vec<String> = ids
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !list.is_empty() {
                        sources.insert(sec.trim().to_string(), list);
                    }
                }
            } else if let Some(ref h) = current_heading {
                sections.entry(h.clone()).or_default().push(bullet.to_string());
            }
        }
    }
```

Add `sources,` to the returned `UserProfile { … }` literal (`:147-156`).

- [ ] **Step 5: Render the `## Sources` block**

In `store.rs` `render_user_md`, add the `sources` param and emit the block after the 6 sections (`:172-203`):

```rust
pub fn render_user_md(
    revision: u32,
    last_session: &str,
    confidence: &str,
    sections: &BTreeMap<String, Vec<String>>,
    sources: &BTreeMap<String, Vec<String>>,
) -> String {
    // ... unchanged frontmatter + ProfileSection::ALL loop ...

    // provenance block (machine-readable; one line per section that has sources)
    if !sources.is_empty() {
        out.push('\n');
        out.push_str("## Sources\n");
        for (section, ids) in sources {
            if !ids.is_empty() {
                out.push_str(&format!("- {section}: {}\n", ids.join(", ")));
            }
        }
    }

    out
}
```

- [ ] **Step 6: Fix existing store.rs render call sites + struct literals**

- `render_produces_valid_md` test (`:283`): pass `&profile.sources` as the 5th arg.
- Any other `parse_user_md`-constructed `UserProfile` literal already gets `sources` from Step 4; bootstrap/other literals (grep `UserProfile {`) must add `sources: Default::default()`.

- [ ] **Step 7: Accumulate sources in synthesizer update**

In `synthesizer.rs` `update` (after `compute_diff` at `:349`, before `render_user_md` at `:351`), merge the current session into each modified section:

```rust
        let mut sources = profile.sources.clone();
        for section in &diff.sections_modified {
            let entry = sources.entry(section.clone()).or_default();
            if !entry.contains(&signal.session_id) {
                entry.push(signal.session_id.clone());
            }
        }
        let md = render_user_md(new_revision, &signal.session_id, &confidence, &sections, &sources);
```

(If the bootstrap path also renders, pass an empty `&BTreeMap::new()`.)

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p alephcore notes::profile -- --nocapture`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src/memory/notes/profile/types.rs src/memory/notes/profile/store.rs src/memory/notes/profile/synthesizer.rs
git commit -m "memory: USER.md structured Sources block (L3 session-level provenance)"
```

---

## Task 6: Provenance read APIs (NoteStore + RawMemoryStore)

**Files:**
- Modify: `src/memory/notes/store.rs` (`NoteStore` trait: `sources_of`, `notes_citing` with default impls)
- Modify: `src/memory/store/raw_memory.rs` (`RawMemoryStore` trait: `get_raws_by_ids`, `get_raws_by_session`)
- Modify: `src/memory/store/sqlite/notes/store_impl.rs` (impl `sources_of`, `notes_citing`)
- Modify: sqlite raw impl (impl raw fetch — find the file implementing `RawMemoryStore` for `SqliteMemoryBackend`)
- Test: store_impl tests

**Interfaces:**
- Produces:
  - `NoteStore::sources_of(&self, agent_id, note_path) -> Result<Vec<String>, AlephError>` (source_refs of a note)
  - `NoteStore::notes_citing(&self, agent_id, source_ref) -> Result<Vec<String>, AlephError>` (reverse: notes citing a raw/note, via `idx_notes_sources_ref`)
  - `RawMemoryStore::get_raws_by_ids(&self, agent_id, ids: &[String]) -> Result<Vec<RawMemory>, AlephError>`
  - `RawMemoryStore::get_raws_by_session(&self, agent_id, session_id) -> Result<Vec<RawMemory>, AlephError>`
- Consumed by Task 7 (`memory_trace`).

- [ ] **Step 1: Write the failing test**

Add to `store_impl.rs` tests:

```rust
#[tokio::test]
async fn sources_of_and_notes_citing_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let backend = SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap();
    let note = KnowledgeNote {
        title: "typescript".into(),
        category: "preference".into(),
        facts: vec!["prefers ts".into()],
        source_notes: vec!["raw-1".into(), "raw-2".into()],
        ..Default::default()
    };
    backend.index_note(&note, "default", "preference").await.unwrap();

    let mut srcs = backend.sources_of("default", "preference/typescript").await.unwrap();
    srcs.sort();
    assert_eq!(srcs, vec!["raw-1".to_string(), "raw-2".to_string()]);

    let citing = backend.notes_citing("default", "raw-1").await.unwrap();
    assert_eq!(citing, vec!["preference/typescript".to_string()]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore store::sqlite::notes::store_impl::tests::sources_of_and_notes_citing_roundtrip -- --exact --nocapture`
Expected: FAIL — methods missing.

- [ ] **Step 3: Add trait methods (default no-op impls)**

In `store.rs` `NoteStore`, near `get_provenance` (`:360`):

```rust
    /// Source refs (raw-memory ids or prior-note paths) a note was distilled
    /// from — forward provenance. Default: empty.
    async fn sources_of(&self, _agent_id: &str, _note_path: &str) -> Result<Vec<String>, AlephError> {
        Ok(Vec::new())
    }

    /// Reverse provenance: note paths that cite a given source ref (raw id or
    /// note path). Backed by `idx_notes_sources_ref`. Default: empty.
    async fn notes_citing(&self, _agent_id: &str, _source_ref: &str) -> Result<Vec<String>, AlephError> {
        Ok(Vec::new())
    }
```

- [ ] **Step 4: Implement on the sqlite backend**

In `store_impl.rs` (the `impl NoteStore for SqliteMemoryBackend` block), add:

```rust
    async fn sources_of(&self, agent_id: &str, note_path: &str) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn.prepare(
            "SELECT source_ref FROM notes_sources WHERE agent_id = ?1 AND note_path = ?2",
        )?;
        let rows = stmt.query_map(params![agent_id, note_path], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows { out.push(r?); }
        Ok(out)
    }

    async fn notes_citing(&self, agent_id: &str, source_ref: &str) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn.prepare(
            "SELECT note_path FROM notes_sources WHERE agent_id = ?1 AND source_ref = ?2",
        )?;
        let rows = stmt.query_map(params![agent_id, source_ref], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows { out.push(r?); }
        Ok(out)
    }
```

(Match the file's existing error-mapping idiom — copy the `.map_err(|e| AlephError::config(...))?` pattern used by neighboring methods if `?` on rusqlite errors does not auto-convert.)

- [ ] **Step 5: Add RawMemoryStore fetch methods**

In `raw_memory.rs` `RawMemoryStore` trait, add (default empty):

```rust
    async fn get_raws_by_ids(&self, _agent_id: &str, _ids: &[String]) -> Result<Vec<RawMemory>, AlephError> {
        Ok(Vec::new())
    }
    async fn get_raws_by_session(&self, _agent_id: &str, _session_id: &str) -> Result<Vec<RawMemory>, AlephError> {
        Ok(Vec::new())
    }
```

Implement them on the sqlite backend (find the `impl RawMemoryStore for SqliteMemoryBackend` — likely `src/memory/store/sqlite/raw_memory.rs` or within `store_impl`). Mirror the existing `get_unprocessed_raw_memories` row-mapping; SQL:

```sql
SELECT id, content, source, source_detail, agent_id, session_id, path, layer, attachment_text, is_processed, created_at
FROM raw_memories WHERE agent_id = ?1 AND id IN (...)      -- get_raws_by_ids (build the IN list)
SELECT ... FROM raw_memories WHERE agent_id = ?1 AND session_id = ?2 ORDER BY created_at ASC  -- by_session
```

Reuse the existing `RawMemory` row-deserialization helper that `get_unprocessed_raw_memories` already uses (do not hand-roll a second mapper).

- [ ] **Step 6: Test raw fetch**

```rust
#[tokio::test]
async fn raw_fetch_by_id_and_session() {
    let dir = tempfile::tempdir().unwrap();
    let backend = SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap();
    let mut r = RawMemory::new("hello".into(), RawMemorySource::Transcript);
    r.agent_id = "default".into();
    r.session_id = Some("ses_x".into());
    backend.insert_raw_memory(&r).await.unwrap(); // use the real insert API

    let by_id = backend.get_raws_by_ids("default", &[r.id.clone()]).await.unwrap();
    assert_eq!(by_id.len(), 1);
    let by_ses = backend.get_raws_by_session("default", "ses_x").await.unwrap();
    assert_eq!(by_ses.len(), 1);
}
```

(Use the project's actual raw-insert method name — confirm from `raw_memory.rs`.)

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore store::sqlite -- --nocapture provenance` and the two new test names.
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/memory/notes/store.rs src/memory/store/raw_memory.rs src/memory/store/sqlite/
git commit -m "memory: provenance read APIs (sources_of/notes_citing + raw fetch by id/session)"
```

---

## Task 7: `memory_trace` builtin tool

**Files:**
- Create: `src/builtin_tools/memory_trace.rs`
- Modify: `src/builtin_tools/mod.rs` (`pub mod memory_trace;`)
- Modify: `src/executor/builtin_registry/builder/constructor/mod.rs` (inject `memory_trace_db`)
- Modify: `src/executor/builtin_registry/registry/tool_registry_impl.rs` (dispatch arm)
- Test: `src/builtin_tools/memory_trace.rs` tests

**Interfaces:**
- Consumes: Task 6 read APIs + Task 5 `UserProfile.sources` + existing `get_provenance`.
- Produces: tool `memory_trace` with args `{ target: String, kind: "note" | "raw" | "profile_section", max_depth?: usize }` returning a `TraceResult` chain. Missing raws degrade to a `pruned: true` node, never an error.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn trace_note_to_raw_and_graceful_prune() {
    let dir = tempfile::tempdir().unwrap();
    let backend = MemoryBackend::new(Arc::new(SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap()));
    // Note citing raw-present and raw-missing.
    let note = KnowledgeNote {
        title: "typescript".into(), category: "preference".into(),
        facts: vec!["prefers ts".into()],
        source_notes: vec!["raw-present".into(), "raw-missing".into()],
        ..Default::default()
    };
    backend.index_note(&note, "default", "preference").await.unwrap();
    let mut r = RawMemory::new("user: I prefer TypeScript".into(), RawMemorySource::Transcript);
    r.id = "raw-present".into(); r.agent_id = "default".into();
    backend.insert_raw_memory(&r).await.unwrap();

    let tool = MemoryTraceTool::new(backend, "default");
    let out = tool.call_impl(MemoryTraceArgs {
        target: "preference/typescript".into(),
        kind: TraceKind::Note,
        max_depth: None,
    }).await.unwrap();

    // One raw resolved with content, one pruned.
    let raws: Vec<_> = out.evidence.iter().collect();
    assert!(raws.iter().any(|e| e.raw_id == "raw-present" && e.content.is_some()));
    assert!(raws.iter().any(|e| e.raw_id == "raw-missing" && e.pruned));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore builtin_tools::memory_trace -- --nocapture`
Expected: FAIL — module/types missing.

- [ ] **Step 3: Implement the tool**

Create `src/builtin_tools/memory_trace.rs` (mirror `recall_context.rs` structure):

```rust
//! `memory_trace` — drill a high-level memory claim down to ground-truth
//! evidence: profile section → notes → raw memories → transcript text.
use crate::error::AlephError;
use crate::memory::store::MemoryBackend;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind { Note, Raw, ProfileSection }

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryTraceArgs {
    /// What to trace: a note path (`category/name`), a raw id, or a USER.md section heading.
    pub target: String,
    pub kind: TraceKind,
    #[serde(default)]
    pub max_depth: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceItem {
    pub raw_id: String,
    pub via_note: Option<String>,
    pub via_session: Option<String>,
    pub content: Option<String>, // None when pruned
    pub pruned: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceResult {
    pub target: String,
    pub notes: Vec<String>,
    pub evidence: Vec<EvidenceItem>,
}

pub struct MemoryTraceTool { db: MemoryBackend, agent_id: String }

impl MemoryTraceTool {
    pub const NAME: &'static str = "memory_trace";
    pub const DESCRIPTION: &'static str =
        "Drill a memory claim down to ground-truth evidence: profile section / note / raw id \
         → source notes → raw memories → original transcript text. Returns the evidence chain; \
         pruned raws are marked rather than erroring.";

    pub fn new(db: MemoryBackend, agent_id: impl Into<String>) -> Self {
        Self { db, agent_id: agent_id.into() }
    }

    pub async fn call_impl(&self, args: MemoryTraceArgs) -> anyhow::Result<TraceResult> {
        let agent = &self.agent_id;
        // 1. Resolve the set of (note, raw_ids) to inspect.
        let notes: Vec<String> = match args.kind {
            TraceKind::Note => vec![args.target.clone()],
            TraceKind::Raw => self.db.notes_citing(agent, &args.target).await?,
            TraceKind::ProfileSection => {
                // section → session_ids (USER.md Sources) → raws → citing notes
                let profile = crate::memory::notes::profile::ProfileStore::new(
                    self.db.note_dir_for(agent), // helper or compute agent note dir
                ).read().await.ok().flatten();
                let mut notes = Vec::new();
                if let Some(p) = profile {
                    if let Some(sessions) = p.sources.get(&args.target) {
                        for sid in sessions {
                            for raw in self.db.get_raws_by_session(agent, sid).await? {
                                for n in self.db.notes_citing(agent, &raw.id).await? {
                                    if !notes.contains(&n) { notes.push(n); }
                                }
                            }
                        }
                    }
                }
                notes
            }
        };

        // 2. Each note → its source raw ids → fetch raw content (graceful prune).
        let mut evidence = Vec::new();
        for note in &notes {
            let raw_ids = self.db.sources_of(agent, note).await?;
            let fetched = self.db.get_raws_by_ids(agent, &raw_ids).await?;
            for rid in &raw_ids {
                let found = fetched.iter().find(|r| &r.id == rid);
                evidence.push(EvidenceItem {
                    raw_id: rid.clone(),
                    via_note: Some(note.clone()),
                    via_session: found.and_then(|r| r.session_id.clone()),
                    content: found.map(|r| r.content.chars().take(800).collect()),
                    pruned: found.is_none(),
                });
            }
        }

        // Raw-kind: also surface the raw itself if present.
        if args.kind == TraceKind::Raw {
            let fetched = self.db.get_raws_by_ids(agent, &[args.target.clone()]).await?;
            if let Some(r) = fetched.first() {
                evidence.push(EvidenceItem {
                    raw_id: r.id.clone(), via_note: None, via_session: r.session_id.clone(),
                    content: Some(r.content.chars().take(800).collect()), pruned: false,
                });
            }
        }

        Ok(TraceResult { target: args.target, notes, evidence })
    }
}
```

> Resolve the agent note-dir the same way `recall_context` / the profile synthesizer obtains it (do NOT invent `note_dir_for` if a real accessor exists — reuse it). If `MemoryBackend` does not expose the note dir, pass it into `MemoryTraceTool::new` from the constructor like the synthesizer does.

Add `pub mod memory_trace;` to `src/builtin_tools/mod.rs`.

- [ ] **Step 4: Register + dispatch**

In `constructor/mod.rs` near `recall_context_db` (`:1090`): `memory_trace_db: config.memory_db.clone(),` (add the field to the registry struct too, mirroring `recall_context_db`).

In `tool_registry_impl.rs`, add an arm next to `"recall_context" =>` (`:1289`):

```rust
"memory_trace" => {
    Box::pin(async move {
        let db = self.memory_trace_db.as_ref().ok_or_else(|| {
            AlephError::tool("memory_trace not available: no memory backend configured")
        })?;
        let args: crate::builtin_tools::memory_trace::MemoryTraceArgs =
            serde_json::from_value(arguments)
                .map_err(|e| AlephError::tool(format!("memory_trace: bad args: {e}")))?;
        let tool = crate::builtin_tools::memory_trace::MemoryTraceTool::new(
            db.clone(), agent_id_for_this_call, // use same agent-resolution as neighboring arms
        );
        let out = tool.call_impl(args).await
            .map_err(|e| AlephError::tool(format!("memory_trace: {e}")))?;
        serde_json::to_value(out)
            .map_err(|e| AlephError::tool(format!("memory_trace: serialize: {e}")))
    })
}
```

Register the tool name in the catalog (`groups.rs` / `definitions.rs`) next to `recall_context` so it is offered to the LLM.

- [ ] **Step 5: Run test**

Run: `cargo test -p alephcore builtin_tools::memory_trace -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/memory_trace.rs src/builtin_tools/mod.rs src/executor/builtin_registry/
git commit -m "memory: memory_trace builtin tool walks the evidence chain"
```

---

## Task 8: `memory.trace` read-only gateway RPC

**Files:**
- Modify: `src/gateway/handlers/memory.rs` (`handle_trace`)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/memory.rs` (register)
- Test: `src/gateway/handlers/memory.rs` tests

**Interfaces:**
- Produces: read-only RPC `memory.trace` with params `{ agent_id, target, kind, max_depth? }` returning the same `TraceResult` JSON. Mirrors `handle_list_corrections`. R4-compliant: forwards to `MemoryTraceTool::call_impl`, no business logic.

- [ ] **Step 1: Write the failing test**

Mirror the existing `handle_list_corrections` test in `memory.rs`; assert `handle_trace` returns a JSON-RPC result with a `notes`/`evidence` payload for a seeded note. (Copy the test harness used by the corrections handler test.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore gateway::handlers::memory -- --nocapture trace`
Expected: FAIL — `handle_trace` missing.

- [ ] **Step 3: Implement the handler**

In `memory.rs`, next to `handle_list_corrections` (`:430`):

```rust
pub async fn handle_trace(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params {
        agent_id: Option<String>,
        target: String,
        kind: crate::builtin_tools::memory_trace::TraceKind,
        #[serde(default)]
        max_depth: Option<usize>,
    }
    let params: Params = match request.params_as() { // use the file's param-extraction idiom
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(request.id, -32602, &format!("invalid params: {e}")),
    };
    let agent = params.agent_id.unwrap_or_else(|| "default".to_string());
    let tool = crate::builtin_tools::memory_trace::MemoryTraceTool::new(db, agent);
    match tool.call_impl(crate::builtin_tools::memory_trace::MemoryTraceArgs {
        target: params.target, kind: params.kind, max_depth: params.max_depth,
    }).await {
        Ok(res) => JsonRpcResponse::success(request.id, serde_json::to_value(res).unwrap_or_default()),
        Err(e) => JsonRpcResponse::error(request.id, -32000, &format!("memory.trace: {e}")),
    }
}
```

(Adapt `JsonRpcResponse::success/error` + `request.params_as` to the exact helpers used by `handle_list_corrections`.)

- [ ] **Step 4: Register the method**

In `builder/handlers/memory.rs` near `memory.list_corrections` (`:80`):

```rust
register_handler!(server, "memory.trace", memory_handlers::handle_trace, memory_db);
```

- [ ] **Step 5: Run test**

Run: `cargo test -p alephcore gateway::handlers::memory -- --nocapture trace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/memory.rs src/bin/aleph-server/commands/start/builder/handlers/memory.rs
git commit -m "memory: read-only memory.trace gateway RPC (panel-ready)"
```

---

## Task 9: Docs — chain + retention invariant

**Files:**
- Modify: `docs/reference/memory/RAW_MEMORY.md` (retention invariant)
- Modify: `docs/reference/MEMORY_SYSTEM.md` (provenance chain section)
- Modify: `docs/reference/FEATURE_LOCATOR.md` (locator entries)

**Interfaces:** none (docs only).

- [ ] **Step 1: RAW_MEMORY.md invariant**

Add a section stating: *Any future `raw_memories` retention/GC sweep MUST exclude rows referenced by `notes_sources` / `notes_provenance` (query via `notes_citing(raw_id)` non-empty). Today no time-based sweep exists; this is the pin to honor when one is added.*

- [ ] **Step 2: MEMORY_SYSTEM.md chain section**

Document the wired evidence chain L3 (`USER.md ## Sources`, session-level) → L2 (`synthesis.source_notes`) → L1 (`source_notes` + `fact_provenance`) → L0 (`raw_memories`), and the `memory_trace` tool + `memory.trace` RPC as the consumer.

- [ ] **Step 3: FEATURE_LOCATOR.md entries**

Add a Context-layer locator row: `记忆溯源 / evidence chain / 下钻 / drill-down → Memory Provenance Chain → apply.rs source_notes + notes_sources/notes_provenance + memory_trace + memory.trace`.

- [ ] **Step 4: Commit**

```bash
git add -f docs/reference/memory/RAW_MEMORY.md docs/reference/MEMORY_SYSTEM.md docs/reference/FEATURE_LOCATOR.md
git commit -m "docs: memory evidence-chain + raw retention pin invariant"
```

---

## Final verification (one allowed full check)

- [ ] **Single compile gate (the only `cargo check` of the plan):**

Run: `cargo check -p alephcore --lib`
Expected: clean. Fix any fallout (most likely: a missed `source_ids: vec![]` literal, a missed `render_user_md` call site, or a `UserProfile { .. }` literal missing `sources`).

- [ ] **North-star integration test (in `src/memory/` or a tests module):**

Write one test that exercises the full chain: insert a raw "user: I prefer TypeScript" with a session id → run `ingest_batch` (or directly stage a Create with `source_ids`) to produce `preference/typescript` → simulate a profile update touching "Identity" with that session → `memory_trace(kind=ProfileSection, target="Identity")` returns evidence whose `content` contains "TypeScript". This is the spec's acceptance scenario (§7.5).

- [ ] **Commit the integration test:**

```bash
git add -A
git commit -m "memory: north-star integration test for L3->L0 evidence chain"
```

---

## Self-Review (completed by plan author)

- **Spec coverage:** §4① → Tasks 1-3; §4② → Task 4; §4③ → Task 5; §4④ → Tasks 6-8; §4⑤ → Tasks 7 (graceful prune) + 9 (invariant); §5 熵减 (keep direct path) → honored (no event-handler reroute); §7 verification → per-task tests + final integration test. No gaps.
- **Placeholder scan:** no TBD/TODO; every code step shows real code. Two spots explicitly defer to "the file's existing idiom" (rusqlite error mapping, JSON-RPC helpers, agent-resolution in dispatch) — these are *match-surrounding-code* instructions, not placeholders, because the exact helper names vary and must mirror neighbors.
- **Type consistency:** `source_ids: Vec<String>` (Tasks 1-3); `with_batch_sources`/`resolve_sources` (Task 2); `sources_of`/`notes_citing`/`get_raws_by_ids`/`get_raws_by_session` (Task 6) consumed unchanged by Task 7; `render_user_md(..., sources)` arity consistent across Tasks 5 call sites; `MemoryTraceArgs`/`TraceKind`/`TraceResult` identical in Tasks 7-8.
