# Memory System Legacy Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete five categories of dead/deprecated code from the Rust Core so `src/` matches the rewritten `docs/reference/memory/` documentation.

**Architecture:** Five atomic commits on `main`, each independently verifiable. No new abstractions, no refactors. One SQLite table rebuild (commit 5) with an idempotent migration function following the existing `migrate_recall_signals_note_path` pattern.

**Tech Stack:** Rust (workspace crate `alephcore`), Serde, rusqlite, cargo, clippy.

**Spec reference:** `docs/superpowers/specs/2026-04-12-memory-legacy-cleanup-design.md`.

---

## Context Engineers Need

**Project rules:**
- Single-branch development: commit directly to `main`. No feature branches.
- English commit messages only.
- Before starting any `aleph-server` launch, run:
  ```bash
  pkill -f "target/release/aleph-server" 2>/dev/null
  pkill -f "target/debug/aleph-server" 2>/dev/null
  sleep 2
  ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
  ```
  Output must be empty before launching. Concurrent instances destroy the vault via `.shared_token` race.
- No tooling skips: don't use `--no-verify`, `--no-gpg-sign`, or similar.

**Working directory:** `/Volumes/TBU4/Workspace/Aleph`

**Key commands:**
- `cargo check -p alephcore` — compile check for the Core crate.
- `cargo test -p alephcore --lib` — unit tests only. Skip integration for this cleanup; nothing here touches external systems.
- `cargo clippy -p alephcore -- -D warnings` — lint gate.
- Grep uses `ripgrep` via the Grep tool. Direct shell `grep -rn PATTERN src/` also works.

**Serde safety:** Verified — no `#[serde(deny_unknown_fields)]` under `src/config/`. Removing struct fields is safe for existing user TOML files.

**Migration pattern to copy (commit 5):** `migrate_recall_signals_note_path` in `src/memory/store/sqlite/schema.rs:37-58`. Uses `PRAGMA table_info` guard for idempotency. Commit 5's `migrate_dream_reports_drop_legacy_fields` must follow the same shape.

---

## File Structure

### Commit 1 — decay.rs removal
- Delete: `src/memory/decay.rs`
- Modify: `src/memory/mod.rs` (remove `pub mod decay;` on line 26 and `pub use decay::{DecayConfig, MemoryStrength};` on line 82)

### Commit 2 — GraphDecayPolicy removal
- Modify: `src/config/types/memory.rs`
- Modify: `src/config/validate.rs`
- Modify: `src/config/ui_hints/definitions.rs`
- Modify: `src/config/tests/serialization.rs`

### Commit 3 — src/wiki/ removal
- Delete directory: `src/wiki/` (all six files: `mod.rs`, `wikilink.rs`, `git.rs`, `index.rs`, `tools/mod.rs`, `tools/manage.rs`)
- Modify: `src/lib.rs` (remove `pub mod wiki;` on line 82)
- Modify: `src/executor/builtin_registry/registry.rs` (remove lines 928-935 `"wiki_manage"` arm)

### Commit 4 — Dreaming config keys removal
- Modify: `src/config/types/memory.rs`

### Commit 5 — dream_reports table rebuild
- Modify: `src/memory/store/sqlite/schema.rs` (replace `DREAM_REPORTS_DDL`, add migration function, call it from `init_schema`)
- Modify: `src/memory/store/sqlite/dream_reports.rs` (trim `PersistedDreamReport`, `insert_dream_report`, `recent_dream_reports`, and tests)

---

## Task 1: Remove `decay.rs` and `MemoryStrength` re-export

**Files:**
- Delete: `src/memory/decay.rs`
- Modify: `src/memory/mod.rs:26` (remove module declaration)
- Modify: `src/memory/mod.rs:82` (remove re-export line)

- [ ] **Step 1: Pre-flight verification**

Confirm no consumers exist before deleting.

Run:
```bash
grep -rn "MemoryStrength\|DecayConfig\|memory::decay" src/ | grep -v "^src/memory/decay\.rs:" | grep -v "^src/memory/mod\.rs:"
```
Expected: empty output. If anything else appears, stop and investigate — the spec's assumption is wrong.

- [ ] **Step 2: Delete the file**

Run:
```bash
rm src/memory/decay.rs
```

- [ ] **Step 3: Remove `pub mod decay;` from mod.rs**

Edit `src/memory/mod.rs`. Remove exactly line 26:
```rust
pub mod decay;
```

- [ ] **Step 4: Remove the re-export line**

Edit `src/memory/mod.rs`. Remove exactly line 82:
```rust
pub use decay::{DecayConfig, MemoryStrength};
```

- [ ] **Step 5: cargo check**

Run: `cargo check -p alephcore`
Expected: PASS with no errors.

If it fails with "cannot find `MemoryStrength` in this scope" anywhere, that points to a consumer the pre-flight grep missed — investigate that file, decide whether it should be deleted too or whether this task is wrong.

- [ ] **Step 6: cargo test**

Run: `cargo test -p alephcore --lib`
Expected: PASS.

- [ ] **Step 7: Grep verification**

Run:
```bash
grep -rn "MemoryStrength\|DecayConfig" src/
```
Expected: empty output.

Run:
```bash
grep -rn "memory::decay\|mod decay" src/
```
Expected: empty output.

