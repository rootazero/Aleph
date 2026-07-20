# Aleph Note Layer — Phase R2: `fact` → `note` Naming Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename residual `fact`-named symbols (event commands, MemoryEvent variants, payload fields, frontmatter `source_facts`, tracing fields, doc strings) to `note`-named equivalents while preserving forward-compatible event-log deserialization via `#[serde(alias)]`. Keep `KnowledgeNote.facts: Vec<String>` (the page-level "claims" abstraction) intact — it is the carrier for paragraph-level provenance and the LLM-wiki "page contains claims" two-layer model.

**Architecture:** Compile-time renames in `src/memory/events/{commands.rs, types.rs, handler.rs, projector.rs}`; serde aliases on `MemoryEvent` enum and on payload fields; frontmatter `source_facts` → `source_notes` with `#[serde(alias = "source_facts")]`; sweep tracing/log/doc strings; update `docs/reference/memory/NOTES.md` §12. No SQLite schema change.

**Tech Stack:** Rust 2021, serde, serde_json, tracing, regex (test fixtures only).

**Spec:** `docs/superpowers/specs/2026-05-03-aleph-note-layer-llm-wiki-optimization-design.md` §5 (Phase R2). R2 ships AFTER C2 because of C2.11 (no new MemoryEvent variants in C2). If a future spec adds NoteSuperseded / NoteReviewApproved events, R2 must land first.

**Verification gate:** Old envelope JSON fixture (`{"FactCreated": {"fact_id": "x"}}`) deserializes as `NoteCreated { note_path: "x", ... }`; new writes produce only `NoteCreated`; legacy `source_facts:` markdown round-trips to `source_notes:`; full `cargo test -p alephcore --lib` green; `rg 'fact_id =' src/` returns zero hits.

---

## Task 1 (R2.2): MemoryEvent enum variants + serde aliases

**Files:**
- Modify: `src/memory/events/types.rs` (`MemoryEvent` enum)
- All call sites: `src/memory/events/{commands.rs, handler.rs, projector.rs}` and any tests referencing `MemoryEvent::Fact*`

- [ ] **Step 1: Write the legacy-deserialize fixture test**

Add to `src/memory/events/types.rs` `mod tests`:

```rust
#[test]
fn legacy_envelope_with_fact_created_deserializes_as_note_created() {
    let json = r#"{"NoteCreated":{"fact_id":"reference/rust","content":"hello"}}"#;
    let parsed: MemoryEvent = serde_json::from_str(json).expect("must parse new shape");
    match parsed {
        MemoryEvent::NoteCreated { note_path, content, .. } => {
            assert_eq!(note_path, "reference/rust");
            assert_eq!(content, "hello");
        }
        _ => panic!("expected NoteCreated"),
    }
}

#[test]
fn legacy_envelope_with_old_variant_name_deserializes_via_alias() {
    let json = r#"{"FactCreated":{"fact_id":"reference/rust","content":"hello"}}"#;
    let parsed: MemoryEvent = serde_json::from_str(json).expect("alias must let old name through");
    match parsed {
        MemoryEvent::NoteCreated { note_path, content, .. } => {
            assert_eq!(note_path, "reference/rust");
            assert_eq!(content, "hello");
        }
        _ => panic!("expected NoteCreated via alias"),
    }
}

#[test]
fn writes_only_note_created_name() {
    let ev = MemoryEvent::NoteCreated { note_path: "x".into(), content: "y".into() };
    let json = serde_json::to_string(&ev).unwrap();
    assert!(json.contains("NoteCreated"));
    assert!(!json.contains("FactCreated"));
    assert!(json.contains("note_path"));
    assert!(!json.contains("fact_id"));
}
```

- [ ] **Step 2: Run tests — should fail**

```bash
cargo test -p alephcore --lib memory::events::types::tests::legacy_envelope_with_fact_created_deserializes_as_note_created memory::events::types::tests::legacy_envelope_with_old_variant_name_deserializes_via_alias memory::events::types::tests::writes_only_note_created_name
```
Expected: fail to compile (`NoteCreated` variant missing).

- [ ] **Step 3: Rename enum variants and add aliases**

