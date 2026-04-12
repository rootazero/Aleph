# Memory System Legacy Cleanup — Design

**Date:** 2026-04-12
**Branch:** main (single-branch development)
**Scope:** Rust Core (`alephcore`) only

## Purpose

Bring `src/` into alignment with `docs/reference/MEMORY_SYSTEM.md` and
`docs/reference/memory/`. The documentation rewrite (commits `025187d2` …
`c5aaea1f`) marked several modules as *dormant*, *deprecated*, or *not wired*.
The source code still contains those modules, creating a gap between what the
docs promise and what the code actually does. This spec closes that gap by
deleting the dead code in five atomic commits.

Out of scope: the `src/memory/cortex/` deprecation (originally item 6 in the
cleanup list). That work involves a non-trivial migration of
`src/memory/value_estimator/cortex.rs` into the POE module and belongs in its
own spec.

## Goals

1. Delete `src/memory/decay.rs` and its `pub use` re-exports.
2. Delete the `GraphDecayPolicy` struct and the `memory.graph_decay` config
   field, along with its validators, UI hints, and serialization tests.
3. Delete the entire `src/wiki/` directory and the `wiki_manage` redirect stub
   in `builtin_registry`.
4. Delete the three unconsumed `DreamingConfig` keys
   (`drift_similarity_threshold`, `cluster_dbscan_eps`,
   `cluster_dbscan_min_samples`) while **keeping** the `dbscan` utility
   function (it is a general tool, not an over-reaching heuristic).
5. Rebuild the `dream_reports` table to keep only the eight fields the writer
   actually produces, dropping the eleven legacy fact/graph counters.

## Success Criteria

Each criterion must hold after **every** commit, not just at the end:

- `cargo check -p alephcore` passes.
- `cargo test -p alephcore --lib` passes.
- `cargo clippy -p alephcore -- -D warnings` introduces no new warnings in
  the touched files.
- Target-specific `grep` checks (listed per cleanup below) return zero hits
  inside `src/`.
- A user config containing any of the removed keys must not break startup
  (verified manually after commits 2 and 4).
- An existing `~/.aleph/data/memory.db` with the legacy `dream_reports`
  schema must migrate cleanly on startup, preserving the eight retained
  fields for every row (verified manually after commit 5).

## Non-Goals

- **No `cortex` deletion.** Item 6 from the source cleanup list is deferred.
- **No POE / crystallization changes.** `src/poe/` is untouched.
- **No new features, refactors, or abstractions.** Only deletions and a
  single table rebuild.
- **No dream pipeline behavior changes.** Only config keys are removed;
  `note_decay`, `note_synthesis`, and `note_drift` stages are untouched.
- **No config schema versioning.** No `config_version` field, no migration
  framework. We rely on Serde's default behavior of silently ignoring unknown
  fields (verified: no `#[serde(deny_unknown_fields)]` in `src/config/`).
- **No UI (Leptos / Tauri) changes.** This is a Rust Core cleanup only.
- **No export of legacy `dream_reports` data.** Historical counter values
  are discarded; `id`, `pipeline_type`, timestamps, `synthesis_count`,
  `errors`, and `namespace` are preserved.

## Constraints

- **Single-branch development.** Commits land directly on `main`.
- **Atomic commits.** Each of the five cleanups is one commit; each commit
  passes the success criteria independently. This preserves
  single-commit revertability.
- **Commit order.** `decay` → `graph_decay` → `wiki` → `dreaming-config` →
  `dream_reports`. This order minimises cross-commit churn.
- **Process hygiene.** Any test run that launches `aleph-server` must be
  preceded by `pkill -f aleph-server` and `sleep 2` (CLAUDE.md redline R4).
  Multiple running instances corrupt `.shared_token` and permanently destroy
  the vault.
- **English commit messages.** Scopes: `memory:`, `config:`, `wiki:`,
  `config:`, `memory:`.

## Architecture of the Cleanup

The cleanups are sequenced so that each commit is a pure delete or a
self-contained rebuild. No commit depends on a later one to compile.

```
commit 1 (memory)  ── decay.rs + re-exports
commit 2 (config)  ── GraphDecayPolicy + validators + hints + tests
commit 3 (wiki)    ── entire src/wiki/ + registry stub
commit 4 (config)  ── three dreaming threshold keys
commit 5 (memory)  ── dream_reports Rust struct + SQL + schema rebuild
```

## Detailed Design

### Cleanup 1 — Remove `decay.rs` and `MemoryStrength`

**Evidence.** `grep -rn "MemoryStrength\|DecayConfig\|use.*memory::decay" src/`
returns only two files: `src/memory/decay.rs` (self) and
`src/memory/mod.rs` (re-export). No consumers.