- [ ] **Step 8: Clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30`
Expected: no new warnings in touched files. Pre-existing warnings elsewhere are acceptable — only changes introduced by this task matter.

- [ ] **Step 9: Commit**

```bash
git add -u src/memory/
git status --short
```
Expected: only `D  src/memory/decay.rs` and `M  src/memory/mod.rs`.

```bash
git commit -m "memory: remove dead decay.rs and MemoryStrength re-export

DecayConfig and MemoryStrength had no consumers after the MemoryFact
tier/strength triad was deleted with the facts table. Note-layer decay
is handled by dreaming::stages::note_decay via recall_signals and is
unrelated to this module."
```

---

## Task 2: Remove `GraphDecayPolicy` and `memory.graph_decay`

**Files:**
- Modify: `src/config/types/memory.rs`
- Modify: `src/config/validate.rs:318-349`
- Modify: `src/config/ui_hints/definitions.rs:197-214`
- Modify: `src/config/tests/serialization.rs:49-53, 70`

- [ ] **Step 1: Pre-flight verification**

Run:
```bash
grep -rn "graph_decay\|GraphDecayPolicy\|default_graph_node_decay_per_day\|default_graph_edge_decay_per_day\|default_graph_min_score" src/
```
Note every match. The plan expects exactly these locations:
- `src/config/types/memory.rs` (struct def, field, impl Default, default_* functions)
- `src/config/validate.rs` (3 validators)
- `src/config/ui_hints/definitions.rs` (3 hint entries)
- `src/config/tests/serialization.rs` (test JSON and assertion)

If other files appear, update this task's scope before proceeding.

- [ ] **Step 2: Delete the `GraphDecayPolicy` struct and impl**

Edit `src/config/types/memory.rs`. Delete lines 398-424 (the entire block between the two section separators):

```rust
// =============================================================================
// GraphDecayPolicy
// =============================================================================

/// Decay policy for graph nodes/edges
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GraphDecayPolicy {
    /// Per-day decay multiplier for nodes (0.0-1.0)
    #[serde(default = "default_graph_node_decay_per_day")]
    pub node_decay_per_day: f32,
    /// Per-day decay multiplier for edges (0.0-1.0)
    #[serde(default = "default_graph_edge_decay_per_day")]
    pub edge_decay_per_day: f32,
    /// Minimum score before pruning
    #[serde(default = "default_graph_min_score")]
    pub min_score: f32,
}

impl Default for GraphDecayPolicy {
    fn default() -> Self {
        Self {
            node_decay_per_day: default_graph_node_decay_per_day(),
            edge_decay_per_day: default_graph_edge_decay_per_day(),
            min_score: default_graph_min_score(),
        }
    }
}
```

- [ ] **Step 3: Delete the `graph_decay` field on `MemoryConfig`**

Edit `src/config/types/memory.rs`. Remove lines 87-89 (the 3-line block):

```rust
    /// Graph decay policy for entity/relationship pruning
    #[serde(default)]
    pub graph_decay: GraphDecayPolicy,
```

- [ ] **Step 4: Delete the assignment in `MemoryConfig::default`**

Edit `src/config/types/memory.rs`. Remove line 718:

```rust
            graph_decay: GraphDecayPolicy::default(),
```

- [ ] **Step 5: Delete the three `default_graph_*` functions**

Edit `src/config/types/memory.rs`. Remove lines 593-604 (the entire "Graph decay defaults" block):

```rust
// Graph decay defaults
pub fn default_graph_node_decay_per_day() -> f32 {
    0.02
}

pub fn default_graph_edge_decay_per_day() -> f32 {
    0.03
}

pub fn default_graph_min_score() -> f32 {
    0.1
}
```

- [ ] **Step 6: Delete the validators in validate.rs**

Edit `src/config/validate.rs`. Remove lines 318-349 (the three `graph_decay` range check blocks, all consecutive):

```rust
        if !(0.0..=1.0).contains(&self.memory.graph_decay.node_decay_per_day) {
            error!(
                value = self.memory.graph_decay.node_decay_per_day,
                "Invalid graph node_decay_per_day"
            );
            return Err(AlephError::invalid_config(format!(
                "memory.graph_decay.node_decay_per_day must be between 0.0 and 1.0, got {}",
                self.memory.graph_decay.node_decay_per_day
            )));
        }

        if !(0.0..=1.0).contains(&self.memory.graph_decay.edge_decay_per_day) {
            error!(
                value = self.memory.graph_decay.edge_decay_per_day,
                "Invalid graph edge_decay_per_day"
            );
            return Err(AlephError::invalid_config(format!(
                "memory.graph_decay.edge_decay_per_day must be between 0.0 and 1.0, got {}",
                self.memory.graph_decay.edge_decay_per_day
            )));
        }

        if !(0.0..=1.0).contains(&self.memory.graph_decay.min_score) {
            error!(
                value = self.memory.graph_decay.min_score,
                "Invalid graph min_score"
            );
            return Err(AlephError::invalid_config(format!(
                "memory.graph_decay.min_score must be between 0.0 and 1.0, got {}",
                self.memory.graph_decay.min_score
            )));
        }

