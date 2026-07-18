# Canvas agent_id Unification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the `agent_id` constant split (`"default"` vs `"main"`) so all production writers and the canvas reader agree on a single value, and migrate existing `default`-agent rows in `memory.db` into `main`.

**Architecture:** Replace one local constant + 8 hardcoded `"default"` literals with `crate::routing::DEFAULT_AGENT_ID`. Add one idempotent SQL migration step to `init_schema` that copies `default`-agent rows into `main` (with `INSERT OR IGNORE` collision policy) and purges `test-memory-validation` residue.

**Tech Stack:** Rust + rusqlite (SQLite), Aleph's existing in-line migration pattern in `src/memory/store/sqlite/schema.rs`.

**Spec:** `docs/superpowers/specs/2026-04-27-canvas-agent-id-unification-design.md`

---

## File Structure

| File | Role | Change kind |
|---|---|---|
| `src/memory/store/sqlite/schema.rs` | Adds new `migrate_unify_default_to_main_agent` function + wires it into `init_schema`; hosts migration tests | Modify (append fn + 6 tests; one wire-up call) |
| `src/memory/extensions/scheduler.rs` | Producer scheduler — first call site that needs unified constant | Modify (delete local const, use unified; add 1 test) |
| `src/memory/notes/orientation/index_md.rs` | Note ingest writer — most likely current source of stray `default` rows | Modify (2 literal replacements) |
| `src/memory/dreaming/mod.rs` | Dreaming module path constructor | Modify (1 literal replacement; remove TODO) |
| `src/memory/events/handler.rs` | Events handler fallback array | Modify (1 element replacement; preserve `"owner"`) |
| `src/builtin_tools/memory_browse.rs` | Builtin LLM tool | Modify (2 literal replacements) |
| `src/builtin_tools/memory_explore.rs` | Builtin LLM tool | Modify (1 literal replacement) |

Each modified file gets `use crate::routing::DEFAULT_AGENT_ID;` at the top if not already present.

---

## Task 1: Migration function + tests

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs` (add fn + wire into init_schema; add 5 tests)

The migration is the lynchpin — implementing it first means all later constant changes can run against a DB that's already been normalized.

- [ ] **Step 1: Write 5 failing migration tests in `schema.rs`**

Append inside the existing `mod tests { ... }` block (around line 670, after the existing tests):

```rust
    fn seed_default_agent_row(conn: &Connection, path: &str, content: &str) {
        let now: i64 = 1_700_000_000_000;
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json,
                 created_at, updated_at, last_accessed_at, content_hash)
             VALUES (?1, ?2, 'default', 'misc', '[]', ?3, ?3, ?3, ?4)",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), now, content],
        )
        .expect("seed default notes_index");
        conn.execute(
            "INSERT INTO notes_fts (path, filename, content, agent_id)
             VALUES (?1, ?2, ?3, 'default')",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), content],
        )
        .expect("seed default notes_fts");
    }

    fn seed_main_agent_row(conn: &Connection, path: &str, content: &str) {
        let now: i64 = 1_700_000_000_000;
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json,
                 created_at, updated_at, last_accessed_at, content_hash)
             VALUES (?1, ?2, 'main', 'misc', '[]', ?3, ?3, ?3, ?4)",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), now, content],
        )
        .expect("seed main notes_index");
        conn.execute(
            "INSERT INTO notes_fts (path, filename, content, agent_id)
             VALUES (?1, ?2, ?3, 'main')",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), content],
        )
        .expect("seed main notes_fts");
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .expect("count query")
    }

    #[test]
    fn migration_moves_default_rows_to_main() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_default_agent_row(&conn, "personal/foo.md", "F");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM notes_index WHERE agent_id='main' AND path='personal/foo.md'"),
            1, "row should be at main"
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM notes_index WHERE agent_id='default'"),
            0, "no default rows should remain"
        );
    }

    #[test]
    fn migration_preserves_existing_main_on_collision() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_main_agent_row(&conn, "personal/foo.md", "M");
        seed_default_agent_row(&conn, "personal/foo.md", "D");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        let surviving_hash: String = conn.query_row(
            "SELECT content_hash FROM notes_index WHERE agent_id='main' AND path='personal/foo.md'",
            [], |r| r.get(0),
        ).expect("query surviving row");
        assert_eq!(surviving_hash, "M", "main row content_hash should win on collision");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM notes_index WHERE agent_id='default'"),
            0, "default row should be deleted even on collision"
        );
    }

    #[test]
    fn migration_purges_test_memory_validation_rows() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        let now: i64 = 1_700_000_000_000;
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json,
                 created_at, updated_at, last_accessed_at, content_hash)
             VALUES ('residue.md', 'residue.md', 'test-memory-validation', 'misc', '[]', ?1, ?1, ?1, 'h')",
            rusqlite::params![now],
        ).expect("seed residue row");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM notes_index WHERE agent_id='test-memory-validation'"),
            0, "test-memory-validation rows should be purged"
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_default_agent_row(&conn, "personal/bar.md", "B");
        migrate_unify_default_to_main_agent(&conn).expect("first run");
        let snapshot1 = count(&conn, "SELECT COUNT(*) FROM notes_index WHERE agent_id='main'");
        migrate_unify_default_to_main_agent(&conn).expect("second run");
        let snapshot2 = count(&conn, "SELECT COUNT(*) FROM notes_index WHERE agent_id='main'");
        assert_eq!(snapshot1, snapshot2, "second run must be a no-op");
    }

    #[test]
    fn migration_preserves_link_topology() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_default_agent_row(&conn, "A.md", "a");
        seed_default_agent_row(&conn, "B.md", "b");
        conn.execute(
            "INSERT INTO notes_links (agent_id, from_note, to_note) VALUES ('default', 'A.md', 'B.md')",
            [],
        ).expect("seed default link");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM notes_links WHERE agent_id='main' AND from_note='A.md' AND to_note='B.md'"),
            1, "link must survive under main"
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM notes_links WHERE agent_id='default'"),
            0, "default links should be deleted"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib schema::tests::migration_ -- --nocapture