**Why it is dead.** The old `MemoryFact` tier/strength triad was deleted
with the `facts` table. Note-layer decay is handled by
`src/memory/dreaming/stages/note_decay.rs` using `recall_signals`, which is
unrelated to `DecayConfig`.

**Changes.**

- Delete `src/memory/decay.rs`.
- In `src/memory/mod.rs`:
  - Remove `pub mod decay;` (currently line 26).
  - Remove the `pub use decay::{DecayConfig, MemoryStrength};` line.

**Verification.**

- `cargo check -p alephcore`.
- `cargo test -p alephcore --lib`.
- `grep -rn "MemoryStrength\|DecayConfig" src/` returns nothing.

**Commit message.** `memory: remove dead decay.rs and MemoryStrength re-export`

### Cleanup 2 — Remove `GraphDecayPolicy` and `memory.graph_decay`

**Evidence.** The `graph_nodes` and `graph_edges` SQLite tables were deleted
in prior refactors (commits `9a3a1556` / `02e99c47`). The `GraphDecayPolicy`
struct and `MemoryConfig.graph_decay` field remain but nothing reads them.

**Changes.**

- `src/config/types/memory.rs`:
  - Delete the `GraphDecayPolicy` struct (approx. lines 399–430).
  - Delete `MemoryConfig.graph_decay` field (line 89).
  - Delete `graph_decay: GraphDecayPolicy::default()` in the `Default` impl
    (line 718).
  - Delete the `default_*` helpers tied to `GraphDecayPolicy`.
- `src/config/validate.rs`: delete the three range validators for
  `memory.graph_decay.{node_decay_per_day, edge_decay_per_day, min_score}`
  (lines 318–347).
- `src/config/ui_hints/definitions.rs`: delete the three hint entries for
  `memory.graph_decay.*` (lines 197–215).
- `src/config/tests/serialization.rs`:
  - Remove the `"graph_decay": {...}` block from the test input (line 49).
  - Remove the `config.graph_decay.min_score` assertion (line 70).

**Compatibility.** Serde silently ignores unknown fields (confirmed: no
`#[serde(deny_unknown_fields)]` anywhere under `src/config/`). Existing
user TOML with `[memory.graph_decay]` sections continues to load.

**Verification.**

- `cargo check -p alephcore`.
- `cargo test -p alephcore --lib`.
- `grep -rn "graph_decay\|GraphDecayPolicy" src/` returns nothing.
- Manual: start `aleph-server` with a config file that contains
  `[memory.graph_decay]`; confirm clean startup.

**Commit message.** `config: drop dead GraphDecayPolicy and memory.graph_decay`

### Cleanup 3 — Delete the entire `src/wiki/` directory

**Evidence.** `grep -rn "crate::wiki::\|use crate::wiki" src/` returns only
hits inside `src/wiki/tools/manage.rs` (the file being deleted). The
exported helpers `is_valid_page_slug`, `wiki_path`, `wiki_parent_path`,
`wiki_file_path`, and `wiki_old_dir` have no external callers.

`src/wiki/wikilink.rs` and `src/memory/notes/wikilink.rs` are separate
files. The *active* wikilink implementation is
`src/memory/notes/wikilink.rs` (17 consumers across memory, builtin_tools,
and dreaming). The `src/wiki/wikilink.rs` sibling has no external
consumers.

Real wiki CRUD is handled by `src/builtin_tools/note_manage.rs` with
`category="wiki"`.

**Changes.**

- Delete the entire `src/wiki/` directory:
  - `mod.rs`, `wikilink.rs`, `git.rs`, `index.rs`
  - `tools/mod.rs`, `tools/manage.rs`
- Remove the `pub mod wiki;` declaration from the crate root.
- `src/executor/builtin_registry/registry.rs` lines 928–932: delete the
  `"wiki_manage"` arm that returns the removed-tool error. The migration
  window is over.
- Remove any test fixtures that reference `wiki_manage` (confirmed present
  only in `registry.rs` tests and the `wiki/` files themselves).

**Verification.**

- `cargo check -p alephcore`.
- `cargo test -p alephcore --lib`.
- `grep -rn "crate::wiki\|wiki_manage\|WikiGitManager\|generate_index_content" src/`
  returns nothing.

**Risk.** Medium. Deletes a whole directory in one shot. The Rust compiler
catches any missed reference, and evidence for zero consumers is solid.

**Commit message.** `wiki: remove entire src/wiki/ directory (replaced by notes with category=wiki)`

### Cleanup 4 — Remove unconsumed dreaming config keys