```

- [ ] **Step 7: Delete the UI hint entries**

Edit `src/config/ui_hints/definitions.rs`. Remove lines 197-214 (three consecutive `memory.graph_decay.*` arms):

```rust
        "memory.graph_decay.node_decay_per_day" => {
            label: "Graph Node Decay/Day",
            help: "Daily decay multiplier for graph nodes",
            group: "memory",
            advanced: true,
        },
        "memory.graph_decay.edge_decay_per_day" => {
            label: "Graph Edge Decay/Day",
            help: "Daily decay multiplier for graph edges",
            group: "memory",
            advanced: true,
        },
        "memory.graph_decay.min_score" => {
            label: "Graph Min Score",
            help: "Minimum graph score before pruning",
            group: "memory",
            advanced: true,
        },
```

- [ ] **Step 8: Fix the serialization test**

Edit `src/config/tests/serialization.rs`. In `test_memory_config_deserialization` (around lines 32-72):

Remove lines 49-53 (the `graph_decay` block in the JSON input):
```rust
        "graph_decay": {
            "node_decay_per_day": 0.05,
            "edge_decay_per_day": 0.06,
            "min_score": 0.2
        },
```

Also remove line 70 assertion:
```rust
    assert_eq!(config.graph_decay.min_score, 0.2);
```

Leave the surrounding JSON intact. The trailing comma on the previous `"dreaming"` block or the leading comma on `"memory_decay"` may need adjusting — after edits, the JSON must be valid. Re-read the file after editing to confirm.

- [ ] **Step 9: cargo check**

Run: `cargo check -p alephcore`
Expected: PASS.

If it fails with references to `graph_decay` or `GraphDecayPolicy` in files this task didn't touch, add them to the task and re-do the edits.

- [ ] **Step 10: cargo test**

Run: `cargo test -p alephcore --lib`
Expected: PASS. In particular, `config::tests::serialization::test_memory_config_deserialization` must pass with the trimmed JSON.

- [ ] **Step 11: Grep verification**

Run:
```bash
grep -rn "graph_decay\|GraphDecayPolicy" src/
```
Expected: empty output.

- [ ] **Step 12: Manual config compatibility test**

Back up your real config, inject a legacy `[memory.graph_decay]` block, confirm startup works, then restore. This exercises the Serde "ignore unknown fields" contract.

```bash
cp ~/.aleph/config.toml /tmp/config.toml.bak
cat >> ~/.aleph/config.toml <<'EOF'

[memory.graph_decay]
node_decay_per_day = 0.01
edge_decay_per_day = 0.02
min_score = 0.3
EOF
```

Kill any running server, then launch (using whatever binary you have built — debug or release):
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v grep
# Expected: empty
```

If you have a built binary:
```bash
target/debug/aleph-server --help  # smoke test the binary loads config
# or start the server briefly and Ctrl-C; watch for deserialize errors.
```

If no built binary exists yet, skip the live test — `cargo check` + `cargo test` already prove the code compiles and the trimmed test JSON deserializes. The legacy-key behavior is purely Serde's default, not new code.

Restore: `cp /tmp/config.toml.bak ~/.aleph/config.toml`

- [ ] **Step 13: Clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30`
Expected: no new warnings.

- [ ] **Step 14: Commit**

```bash
git add -u src/config/
git status --short
```
Expected: only `M` entries under `src/config/` (types/memory.rs, validate.rs, ui_hints/definitions.rs, tests/serialization.rs).

```bash
git commit -m "config: drop dead GraphDecayPolicy and memory.graph_decay

The graph_nodes and graph_edges SQLite tables were removed in earlier
refactors but GraphDecayPolicy and memory.graph_decay stayed behind
with no consumers. Drop the struct, the validators, the UI hints, and
the serialization test assertion.

Existing user TOML with [memory.graph_decay] sections continues to
load cleanly — Serde silently ignores unknown fields (no
deny_unknown_fields anywhere under src/config/)."
```

---

## Task 3: Delete the entire `src/wiki/` directory

**Files:**
- Delete directory: `src/wiki/` (6 files)
- Modify: `src/lib.rs:82` (remove `pub mod wiki;`)
- Modify: `src/executor/builtin_registry/registry.rs:928-935` (remove `"wiki_manage"` match arm)

- [ ] **Step 1: Pre-flight verification**

Confirm no external consumers of `crate::wiki::*`:
```bash
grep -rn "crate::wiki\|use crate::wiki" src/ | grep -v "^src/wiki/"
```
Expected: empty output.

Confirm `wiki_manage` only appears in registry.rs and src/wiki/ itself:
```bash
grep -rn "wiki_manage" src/
```
Expected: only matches inside `src/wiki/` (which we're deleting) and `src/executor/builtin_registry/registry.rs` (which we're editing) and possibly `src/builtin_tools/note_manage.rs` — open that file and confirm the mentions are just comments describing the replacement relationship. Those comment-only mentions are fine to leave.

If unexpected matches appear elsewhere, stop and investigate.

- [ ] **Step 2: Delete the directory**

Run:
```bash
rm -rf src/wiki/
```

- [ ] **Step 3: Remove the `pub mod wiki;` declaration**

Edit `src/lib.rs`. Remove line 82:
```rust
pub mod wiki;
```

- [ ] **Step 4: Remove the `wiki_manage` stub from registry.rs**

Edit `src/executor/builtin_registry/registry.rs`. Remove lines 928-935:
```rust
            "wiki_manage" => {
                // wiki_manage has been removed — redirect to note_manage
                Box::pin(async move {
                    Err(AlephError::tool(
                        "wiki_manage has been removed. Use note_manage instead.",
                    ))
                })
            }
