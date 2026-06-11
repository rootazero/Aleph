# Incoming-Link Full-Path Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make NoteWeave orphan detection and NoteDecay link-protection count incoming links correctly by matching the full-path `to_note` stored in `notes_links` (with a bare-filename union for legacy rows).

**Architecture:** Add one `NoteStore` method `get_incoming_links_any(path, filename, agent)` running `WHERE to_note IN (?path, ?filename)`. Switch the two dream-stage call sites to it. Fix three false-premise comments. The existing `get_incoming_links` is untouched (the graph handler already uses it correctly with a full path).

**Tech Stack:** Rust, rusqlite, async-trait, tokio, MockProvider/RecordingMockProvider test doubles.

---

### Task 1: Add `get_incoming_links_any` to the `NoteStore` trait + SQLite impl

**Files:**
- Modify: `src/memory/notes/store.rs` (trait decl, after `get_incoming_links` ~line 109-114)
- Modify: `src/memory/store/sqlite/notes.rs` (impl, after `get_incoming_links` ~line 419-440)
- Test: `src/memory/store/sqlite/notes/tests.rs`

- [ ] **Step 1: Write the failing test**

In `src/memory/store/sqlite/notes/tests.rs`, add a test that indexes two notes
linking to one target — one link resolved to a full path, one stored as a bare
filename — and asserts `get_incoming_links_any` finds both. Mirror the existing
`get_incoming_links` test at `tests.rs:215` (`"backlinks/target"`, `"agent1"`)
for fixture setup conventions.

```rust
#[tokio::test]
async fn incoming_links_any_matches_fullpath_and_filename() {
    let (backend, _tmp) = test_backend().await; // use the same helper the
    // existing tests use to build a SqliteMemoryBackend
    const AGENT: &str = "agent1";

    // Target note path = "reference/target", filename = "target".
    backend
        .index_note(
            &KnowledgeNote { title: "target".into(), category: "reference".into(),
                content_hash: "h0".into(), ..Default::default() },
            AGENT, "reference",
        ).await.unwrap();

    // Source A links by full path -> resolve_target keeps full path in to_note.
    backend
        .index_note(
            &KnowledgeNote { title: "srcA".into(), category: "notes".into(),
                links: vec!["reference/target".into()], content_hash: "hA".into(),
                ..Default::default() },
            AGENT, "notes",
        ).await.unwrap();

    // Source B links by bare filename; force a row whose to_note is the bare
    // filename via add_link_with_relation (legacy/unresolved shape).
    backend
        .index_note(
            &KnowledgeNote { title: "srcB".into(), category: "notes".into(),
                content_hash: "hB".into(), ..Default::default() },
            AGENT, "notes",
        ).await.unwrap();
    backend
        .add_link_with_relation(AGENT, "notes/srcB", "target", "related")
        .await.unwrap();

    let incoming = backend
        .get_incoming_links_any("reference/target", "target", AGENT)
        .await.unwrap();

    assert!(incoming.iter().any(|f| f == "notes/srcA"),
        "full-path row missing: {incoming:?}");
    assert!(incoming.iter().any(|f| f == "notes/srcB"),
        "bare-filename row missing: {incoming:?}");
}
```

If `tests.rs` uses a different backend-construction helper than `test_backend()`,
match the helper already used by `get_incoming_links` test at line 215.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::store::sqlite::notes::tests::incoming_links_any_matches_fullpath_and_filename`
Expected: FAIL — `no method named get_incoming_links_any` (compile error).

- [ ] **Step 3: Add the trait method**

In `src/memory/notes/store.rs`, immediately after the `get_incoming_links`
declaration (the block ending ~line 114):

```rust
    /// Paths of notes that link to this note, matching either the resolved
    /// full path or the bare filename. `notes_links.to_note` holds the resolved
    /// target — a full path when `resolve_target` matched a unique filename,
    /// otherwise the bare wikilink text. Dream stages walking `category/title`
    /// notes pass both forms so resolved and legacy rows are counted alike.
    async fn get_incoming_links_any(
        &self,
        path: &str,
        filename: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError>;
```

- [ ] **Step 4: Add the SQLite impl**

In `src/memory/store/sqlite/notes.rs`, immediately after the `get_incoming_links`
impl (ends ~line 440), mirroring its structure:

```rust
    async fn get_incoming_links_any(
        &self,
        path: &str,
        filename: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT from_note FROM notes_links \
                 WHERE to_note IN (?1, ?2) AND agent_id = ?3",
            )
            .map_err(|e| AlephError::config(format!("get_incoming_links_any prepare: {e}")))?;

        let rows = stmt
            .query_map(params![path, filename, agent_id], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("get_incoming_links_any query: {e}")))?;

        let mut links = Vec::new();
        for row in rows {
            links.push(
                row.map_err(|e| AlephError::config(format!("get_incoming_links_any row: {e}")))?,
            );
        }
        Ok(links)
    }