**Evidence.** None of `drift_similarity_threshold`, `cluster_dbscan_eps`,
`cluster_dbscan_min_samples` are read by any stage. `NoteDrift` walks the
wikilink graph directly; `NoteSynthesis` groups by category.

**Changes.** In `src/config/types/memory.rs`:

- Remove the three fields from `DreamingConfig`:
  `drift_similarity_threshold`, `cluster_dbscan_eps`,
  `cluster_dbscan_min_samples`.
- Remove the three `default_*` functions (lines 569, 573, 577).
- Remove the three accessor methods (lines 378–387).
- Remove the three assignments in `Default` impl (lines 361–363).
- Remove the test assertions that read them (lines 749–768).

**Kept intentionally.** The `dbscan` utility function in
`src/memory/dreaming/stages/types.rs` stays. A utility function with no
active callers is reusable infrastructure; a configuration key that
pre-decides heuristics is "acting on behalf of the LLM" and violates the
R8 redline.

**Compatibility.** Same as Cleanup 2 — Serde ignores unknown fields.

**Verification.**

- `cargo check -p alephcore`.
- `cargo test -p alephcore --lib`.
- `grep -rn "drift_similarity_threshold\|cluster_dbscan_eps\|cluster_dbscan_min_samples" src/`
  returns nothing.
- Manual: start `aleph-server` with a config that contains those three keys
  under `[memory.dreaming]`; confirm clean startup.

**Commit message.** `config: drop dead dreaming DBSCAN and drift similarity thresholds`

### Cleanup 5 — Rebuild the `dream_reports` table

**Evidence.** The `dream_reports` table DDL in
`src/memory/store/sqlite/schema.rs` defines 19 columns. Callers upstream
supply zero / `NULL` / `false` for the eleven legacy columns; only eight
carry real data: `id`, `pipeline_type`, `started_at`, `finished_at`,
`duration_ms`, `synthesis_count`, `errors`, `namespace`.

**Changes.**

#### 5.1 Rust side (`src/memory/store/sqlite/dream_reports.rs`)

- Trim `PersistedDreamReport` to eight fields:
  ```
  id, pipeline_type, started_at, finished_at, duration_ms,
  synthesis_count, errors, namespace
  ```
- `insert_dream_report`: reduce SQL column list and `params!` from 19 to 8.
- `recent_dream_reports`: reduce `SELECT` column list and row mapping.
- Follow compile errors to every call site that currently constructs a
  `PersistedDreamReport` with legacy zeros; remove those field assignments.

#### 5.2 Schema side (`src/memory/store/sqlite/schema.rs`)

- Replace `DREAM_REPORTS_DDL` with a new definition that declares only the
  eight fields.
- Add `migrate_dream_reports_drop_legacy_fields(conn)`, following the
  established pattern used by `migrate_recall_signals_note_path`:
  1. `PRAGMA table_info(dream_reports)` — look for the `facts_collected`
     column as the "is legacy" marker.
  2. If absent (fresh DB or already migrated), return `Ok(())`.
  3. Otherwise, inside a transaction:
     ```
     CREATE TABLE dream_reports_new (... 8 columns ...);
     INSERT INTO dream_reports_new SELECT <8 columns> FROM dream_reports;
     DROP TABLE dream_reports;
     ALTER TABLE dream_reports_new RENAME TO dream_reports;
     CREATE INDEX IF NOT EXISTS idx_dream_reports_started
         ON dream_reports(started_at);
     ```
- Call the migration from the schema initialization entry point, **before**
  the `CREATE TABLE IF NOT EXISTS dream_reports (...)` line. This ordering
  ensures:
  - Fresh DB: migration detects no `facts_collected`, returns; `CREATE
    TABLE IF NOT EXISTS` builds the new schema.
  - Legacy DB: migration rebuilds the table with the new schema; `CREATE
    TABLE IF NOT EXISTS` is a no-op.
- Exact position inside the init function is left to the plan phase.

#### 5.3 Data policy

Legacy column data (`facts_collected`, `clusters_found`, `drift_detected`,
`drift_summary`, `candidates_evaluated`, `facts_promoted`,
`promotion_details`, `facts_decayed`, `facts_pruned`, `nodes_decayed`,
`edges_decayed`) is dropped without export. User confirmed no preservation
need.

**Verification.**

- `cargo check -p alephcore`.
- `cargo test -p alephcore --lib`.
- New unit test `dream_reports_legacy_schema_migrates`:
  1. Open in-memory SQLite, manually create the 19-column legacy schema.
  2. Insert two rows with realistic legacy data.
  3. Call the normal schema-init entry point.
  4. Assert `PRAGMA table_info(dream_reports)` returns exactly the eight
     new column names.
  5. Assert both rows remain; their eight retained fields match the
     original values.
