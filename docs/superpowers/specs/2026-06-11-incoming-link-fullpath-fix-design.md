# Incoming-Link Detection Full-Path Fix — Design

**Date:** 2026-06-11
**Status:** Approved
**Scope:** `src/memory/dreaming/stages/note_weave.rs`, `note_decay.rs`, `src/memory/notes/store.rs`, `src/memory/store/sqlite/notes.rs`

## Problem

`notes_links.to_note` stores **resolved full paths** (e.g. `entity/dreame`), verified
against the live `memory.db` — all rows have `to_note LIKE '%/%'`. The `to_raw`
column holds the bare wikilink text; `to_note` is what `resolve_target` produced
(full path when the filename uniquely resolves).

Two dream stages query incoming links by **bare filename** instead:

- `note_weave.rs:71` — orphan detection: `get_incoming_links(filename, …)`
- `note_decay.rs:166` — protection rule: `get_incoming_links(filename, …)`

Both run `WHERE to_note = 'dreame'`, which **never matches** a full-path
`to_note`. Three inline comments assert the false premise that "notes_links
stores raw wikilink targets by filename" (`note_weave.rs:66-67`,
`note_decay.rs:152-154`).

`gateway/handlers/graph.rs:282` already queries incoming by full path
(`&params.node_id`) and is correct — the panel canvas is unaffected. The bug is
localized to the two dream stages.

### Consequences

1. **NoteWeave waste + mis-classification.** A note with only *incoming* links
   (a link target with no outgoing links) is mis-detected as an orphan because
   its incoming count reads 0. Every dream cycle re-includes it in the batched
   LLM keyword-extraction call and re-pairs it. With no keyword-overlap partner
   it churns every cycle forever. (The six currently-linked live notes are
   masked only because NoteWeave writes **bidirectional** links, so the
   *outgoing* half of the orphan test catches both endpoints.)

2. **NoteDecay latent correctness bug.** `incoming_count` is always 0, so the
   "≥3 incoming links → protected" rule never fires and `link_weight` is always
   0. Heavily-referenced notes get low scores and are archived early — exactly
   the "vicious cycle" NoteWeave's doc comment claims to break, except the bug
   makes even properly-linked notes suffer it.

## Approach

Fix the incoming-link query to match how `to_note` is actually stored, at both
dream call sites. Add a single store method that matches **either** the full
path **or** the bare filename, so resolved rows (full path) and any legacy /
ambiguous unresolved rows (bare filename, where `resolve_target` could not
uniquely resolve) are both counted.

Rejected alternatives:

- **Track woven notes / de-dupe at write time.** Adds state, treats the symptom,
  does not fix the NoteDecay protection bug. Violates YAGNI.
- **Normalize `to_note` to bare filenames everywhere** so the comments become
  true. Large, risky data migration; `get_graph_data` / `collect_edges_between`
  and the panel canvas all join on full-path `to_note`. The comments are wrong,
  not the data.

## Design

### New store method (union query)

`NoteStore` trait + `SqliteMemoryBackend` impl:

```rust
/// Paths of notes that link to this note, matching either the resolved
/// full path or the bare filename. `notes_links.to_note` is the resolved
/// target — a full path when `resolve_target` matched a unique filename,
/// otherwise the bare wikilink text. Callers that hold both forms (dream
/// stages walking `category/title` notes) pass both so resolved and legacy
/// rows are counted alike.
async fn get_incoming_links_any(
    &self,
    path: &str,
    filename: &str,
    agent_id: &str,
) -> Result<Vec<String>, AlephError>;
```

SQLite impl:

```sql
SELECT from_note FROM notes_links
WHERE to_note IN (?1, ?2) AND agent_id = ?3
```

The existing `get_incoming_links` is left unchanged — `graph.rs` uses it
correctly with a full path.

### Call-site changes

- **`note_weave.rs`** orphan detection loop: replace
  `get_incoming_links(filename, …)` with
  `get_incoming_links_any(&note.path, filename, …)`. Orphan detection becomes
  accurate; incoming-only notes are no longer re-extracted every cycle.
- **`note_decay.rs`** protection loop: replace
  `get_incoming_links(filename, …)` with
  `get_incoming_links_any(&note.path, filename, …)`. `incoming_count`,
  `link_weight`, and the ≥3-incoming protection recover.

### Comment corrections

Rewrite the three false-premise comments to state: `to_note` is the resolved
target (full path when resolvable), `to_raw` is the bare wikilink text, and the
union query covers both.

## Testing

- **Store layer:** `get_incoming_links_any` returns a `from_note` whose link row
  has a full-path `to_note`, AND a `from_note` whose row has a bare-filename
  `to_note` — one fixture with both shapes, asserting both are found.
- **NoteWeave regression:** an incoming-only note (A unidirectionally
  `[[B]]`, B has no outgoing links) is **not** treated as an orphan on a second
  `execute` — locks in "no repeated re-weave."
- **NoteDecay:** a note referenced by 3+ other notes via full-path `to_note`
  reads `incoming_count >= 3` and is protected (this assertion FAILS against the
  current bug, PASSES after the fix).

## Blast Radius

1 trait method + 1 SQLite impl + 2 call-site edits + 3 comment fixes + tests.
No data migration, no write-path change, no graph-query change.
