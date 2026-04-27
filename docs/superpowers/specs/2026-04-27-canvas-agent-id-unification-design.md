# Canvas agent_id Unification — Design

**Date:** 2026-04-27
**Status:** Spec — pending implementation plan
**Scope:** Eliminate the `agent_id` constant split between `"default"` and `"main"` that hides user notes from the canvas. Production code is unified onto a single constant; existing DB rows are migrated; test residue is purged.

---

## 1. Problem

The radial canvas (`interfaces/webchat/src/views/canvas/mod.rs::RadialCanvasView`) reads notes via `handle_neighbors_impl(req, db: MemoryBackend)` (`src/gateway/handlers/graph.rs`), which in turn calls `db.get_graph_data(crate::routing::DEFAULT_AGENT_ID, params.limit)`. `routing::DEFAULT_AGENT_ID` is the string `"main"`.

However, several production-code writers do **not** use that constant. They write to `agent_id = "default"`:

| File | Line | Mechanism |
|---|---|---|
| `src/memory/extensions/scheduler.rs` | 17, 61 | Defines its own `DEFAULT_AGENT_ID_FOR_PRODUCERS = "default"` and uses it |
| `src/memory/notes/orientation/index_md.rs` | 149, 280 | Hardcoded `"default".into()` literal in note ingestion |
| `src/memory/dreaming/mod.rs` | 627 | Hardcoded `"default"` with a `// TODO: use actual agent_id when available` comment |
| `src/memory/events/handler.rs` | 96 | Fallback array `["default", "owner"]` for historical lookups |
| `src/builtin_tools/memory_browse.rs` | 189, 205 | Hardcoded `"default"` in builtin tool |
| `src/builtin_tools/memory_explore.rs` | 89 | Hardcoded `"default"` in builtin tool |

As a result, notes produced by these subsystems land under the wrong agent and are invisible to the canvas. The user perceives the canvas as "almost empty" even when the notes table has 25+ rows.

Concrete state at time of writing (`memory.db`):

| agent_id | notes | links |
|---|---|---|
| `main` | 12 | 22 |
| `default` | 5 | 19 |
| `test-memory-validation` | 8 | 16 |

The 5 `default` rows are real user content (personal/, projects/, preferences/) — most likely produced by the orientation/index_md ingest path. The 8 `test-memory-validation` rows are residue from prior test runs that never got cleaned up.

## 2. Goals

- One canonical constant for "default agent" in production code: `crate::routing::DEFAULT_AGENT_ID` (value `"main"`).
- All existing `default`-agent rows in `memory.db` migrated to `main`, without losing data or duplicating canvas-visible nodes.
- All `test-memory-validation` residue rows purged.
- Compile-time enforcement: deleting `DEFAULT_AGENT_ID_FOR_PRODUCERS` makes any future hardcoded writer fail to build, forcing use of the unified constant.

## 3. Non-goals

- Threading `agent_id` through session context everywhere (the C1c "architectural" path). Several call sites have TODO comments wanting this; explicitly deferred.
- Multi-agent picker UI in the canvas toolbar (the C3 path). Single-agent model retained.
- Touching `"default"` string literals inside `#[cfg(test)]` modules. Tests use isolated DBs; their literals don't pollute production state.
- Refactoring the deeper `["default", "owner"]` fallback logic in `events/handler.rs` beyond replacing the `default` element. The `owner` token's history is unclear and worth a separate audit, not this spec.
- Changing `DEFAULT_AGENT_ID` itself (it stays `"main"`).

## 4. Approach overview

1. Replace the local `DEFAULT_AGENT_ID_FOR_PRODUCERS` constant with a use of `crate::routing::DEFAULT_AGENT_ID`.
2. Replace each production-code `"default"` agent_id literal with the same constant.
3. Add one new schema migration step that:
   - Copies `default`-agent rows into `main` (using `INSERT OR IGNORE` so existing `main` rows win on path collision)
   - Deletes the `default`-agent rows
   - Deletes the `test-memory-validation` rows (notes_index + notes_links + notes_fts)
4. Add tests covering the migration and one writer-path assertion.

## 5. Architecture

### 5.1 Constant unification

Pre-state (multiple sources of truth):