```

Expected output: 5 compile-time errors complaining `cannot find function migrate_unify_default_to_main_agent in this scope` (because we haven't written it yet).

- [ ] **Step 3: Implement `migrate_unify_default_to_main_agent`**

Add this function to `src/memory/store/sqlite/schema.rs`, immediately after `drop_obsolete_facts_tables` (around line 363, before `pub fn init_schema`):

```rust
/// Unify legacy `default`-agent notes rows into `main`, and purge
/// `test-memory-validation` residue. Idempotent: re-running has no effect
/// once `default`/`test-memory-validation` rows are gone.
///
/// Conflict policy: when a `default` row shares its primary key with an
/// existing `main` row, the `main` row wins (INSERT OR IGNORE). The
/// `default` row's content is dropped, since `main` is the canonical,
/// canvas-visible source of truth.
pub fn migrate_unify_default_to_main_agent(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(
        "INSERT OR IGNORE INTO notes_index
            (path, filename, agent_id, category, tags_json,
             created_at, updated_at, last_accessed_at, content_hash)
         SELECT path, filename, 'main', category, tags_json,
                created_at, updated_at, last_accessed_at, content_hash
         FROM notes_index WHERE agent_id = 'default';

         INSERT OR IGNORE INTO notes_links (agent_id, from_note, to_note)
         SELECT 'main', from_note, to_note
         FROM notes_links WHERE agent_id = 'default';

         INSERT INTO notes_fts (path, filename, content, agent_id)
         SELECT path, filename, content, 'main'
         FROM notes_fts AS f
         WHERE f.agent_id = 'default'
           AND NOT EXISTS (
             SELECT 1 FROM notes_fts WHERE path = f.path AND agent_id = 'main'
           );

         DELETE FROM notes_index WHERE agent_id = 'default';
         DELETE FROM notes_links WHERE agent_id = 'default';
         DELETE FROM notes_fts   WHERE agent_id = 'default';

         DELETE FROM notes_index WHERE agent_id = 'test-memory-validation';
         DELETE FROM notes_links WHERE agent_id = 'test-memory-validation';
         DELETE FROM notes_fts   WHERE agent_id = 'test-memory-validation';",
    )
    .map_err(|e| AlephError::config(format!("Failed to migrate default agent rows to main: {e}")))?;
    Ok(())
}
```

- [ ] **Step 4: Wire migration into `init_schema`**

In `src/memory/store/sqlite/schema.rs`, locate the line `conn.execute_batch(NOTES_FTS_DDL)` (around line 424). Immediately AFTER its `.map_err(...)?;` line, insert:

```rust
    // Spec 2026-04-27: unify legacy `default`-agent notes rows into `main`,
    // and purge `test-memory-validation` residue. Idempotent.
    migrate_unify_default_to_main_agent(conn)?;
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p alephcore --lib schema::tests::migration_ -- --nocapture
```

Expected: 5 tests pass. Also confirm the existing `init_schema_is_idempotent` test still passes (it now exercises the new migration on an empty schema).

```bash
cargo test -p alephcore --lib schema::tests
```

Expected: all schema tests green.

- [ ] **Step 6: Commit**

```bash
git add src/memory/store/sqlite/schema.rs
git commit -m "memory(schema): migrate legacy default-agent notes into main"
```

---

## Task 2: Unify `scheduler.rs`

**Files:**
- Modify: `src/memory/extensions/scheduler.rs` (delete local const, use unified; add 1 test)

- [ ] **Step 1: Write the failing test**

Append to (or create) the `#[cfg(test)] mod tests { ... }` block at the bottom of `src/memory/extensions/scheduler.rs`:

```rust
#[cfg(test)]
mod agent_id_unification_tests {
    use super::*;

    #[test]
    fn produce_ctx_uses_routing_default_agent_id() {
        // The scheduler must produce memories under crate::routing::DEFAULT_AGENT_ID
        // (= "main"), not under a separate "default" constant.
        let agent = crate::memory::extensions::scheduler::scheduler_default_agent_id();
        assert_eq!(
            agent,
            crate::routing::DEFAULT_AGENT_ID,
            "scheduler must use the unified DEFAULT_AGENT_ID, not a local 'default' constant"
        );
        assert_eq!(agent, "main", "DEFAULT_AGENT_ID is expected to be 'main'");
    }
}
```

The test calls a helper `scheduler_default_agent_id()` we'll introduce in Step 3.

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib scheduler::agent_id_unification_tests -- --nocapture
```

Expected: compile error — `cannot find function scheduler_default_agent_id`.

- [ ] **Step 3: Replace the local constant with the unified one and add the test helper**

In `src/memory/extensions/scheduler.rs`:

(a) At the top (around line 5), add the import (place it alphabetically among existing `use` statements):

```rust
use crate::routing::DEFAULT_AGENT_ID;
```

(b) Delete line 17:

```rust
pub const DEFAULT_AGENT_ID_FOR_PRODUCERS: &str = "default";
```

(c) Replace line 61 (inside `run_once`):

Before:
```rust
            agent_id: DEFAULT_AGENT_ID_FOR_PRODUCERS.to_string(),
```

After:
```rust
            agent_id: DEFAULT_AGENT_ID.to_string(),
```

(d) Add a small helper at module scope (just below the `use` lines, before `pub const DEFAULT_TICK_SECONDS`) so the test has a stable point of reference:

```rust
/// Returns the agent_id used by the producer scheduler. Exposed for
/// test assertions; equivalent to `crate::routing::DEFAULT_AGENT_ID`.
pub fn scheduler_default_agent_id() -> &'static str {
    DEFAULT_AGENT_ID
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib scheduler::agent_id_unification_tests -- --nocapture
```

Expected: 1 test passes.

Also ensure nothing else broke:

```bash
cargo build -p alephcore
```

Expected: clean build (no remaining references to `DEFAULT_AGENT_ID_FOR_PRODUCERS`).

If the build fails with `cannot find DEFAULT_AGENT_ID_FOR_PRODUCERS`, search for stragglers:

```bash
grep -rn "DEFAULT_AGENT_ID_FOR_PRODUCERS" src/
```

Replace each remaining usage with `crate::routing::DEFAULT_AGENT_ID` (matching scheduler.rs's pattern), then re-run.

- [ ] **Step 5: Commit**

```bash
git add src/memory/extensions/scheduler.rs
git commit -m "memory(scheduler): use routing::DEFAULT_AGENT_ID instead of local 'default' const"
```

---

## Task 3: Unify `orientation/index_md.rs` (2 sites)

**Files:**
- Modify: `src/memory/notes/orientation/index_md.rs`

- [ ] **Step 1: Add the import**

At the top of `src/memory/notes/orientation/index_md.rs`, add (alphabetical position among existing `use` lines):

```rust
use crate::routing::DEFAULT_AGENT_ID;
```

- [ ] **Step 2: Replace the two literal sites**

Line 149 — change:

```rust
            agent_id: "default".into(),