```

Note: `IN (?1, ?2)` with `?1 == ?2` (when a caller passes the same value twice)
still returns each `from_note` once per row — no duplicate rows are produced
because the dedup is at the `to_note` set level, not the result level. Dream
callers always pass distinct path/filename, so this is moot in practice.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::store::sqlite::notes::tests::incoming_links_any_matches_fullpath_and_filename`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/store.rs src/memory/store/sqlite/notes.rs src/memory/store/sqlite/notes/tests.rs
git commit -m "memory: add get_incoming_links_any union query for incoming-link detection"
```

---

### Task 2: Switch NoteWeave orphan detection to the union query + fix comments

**Files:**
- Modify: `src/memory/dreaming/stages/note_weave.rs:56-78` (orphan loop) and `:66-67` (comment)
- Test: `src/memory/dreaming/stages/note_weave.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing regression test**

Add a test to `note_weave.rs`'s test module proving an **incoming-only** note is
not treated as an orphan. Build a store where `srcA` links to `target` (so
`target` has an incoming full-path link but no outgoing link), put only `target`
in `ctx.notes`, and assert `notes_woven == 0` because `target` is correctly
non-orphan. Reuse the `build_ctx` / `entry` helpers already in the module; the
LLM response can be `{"links": []}` since no extraction should drive a write.

```rust
#[tokio::test]
async fn incoming_only_note_is_not_orphan() {
    let (mut ctx, store) = build_ctx("{\"links\": []}").await;
    // target has an incoming link from src (full-path to_note) but no outgoing.
    store
        .index_note(
            &KnowledgeNote { title: "src".into(), category: "notes".into(),
                facts: vec!["see [[target]]".into()],
                links: vec!["reference/target".into()],
                content_hash: "hs".into(), ..Default::default() },
            "default", "notes",
        ).await.unwrap();
    store
        .index_note(
            &KnowledgeNote { title: "target".into(), category: "reference".into(),
                facts: vec!["fact".into()], content_hash: "ht".into(),
                ..Default::default() },
            "default", "reference",
        ).await.unwrap();
    // Only target is walked this cycle.
    ctx.notes = vec![entry("reference/target")];

    let out = NoteWeaveStage::default().execute(ctx).await.unwrap();
    assert_eq!(out.report.notes_woven, 0,
        "incoming-only note must not be re-wove as an orphan");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::dreaming::stages::note_weave::tests::incoming_only_note_is_not_orphan`
Expected: FAIL — with the bare-filename query, `target`'s incoming reads empty,
`target` is classified orphan; depending on extraction it may attempt a write or
report differently. (If it happens to pass because the lone orphan has no
partner, keep the test — it still locks the contract — but ALSO assert the note
is excluded from the orphan set; see Step 3 note.)

- [ ] **Step 3: Apply the fix**

In `src/memory/dreaming/stages/note_weave.rs`, in the orphan-detection loop,
replace:

```rust
            // notes_links stores raw wikilink targets by filename — query by
            // filename, mirroring NoteDecay's incoming-link count.
            let incoming = ctx
                .indexer
                .store()
                .get_incoming_links(filename, &ctx.agent_id)
                .await
                .unwrap_or_default();
```

with:

```rust
            // `notes_links.to_note` is the resolved target — a full path when
            // the wikilink filename uniquely resolved, otherwise the bare text.
            // Match either so resolved and legacy rows both count; querying by
            // filename alone (the old code) never matched a full-path to_note,
            // so incoming-only notes were wrongly seen as orphans every cycle.
            let incoming = ctx
                .indexer
                .store()
                .get_incoming_links_any(&note.path, filename, &ctx.agent_id)
                .await
                .unwrap_or_default();
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::dreaming::stages::note_weave`
Expected: PASS — including the existing `linked_note_is_not_treated_as_orphan`,
`note_weave_links_orphans_by_keyword_overlap`, and
`lone_orphan_has_no_overlap_partner_so_nothing_woven`.

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/note_weave.rs
git commit -m "dreaming: fix NoteWeave orphan detection to match full-path to_note"
```

---

### Task 3: Switch NoteDecay protection to the union query + fix comments

**Files:**
- Modify: `src/memory/dreaming/stages/note_decay.rs:152-168` (comment + incoming count)
- Test: `src/memory/dreaming/stages/note_decay.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add a stage-level test proving a note referenced by 3+ other notes via full-path
`to_note` is protected (`incoming_count >= 3`). If `note_decay.rs`'s test module
has no SQLite-backed stage fixture, mirror `note_weave.rs`'s `build_ctx`/`entry`
helpers (copy the minimal builder into `note_decay.rs`'s test module). Index a
`hot` target plus three sources each linking `reference/hot` by full path, walk
`hot` through `execute`, and assert it is counted in `notes_protected` (or
whatever the report field is — read the existing decay tests for the exact
assertion surface; e.g. `out.report.notes_archived` must NOT include `hot`).

```rust
#[tokio::test]
async fn note_with_three_incoming_is_protected() {
    let (mut ctx, store) = build_decay_ctx().await; // mirror note_weave build_ctx
    store.index_note(&KnowledgeNote { title: "hot".into(),
        category: "reference".into(), content_hash: "hh".into(),
        // created_at far in the past so the <7d protection does NOT mask this
        created_at: 1, updated_at: 1, ..Default::default() },
        "default", "reference").await.unwrap();
    for s in ["a", "b", "c"] {
        store.index_note(&KnowledgeNote { title: s.into(),
            category: "notes".into(), links: vec!["reference/hot".into()],
            content_hash: format!("h{s}"), created_at: 1, updated_at: 1,
            ..Default::default() }, "default", "notes").await.unwrap();
    }
    ctx.notes = vec![entry("reference/hot")];

    let out = NoteDecayStage::default().execute(ctx).await.unwrap();
    // hot has 3 incoming full-path links -> protected, never archived.
    assert!(!archived_paths(&out).contains(&"reference/hot".to_string()),
        "3-incoming note must be protected from archival");
}
```

Adjust `NoteDecayStage::default()`, the report-field accessor, and
`build_decay_ctx`/`archived_paths` to the actual symbols in `note_decay.rs`
(read its existing tests first; do not invent names).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib memory::dreaming::stages::note_decay::tests::note_with_three_incoming_is_protected`
Expected: FAIL — current bare-filename query yields `incoming_count == 0`, so the
note is not protected and is archived.

- [ ] **Step 3: Apply the fix**

In `src/memory/dreaming/stages/note_decay.rs`, replace:

```rust
            // --- Count incoming links (raw filename used as wikilink target) ---
            // The notes_links table stores raw wikilink targets (filename without
            // category), so we query by filename extracted from the path.
            let filename = match note.path.split_once('/') {
                Some((_, f)) => f,
                None => {
                    tracing::warn!(path = %note.path, "NoteDecay: cannot parse path, skipping");
                    continue;
                }
            };

            let incoming_count = ctx
                .indexer
                .store()
                .get_incoming_links(filename, &ctx.agent_id)
                .await
                .map_or(0, |links| links.len());
```

with:

```rust
            // --- Count incoming links ---
            // `notes_links.to_note` is the resolved target: a full path when the
            // wikilink filename uniquely resolved, otherwise the bare text. Match
            // either. Querying by filename alone (the old code) never matched a
            // full-path to_note, so this count was always 0 — silently disabling
            // both the >=3-incoming protection and link_weight below.
            let filename = match note.path.split_once('/') {
                Some((_, f)) => f,
                None => {
                    tracing::warn!(path = %note.path, "NoteDecay: cannot parse path, skipping");
                    continue;
                }
            };

            let incoming_count = ctx
                .indexer
                .store()
                .get_incoming_links_any(&note.path, filename, &ctx.agent_id)
                .await
                .map_or(0, |links| links.len());
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::dreaming::stages::note_decay`
Expected: PASS — new test plus all existing decay tests (`compute_score`,
protection rules) still green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/note_decay.rs
git commit -m "dreaming: fix NoteDecay incoming-link protection to match full-path to_note"
```

---

### Task 4: Full verification

- [ ] **Step 1: Run the memory + dreaming test suites**

Run: `cargo test -p alephcore --lib memory::`
Expected: PASS (no regressions in store, notes, dreaming).

- [ ] **Step 2: Clippy**

Run: `cargo clippy -p alephcore --lib -- -D warnings`
Expected: no warnings on touched files.

- [ ] **Step 3: Confirm no other bare-filename incoming callers remain in production**

Run: `grep -rn "get_incoming_links(" src --include="*.rs" | grep -v get_incoming_links_any`
Expected: only `graph.rs:282` (full path, correct), the SQLite impl/`store.rs`
decl, and test files remain. No production dream-stage caller uses the bare
form.