```rust
// src/routing/session_key.rs:91
pub const DEFAULT_AGENT_ID: &str = "main";

// src/memory/extensions/scheduler.rs:17
pub const DEFAULT_AGENT_ID_FOR_PRODUCERS: &str = "default";
```

Post-state (single source of truth):

```rust
// src/routing/session_key.rs:91 — unchanged
pub const DEFAULT_AGENT_ID: &str = "main";

// src/memory/extensions/scheduler.rs — constant deleted; call sites updated
use crate::routing::DEFAULT_AGENT_ID;
// ... agent_id: DEFAULT_AGENT_ID.to_string(),
```

### 5.2 Production callers updated

Each file replaces the literal `"default"` with `crate::routing::DEFAULT_AGENT_ID`:

| File | Edit |
|---|---|
| `src/memory/extensions/scheduler.rs:17` | Delete `pub const DEFAULT_AGENT_ID_FOR_PRODUCERS` |
| `src/memory/extensions/scheduler.rs:61` | `agent_id: DEFAULT_AGENT_ID.to_string()` |
| `src/memory/notes/orientation/index_md.rs:149` | `agent_id: DEFAULT_AGENT_ID.into()` |
| `src/memory/notes/orientation/index_md.rs:280` | `agent_id: DEFAULT_AGENT_ID.into()` |
| `src/memory/dreaming/mod.rs:627` | `let agent_dir = memory_dir.join(DEFAULT_AGENT_ID);` (and remove the TODO) |
| `src/memory/events/handler.rs:96` | `let agents_to_try = [DEFAULT_AGENT_ID, "owner"];` (preserve "owner" until separately audited) |
| `src/builtin_tools/memory_browse.rs:189` | `let agent = dir.path().join(DEFAULT_AGENT_ID);` |
| `src/builtin_tools/memory_browse.rs:205` | `let agent_dir = dir.path().join(DEFAULT_AGENT_ID);` |
| `src/builtin_tools/memory_explore.rs:89` | `agent_id: DEFAULT_AGENT_ID.to_string()` |

Each file gets one `use crate::routing::DEFAULT_AGENT_ID;` at the top.

Note: `dreaming/mod.rs` and `memory_browse.rs` use the constant as a **directory name** (`memory_dir.join("default")`), not as a DB `agent_id` column. The constant is reused there for symmetry — same source of truth for "what's the user's default agent." If they ever diverge (e.g., directory layout vs. DB key), revisit. For now they're treated as one concept.

### 5.3 Migration step

Append a new migration to `src/memory/store/sqlite/schema.rs`'s migration list. Migration body:

```sql
-- Migration: unify legacy 'default' agent_id rows into 'main'; purge test residue.
-- Idempotent: re-running has no effect once 'default'/'test-memory-validation' rows are gone.

INSERT OR IGNORE INTO notes_index
  (path, filename, agent_id, category, tags_json,
   created_at, updated_at, last_accessed_at, content_hash)
SELECT path, filename, 'main', category, tags_json,
       created_at, updated_at, last_accessed_at, content_hash
FROM notes_index
WHERE agent_id = 'default';

INSERT OR IGNORE INTO notes_links (agent_id, from_note, to_note)
SELECT 'main', from_note, to_note
FROM notes_links
WHERE agent_id = 'default';

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
DELETE FROM notes_fts   WHERE agent_id = 'test-memory-validation';
```

Conflict policy: `INSERT OR IGNORE` preserves any existing `main` row when a `default` row shares the same primary key (`(agent_id, path)` for notes_index; `(agent_id, from_note, to_note)` for notes_links). The canvas already shows `main` rows, so user-visible state is preserved.

`notes_fts` uses an explicit `NOT EXISTS` check because FTS5 has no UNIQUE constraint and would otherwise duplicate rows on path collision.

### 5.4 Edge cases

| Case | Behavior |
|---|---|
| `default` row links to another `default` row | Link's from/to paths are unchanged by migration (only agent_id changes). Topology preserved. |
| `default` row links to a path that also exists in `main` | After migration the link is attached to the `main` version of that path. The two agents' same-named notes are coalesced — consistent with the user's intent that there is exactly one `personal/zou-guojun.md`. |
| `default` row exists with same path as a `main` row | `INSERT OR IGNORE` keeps the `main` row. The `default` row's content is dropped. Acceptable because `main` is the canonical, canvas-visible source of truth. |
| Migration runs on a fresh DB (no `default` rows) | All statements are no-ops. ✓ |
| Migration runs twice | Idempotent: after the first run, `default` rows are gone; second run's INSERTs and DELETEs are no-ops. ✓ |
| Future code adds `agent_id = "default"` somewhere | The unified constant is `"main"`; literal `"default"` will diverge again. Static enforcement is partial — see §5.6. |