- Manual (on a copy of `~/.aleph/data/memory.db`):
  1. `pkill -f "target/release/aleph-server"` →
     `pkill -f "target/debug/aleph-server"` → `sleep 2`.
  2. `cp ~/.aleph/data/memory.db /tmp/memory.db.pre-cleanup`.
  3. `target/release/aleph-server start`; observe init logs.
  4. `sqlite3 ~/.aleph/data/memory.db "PRAGMA table_info(dream_reports);"`
     → expect 8 rows.
  5. `sqlite3 ~/.aleph/data/memory.db "SELECT COUNT(*) FROM dream_reports;"`
     → expect the pre-cleanup count.

**Risk.** Medium-high — only commit that touches user data. Mitigations:
- `has_legacy` guard makes the migration idempotent.
- `BEGIN ... COMMIT` wraps the rebuild; a crash mid-migration leaves the
  original table intact.
- Pre-flight backup step is mandatory in the manual test.

**Commit message.** `memory: rebuild dream_reports table, drop legacy fact/graph counters`

## Migration and Compatibility Strategy

### Config Compatibility (Cleanups 2, 4)

Mechanism: Serde's default behavior is to ignore unknown fields. No
`#[serde(deny_unknown_fields)]` exists under `src/config/`.

Consequence: a user `config.toml` containing `[memory.graph_decay]` or any
of the three dreaming keys loads cleanly after this cleanup. No warning
logs, no migration scripts, no schema versioning. Silent forward
compatibility is sufficient.

Verification is manual: between commits 2 and 3, and again between 4 and 5,
inject the removed keys into a config file, start the server, confirm
clean startup, then restore the real config.

### SQLite Migration (Cleanup 5)

SQLite's `ALTER TABLE DROP COLUMN` is only available in 3.35+ and drops one
column at a time. Removing eleven columns via that path is neither portable
nor atomic, so we rebuild the table.

The migration:
- Detects legacy schema via `PRAGMA table_info` looking for
  `facts_collected`.
- Rebuilds inside `BEGIN ... COMMIT` for atomicity.
- Runs before `CREATE TABLE IF NOT EXISTS` so new databases are unaffected.
- Is idempotent: repeated startups on an already-migrated DB are no-ops.

No rollback migration is provided. The legacy columns are dropped
permanently; restoration means restoring the `.db` file from backup.

### Complexity Explicitly Rejected

- No `config_version` field.
- No generic migrations framework or `migrations/` directory.
- No legacy-data export path.
- No new concurrency primitives (schema init is single-threaded).

## Verification Matrix

| Check | Command | Applies to |
|---|---|---|
| Compile | `cargo check -p alephcore` | All commits |
| Unit tests | `cargo test -p alephcore --lib` | All commits |
| Lint | `cargo clippy -p alephcore -- -D warnings` | All commits |
| Dead symbol grep | per-cleanup grep above | Each commit |
| Config smoke test | manual `aleph-server` launch with legacy keys | After 2, 4 |
| Schema migration test | new `dream_reports_legacy_schema_migrates` test | Commit 5 |
| Live DB migration | manual `aleph-server` launch on copied `memory.db` | Commit 5 |

### Process Management Protocol

Before every manual `aleph-server` startup:

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
# Output must be empty before launching.
```

Concurrent instances compete for `.shared_token` and permanently corrupt
the vault, destroying every stored API key, OAuth token, and embedding
key. This step is not optional.

## Rollback

| Situation | Action |
|---|---|
| A commit fails `cargo check` | Fix in place; do not land |
| Cleanup 1–4 regression after landing | `git revert <commit>` |
| Cleanup 5 corrupts the DB | Restore `memory.db` from `/tmp/memory.db.pre-cleanup` backup, then `git revert` |
| Multiple cleanups need to be backed out | Revert in reverse order: dream_reports → dreaming-config → wiki → graph_decay → decay |

## What This Spec Does Not Deliver

- No E2E tests — no changes to gateway, UI, IPC surfaces.
- No performance benchmarks — deletions have no performance surface.
- No Loom concurrency tests — no concurrency semantics change.
- No external review — single-developer project.

## References

- Documentation rewrite: `docs/reference/MEMORY_SYSTEM.md` +
  `docs/reference/memory/`. Modules flagged *dormant* / *not wired* /
  *deprecated* there are the source of truth for what to delete.
- Original memory docs restructure spec:
  `docs/superpowers/specs/2026-04-12-memory-docs-restructure-design.md`.
- Prior schema-removal commits referenced for evidence: `9a3a1556`,
  `02e99c47`.
- Existing migration pattern to follow:
  `migrate_recall_signals_note_path` in
  `src/memory/store/sqlite/schema.rs`.