```

The preceding arm (`"skill_manage"`, around line 925) ends with `}` and the following arm (`"note_manage"`, around line 936) should now sit immediately after. Verify the match expression stays syntactically valid by re-reading lines 920-940 after the edit.

- [ ] **Step 5: cargo check**

Run: `cargo check -p alephcore`
Expected: PASS.

If it fails on a missing `crate::wiki::*` symbol in an unexpected file, the pre-flight grep missed it. Open that file, decide whether that consumer should also be deleted or retained, and adjust.

- [ ] **Step 6: cargo test**

Run: `cargo test -p alephcore --lib`
Expected: PASS. Any test named after `wiki_` should now be gone (they lived in the deleted directory).

- [ ] **Step 7: Grep verification**

Run:
```bash
grep -rn "crate::wiki\|wiki_manage\|WikiGitManager\|generate_index_content\|use.*wiki::" src/
```
Expected: nothing in the source code. `note_manage.rs` may still contain the comment `"Replaces `wiki_manage`"` — that historical note is fine (it's a comment, not a symbol reference). If you want a stricter grep:
```bash
grep -rn "wiki_manage" src/
```
Expected: only comments in `src/builtin_tools/note_manage.rs`.

- [ ] **Step 8: Clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30`
Expected: no new warnings.

- [ ] **Step 9: Commit**

```bash
git add -A src/wiki src/lib.rs src/executor/builtin_registry/registry.rs
git status --short
```
Expected: six `D` entries for the `src/wiki/` tree plus two `M` entries.

```bash
git commit -m "wiki: remove entire src/wiki/ directory

All wiki operations are handled by src/builtin_tools/note_manage.rs
with category=\"wiki\". The legacy src/wiki/ module (manage.rs,
wikilink.rs, git.rs, index.rs, and its path helpers) had no external
callers, verified by grep for crate::wiki:: across src/.

The src/memory/notes/wikilink.rs file is a separate, active parser
used by note_manage and dreaming stages and is untouched here.

Also drop the wiki_manage stub in builtin_registry that returned a
\"removed, use note_manage\" error — the migration window is over."
```

---

## Task 4: Remove unconsumed dreaming config keys

**Files:**
- Modify: `src/config/types/memory.rs`

Keeps `src/memory/dreaming/stages/types.rs::dbscan` (the utility function) untouched.

- [ ] **Step 1: Pre-flight verification**

Confirm the three keys are unread except in the config file itself:
```bash
grep -rn "drift_similarity_threshold\|cluster_dbscan_eps\|cluster_dbscan_min_samples" src/
```
Expected: only matches in `src/config/types/memory.rs` (field definitions, defaults, accessors, tests).

If dreaming stages reference them, stop — the spec's assumption is wrong, re-evaluate.

- [ ] **Step 2: Delete the three field declarations on `DreamingConfig`**

Edit `src/config/types/memory.rs`. Remove these lines from the `DreamingConfig` struct (lines 331-339):

```rust
    /// DBSCAN epsilon (cosine distance threshold)
    #[serde(default = "default_cluster_dbscan_eps")]
    pub cluster_dbscan_eps: f32,
    /// DBSCAN minimum samples per cluster
    #[serde(default = "default_cluster_dbscan_min_samples")]
    pub cluster_dbscan_min_samples: usize,
    /// Drift detection similarity threshold
    #[serde(default = "default_drift_similarity_threshold")]
    pub drift_similarity_threshold: f32,
```

- [ ] **Step 3: Delete the corresponding lines in `impl Default for DreamingConfig`**

Edit `src/config/types/memory.rs`. In the `Default` impl (around lines 351-369), remove these three lines (they sit as lines 361-363):
```rust
            cluster_dbscan_eps: default_cluster_dbscan_eps(),
            cluster_dbscan_min_samples: default_cluster_dbscan_min_samples(),
            drift_similarity_threshold: default_drift_similarity_threshold(),
```

- [ ] **Step 4: Delete the three accessor methods**

Edit `src/config/types/memory.rs`. In `impl DreamingConfig` (around lines 371-396), remove these three accessor methods (currently lines 378-386):

```rust
    pub fn cluster_dbscan_eps(&self) -> f32 {
        self.cluster_dbscan_eps
    }
    pub fn cluster_dbscan_min_samples(&self) -> usize {
        self.cluster_dbscan_min_samples
    }
    pub fn drift_similarity_threshold(&self) -> f32 {
        self.drift_similarity_threshold
    }
```

- [ ] **Step 5: Delete the three `default_*` helper functions**

Edit `src/config/types/memory.rs`. Remove lines 569-579:

```rust
pub fn default_cluster_dbscan_eps() -> f32 {
    0.3
}

pub fn default_cluster_dbscan_min_samples() -> usize {
    2
}

pub fn default_drift_similarity_threshold() -> f32 {
    0.85
}

```

- [ ] **Step 6: Update the two test assertions**

Edit `src/config/types/memory.rs`. In the `tests` module (around lines 740-784):

In `dreaming_config_defaults_include_new_fields` (around lines 745-755), remove these assertion lines (currently lines 749-751):
```rust
        assert!((config.cluster_dbscan_eps - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.cluster_dbscan_min_samples, 2);
        assert!((config.drift_similarity_threshold - 0.85).abs() < f32::EPSILON);
```