### 5.5 Tests

New tests in `src/memory/store/sqlite/schema.rs` (or sibling test module):

| Test | Asserts |
|---|---|
| `migration_moves_default_rows_to_main` | Insert `(default, personal/foo.md)`; run migration; assert row is at `(main, personal/foo.md)` and no row at `(default, personal/foo.md)`. |
| `migration_preserves_existing_main_on_collision` | Insert `(main, personal/foo.md)` with content "M" and `(default, personal/foo.md)` with content "D"; run migration; assert the surviving `main` row's content is "M" (not "D"). |
| `migration_purges_test_memory_validation_rows` | Insert rows under `agent_id='test-memory-validation'`; run migration; assert all such rows gone. |
| `migration_is_idempotent` | Insert default rows; run migration twice; assert final state matches one-run state. |
| `migration_preserves_link_topology` | Insert `(default, A)`, `(default, B)`, link `A→B` under default; run migration; assert link `A→B` exists under `main`. |

New test in `src/memory/extensions/scheduler.rs` (test module):

| Test | Asserts |
|---|---|
| `scheduler_uses_routing_default_agent_id` | Construct a producer task; assert the resulting structure carries `agent_id == crate::routing::DEFAULT_AGENT_ID` (i.e., `"main"`). |

### 5.6 Static enforcement (partial)

Deleting `DEFAULT_AGENT_ID_FOR_PRODUCERS` ensures `scheduler.rs` callers must use a real symbol. But Rust cannot prevent future code from typing the literal `"default"` in some new file.

Two cheap mitigations, both **out of scope** for this spec but worth noting:

1. A `clippy::disallowed_strings` lint entry banning `"default"` in agent_id contexts (clippy doesn't support context-aware string bans, so this is not viable today).
2. A grep-based CI check: `grep -rn '"default"' src/ | grep -i agent_id` and fail if matches found. Plausible but introduces CI complexity for marginal benefit. Deferred.

The strongest enforcement remains social: the unified constant is the documented way; reviewers reject string literals.

## 6. Out of scope (deferred)

| Item | Reason for deferral |
|---|---|
| Source agent_id from session context (C1c) | Multi-module refactor; `routing::DEFAULT_AGENT_ID` is sufficient for the single-agent model the user actually uses. |
| Multi-agent picker UI (C3) | Per CLAUDE.md R8/R10, agent_id should be hidden from the user; exposing it in the UI is the wrong direction. |
| Investigate `"owner"` in `events/handler.rs:96` | Historical token, semantics unclear without a separate audit. Preserved as-is to keep this PR focused. |
| Test-code `"default"` literals | Tests use isolated DBs and don't pollute production state. Touching them adds churn without benefit. |
| Storage-layer unique-by-`path` enforcement (drop the `(agent_id, path)` composite key) | Would force single-agent at the DB level. Premature commitment to the model — keep flexibility in case the user ever wants per-agent sandboxes. |
| CI grep guard against `"default"` literals (§5.6) | Marginal value vs. tooling complexity. |

## 7. Verification (post-implementation)

After implementation:

1. `cargo test -p alephcore --lib` — all migration tests + scheduler test pass
2. `cargo build --release` — verify no remaining references to `DEFAULT_AGENT_ID_FOR_PRODUCERS`
3. Restart aleph-server with the migrated DB; load `/memory` in browser; verify the previously-hidden notes are now selectable (e.g., `personal/li-wei.md`, `preferences/coding-style.md`, `preferences/toolchain.md`, `projects/aleph.md` should appear). Exact node count depends on path overlap with existing `main` rows; expect ~16 (12 main + 5 default − 1 collision on `personal/zou-guojun.md`).
4. SQL post-check: `SELECT agent_id, COUNT(*) FROM notes_index GROUP BY agent_id` should return only `main`