In `src/memory/events/types.rs`, replace the `MemoryEvent` enum body:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum MemoryEvent {
    #[serde(rename = "NoteCreated", alias = "FactCreated")]
    NoteCreated {
        #[serde(alias = "fact_id")] note_path: String,
        content: String,
    },

    #[serde(rename = "NoteContentUpdated", alias = "FactContentUpdated")]
    NoteContentUpdated {
        #[serde(alias = "fact_id")] note_path: String,
        old_content: String,
        new_content: String,
        reason: String,
    },

    #[serde(rename = "NoteInvalidated", alias = "FactInvalidated")]
    NoteInvalidated {
        #[serde(alias = "fact_id")] note_path: String,
        reason: String,
    },

    #[serde(rename = "NoteRestored", alias = "FactRestored")]
    NoteRestored {
        #[serde(alias = "fact_id")] note_path: String,
        new_strength: f32,
    },

    #[serde(rename = "NoteAccessed", alias = "FactAccessed")]
    NoteAccessed {
        #[serde(alias = "fact_id")] note_path: String,
        query: String,
        relevance_score: f32,
        used_in_response: bool,
        new_access_count: u32,
    },

    #[serde(rename = "NoteConsolidated", alias = "FactConsolidated")]
    NoteConsolidated {
        #[serde(alias = "source_fact_ids")] source_note_paths: Vec<String>,
        consolidated_content: String,
    },

    #[serde(rename = "NoteDeleted", alias = "FactDeleted")]
    NoteDeleted {
        #[serde(alias = "fact_id")] note_path: String,
        reason: String,
    },
}
```

- [ ] **Step 4: Update every match site**

Run:

```bash
rg -n "MemoryEvent::Fact" --no-heading src/
```

For every match, rename the variant. Pattern matching is the only consumer that breaks at compile time — work through the list end-to-end. Common files to update:
- `src/memory/events/handler.rs`
- `src/memory/events/projector.rs`
- `src/memory/events/commands.rs`
- Tests in those files

For `EventProjector::fold_events_to_fact`, rename to `fold_events_to_note` simultaneously (Task 2 expands this).

- [ ] **Step 5: Run all events tests**

```bash
cargo test -p alephcore --lib memory::events
```
Expected: green; the three new fixtures pass.

- [ ] **Step 6: Commit**

```bash
git add src/memory/events/types.rs src/memory/events/handler.rs src/memory/events/projector.rs src/memory/events/commands.rs
git commit -m "refactor(events): rename MemoryEvent::Fact* to Note* with serde alias for legacy"
```

---

## Task 2 (R2.1): Rename command structs and projector function

**Files:**
- Modify: `src/memory/events/commands.rs` (rename five `*FactCommand` structs)
- Modify: `src/memory/events/projector.rs` (`fold_events_to_fact` → `fold_events_to_note`)
- Modify: every caller (use `rg` to find them)

- [ ] **Step 1: Rename structs**

In `src/memory/events/commands.rs`, change:

| Old | New |
|---|---|
| `CreateFactCommand` | `CreateNoteCommand` |
| `InvalidateFactCommand` | `InvalidateNoteCommand` |
| `RestoreFactCommand` | `RestoreNoteCommand` |
| `RecordAccessCommand` | `RecordNoteAccessCommand` |
| `DeleteFactCommand` | `DeleteNoteCommand` |

For each renamed struct, also rename the `fact_id: String` field to `note_path: String`. Update the `execute` / `apply` impls correspondingly.

`UpdateContentCommand` and `ConsolidateCommand` already have neutral names — no rename, but they still hold a `fact_id` field that becomes `note_path`.

- [ ] **Step 2: Rename projector**

In `src/memory/events/projector.rs`:

```rust
impl EventProjector {
    /// Fold all events for a given note path into a current projection.
    pub fn fold_events_to_note(events: &[MemoryEventEnvelope]) -> Option<NoteProjection> {
        // ...existing body, with `fact_id` → `note_path` everywhere internally...
    }
}
```

If `FactProjection` exists as a type, rename to `NoteProjection`.

- [ ] **Step 3: Update callers**

```bash
rg -n "CreateFactCommand|InvalidateFactCommand|RestoreFactCommand|RecordAccessCommand|DeleteFactCommand|fold_events_to_fact|FactProjection" --no-heading src/
```

For every hit, rewrite to the new name. Compilation surfaces every site automatically.

- [ ] **Step 4: Run full lib test set**

```bash
cargo test -p alephcore --lib
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/events/ src/
git commit -m "refactor(events): rename *FactCommand structs and fold_events_to_fact"
```

(Use `git add src/` if changes spread across many files.)

---

## Task 3 (R2.4): Frontmatter `source_facts` → `source_notes`

**Files:**
- Modify: `src/memory/notes/note.rs:43, 78-80, 159-166`

- [ ] **Step 1: Write failing test**

Add to `src/memory/notes/note.rs` `mod tests`:

```rust
#[test]
fn legacy_source_facts_yaml_round_trips_to_source_notes() {
    let md = "---\ncategory: skill\ntags: []\nsource_facts: [synthesis/x]\n---\n\n- fact\n";
    let n = KnowledgeNote::from_markdown("legacy", md).unwrap();
    assert_eq!(n.source_notes, vec!["synthesis/x".to_string()]);

    let md_out = n.to_markdown();
    assert!(md_out.contains("source_notes: [synthesis/x]"));
    assert!(!md_out.contains("source_facts:"));
}