```

to:

```rust
            agent_id: DEFAULT_AGENT_ID.into(),
```

Line 280 — change:

```rust
                    agent_id: "default".into(),
```

to:

```rust
                    agent_id: DEFAULT_AGENT_ID.into(),
```

- [ ] **Step 3: Verify compile + existing tests still pass**

```bash
cargo test -p alephcore --lib notes::orientation::index_md
```

Expected: all existing tests pass. (No new behavior to test — these are write-side literal swaps to a value that's already used by readers.)

- [ ] **Step 4: Commit**

```bash
git add src/memory/notes/orientation/index_md.rs
git commit -m "memory(notes): use routing::DEFAULT_AGENT_ID in orientation index_md"
```

---

## Task 4: Unify `dreaming/mod.rs` (1 site, remove TODO)

**Files:**
- Modify: `src/memory/dreaming/mod.rs`

- [ ] **Step 1: Add the import**

At the top of `src/memory/dreaming/mod.rs`, add:

```rust
use crate::routing::DEFAULT_AGENT_ID;
```

- [ ] **Step 2: Replace the literal and remove the stale TODO**

Line 627 — change:

```rust
        let agent_dir = memory_dir.join("default"); // TODO: use actual agent_id when available
```

to:

```rust
        let agent_dir = memory_dir.join(DEFAULT_AGENT_ID);
```

- [ ] **Step 3: Verify compile**

```bash
cargo build -p alephcore
```

Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/mod.rs
git commit -m "memory(dreaming): use routing::DEFAULT_AGENT_ID for agent dir name"
```

---

## Task 5: Unify `events/handler.rs` (1 site, preserve `"owner"`)

**Files:**
- Modify: `src/memory/events/handler.rs`

- [ ] **Step 1: Add the import**

At the top of `src/memory/events/handler.rs`, add:

```rust
use crate::routing::DEFAULT_AGENT_ID;
```

- [ ] **Step 2: Replace only the first element of the fallback array**

Line 96 — change:

```rust
                let agents_to_try = ["default", "owner"]; // common agent IDs
```

to:

```rust
                let agents_to_try = [DEFAULT_AGENT_ID, "owner"]; // common agent IDs (owner is legacy; audit separately)
```

(`"owner"` is preserved per spec §3 — its history is unclear and removing it is out of scope for this PR.)

- [ ] **Step 3: Verify compile + tests**

```bash
cargo test -p alephcore --lib memory::events
```

Expected: existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/memory/events/handler.rs
git commit -m "memory(events): use routing::DEFAULT_AGENT_ID in lookup fallback"
```

---

## Task 6: Unify `builtin_tools/memory_browse.rs` (2 sites)

**Files:**
- Modify: `src/builtin_tools/memory_browse.rs`

- [ ] **Step 1: Add the import**

At the top of `src/builtin_tools/memory_browse.rs`, add:

```rust
use crate::routing::DEFAULT_AGENT_ID;
```

- [ ] **Step 2: Replace the two literal sites**

Line 189 — change:

```rust
        let agent = dir.path().join("default");
```

to:

```rust
        let agent = dir.path().join(DEFAULT_AGENT_ID);
```

Line 205 — change:

```rust
        let agent_dir = dir.path().join("default");
```

to:

```rust
        let agent_dir = dir.path().join(DEFAULT_AGENT_ID);
