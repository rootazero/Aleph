# Memory Notes Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the facts→notes migration for the compression pipeline and panel display layer.

**Architecture:** CompressionService already defaults to `compress_to_notes()`. Fix remaining agent_id hardcoding in compression path, verify end-to-end flow, and commit all changes from this session.

**Tech Stack:** Rust, SQLite, Leptos/WASM

---

### Task 1: Fix agent_id in compress_to_notes

**Files:**
- Modify: `src/memory/compression/service.rs:485`

The `compress_to_notes` method hardcodes `"default"` when listing existing notes. This must use the workspace_id parameter (which is already passed in).

- [ ] **Step 1: Fix the hardcoded agent_id**

In `src/memory/compression/service.rs`, line 485:

```rust
// Before:
let existing_notes = indexer.store().list_notes("default").await.unwrap_or_default();

// After:
let existing_notes = indexer.store().list_notes(workspace_id).await.unwrap_or_default();
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add src/memory/compression/service.rs
git commit -m "fix(compression): use workspace_id instead of hardcoded 'default' in compress_to_notes"
```

---

### Task 2: Commit all session changes — gateway handlers + startup indexer

**Files:**
- Modified: `src/gateway/handlers/memory.rs`
- Modified: `src/gateway/handlers/graph.rs`
- Modified: `src/bin/aleph-server/commands/start/mod.rs`
- Modified: `src/memory/notes/store.rs`
- Modified: `src/memory/store/sqlite/notes.rs`

These changes were made during this session:
- `handle_stats` queries `notes_index` instead of hardcoded 0
- `handle_list_facts` returns notes from `notes_index` instead of old facts table
- `graph.rs` uses `DEFAULT_AGENT_ID` instead of `"default"`
- Startup runs `NoteIndexer::full_rebuild()`
- `NoteStore` trait gains `count_all_notes()` method

- [ ] **Step 1: Verify all changes compile**

Run: `cargo check`
Expected: Compiles with no errors

- [ ] **Step 2: Run core tests**

Run: `cargo test -p alephcore --lib -- notes`
Expected: All note-related tests pass

- [ ] **Step 3: Commit gateway and store changes**

```bash
git add src/gateway/handlers/memory.rs src/gateway/handlers/graph.rs
git add src/memory/notes/store.rs src/memory/store/sqlite/notes.rs
git commit -m "refactor(memory): stats and listFacts query notes_index instead of facts table

- handle_stats: totalFacts from count_all_notes(), graph stats from get_graph_data()
- handle_list_facts: returns notes_index entries instead of old compressed facts
- graph.rs: all agent_id references use DEFAULT_AGENT_ID
- NoteStore: add count_all_notes() method"
```

- [ ] **Step 4: Commit startup indexer**

```bash
git add src/bin/aleph-server/commands/start/mod.rs
git commit -m "feat(memory): rebuild note index at startup

Runs NoteIndexer::full_rebuild() asynchronously on server start,
ensuring notes_index stays in sync with markdown files on disk."
```

---

### Task 3: Commit design spec and seed notes

**Files:**
- Created: `docs/superpowers/specs/2026-04-11-memory-notes-migration-design.md`
- Created (on disk): `~/.aleph/memory/note/main/project/Aleph Project Overview.md`
- Created (on disk): `~/.aleph/memory/note/main/learning/LLM Wiki Pattern.md`

- [ ] **Step 1: Commit spec**

```bash
git add docs/superpowers/specs/2026-04-11-memory-notes-migration-design.md
git commit -m "docs: add memory notes migration design spec"
```

---

### Task 4: End-to-end verification

- [ ] **Step 1: Build release**

Run: `just build`
Expected: Build succeeds

- [ ] **Step 2: Restart Aleph and verify panel**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null || true
sleep 2
target/release/aleph-server start
```

Open `http://127.0.0.1:18790` and verify:
- Dashboard → Memory: "编译记忆" shows 2, notes tab lists 2 entries
- Memory tab (sidebar): Graph shows 2 nodes with edges
- Stats card: Graph Nodes shows 2

- [ ] **Step 3: Verify compression targets notes**

Trigger a conversation and then check:
- New notes appear in `~/.aleph/memory/note/main/{category}/`
- `notes_index` count increases
- No new rows in `facts` table with `fact_source='extracted'`