In `dreaming_config_accessors_match_fields` (around lines 757-784), remove these assertion blocks (currently lines 762-770):
```rust
        assert!((config.cluster_dbscan_eps() - config.cluster_dbscan_eps).abs() < f32::EPSILON);
        assert_eq!(
            config.cluster_dbscan_min_samples(),
            config.cluster_dbscan_min_samples
        );
        assert!(
            (config.drift_similarity_threshold() - config.drift_similarity_threshold).abs()
                < f32::EPSILON
        );
```

- [ ] **Step 7: cargo check**

Run: `cargo check -p alephcore`
Expected: PASS.

- [ ] **Step 8: cargo test**

Run: `cargo test -p alephcore --lib`
Expected: PASS. The two trimmed dreaming config tests must pass.

- [ ] **Step 9: Grep verification**

Run:
```bash
grep -rn "drift_similarity_threshold\|cluster_dbscan_eps\|cluster_dbscan_min_samples" src/
```
Expected: empty output.

Sanity-check the `dbscan` utility function was **not** deleted:
```bash
grep -n "fn dbscan" src/memory/dreaming/stages/types.rs
```
Expected: a match. This function stays (it's reusable infrastructure, unlike the config keys).

- [ ] **Step 10: Clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30`
Expected: no new warnings.

- [ ] **Step 11: Commit**

```bash
git add -u src/config/types/memory.rs
git status --short
```
Expected: one `M` entry.

```bash
git commit -m "config: drop dead dreaming DBSCAN and drift similarity thresholds

drift_similarity_threshold, cluster_dbscan_eps, and
cluster_dbscan_min_samples on DreamingConfig were unread by any
stage. NoteDrift walks the wikilink graph; NoteSynthesis groups by
category. These config keys were acting on behalf of the LLM
(R8 redline) and are removed.

The dbscan utility function in src/memory/dreaming/stages/types.rs
is retained — it is reusable infrastructure, not an opinionated
heuristic."
```

---

## Task 5: Rebuild the `dream_reports` table

This task has real data-migration risk. It is the only task that writes schema migrations. Follow TDD: write the migration test first, run it red, implement, run it green.

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/sqlite/dream_reports.rs`

**Legacy columns to drop:** `facts_collected`, `clusters_found`, `drift_detected`, `drift_summary`, `candidates_evaluated`, `facts_promoted`, `promotion_details`, `facts_decayed`, `facts_pruned`, `nodes_decayed`, `edges_decayed`.

**Columns retained:** `id`, `pipeline_type`, `started_at`, `finished_at`, `duration_ms`, `synthesis_count`, `errors`, `namespace`.

### 5A: Write the migration test first (RED)

- [ ] **Step 1: Open the schema.rs test module**

Existing tests live at the bottom of `src/memory/store/sqlite/schema.rs` (around line 314). The new test goes in the same module.

- [ ] **Step 2: Add the failing migration test**

Append this test inside the `#[cfg(test)] mod tests { ... }` block in `src/memory/store/sqlite/schema.rs`:

```rust
    #[test]
    fn dream_reports_legacy_schema_migrates_to_new_layout() {
        let conn = Connection::open_in_memory().expect("in-memory db");

        // Build the 19-column legacy schema by hand.
        conn.execute_batch(
            r#"CREATE TABLE dream_reports (
                id                    TEXT PRIMARY KEY,
                pipeline_type         TEXT NOT NULL,
                started_at            INTEGER NOT NULL,
                finished_at           INTEGER NOT NULL,
                duration_ms           INTEGER NOT NULL,
                facts_collected       INTEGER NOT NULL DEFAULT 0,
                clusters_found        INTEGER NOT NULL DEFAULT 0,
                drift_detected        INTEGER NOT NULL DEFAULT 0,
                drift_summary         TEXT,
                candidates_evaluated  INTEGER NOT NULL DEFAULT 0,
                facts_promoted        INTEGER NOT NULL DEFAULT 0,
                promotion_details     TEXT,
                facts_decayed         INTEGER NOT NULL DEFAULT 0,
                facts_pruned          INTEGER NOT NULL DEFAULT 0,
                nodes_decayed         INTEGER NOT NULL DEFAULT 0,
                edges_decayed         INTEGER NOT NULL DEFAULT 0,
                synthesis_count       INTEGER NOT NULL DEFAULT 0,
                errors                TEXT,
                namespace             TEXT NOT NULL DEFAULT 'owner'
            );"#,
        )
        .expect("create legacy dream_reports");

        conn.execute_batch(
            "INSERT INTO dream_reports
               (id, pipeline_type, started_at, finished_at, duration_ms,
                facts_collected, clusters_found, drift_detected, drift_summary,
                candidates_evaluated, facts_promoted, promotion_details,
                facts_decayed, facts_pruned, nodes_decayed, edges_decayed,
                synthesis_count, errors, namespace)
             VALUES
               ('r1', 'full', 1000, 2000, 1000,
                42, 3, 1, 'topic shift',
                10, 2, 'promoted f1',
                5, 1, 3, 2,
                1, NULL, 'owner'),
               ('r2', 'weekly', 3000, 5000, 2000,
                7, 0, 0, NULL,
                0, 0, NULL,
                0, 0, 0, 0,
                4, 'retry', 'owner');",
        )
        .expect("insert legacy rows");

        // Run the normal schema-init path; this must migrate the table.
        init_schema(&conn).expect("init_schema");

        // Assert: exactly the 8 new columns, in any order.
        let mut stmt = conn
            .prepare("PRAGMA table_info(dream_reports)")
            .expect("prepare pragma");
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("pragma query")
            .collect::<Result<Vec<_>, _>>()
            .expect("pragma collect");

        let mut sorted = cols.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            vec![
                "duration_ms",
                "errors",
                "finished_at",
                "id",
                "namespace",
                "pipeline_type",
                "started_at",
                "synthesis_count",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>(),
            "dream_reports must have exactly the 8 retained columns"
        );

        // Assert: both rows remain with retained data intact.
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dream_reports", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 2);

        let r1: (String, String, i64, i64, i64, u32, Option<String>, String) = conn
            .query_row(
                "SELECT id, pipeline_type, started_at, finished_at,
                        duration_ms, synthesis_count, errors, namespace
                   FROM dream_reports WHERE id = 'r1'",
                [],
                |row| {
                    Ok((
                        row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                        row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(r1.0, "r1");
        assert_eq!(r1.1, "full");
        assert_eq!(r1.2, 1000);
        assert_eq!(r1.3, 2000);
        assert_eq!(r1.4, 1000);
        assert_eq!(r1.5, 1);
        assert_eq!(r1.6, None);
        assert_eq!(r1.7, "owner");

        // Idempotency: running init_schema again must not error.
        init_schema(&conn).expect("second init_schema should be idempotent");

        // Same row still there.
        let row_count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM dream_reports", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count_after, 2);
    }
```

- [ ] **Step 3: Run the new test — expect RED**

Run:
```bash
cargo test -p alephcore --lib dream_reports_legacy_schema_migrates_to_new_layout -- --nocapture
```
Expected: FAIL. Message will likely be about column mismatch (new `init_schema` still creates the 19-column DDL) or the `CREATE TABLE IF NOT EXISTS` clashing.

This confirms the test actually exercises the migration path.

### 5B: Implement the new schema + migration (GREEN)

- [ ] **Step 4: Replace `DREAM_REPORTS_DDL` with the new 8-column layout**

Edit `src/memory/store/sqlite/schema.rs`. Replace the existing `DREAM_REPORTS_DDL` constant (lines 64-89) with:

```rust
const DREAM_REPORTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS dream_reports (
    id              TEXT PRIMARY KEY,
    pipeline_type   TEXT NOT NULL,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER NOT NULL,
    duration_ms     INTEGER NOT NULL,
    synthesis_count INTEGER NOT NULL DEFAULT 0,
    errors          TEXT,
    namespace       TEXT NOT NULL DEFAULT 'owner'
);

CREATE INDEX IF NOT EXISTS idx_dream_reports_started
    ON dream_reports(started_at);
"#;
```

- [ ] **Step 5: Add the migration function**

Edit `src/memory/store/sqlite/schema.rs`. Add this function immediately after `migrate_recall_signals_note_path` (so around line 58, before the `// Dream reports table` section):

```rust
/// Rebuild `dream_reports` without the legacy fact/graph counters.
///
/// The presence of the `facts_collected` column signals the legacy
/// 19-column schema. When seen, rebuild the table in a transaction,
/// preserving the 8 retained columns for every row.
///
/// Safe to call on fresh or already-migrated databases.
fn migrate_dream_reports_drop_legacy_fields(conn: &Connection) -> Result<(), AlephError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(dream_reports)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info dream_reports: {e}")))?;
    let has_legacy = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.map(|n| n == "facts_collected").unwrap_or(false));
    drop(stmt);

    if !has_legacy {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        BEGIN;
        CREATE TABLE dream_reports_new (
            id              TEXT PRIMARY KEY,
            pipeline_type   TEXT NOT NULL,
            started_at      INTEGER NOT NULL,
            finished_at     INTEGER NOT NULL,
            duration_ms     INTEGER NOT NULL,
            synthesis_count INTEGER NOT NULL DEFAULT 0,
            errors          TEXT,
            namespace       TEXT NOT NULL DEFAULT 'owner'
        );
        INSERT INTO dream_reports_new
            (id, pipeline_type, started_at, finished_at,
             duration_ms, synthesis_count, errors, namespace)
        SELECT id, pipeline_type, started_at, finished_at,
               duration_ms, synthesis_count, errors, namespace
          FROM dream_reports;
        DROP TABLE dream_reports;
        ALTER TABLE dream_reports_new RENAME TO dream_reports;
        CREATE INDEX IF NOT EXISTS idx_dream_reports_started
            ON dream_reports(started_at);
        COMMIT;
        "#,
    )
    .map_err(|e| AlephError::config(format!(
        "Failed to migrate dream_reports to new schema: {e}"
    )))?;

    Ok(())
}
```

- [ ] **Step 6: Call the migration from `init_schema`**

Edit `src/memory/store/sqlite/schema.rs`. The current `init_schema` function (around lines 235-269) runs `CREATE TABLE IF NOT EXISTS dream_reports` at line 239-240. **Before** that line (between the `recall_signals` batch at line 236-237 and the `dream_reports` batch at line 239-240), insert the migration call:

```rust
    // Migrate legacy 19-column dream_reports to the new 8-column layout
    // before the CREATE TABLE IF NOT EXISTS no-ops on the rebuilt table.
    migrate_dream_reports_drop_legacy_fields(conn)?;

```

The resulting `init_schema` fragment should look like:

```rust
    conn.execute_batch(RECALL_SIGNALS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create recall_signals table: {e}")))?;

    // Migrate legacy 19-column dream_reports to the new 8-column layout
    // before the CREATE TABLE IF NOT EXISTS no-ops on the rebuilt table.
    migrate_dream_reports_drop_legacy_fields(conn)?;

    conn.execute_batch(DREAM_REPORTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create dream_reports table: {e}")))?;
```

- [ ] **Step 7: Run the migration test — expect GREEN**

Run:
```bash
cargo test -p alephcore --lib dream_reports_legacy_schema_migrates_to_new_layout -- --nocapture
```
Expected: PASS.

### 5C: Trim the Rust struct + SQL in dream_reports.rs

- [ ] **Step 8: Trim `PersistedDreamReport`**

Edit `src/memory/store/sqlite/dream_reports.rs`. Replace the entire struct (lines 17-39) with:

```rust
/// A persisted dream pipeline execution report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedDreamReport {
    pub id: String,
    pub pipeline_type: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: i64,
    pub synthesis_count: u32,
    pub errors: Option<String>,
    pub namespace: String,
}
```

- [ ] **Step 9: Shrink the INSERT in `insert_dream_report`**

Edit `src/memory/store/sqlite/dream_reports.rs`. Replace `insert_dream_report` (lines 47-86) with:

```rust
    /// Insert a dream pipeline report into the audit log.
    pub fn insert_dream_report(&self, report: &PersistedDreamReport) -> Result<(), AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        conn.execute(
            "INSERT INTO dream_reports \
             (id, pipeline_type, started_at, finished_at, duration_ms, \
              synthesis_count, errors, namespace) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                report.id,
                report.pipeline_type,
                report.started_at,
                report.finished_at,
                report.duration_ms,
                report.synthesis_count,
                report.errors,
                report.namespace,
            ],
        )
        .map_err(|e| AlephError::config(format!("insert_dream_report: {e}")))?;

        Ok(())
    }
```

- [ ] **Step 10: Shrink the SELECT and row mapping in `recent_dream_reports`**

Edit `src/memory/store/sqlite/dream_reports.rs`. Replace `recent_dream_reports` (lines 88-145) with:

```rust
    /// Query recent dream reports, ordered by `started_at` descending.
    pub fn recent_dream_reports(
        &self,
        limit: usize,
    ) -> Result<Vec<PersistedDreamReport>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, pipeline_type, started_at, finished_at, duration_ms, \
                 synthesis_count, errors, namespace \
                 FROM dream_reports ORDER BY started_at DESC LIMIT ?1",
            )
            .map_err(|e| AlephError::config(format!("recent_dream_reports prepare: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(PersistedDreamReport {
                    id: row.get("id")?,
                    pipeline_type: row.get("pipeline_type")?,
                    started_at: row.get("started_at")?,
                    finished_at: row.get("finished_at")?,
                    duration_ms: row.get("duration_ms")?,
                    synthesis_count: row.get("synthesis_count")?,
                    errors: row.get("errors")?,
                    namespace: row.get("namespace")?,
                })
            })
            .map_err(|e| AlephError::config(format!("recent_dream_reports query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| {
                    AlephError::config(format!("recent_dream_reports row: {e}"))
                })?,
            );
        }
        Ok(results)
    }
```

- [ ] **Step 11: Trim the `sample_report` test helper**

Edit `src/memory/store/sqlite/dream_reports.rs`. Replace the `sample_report` helper (lines 180-202) with:

```rust
    fn sample_report(id: &str, started: i64, finished: i64) -> PersistedDreamReport {
        PersistedDreamReport {
            id: id.to_string(),
            pipeline_type: "full".to_string(),
            started_at: started,
            finished_at: finished,
            duration_ms: finished - started,
            synthesis_count: 1,
            errors: None,
            namespace: "owner".to_string(),
        }
    }
```

- [ ] **Step 12: Trim the `insert_and_query_report` test**

Edit `src/memory/store/sqlite/dream_reports.rs`. Replace `insert_and_query_report` (lines 204-234) with:

```rust
    #[test]
    fn insert_and_query_report() {
        let store = setup();
        let report = sample_report("r1", 1000, 2000);

        store.insert_dream_report(&report).unwrap();

        let reports = store.recent_dream_reports(10).unwrap();
        assert_eq!(reports.len(), 1);

        let r = &reports[0];
        assert_eq!(r.id, "r1");
        assert_eq!(r.pipeline_type, "full");
        assert_eq!(r.started_at, 1000);
        assert_eq!(r.finished_at, 2000);
        assert_eq!(r.duration_ms, 1000);
        assert_eq!(r.synthesis_count, 1);
        assert!(r.errors.is_none());
        assert_eq!(r.namespace, "owner");
    }
```

The other two tests (`latest_ts_empty`, `latest_ts_after_insert`) do not reference any dropped field and need no change.

- [ ] **Step 13: cargo check**

Run: `cargo check -p alephcore`
Expected: PASS.

If it fails because some upstream caller constructs a `PersistedDreamReport` with the legacy fields, open that file and delete those field assignments. (The pre-flight grep in Task 5A pre-flight should confirm no such caller exists outside `dream_reports.rs` — but compile errors are authoritative.)

- [ ] **Step 14: Run all dream_reports tests**

Run:
```bash
cargo test -p alephcore --lib dream_reports -- --nocapture
```
Expected: PASS (including the new `dream_reports_legacy_schema_migrates_to_new_layout` test and the existing idempotency test).

- [ ] **Step 15: Run the full test suite**

Run: `cargo test -p alephcore --lib`
Expected: PASS.

- [ ] **Step 16: Grep verification**

Run:
```bash
grep -rn "facts_collected\|facts_promoted\|facts_decayed\|facts_pruned\|nodes_decayed\|edges_decayed\|clusters_found\|drift_detected\|drift_summary\|promotion_details\|candidates_evaluated" src/
```
Expected: matches only inside the new migration test (the 19-column DDL in `schema.rs` tests and the INSERT that seeds the legacy rows) and inside `migrate_dream_reports_drop_legacy_fields` (the legacy-column marker `facts_collected`). No matches in production code outside those two sites.

- [ ] **Step 17: Clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30`
Expected: no new warnings.

- [ ] **Step 18: Manual live-DB migration (optional but strongly recommended)**

Only run this if you have a built `aleph-server` binary and a real `memory.db`.

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v grep
# Must be empty.

cp ~/.aleph/data/memory.db /tmp/memory.db.pre-cleanup

# Start the server briefly (Ctrl-C after you see the init logs).
target/release/aleph-server start
# Or cargo run --release --bin aleph-server -- start
```

Watch the init logs. They should complete without errors.

```bash
# Kill again before inspecting.
pkill -f "target/release/aleph-server" 2>/dev/null
sleep 2

sqlite3 ~/.aleph/data/memory.db "PRAGMA table_info(dream_reports);"
# Expected: exactly 8 rows (id, pipeline_type, started_at, finished_at,
# duration_ms, synthesis_count, errors, namespace).

sqlite3 ~/.aleph/data/memory.db "SELECT COUNT(*) FROM dream_reports;"
# Expected: same row count as /tmp/memory.db.pre-cleanup had.
```

If the migration corrupts something, restore: `cp /tmp/memory.db.pre-cleanup ~/.aleph/data/memory.db` and `git revert HEAD` (after committing, or undo your uncommitted changes with `git checkout -- src/memory/store/sqlite/`).

- [ ] **Step 19: Commit**

```bash
git add -u src/memory/store/sqlite/
git status --short
```
Expected: `M  src/memory/store/sqlite/schema.rs` and `M  src/memory/store/sqlite/dream_reports.rs`.

```bash
git commit -m "memory: rebuild dream_reports table, drop legacy fact/graph counters

The dream_reports table carried 11 columns that no writer populated
with real data (facts_collected, clusters_found, drift_detected,
drift_summary, candidates_evaluated, facts_promoted,
promotion_details, facts_decayed, facts_pruned, nodes_decayed,
edges_decayed). Trim the table to the 8 fields that carry real
signal: id, pipeline_type, started_at, finished_at, duration_ms,
synthesis_count, errors, namespace.

A new migrate_dream_reports_drop_legacy_fields function rebuilds
the table inside a transaction when the legacy schema is detected
(via PRAGMA table_info looking for facts_collected). It runs
before the CREATE TABLE IF NOT EXISTS in init_schema so that new
databases get the new schema directly and legacy databases are
migrated in place. The eight retained columns are preserved for
every row; the eleven legacy columns are dropped without export.

Migration pattern follows migrate_recall_signals_note_path: the
PRAGMA guard makes it idempotent across repeated startups."
```

---

## Final Verification

After all five commits land, run the whole matrix once more:

- [ ] **Step 1: Full check**

Run: `cargo check -p alephcore`
Expected: PASS.

- [ ] **Step 2: Full test**

Run: `cargo test -p alephcore --lib`
Expected: PASS.

- [ ] **Step 3: Lint**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -40`
Expected: no new warnings.

- [ ] **Step 4: Global dead-symbol sweep**

Run:
```bash
grep -rn "MemoryStrength\|GraphDecayPolicy\|wiki_manage\|drift_similarity_threshold\|cluster_dbscan_eps\|cluster_dbscan_min_samples" src/ | grep -v "^src/builtin_tools/note_manage.rs:.*//"
```
Expected: empty output (the filter lets through comment-only mentions in `note_manage.rs` if any exist).

- [ ] **Step 5: Git log sanity check**

Run: `git log --oneline -5`
Expected: five new commits on top of `83a8bd16`, in this order (most recent first):
1. `memory: rebuild dream_reports table, drop legacy fact/graph counters`
2. `config: drop dead dreaming DBSCAN and drift similarity thresholds`
3. `wiki: remove entire src/wiki/ directory`
4. `config: drop dead GraphDecayPolicy and memory.graph_decay`
5. `memory: remove dead decay.rs and MemoryStrength re-export`

---

## Rollback Cheat Sheet

| Situation | Action |
|---|---|
| Mid-task compile failure | Fix in place; do not commit half-done work |
| Cleanups 1–4 regression post-commit | `git revert <sha>` |
| Cleanup 5 corrupts live DB | Restore `memory.db` from `/tmp/memory.db.pre-cleanup`, then `git revert <sha>` |
| Need to back out multiple commits | Revert in reverse order (dream_reports → dreaming → wiki → graph_decay → decay) |