```

- [ ] **Step 3: Verify compile + tests**

```bash
cargo test -p alephcore --lib builtin_tools::memory_browse
```

Expected: existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/memory_browse.rs
git commit -m "tools(memory_browse): use routing::DEFAULT_AGENT_ID for agent dir"
```

---

## Task 7: Unify `builtin_tools/memory_explore.rs` (1 site)

**Files:**
- Modify: `src/builtin_tools/memory_explore.rs`

- [ ] **Step 1: Add the import**

At the top of `src/builtin_tools/memory_explore.rs`, add:

```rust
use crate::routing::DEFAULT_AGENT_ID;
```

- [ ] **Step 2: Replace the literal**

Line 89 — change:

```rust
            agent_id: "default".to_string(),
```

to:

```rust
            agent_id: DEFAULT_AGENT_ID.to_string(),
```

- [ ] **Step 3: Verify compile + tests**

```bash
cargo test -p alephcore --lib builtin_tools::memory_explore
```

Expected: existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/memory_explore.rs
git commit -m "tools(memory_explore): use routing::DEFAULT_AGENT_ID for agent_id"
```

---

## Task 8: Final guard + end-to-end verification

**Files:**
- (Read-only verification — no code changes)

- [ ] **Step 1: Static guard — verify no production `agent_id="default"` literals remain**

```bash
grep -rn '"default"' src/memory src/builtin_tools src/routing 2>/dev/null \
  | grep -iE "agent_id|agent[_ ]?dir|join\(\"default\"\)" \
  | grep -v "test"
```

Expected: empty output (or only references inside `#[cfg(test)]` modules, which are out of scope per spec §6).

If matches appear in production code, replace each with `crate::routing::DEFAULT_AGENT_ID` following the pattern in Tasks 3-7 and create an additional commit.

- [ ] **Step 2: Full test suite**

```bash
cargo test -p alephcore --lib
```

Expected: all tests pass, including the 5 new migration tests and the 1 new scheduler test.

- [ ] **Step 3: Release build**

```bash
cargo build --release --bin aleph-server
```

Expected: clean build with no warnings about unused imports of `DEFAULT_AGENT_ID` (means every modified file actually uses it).

- [ ] **Step 4: Restart aleph-server (will run the new migration on the live DB)**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh
# Expected: no processes left

target/release/aleph-server start &
sleep 5
ps aux | grep "[a]leph-server" | grep -v grep
# Expected: exactly one process
```

- [ ] **Step 5: SQL post-check — confirm migration actually ran**

```bash
sqlite3 ~/.aleph/data/memory.db "SELECT agent_id, COUNT(*) FROM notes_index GROUP BY agent_id;"
```

Expected: only one row, `main|N` where `N >= 12`. No `default` or `test-memory-validation` rows.

```bash
sqlite3 ~/.aleph/data/memory.db "SELECT path FROM notes_index WHERE agent_id='main' AND path IN ('personal/li-wei.md','preferences/coding-style.md','preferences/toolchain.md','projects/aleph.md');"
```

Expected: 4 rows printed (the previously-hidden default-agent notes are now under main).

- [ ] **Step 6: Browser smoke test**

Open `http://127.0.0.1:18790/memory`, observe the `(K of N)` counter in the top-right toolbar.

Expected: `N` is greater than 12 (was previously 12 because canvas only saw `main`). The previously-hidden notes (e.g., `personal/li-wei.md`) should be reachable via the search box or by clicking through the breadcrumb.

- [ ] **Step 7: Final commit (if any cleanup happened in Step 1)**

If Step 1 found additional stragglers and you committed them separately, this step is a no-op. Otherwise nothing to commit here.

---

## Rollback

If migration produces unexpected DB state on the live instance:

```bash
# Stop server
pkill -f "target/release/aleph-server"
sleep 2

# Restore from a pre-migration backup (the user should snapshot ~/.aleph/data/memory.db
# before Step 4 if paranoid). Aleph does NOT auto-backup.

# Or, if the issue is only the migration code:
git revert <migration-commit-sha>
cargo build --release
target/release/aleph-server start
```

The migration is idempotent and only adds rows + deletes specific agent_id values. The SQL is recoverable from git.