#[test]
fn new_source_notes_field_round_trips() {
    let n = KnowledgeNote {
        title: "x".into(),
        category: "preference".into(),
        facts: vec!["body".into()],
        source_notes: vec!["synthesis/y".into()],
        ..Default::default()
    };
    let md = n.to_markdown();
    let parsed = KnowledgeNote::from_markdown("x", &md).unwrap();
    assert_eq!(parsed.source_notes, vec!["synthesis/y".to_string()]);
}
```

- [ ] **Step 2: Run tests — should fail**

Expected: compile error (`source_notes` field missing).

- [ ] **Step 3: Rename field with serde alias**

In `Frontmatter`:

```rust
#[serde(default, alias = "source_facts")] source_notes: Vec<String>,
```

In `KnowledgeNote`:

```rust
pub source_notes: Vec<String>,
```

In `Default for KnowledgeNote`:

```rust
source_notes: Vec::new(),
```

In `from_markdown`:

```rust
source_notes: frontmatter.source_notes,
```

In `to_markdown` (replace the `source_facts:` line):

```rust
out.push_str(&format!("source_notes: {}\n", yaml_inline_array(&self.source_notes)));
```

- [ ] **Step 4: Update existing callers of `note.source_facts`**

```bash
rg -n "\.source_facts\b" --no-heading src/
```

Rewrite each to `.source_notes`. Compilation catches everything.

- [ ] **Step 5: Run tests**

```bash
cargo test -p alephcore --lib memory::notes::note
```
Expected: green; legacy markdown still parses; writer emits `source_notes:` only.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/note.rs src/
git commit -m "refactor(notes): rename source_facts to source_notes with serde alias for legacy"
```

---

## Task 4 (R2.5): Tracing / log / doc string sweep

**Files:** every file matching `rg -l 'fact_id|FactProjection|memory.fact|Fact[A-Z]'` under `src/`

- [ ] **Step 1: Sweep tracing field names**

```bash
rg -n "fact_id\s*=" --no-heading src/
```

For every hit, rewrite the field key to `note_path`. Example:

```rust
// before
tracing::info!(fact_id = %p, "memory event recorded");
// after
tracing::info!(note_path = %p, "memory event recorded");
```

- [ ] **Step 2: Sweep tracing target strings**

```bash
rg -n 'event!\(target\s*=\s*"memory\.fact' --no-heading src/
```

Replace `"memory.fact"` with `"memory.note"`.

- [ ] **Step 3: Sweep module / function doc comments**

```bash
rg -n "^//.*\bfact\b" --no-heading src/memory/events/ src/memory/notes/
```

Review each line. If the comment is referring to the event-sourcing entity (now "note"), rewrite. If referring to within-note bullets/claims (the `Vec<String>` named `facts`), leave intact — that abstraction layer is preserved per spec §5.0.

- [ ] **Step 4: Verify zero remaining tracing hits**

```bash
rg -n 'fact_id\s*=' --no-heading src/
```
Expected: no output.

```bash
rg -n '"memory\.fact' --no-heading src/
```
Expected: no output.

- [ ] **Step 5: Run full lib build**

```bash
cargo build -p alephcore && cargo test -p alephcore --lib
```
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/
git commit -m "refactor: sweep fact_id and memory.fact tracing fields to note_path / memory.note"
```

---

## Task 5 (R2.5.2): Update `docs/reference/memory/NOTES.md` §12

**Files:**
- Modify: `docs/reference/memory/NOTES.md` §12 (Event Sourcing)

- [ ] **Step 1: Read current §12**

```bash
sed -n '/^## 12\. Event Sourcing/,/^## 13/p' docs/reference/memory/NOTES.md
```

- [ ] **Step 2: Rewrite §12**

Replace the section body with text like:

```markdown
## 12. Event Sourcing

Commands in `src/memory/events/commands.rs`:

- `CreateNoteCommand` — emits `MemoryEvent::NoteCreated` at seq 1.
- `UpdateContentCommand` — rebuilds current content via `EventProjector::fold_events_to_note`, then emits `NoteContentUpdated { old_content, new_content, reason }`.
- `InvalidateNoteCommand` — soft delete; emits `NoteInvalidated { reason }`.
- `RestoreNoteCommand` — revives an invalidated note; emits `NoteRestored { new_strength }`.
- `RecordNoteAccessCommand` — emits `NoteAccessed { query, relevance_score, used_in_response, new_access_count }` with `EventActor::Agent`.
- `ConsolidateCommand` — emits `NoteConsolidated { source_note_paths, consolidated_content }`.
- `DeleteNoteCommand` — hard delete; emits `NoteDeleted { reason }`.

Pre-Phase-R2 events written with the legacy `Fact*` variant names and the
`fact_id` payload field still deserialize correctly because every variant
carries `#[serde(alias = "Fact...")]` and every `note_path` field carries
`#[serde(alias = "fact_id")]`. Writes only emit the new names.

(Continue with the rest of §12 as previously written, with `Fact*` → `Note*`
applied throughout.)
```

- [ ] **Step 3: Spell-check the document**

Skim from §10 (Skills as Notes) onward; verify cross-references still parse.

- [ ] **Step 4: Commit**

```bash
git add docs/reference/memory/NOTES.md
git commit -m "docs: update NOTES.md §12 to reflect Note* event names with serde alias"
```

---

## Task 6 (Phase R2 verification gate)

**Files:** none (verification only)

- [ ] **Step 1: Verify legacy event JSON deserializes**

```bash
cargo test -p alephcore --lib memory::events::types::tests::legacy_envelope_with_old_variant_name_deserializes_via_alias memory::events::types::tests::writes_only_note_created_name
```
Expected: green.

- [ ] **Step 2: Verify legacy markdown source_facts round-trips**

```bash
cargo test -p alephcore --lib memory::notes::note::tests::legacy_source_facts_yaml_round_trips_to_source_notes
```
Expected: green.

- [ ] **Step 3: Verify tracing field sweep is complete**

```bash
rg -n 'fact_id\s*=' --no-heading src/
```
Expected: no output.

- [ ] **Step 4: Full regression**

```bash
cargo test -p alephcore --lib
cargo test -p alephcore --tests
```
Expected: all green; A, B, C2 all still pass.

- [ ] **Step 5: Verify writes only emit new names against a real ingest**

Run a quick integration:

```bash
cargo run --bin aleph-server -- start &
SERVER_PID=$!
# (wait a few seconds, send a note_manage create via HTTP / IPC, then stop)
kill $SERVER_PID
sqlite3 ~/.aleph/data/aleph.sqlite \
  "SELECT event FROM memory_events ORDER BY id DESC LIMIT 1;"
```

Expected: the JSON envelope contains `NoteCreated` and `note_path`, not `FactCreated` or `fact_id`.

- [ ] **Step 6: Tag the phase**

```bash
git tag note-layer-phase-r2-complete
```

Phase R2 done. The whole 4-phase sequence (A → B → C2 → R2) is complete.
