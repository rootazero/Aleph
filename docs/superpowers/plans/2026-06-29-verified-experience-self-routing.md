# Verified-Experience Self-Routing (VESR) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Work in an isolated git worktree (never touch `main` directly); choose the worktree base accounting for the pre-existing uncommitted changes in `runner_impl.rs` / `model_catalog/*` / `orchestrator_init.rs` / `harness_bridge/mod.rs`.

**Goal:** Close the model-routing "information deficit" (paper 2606.22902) by capturing verified per-run execution outcomes and recalling them at run-start, so Aleph's existing LLM self-routing (`select_model`) decides with real experience — without building any router module.

**Architecture:** Routing-experience capture + run-start recall, built as a `TraceSink` decorator (`OutcomeObserver`) + sqlite-vec mirror tables, with a `RoutingRecall` provider injecting fenced experience at the run-start prompt seam. Decision authority stays 100% with the LLM; the system only stores raw facts and surfaces them. Zero `src/harness/` changes (R10), zero second embedder (R3), no judgment columns (R7).

**Tech Stack:** Rust (tokio), SQLite + sqlite-vec (`vec0` MATCH kNN), `rusqlite`, the existing `Arc<dyn EmbeddingProvider>`, the `TraceSink` decorator pattern, the `PromptBuilder` run-start seam.

Locked defaults honored: §5.5 (`list_models` per-model aggregates) **deferred to v1.1**; §7 **sink-construction model capture** is the primary path (`src/harness/trace.rs` stays byte-identical); **O4 = mark-not-filter** for unavailable providers; **per-agent isolation**; **self-bounded retention**; v1 captures **top-level runs only** (subagent experience capture deferred — see Global Constraints "Subagent scope").

## Global Constraints

- **Crate-path discipline (BLOCKER fix).** Code inside the `alephcore` **library** (`src/memory/…`, `src/routing/…`, `src/orchestrator/…`, `src/thinker/…`) refers to crate items as `crate::…`. Code in the **binary** (`src/bin/aleph-server/…`) refers to library items as `alephcore::…`. Task 4b runs in the binary (`alephcore::`); Tasks 1–3, 4a, 4c, 4d run in the library (`crate::`).
- **`pub(crate)` boundary (BLOCKER fix).** `crate::gateway::handlers::resolve_vault_secret` is `pub(crate)` (`src/gateway/handlers/mod.rs:161`) and is invisible to the binary. The availability gate is therefore built by a **new `pub` lib helper** (`crate::routing::provider_availability_from_config`, Task 4b) that calls `resolve_vault_secret` from inside the library; the binary only calls the `pub` helper.
- **Harness budget untouched (R10 / N1).** No file under `src/harness/` is edited. `src/harness/trace.rs` has zero diff. `routing_store`/`routing_recall` live on the **orchestrator runner struct** (`AgentHarnessRunner`), never on `HarnessDeps`.
- **Agent-id key (TYPE fix).** The runner's agent id is `spec.agent` (there is no bare `agent_id` local — see `runner_impl.rs:124`). Record and recall both key on `spec.agent`, or per-agent isolation silently breaks.
- **Single embedder (R3).** `RoutingExperienceStore` reuses the boot `Arc<dyn EmbeddingProvider>` (`agent_result.embedder`). Stub embedders appear only in `#[cfg(test)]`.
- **Build policy.** The only broad command is the single `cargo check -p alephcore --lib` where prescribed; everything else is a narrow `cargo test -p alephcore --lib <filter>`. The crate builds under plain `cargo check`/`cargo test` (not `-D warnings`); transient unused-import warnings between a "create types" step and its "implement" step are acceptable and disappear by the task's final step. Do **not** run `just test-all`.
- **Seam deviation (acknowledged, not a violation).** Recall is invoked **once per run** in `runner_impl::run` (run-start, pre-loop) and its fenced text is threaded into `build_system_prompt` as an `Option<String>` param, mirroring how `memory_text: Option<String>` is threaded. This diverges from spec §5.4 (which co-locates the recall *call* inside `build_system_prompt`) but is still once-per-run pre-loop, so R10 holds. `RoutingRecall` returns `Option<String>` (already fence-wrapped), not `Option<UnifiedMessage>` (spec §5.4) — the prompt-build seam consumes a `String`, so no `UnifiedMessage` is constructed in `src/routing/` (R3/P6).
- **Three post-trace fields are `None`/`0` in v1.** `LoopTraceEvent::SessionCompleted` carries iterations / tool_calls_made / terminate_reason / token_breakdown / duration_ms / tool_timeline **verbatim** but NOT `estimated_cost` / `context_tokens` / `context_window` (those are synthesized later into `FlowOutcome`). To keep the observer purely verbatim (R7: no pricing call) and `trace.rs` zero-diff, v1 sets those three to `None`/`0`. The raw `token_breakdown` columns carry the cost-relevant facts (cost-as-data, D2); USD enrichment rides with deferred §5.5/v1.1.
- **Subagent scope (W3 / E2, verified).** Verified data flow: the gateway run loop builds the per-run `trace_sink` (`inner.rs:644-688`), hands the **raw** sink both to `SubagentTool::with_trace_sink` (`inner.rs:760`) and to `orchestrator.run` (`inner.rs:851`). `runner_impl::run` wraps the raw sink with `OutcomeObserver` **after** the tool already captured the raw one, and the wrapped observer lands **only** in the *top-level* `HarnessDeps.trace_sink` (`:264`). Subagents inherit `base.trace_sink` (`subagent_spawner/mod.rs:362`) = the **raw** sink, so a child's `SessionCompleted` never reaches the parent's observer. Consequence: **v1 records top-level runs only; subagent routing-experience capture is deferred**, and the wrap-after-construction ordering structurally prevents any cross-agent contamination — no spawn-side code is needed to stay safe. The model-attribution precedence (`explicit → model_hint → native`) the spawn path uses (`subagent_spawner/mod.rs:297`) is captured as a pure, tested helper (`resolve_routing_model_id`, Task 2) and reused by the runner.

## File Structure

| File | Action | Purpose |
|------|--------|---------|
| `src/memory/store/sqlite/schema/ddl.rs` | Modify | Add `ROUTING_EXPERIENCE_DDL` const (raw-fact columns + vec map). |
| `src/memory/store/sqlite/schema/mod.rs` | Modify | `init_schema()` runs `ROUTING_EXPERIENCE_DDL`; `init_vec_tables()` creates `routing_exp_vec_{768,1024,1536}`. |
| `src/memory/store/sqlite/vec.rs` | Modify | Add `routing_exp_vec_table_for_dim(dim)` (mirror of `notes_vec_table_for_dim`). |
| `src/memory/store/sqlite/routing_experience.rs` | **Create** | `RoutingExperienceRow` / `RoutingNeighbor` + `record` / `recall` (kNN) / `prune` on `SqliteMemoryBackend`. |
| `src/memory/store/sqlite/mod.rs` | Modify (~:10) | `pub mod routing_experience;`. |
| `src/routing/experience_store.rs` | **Create** | `RoutingOutcome` + `RoutingExperienceStore` facade over the shared embedder + self-bounded retention. |
| `src/routing/observer.rs` | **Create** | `OutcomeObserver` (`TraceSink` decorator, sink-construction model capture) + pure `outcome_from_session_completed`. |
| `src/routing/recall.rs` | **Create** | `RoutingRecall` + `ProviderAvailability` + `provider_availability_from_config` (lib gate) + neighbor rendering (O4 mark). |
| `src/routing/mod.rs` | Modify | `pub mod` decls + `RoutingAttribution` + `resolve_routing_model_id` (W3) + re-exports + e2e tests. |
| `src/thinker/prompt_layer.rs` | Modify | `LayerInput.routing_experience_user_message` field + `None` in 4 ctors + `with_routing_experience_message` setter. |
| `src/thinker/prompt_builder/mod.rs` | Modify | `PromptBuilder.routing_experience_user_message` field + `None` default + `with_routing_experience_message` setter. |
| `src/thinker/prompt_builder/cache.rs` | Modify (~:68) | Thread routing field into `LayerInput` on the production cached path + W1 emission test. |
| `src/thinker/layers/memory_augmentation.rs` | Modify (~:42) | Emit `routing_experience_user_message` verbatim after memory. |
| `src/orchestrator/harness_bridge/mod.rs` | Modify (~:222) | Add `routing_store` / `routing_recall` fields to `AgentHarnessRunner`. |
| `src/bin/aleph-server/commands/start/orchestrator_init.rs` | Modify (~:41, :223) | New `embedder` param; assemble store + recall; set struct fields. |
| `src/bin/aleph-server/commands/start/mod.rs` | Modify (~:1173) | Pass `agent_result.embedder.clone()` into `initialize_orchestrator`. |
| `src/orchestrator/harness_bridge/runner_impl.rs` | Modify (~:124, :152, :254) | Per-run attribution + recall → `routing_text`, frozen model id, observer wrap. |
| `src/orchestrator/harness_bridge/prompt_build.rs` | Modify (~:140, :349) | `build_system_prompt` gains `routing_text: Option<String>`; inject next to memory. |

---

### Task 1: `routing_experiences` storage primitive (DDL + record + kNN recall + agent isolation + dimension targeting + self-bounded retention)

**Files:**
- Modify `src/memory/store/sqlite/schema/ddl.rs` (add `ROUTING_EXPERIENCE_DDL`)
- Modify `src/memory/store/sqlite/schema/mod.rs` (`init_schema` + `init_vec_tables`)
- Modify `src/memory/store/sqlite/vec.rs` (add `routing_exp_vec_table_for_dim`)
- Create `src/memory/store/sqlite/routing_experience.rs`
- Modify `src/memory/store/sqlite/mod.rs` (~:10, add `pub mod routing_experience;`)

**Interfaces consumed (verbatim):** `crate::error::AlephError` (`AlephError::config(format!(...))`); `SqliteMemoryBackend { conn: Mutex<Connection> }` (field `conn` is private to the `sqlite` module but reachable from the child module `routing_experience`); `super::vec::embedding_to_blob(&[f32]) -> Vec<u8>`; `crate::memory::store::sqlite::schema::ddl::vec_table_ddl(dim: u32, name: &str) -> String`; `rusqlite::{params, OptionalExtension}`; `init_vec_tables` already creates `notes_vec_{768,1024,1536}` so the isolation test below has a `notes_vec_768` to assert against.

- [ ] **Step 1: Add DDL const + schema/vec wiring (scaffolding the later tests require).** In `src/memory/store/sqlite/schema/ddl.rs` add:
```rust
pub const ROUTING_EXPERIENCE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS routing_experiences (
    id                  TEXT PRIMARY KEY,
    agent_id            TEXT NOT NULL,
    model_id            TEXT NOT NULL,
    provider_id         TEXT NOT NULL,
    terminate_reason    TEXT NOT NULL,
    iterations          INTEGER NOT NULL,
    tool_calls          INTEGER NOT NULL,
    tool_error_count    INTEGER NOT NULL,
    tool_call_total     INTEGER NOT NULL,
    tok_input           INTEGER NOT NULL,
    tok_output          INTEGER NOT NULL,
    tok_cache_read      INTEGER NOT NULL,
    tok_cache_creation  INTEGER NOT NULL,
    tok_reasoning       INTEGER NOT NULL,
    estimated_cost      REAL,
    duration_ms         INTEGER NOT NULL,
    context_tokens      INTEGER NOT NULL,
    context_window      INTEGER NOT NULL,
    created_at          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_routing_experiences_agent
    ON routing_experiences(agent_id, created_at DESC);
CREATE TABLE IF NOT EXISTS routing_exp_vec_map (
    rowid           INTEGER PRIMARY KEY AUTOINCREMENT,
    routing_exp_id  TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    dim             INTEGER NOT NULL DEFAULT 768,
    UNIQUE(agent_id, routing_exp_id)
);
CREATE INDEX IF NOT EXISTS idx_routing_exp_vec_map_agent ON routing_exp_vec_map(agent_id);
"#;
```
In `src/memory/store/sqlite/schema/mod.rs`, inside `init_schema()`, after an existing `conn.execute_batch(...)` call (e.g. after the `DAILY_INSIGHTS_DDL` batch):
```rust
    conn.execute_batch(ddl::ROUTING_EXPERIENCE_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create routing_experiences table: {e}")))?;
```
Inside `init_vec_tables()`, after the notes vec0 creation block:
```rust
    for (dim, name) in [
        (768u32, "routing_exp_vec_768"),
        (1024u32, "routing_exp_vec_1024"),
        (1536u32, "routing_exp_vec_1536"),
    ] {
        conn.execute_batch(&ddl::vec_table_ddl(dim, name))
            .map_err(|e| AlephError::config(format!("Failed to create {name}: {e}")))?;
    }
```
In `src/memory/store/sqlite/vec.rs` add (mirror `notes_vec_table_for_dim`):
```rust
pub fn routing_exp_vec_table_for_dim(dim: u32) -> Result<&'static str, AlephError> {
    match dim {
        768 => Ok("routing_exp_vec_768"),
        1024 => Ok("routing_exp_vec_1024"),
        1536 => Ok("routing_exp_vec_1536"),
        _ => Err(AlephError::config(format!(
            "unsupported embedding dimension: {dim} (expected 768, 1024, or 1536)"
        ))),
    }
}
```
In `src/memory/store/sqlite/mod.rs` (~:10) add `pub mod routing_experience;`.
- **Commit:** `memory: add routing_experiences schema + vec0 mirror tables`

- [ ] **Step 2: RED — kNN ordering + agent isolation + dimension targeting (U1).** Create `src/memory/store/sqlite/routing_experience.rs` with the types and a `#[cfg(test)] mod tests` containing U1 (types must exist for the test to name them; the `record`/`recall` methods do not yet — RED is a compile failure on the missing methods):
```rust
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use crate::error::AlephError;
use super::SqliteMemoryBackend;
use super::vec;

#[derive(Debug, Clone)]
pub struct RoutingExperienceRow {
    pub id: String,
    pub agent_id: String,
    pub model_id: String,
    pub provider_id: String,
    pub terminate_reason: String,
    pub iterations: i64,
    pub tool_calls: i64,
    pub tool_error_count: i64,
    pub tool_call_total: i64,
    pub tok_input: i64,
    pub tok_output: i64,
    pub tok_cache_read: i64,
    pub tok_cache_creation: i64,
    pub tok_reasoning: i64,
    pub estimated_cost: Option<f64>,
    pub duration_ms: i64,
    pub context_tokens: i64,
    pub context_window: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingNeighbor {
    pub id: String,
    pub agent_id: String,
    pub model_id: String,
    pub provider_id: String,
    pub terminate_reason: String,
    pub iterations: i64,
    pub tool_calls: i64,
    pub tool_error_count: i64,
    pub tool_call_total: i64,
    pub tok_input: i64,
    pub tok_output: i64,
    pub tok_cache_read: i64,
    pub tok_cache_creation: i64,
    pub tok_reasoning: i64,
    pub estimated_cost: Option<f64>,
    pub duration_ms: i64,
    pub context_tokens: i64,
    pub context_window: i64,
    pub created_at: i64,
    pub distance: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_backend() -> SqliteMemoryBackend {
        let dir = std::env::temp_dir().join(format!("aleph-routing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap()
    }
    fn emb(seed: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[0] = seed;
        v
    }
    fn row(id: &str, agent: &str, model: &str, created_at: i64) -> RoutingExperienceRow {
        RoutingExperienceRow {
            id: id.into(), agent_id: agent.into(), model_id: model.into(), provider_id: "p".into(),
            terminate_reason: "{\"kind\":\"completed\"}".into(),
            iterations: 0, tool_calls: 0, tool_error_count: 0, tool_call_total: 0,
            tok_input: 0, tok_output: 0, tok_cache_read: 0, tok_cache_creation: 0, tok_reasoning: 0,
            estimated_cost: None, duration_ms: 0, context_tokens: 0, context_window: 0, created_at,
        }
    }

    #[test]
    fn recall_orders_by_distance_and_isolates_agents() {
        let backend = temp_backend();
        backend.record_routing_experience(&row("1", "a", "m1", 1), &emb(1.0), 768).unwrap();
        backend.record_routing_experience(&row("2", "a", "m2", 2), &emb(2.0), 768).unwrap();
        backend.record_routing_experience(&row("3", "a", "m3", 3), &emb(3.0), 768).unwrap();
        backend.record_routing_experience(&row("b1", "b", "mb", 4), &emb(1.0), 768).unwrap();

        let got = backend.recall_routing_experience(&emb(0.0), 768, "a", 3).unwrap();
        let models: Vec<String> = got.iter().map(|n| n.model_id.clone()).collect();
        assert_eq!(models, vec!["m1", "m2", "m3"]);
        assert!(got.iter().all(|n| n.agent_id == "a"));
        assert!(got.iter().all(|n| n.model_id != "mb"));
    }

    #[test]
    fn record_targets_routing_dim_table_and_not_notes() {
        let backend = temp_backend();
        backend.record_routing_experience(&row("1", "a", "m1", 1), &emb(1.0), 768).unwrap();
        // `conn` is private to the sqlite module but reachable from this child module.
        let conn = backend.conn.lock().unwrap();
        let routing_count: i64 = conn
            .query_row("SELECT count(*) FROM routing_exp_vec_768", [], |r| r.get(0))
            .unwrap();
        assert_eq!(routing_count, 1, "embedding must land in routing_exp_vec_768");
        let notes_count: i64 = conn
            .query_row("SELECT count(*) FROM notes_vec_768", [], |r| r.get(0))
            .unwrap();
        assert_eq!(notes_count, 0, "routing write must not touch notes_vec_768");
    }
}
```
- **Run:** `cargo test -p alephcore --lib routing_experience` → **FAIL** (does not compile: `record_routing_experience` / `recall_routing_experience` not found).
- **Commit:** `memory: red — routing kNN + agent isolation + dim-targeting tests`

- [ ] **Step 3: GREEN — implement `record` + `recall`.** Append to `routing_experience.rs` (before `mod tests`):
```rust
impl SqliteMemoryBackend {
    pub fn record_routing_experience(
        &self,
        row: &RoutingExperienceRow,
        embedding: &[f32],
        dim: u32,
    ) -> Result<(), AlephError> {
        let table = vec::routing_exp_vec_table_for_dim(dim)?;
        let blob = vec::embedding_to_blob(embedding);
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        conn.execute(
            "INSERT INTO routing_experiences \
             (id, agent_id, model_id, provider_id, terminate_reason, iterations, tool_calls, \
              tool_error_count, tool_call_total, tok_input, tok_output, tok_cache_read, \
              tok_cache_creation, tok_reasoning, estimated_cost, duration_ms, context_tokens, \
              context_window, created_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                row.id, row.agent_id, row.model_id, row.provider_id, row.terminate_reason,
                row.iterations, row.tool_calls, row.tool_error_count, row.tool_call_total,
                row.tok_input, row.tok_output, row.tok_cache_read, row.tok_cache_creation,
                row.tok_reasoning, row.estimated_cost, row.duration_ms, row.context_tokens,
                row.context_window, row.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("record_routing_experience insert: {e}")))?;

        conn.execute(
            "INSERT INTO routing_exp_vec_map (routing_exp_id, agent_id, dim) VALUES (?1, ?2, ?3)",
            params![row.id, row.agent_id, dim as i64],
        )
        .map_err(|e| AlephError::config(format!("record_routing_experience map insert: {e}")))?;
        let rowid = conn.last_insert_rowid();

        conn.execute(
            &format!("INSERT INTO {table} (rowid, embedding) VALUES (?1, ?2)"),
            params![rowid, blob],
        )
        .map_err(|e| AlephError::config(format!("record_routing_experience vec insert: {e}")))?;

        Ok(())
    }

    pub fn recall_routing_experience(
        &self,
        task_emb: &[f32],
        dim: u32,
        agent_id: &str,
        k: usize,
    ) -> Result<Vec<RoutingNeighbor>, AlephError> {
        let table = vec::routing_exp_vec_table_for_dim(dim)?;
        let blob = vec::embedding_to_blob(task_emb);
        let k_over = k.saturating_mul(3).max(k) as i64;

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        // Step 1: kNN on the routing vec0 table alone (sqlite-vec requirement).
        let knn: Vec<(i64, f64)> = {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT rowid, distance FROM {table} WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2"
                ))
                .map_err(|e| AlephError::config(format!("recall_routing_experience knn prepare: {e}")))?;
            let rows = stmt
                .query_map(params![blob, k_over], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
                })
                .map_err(|e| AlephError::config(format!("recall_routing_experience knn query: {e}")))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| AlephError::config(format!("recall_routing_experience knn row: {e}")))?);
            }
            out
        };

        // Step 2: rowid -> routing_exp_id filtered by agent (post-hoc isolation), then load the row.
        let mut map_stmt = conn
            .prepare("SELECT routing_exp_id FROM routing_exp_vec_map WHERE rowid = ?1 AND agent_id = ?2")
            .map_err(|e| AlephError::config(format!("recall_routing_experience map prepare: {e}")))?;
        let mut row_stmt = conn
            .prepare(
                "SELECT id, agent_id, model_id, provider_id, terminate_reason, iterations, tool_calls, \
                 tool_error_count, tool_call_total, tok_input, tok_output, tok_cache_read, \
                 tok_cache_creation, tok_reasoning, estimated_cost, duration_ms, context_tokens, \
                 context_window, created_at FROM routing_experiences WHERE id = ?1",
            )
            .map_err(|e| AlephError::config(format!("recall_routing_experience row prepare: {e}")))?;

        let mut neighbors: Vec<RoutingNeighbor> = Vec::with_capacity(k);
        for (rowid, distance) in &knn {
            let exp_id: Option<String> = map_stmt
                .query_row(params![rowid, agent_id], |r| r.get(0))
                .optional()
                .map_err(|e| AlephError::config(format!("recall_routing_experience map row: {e}")))?;
            let Some(exp_id) = exp_id else { continue };
            let neighbor = row_stmt
                .query_row(params![exp_id], |row| {
                    Ok(RoutingNeighbor {
                        id: row.get("id")?,
                        agent_id: row.get("agent_id")?,
                        model_id: row.get("model_id")?,
                        provider_id: row.get("provider_id")?,
                        terminate_reason: row.get("terminate_reason")?,
                        iterations: row.get("iterations")?,
                        tool_calls: row.get("tool_calls")?,
                        tool_error_count: row.get("tool_error_count")?,
                        tool_call_total: row.get("tool_call_total")?,
                        tok_input: row.get("tok_input")?,
                        tok_output: row.get("tok_output")?,
                        tok_cache_read: row.get("tok_cache_read")?,
                        tok_cache_creation: row.get("tok_cache_creation")?,
                        tok_reasoning: row.get("tok_reasoning")?,
                        estimated_cost: row.get("estimated_cost")?,
                        duration_ms: row.get("duration_ms")?,
                        context_tokens: row.get("context_tokens")?,
                        context_window: row.get("context_window")?,
                        created_at: row.get("created_at")?,
                        distance: *distance as f32,
                    })
                })
                .optional()
                .map_err(|e| AlephError::config(format!("recall_routing_experience row: {e}")))?;
            if let Some(neighbor) = neighbor {
                neighbors.push(neighbor);
            }
        }

        neighbors.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        neighbors.truncate(k);
        Ok(neighbors)
    }
}
```
- **Run:** `cargo test -p alephcore --lib routing_experience` → **PASS** (`recall_orders_by_distance_and_isolates_agents`, `record_targets_routing_dim_table_and_not_notes`).
- **Commit:** `memory: implement routing record + kNN recall with agent isolation`

- [ ] **Step 4: RED — retention caps by recency, not distance.** Add to `mod tests`:
```rust
    #[test]
    fn prune_keeps_newest_by_recency_not_distance() {
        let backend = temp_backend();
        backend.record_routing_experience(&row("1", "a", "m1", 1), &emb(1.0), 768).unwrap(); // nearest, oldest
        backend.record_routing_experience(&row("2", "a", "m2", 2), &emb(2.0), 768).unwrap();
        backend.record_routing_experience(&row("3", "a", "m3", 3), &emb(3.0), 768).unwrap();
        backend.prune_routing_experiences("a", 768, 2).unwrap();
        let got = backend.recall_routing_experience(&emb(0.0), 768, "a", 5).unwrap();
        let models: Vec<String> = got.iter().map(|n| n.model_id.clone()).collect();
        assert!(!models.contains(&"m1".to_string())); // oldest pruned despite being nearest
        assert!(models.contains(&"m2".to_string()));
        assert!(models.contains(&"m3".to_string()));
    }
```
- **Run:** `cargo test -p alephcore --lib routing_experience` → **FAIL** (`prune_routing_experiences` not found).
- **Commit:** `memory: red — routing self-bounded retention test`

- [ ] **Step 5: GREEN — implement `prune_routing_experiences`.** Add to `impl SqliteMemoryBackend`:
```rust
    pub fn prune_routing_experiences(
        &self,
        agent_id: &str,
        dim: u32,
        cap: usize,
    ) -> Result<(), AlephError> {
        let table = vec::routing_exp_vec_table_for_dim(dim)?;
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let drop_ids: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM routing_experiences WHERE agent_id = ?1 \
                     ORDER BY created_at DESC LIMIT -1 OFFSET ?2",
                )
                .map_err(|e| AlephError::config(format!("prune_routing_experiences select: {e}")))?;
            let rows = stmt
                .query_map(params![agent_id, cap as i64], |r| r.get::<_, String>(0))
                .map_err(|e| AlephError::config(format!("prune_routing_experiences query: {e}")))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| AlephError::config(format!("prune_routing_experiences row: {e}")))?);
            }
            out
        };

        for id in drop_ids {
            let rowid: Option<i64> = conn
                .query_row(
                    "SELECT rowid FROM routing_exp_vec_map WHERE agent_id = ?1 AND routing_exp_id = ?2",
                    params![agent_id, id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| AlephError::config(format!("prune_routing_experiences map: {e}")))?;
            if let Some(rowid) = rowid {
                conn.execute(&format!("DELETE FROM {table} WHERE rowid = ?1"), params![rowid])
                    .map_err(|e| AlephError::config(format!("prune_routing_experiences vec del: {e}")))?;
                conn.execute("DELETE FROM routing_exp_vec_map WHERE rowid = ?1", params![rowid])
                    .map_err(|e| AlephError::config(format!("prune_routing_experiences map del: {e}")))?;
            }
            conn.execute("DELETE FROM routing_experiences WHERE id = ?1", params![id])
                .map_err(|e| AlephError::config(format!("prune_routing_experiences exp del: {e}")))?;
        }
        Ok(())
    }
```
- **Run:** `cargo test -p alephcore --lib routing_experience` → **PASS** (all three).
- **Commit:** `memory: self-bounded per-agent retention for routing experiences`

- [ ] **Step 6: R7 guard — DDL carries no judgment columns.** Add to `mod tests`:
```rust
    #[test]
    fn ddl_has_no_judgment_columns() {
        let ddl = crate::memory::store::sqlite::schema::ddl::ROUTING_EXPERIENCE_DDL.to_lowercase();
        assert!(!ddl.contains("success"));
        assert!(!ddl.contains("score"));
        assert!(!ddl.contains("rank"));
        assert!(!ddl.contains("best_for"));
    }
```
- **Run:** `cargo test -p alephcore --lib routing_experience` → **PASS**.
- **Commit:** `memory: assert routing DDL carries raw facts only (R7)`

---

### Task 2: `RoutingOutcome` + `OutcomeObserver` + store facade + model-precedence helper (W3) + anti-fabrication guard (U2)

**Files:**
- Create `src/routing/experience_store.rs`
- Create `src/routing/observer.rs`
- Modify `src/routing/mod.rs` (add `RoutingAttribution`, `resolve_routing_model_id`, `pub mod` decls, re-exports, W3 test)

**Interfaces consumed (verbatim):**
- `crate::harness::TraceSink` — `fn on_trace(&self, event: &LoopTraceEvent); fn flush(&self); fn on_init_seam(&self, _: &'static str, _: &'static str, _: bool) {}` (non-blocking contract — `trace_sink.rs:8-25`).
- `crate::harness::trace::LoopTraceEvent::SessionCompleted { outcome: LoopTraceSessionOutcome, iterations: usize, tool_calls_made: usize, total_tokens: usize, hit_limit: bool, final_text: Option<String>, terminate_reason: Option<TerminateReason>, duration_ms: Option<u64>, token_breakdown: Option<TokenBreakdown>, tool_timeline: Vec<ToolInvocation> }`; `crate::harness::trace::LoopTraceSessionOutcome::{Completed,HitLimit,Cancelled}`.
- `crate::orchestrator::dispatch::{TerminateReason, TokenBreakdown, ToolInvocation}` (`ToolInvocation { id, name, duration_ms, success: bool, error }`; `TokenBreakdown { input, output, cache_read, cache_creation, reasoning: u32 }`; `TerminateReason` is `#[serde(tag="kind", rename_all="snake_case")]`).
- `crate::memory::EmbeddingProvider` (`async fn embed`, `async fn embed_batch`, `fn dimensions`, `fn model_name`, `fn provider_id`); `crate::memory::store::sqlite::SqliteMemoryBackend`; `RoutingExperienceRow`/`RoutingNeighbor` (Task 1).

> **U2 — verbatim, never read judgment signals.** `LoopTraceEvent::SessionCompleted` does **not** carry `user_re_steer` or `consecutive_errors` (those live on `LoopTraceTurnMetrics`). `outcome_from_session_completed` takes only the six verbatim `SessionCompleted` fields, so it structurally cannot fabricate or read those. Step 4 adds an explicit grep-test guard.

- [ ] **Step 1: `mod.rs` — module decls + `RoutingAttribution` + `resolve_routing_model_id` (W3, RED→GREEN).** First add the W3 test referencing a not-yet-existing helper. Append to `src/routing/mod.rs` (keep existing `session_key` etc.):
```rust
pub mod experience_store;
pub mod observer;
pub mod recall;

pub use experience_store::{RoutingExperienceStore, RoutingOutcome};
pub use observer::{outcome_from_session_completed, OutcomeObserver};
pub use recall::{provider_availability_from_config, ProviderAvailability, RoutingRecall};

/// Per-run handle correlating run-start recall (writes `task_emb`) with the
/// completion observer (reads it). One per run; lives in the gateway run loop,
/// outside the harness. `session_id` is read by the observer for trace logging.
///
/// Spec §6 types `task_emb` as `OnceCell`; we use `std::sync::OnceLock`
/// (std, no extra dep) — same write-once semantics. Flagged divergence.
pub struct RoutingAttribution {
    pub session_id: String,
    pub task_emb: std::sync::OnceLock<Vec<f32>>,
}

impl RoutingAttribution {
    #[must_use]
    pub fn new(session_id: String) -> Self {
        Self { session_id, task_emb: std::sync::OnceLock::new() }
    }
}

/// Frozen-model precedence for routing attribution — mirrors the subagent
/// spawn chain `explicit > model_hint > native` (subagent_spawner/mod.rs:297).
#[must_use]
pub fn resolve_routing_model_id(
    explicit: Option<&str>,
    model_hint: Option<&str>,
    native_default: &str,
) -> String {
    explicit.or(model_hint).unwrap_or(native_default).to_string()
}

#[cfg(test)]
mod model_precedence_tests {
    use super::resolve_routing_model_id;

    #[test]
    fn routing_model_precedence_explicit_then_hint_then_native() {
        assert_eq!(resolve_routing_model_id(Some("EXPLICIT"), Some("HINT"), "NATIVE"), "EXPLICIT");
        assert_eq!(resolve_routing_model_id(None, Some("HINT"), "NATIVE"), "HINT");
        assert_eq!(resolve_routing_model_id(None, None, "NATIVE"), "NATIVE");
    }
}
```
(The `pub mod experience_store/observer/recall;` lines reference files created in Steps 2–3 of this task and Task 3 Step 1; until those exist the crate will not compile. Create the three files as the next steps prescribe before running any crate-wide check. The W3 test itself is self-contained.)
- **Run:** (deferred to Step 2's compile — module files must exist first).
- **Commit:** `routing: RoutingAttribution + model-precedence helper + module decls (W3)`

- [ ] **Step 2: Create `experience_store.rs` (`RoutingOutcome` + facade) with round-trip test.** Create `src/routing/experience_store.rs`:
```rust
use std::sync::Arc;

use crate::error::AlephError;
use crate::memory::store::sqlite::routing_experience::{RoutingExperienceRow, RoutingNeighbor};
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::orchestrator::dispatch::TokenBreakdown;

/// Zero-judgment feedback surface — every field is a raw fact (§5.2). No
/// `success: bool`, no `quality_score`, no `user_re_steer`, no `consecutive_errors`.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingOutcome {
    pub iterations: u32,
    pub tool_calls_made: u32,
    pub terminate_reason: String,
    pub token_breakdown: TokenBreakdown,
    pub estimated_cost: Option<f64>,
    pub duration_ms: u64,
    pub context_tokens: u32,
    pub context_window: u32,
    pub tool_error_count: u32,
    pub tool_call_total: u32,
}

pub struct RoutingExperienceStore {
    backend: Arc<SqliteMemoryBackend>,
    embedder: Arc<dyn EmbeddingProvider>,
    retention_cap: usize,
}

impl RoutingExperienceStore {
    #[must_use]
    pub fn new(backend: Arc<SqliteMemoryBackend>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self { backend, embedder, retention_cap: 5000 }
    }

    pub async fn embed_task(&self, text: &str) -> Result<Vec<f32>, AlephError> {
        self.embedder.embed(text).await
    }

    pub async fn record(
        &self,
        agent_id: &str,
        model_id: &str,
        provider_id: &str,
        task_emb: &[f32],
        outcome: &RoutingOutcome,
    ) -> Result<(), AlephError> {
        let dim = self.embedder.dimensions() as u32;
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let row = RoutingExperienceRow {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            model_id: model_id.to_string(),
            provider_id: provider_id.to_string(),
            terminate_reason: outcome.terminate_reason.clone(),
            iterations: outcome.iterations as i64,
            tool_calls: outcome.tool_calls_made as i64,
            tool_error_count: outcome.tool_error_count as i64,
            tool_call_total: outcome.tool_call_total as i64,
            tok_input: outcome.token_breakdown.input as i64,
            tok_output: outcome.token_breakdown.output as i64,
            tok_cache_read: outcome.token_breakdown.cache_read as i64,
            tok_cache_creation: outcome.token_breakdown.cache_creation as i64,
            tok_reasoning: outcome.token_breakdown.reasoning as i64,
            estimated_cost: outcome.estimated_cost,
            duration_ms: outcome.duration_ms as i64,
            context_tokens: outcome.context_tokens as i64,
            context_window: outcome.context_window as i64,
            created_at,
        };
        self.backend.record_routing_experience(&row, task_emb, dim)?;
        self.backend.prune_routing_experiences(agent_id, dim, self.retention_cap)?;
        Ok(())
    }

    pub async fn recall(
        &self,
        agent_id: &str,
        task_emb: &[f32],
        k: usize,
    ) -> Result<Vec<RoutingNeighbor>, AlephError> {
        let dim = self.embedder.dimensions() as u32;
        self.backend.recall_routing_experience(task_emb, dim, agent_id, k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_backend() -> SqliteMemoryBackend {
        let dir = std::env::temp_dir().join(format!("aleph-routing-fac-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap()
    }
    fn emb(seed: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[0] = seed;
        v
    }
    struct StubEmbedder { dim: usize, vec: Vec<f32> }
    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> { Ok(self.vec.clone()) }
        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(texts.iter().map(|_| self.vec.clone()).collect())
        }
        fn dimensions(&self) -> usize { self.dim }
        fn model_name(&self) -> &str { "stub" }
        fn provider_id(&self) -> &str { "stub" }
    }

    #[tokio::test]
    async fn facade_record_then_recall_roundtrip() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { dim: 768, vec: emb(1.0) });
        let store = RoutingExperienceStore::new(backend, embedder);
        let outcome = RoutingOutcome {
            iterations: 2, tool_calls_made: 1, terminate_reason: "{\"kind\":\"completed\"}".into(),
            token_breakdown: TokenBreakdown::default(), estimated_cost: None, duration_ms: 10,
            context_tokens: 0, context_window: 0, tool_error_count: 0, tool_call_total: 1,
        };
        store.record("a", "MODEL_X", "PROV_Y", &emb(1.0), &outcome).await.unwrap();
        let got = store.recall("a", &emb(0.0), 5).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model_id, "MODEL_X");
        assert_eq!(got[0].provider_id, "PROV_Y");
        assert_eq!(got[0].iterations, 2);
    }
}
```
- **Run:** `cargo test -p alephcore --lib routing::experience_store` → **PASS** (`facade_record_then_recall_roundtrip`). (`observer`/`recall` modules are still empty/missing; see Step 3 + Task 3 — run this filter only after Step 3 so the crate compiles, or temporarily comment the `pub mod observer; pub mod recall;` lines while iterating. Final crate compile is gated in Task 4d Step 6.)
- **Commit:** `routing: RoutingExperienceStore facade over shared embedder + sqlite-vec`

- [ ] **Step 3: Create `observer.rs` — pure mapper (RED→GREEN, U2) + `OutcomeObserver` decorator (U3).** Create `src/routing/observer.rs`:
```rust
use std::sync::Arc;

use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::orchestrator::dispatch::{TerminateReason, TokenBreakdown, ToolInvocation};

use super::experience_store::{RoutingExperienceStore, RoutingOutcome};
use super::RoutingAttribution;

/// Stringify a `TerminateReason` verbatim (discriminant + embedded fields) via
/// its own serde tagging — no collapse to success/failure (R7).
fn terminate_reason_to_raw(tr: &Option<TerminateReason>) -> String {
    match tr {
        Some(r) => serde_json::to_string(r).unwrap_or_else(|_| "unknown".to_string()),
        None => "unknown".to_string(),
    }
}

/// Derive a `RoutingOutcome` from the verbatim `SessionCompleted` fields. Pure:
/// counts and discriminants only, zero interpretation; never reads
/// `user_re_steer` / `consecutive_errors` (not present on `SessionCompleted`).
#[must_use]
pub fn outcome_from_session_completed(
    iterations: usize,
    tool_calls_made: usize,
    terminate_reason: &Option<TerminateReason>,
    token_breakdown: &Option<TokenBreakdown>,
    duration_ms: &Option<u64>,
    tool_timeline: &[ToolInvocation],
) -> RoutingOutcome {
    RoutingOutcome {
        iterations: iterations.min(u32::MAX as usize) as u32,
        tool_calls_made: tool_calls_made.min(u32::MAX as usize) as u32,
        terminate_reason: terminate_reason_to_raw(terminate_reason),
        token_breakdown: token_breakdown.clone().unwrap_or_default(),
        estimated_cost: None,
        duration_ms: duration_ms.unwrap_or(0),
        context_tokens: 0,
        context_window: 0,
        tool_error_count: tool_timeline.iter().filter(|t| !t.success).count() as u32,
        tool_call_total: tool_timeline.len() as u32,
    }
}

pub struct OutcomeObserver {
    inner: Arc<dyn TraceSink>,
    store: Arc<RoutingExperienceStore>,
    attribution: Arc<RoutingAttribution>,
    model_id: String,
    provider_id: String,
    agent_id: String,
}

impl OutcomeObserver {
    #[must_use]
    pub fn new(
        inner: Arc<dyn TraceSink>,
        store: Arc<RoutingExperienceStore>,
        attribution: Arc<RoutingAttribution>,
        model_id: String,
        provider_id: String,
        agent_id: String,
    ) -> Self {
        Self { inner, store, attribution, model_id, provider_id, agent_id }
    }

    /// Fire-and-forget body, a free async fn so `on_trace` can `tokio::spawn`
    /// it with owned clones (the 'static bound forbids borrowing `self`).
    pub(crate) async fn record_to_store(
        store: Arc<RoutingExperienceStore>,
        agent_id: String,
        model_id: String,
        provider_id: String,
        task_emb: Vec<f32>,
        outcome: RoutingOutcome,
    ) {
        if let Err(e) = store
            .record(&agent_id, &model_id, &provider_id, &task_emb, &outcome)
            .await
        {
            tracing::warn!(error = %e, "routing experience record failed");
        }
    }
}

impl TraceSink for OutcomeObserver {
    fn on_trace(&self, event: &LoopTraceEvent) {
        if let LoopTraceEvent::SessionCompleted {
            iterations, tool_calls_made, terminate_reason, token_breakdown, duration_ms, tool_timeline, ..
        } = event
        {
            let outcome = outcome_from_session_completed(
                *iterations, *tool_calls_made, terminate_reason, token_breakdown, duration_ms, tool_timeline,
            );
            if let Some(task_emb) = self.attribution.task_emb.get().cloned() {
                tracing::debug!(
                    session_id = %self.attribution.session_id,
                    model = %self.model_id,
                    "recording routing experience"
                );
                tokio::spawn(Self::record_to_store(
                    self.store.clone(),
                    self.agent_id.clone(),
                    self.model_id.clone(),
                    self.provider_id.clone(),
                    task_emb,
                    outcome,
                ));
            }
        }
        self.inner.on_trace(event); // MUST forward unchanged + non-blocking (trace_sink.rs:12-25)
    }

    fn flush(&self) {
        self.inner.flush();
    }

    fn on_init_seam(&self, stage: &'static str, seam: &'static str, configured: bool) {
        self.inner.on_init_seam(stage, seam, configured);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AlephError;
    use crate::routing::RoutingExperienceStore;

    fn emb(seed: f32) -> Vec<f32> { let mut v = vec![0.0f32; 768]; v[0] = seed; v }
    fn temp_backend() -> crate::memory::store::sqlite::SqliteMemoryBackend {
        let dir = std::env::temp_dir().join(format!("aleph-routing-obs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::memory::store::sqlite::SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap()
    }
    struct StubEmbedder;
    #[async_trait::async_trait]
    impl crate::memory::EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> { Ok(emb(1.0)) }
        async fn embed_batch(&self, t: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> { Ok(t.iter().map(|_| emb(1.0)).collect()) }
        fn dimensions(&self) -> usize { 768 }
        fn model_name(&self) -> &str { "stub" }
        fn provider_id(&self) -> &str { "stub" }
    }

    #[test]
    fn outcome_maps_raw_without_verdict() {
        let timeline = vec![
            ToolInvocation { id: "1".into(), name: "bash".into(), duration_ms: 5, success: true, error: None },
            ToolInvocation { id: "2".into(), name: "web".into(), duration_ms: 5, success: false, error: Some("boom".into()) },
            ToolInvocation { id: "3".into(), name: "web".into(), duration_ms: 5, success: false, error: Some("boom".into()) },
        ];
        let tr = Some(TerminateReason::VerifierVeto { vetos: 3 });
        let tb = Some(TokenBreakdown { input: 10, output: 20, cache_read: 0, cache_creation: 0, reasoning: 5 });
        let dur = Some(1234u64);
        let outcome = outcome_from_session_completed(7, 3, &tr, &tb, &dur, &timeline);
        assert_eq!(outcome.iterations, 7);
        assert_eq!(outcome.tool_calls_made, 3);
        assert_eq!(outcome.tool_error_count, 2);
        assert_eq!(outcome.tool_call_total, 3);
        assert_eq!(outcome.duration_ms, 1234);
        assert_eq!(outcome.terminate_reason, "{\"kind\":\"verifier_veto\",\"vetos\":3}");
        assert_eq!(outcome.token_breakdown.reasoning, 5);
    }

    #[test]
    fn mapper_never_fabricates_or_reads_judgment_signals() {
        let src = include_str!("observer.rs");
        assert!(!src.contains("user_re_steer"), "must not read user_re_steer (U2)");
        assert!(!src.contains("consecutive_errors"), "must not read consecutive_errors (U2)");
    }

    #[tokio::test]
    async fn observer_records_injected_model_not_provider_usage() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn crate::memory::EmbeddingProvider> = Arc::new(StubEmbedder);
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let outcome = RoutingOutcome {
            iterations: 0, tool_calls_made: 0, terminate_reason: "{\"kind\":\"completed\"}".into(),
            token_breakdown: TokenBreakdown::default(), estimated_cost: None, duration_ms: 0,
            context_tokens: 0, context_window: 0, tool_error_count: 0, tool_call_total: 0,
        };
        OutcomeObserver::record_to_store(
            store.clone(), "a".into(), "MODEL_X".into(), "PROV_Y".into(), emb(1.0), outcome,
        ).await;
        let got = store.recall("a", &emb(0.0), 5).await.unwrap();
        assert_eq!(got[0].model_id, "MODEL_X"); // injected at construction, not from ProviderUsage
        assert_eq!(got[0].provider_id, "PROV_Y");
    }
}
```
- **Run:** `cargo test -p alephcore --lib routing::observer` → **PASS** (`outcome_maps_raw_without_verdict`, `mapper_never_fabricates_or_reads_judgment_signals`, `observer_records_injected_model_not_provider_usage`). To see RED for the mapper first: comment out `outcome_from_session_completed`'s body / the fn, run → compile FAIL, restore → PASS.
- **Commit:** `routing: OutcomeObserver decorator + verbatim mapper + U2 guard (U2/U3)`

---

### Task 3: `RoutingRecall` (embed + recall + availability-MARK + fence-wrap + cold-start) + lib availability gate

**Files:**
- Create `src/routing/recall.rs`

**Interfaces consumed:** `RoutingExperienceStore` (`embed_task`, `recall`) + `RoutingNeighbor`; `RoutingAttribution`; `crate::memory::assembler::context_block::wrap_memory_context(&str) -> String` (fence `<memory-context>` + system note); `crate::gateway::handlers::resolve_vault_secret(key: &str, vault: &SharedTokenManager) -> Option<String>` (pub(crate), visible here in the lib); `crate::gateway::security::SharedTokenManager`; `crate::config::ProviderConfig` (`api_key: Option<String>`).

> **Run-start only (R10).** `build_routing_experience_message` is invoked once per run at the orchestrator run-start (Task 4c), never from `think.rs`/`prompt.rs`. It embeds `user_query` once, backfills `attribution.task_emb` (record/recall symmetry, §8 D6), recalls k-NN neighbors, **marks** each neighbor `UNAVAILABLE` when its provider is not currently configured (O4: mark, not filter), and fence-wraps. Returns `Option<String>` (already fenced) — the prompt-build seam consumes a `String` (R3/P6).

> **`provider_availability_from_config` (BLOCKER fix).** Built inside the library so it can call `pub(crate)` `resolve_vault_secret`. The binary (Task 4b) calls only this `pub` helper, never `resolve_vault_secret` directly. Gate semantics are identical to `list_models::provider_configured` (config `api_key` OR vault secret `ai:{provider}`) — reused without touching `list_models.rs` (N3 zero-diff).

- [ ] **Step 1: RED→GREEN — cold-start returns `None` (U4) + the lib availability gate.** Create `src/routing/recall.rs`:
```rust
use std::collections::HashMap;
use std::sync::Arc;

use crate::config::ProviderConfig;
use crate::error::AlephError;
use crate::gateway::security::SharedTokenManager;
use crate::memory::assembler::context_block::wrap_memory_context;
use crate::memory::store::sqlite::routing_experience::RoutingNeighbor;

use super::experience_store::RoutingExperienceStore;
use super::RoutingAttribution;

/// Currently-configured predicate over a provider id.
pub type ProviderAvailability = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Build the availability gate from boot config + vault. Lives in the lib so it
/// can call the `pub(crate)` `resolve_vault_secret`; the binary calls only this
/// `pub` constructor. Same gate semantics as `list_models::provider_configured`.
#[must_use]
pub fn provider_availability_from_config(
    providers: HashMap<String, ProviderConfig>,
    token_manager: Option<Arc<SharedTokenManager>>,
) -> ProviderAvailability {
    Arc::new(move |provider: &str| {
        if providers.get(provider).and_then(|c| c.api_key.as_ref()).is_some() {
            return true;
        }
        match &token_manager {
            Some(tm) => {
                crate::gateway::handlers::resolve_vault_secret(&format!("ai:{provider}"), tm).is_some()
            }
            None => false,
        }
    })
}

pub struct RoutingRecall {
    store: Arc<RoutingExperienceStore>,
    availability: ProviderAvailability,
    k: usize,
}

impl RoutingRecall {
    #[must_use]
    pub fn new(store: Arc<RoutingExperienceStore>, availability: ProviderAvailability) -> Self {
        Self { store, availability, k: 5 }
    }

    pub async fn build_routing_experience_message(
        &self,
        user_query: &str,
        agent_id: &str,
        _available_tokens: Option<u32>,
        attribution: &RoutingAttribution,
    ) -> Result<Option<String>, AlephError> {
        // Embed once; backfill attribution so the observer attributes with the
        // SAME key recall queried with (§8 D6).
        let task_emb = self.store.embed_task(user_query).await?;
        let _ = attribution.task_emb.set(task_emb.clone());

        let neighbors = self.store.recall(agent_id, &task_emb, self.k).await?;
        if neighbors.is_empty() {
            return Ok(None); // cold start: behave exactly like today's blind selection (D1)
        }
        let rendered = render_neighbors(&neighbors, &self.availability);
        if rendered.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(wrap_memory_context(&rendered)))
    }
}

fn render_neighbors(neighbors: &[RoutingNeighbor], availability: &ProviderAvailability) -> String {
    let mut out = String::new();
    out.push_str(
        "Verified routing experience from semantically similar past tasks (raw observations, \
         NOT a recommendation — weigh them yourself; discount far/old/low-sample entries):\n",
    );
    for n in neighbors {
        let avail_tag = if (availability)(&n.provider_id) {
            ""
        } else {
            " [UNAVAILABLE: provider not currently configured — do NOT select]"
        };
        out.push_str(&format!(
            "- model={} provider={}{} distance={:.4} terminate_reason={} iterations={} \
             tool_errors={}/{} tokens(in/out/cache_r/cache_c/reason)={}/{}/{}/{}/{} \
             duration_ms={} age_unix={}\n",
            n.model_id, n.provider_id, avail_tag, n.distance, n.terminate_reason,
            n.iterations, n.tool_error_count, n.tool_call_total,
            n.tok_input, n.tok_output, n.tok_cache_read, n.tok_cache_creation, n.tok_reasoning,
            n.duration_ms, n.created_at,
        ));
    }
    out.push_str(
        "Models without observations on this kind of task are unproven, not bad — you may \
         explore one if it fits.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::sqlite::routing_experience::RoutingExperienceRow;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::memory::EmbeddingProvider;

    fn emb(seed: f32) -> Vec<f32> { let mut v = vec![0.0f32; 768]; v[0] = seed; v }
    fn temp_backend() -> SqliteMemoryBackend {
        let dir = std::env::temp_dir().join(format!("aleph-routing-rec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap()
    }
    struct StubEmbedder { vec: Vec<f32> }
    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> { Ok(self.vec.clone()) }
        async fn embed_batch(&self, t: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> { Ok(t.iter().map(|_| self.vec.clone()).collect()) }
        fn dimensions(&self) -> usize { 768 }
        fn model_name(&self) -> &str { "stub" }
        fn provider_id(&self) -> &str { "stub" }
    }
    fn row(id: &str, agent: &str, model: &str, provider: &str) -> RoutingExperienceRow {
        RoutingExperienceRow {
            id: id.into(), agent_id: agent.into(), model_id: model.into(), provider_id: provider.into(),
            terminate_reason: "{\"kind\":\"completed\"}".into(),
            iterations: 0, tool_calls: 0, tool_error_count: 0, tool_call_total: 0,
            tok_input: 0, tok_output: 0, tok_cache_read: 0, tok_cache_creation: 0, tok_reasoning: 0,
            estimated_cost: None, duration_ms: 0, context_tokens: 0, context_window: 0, created_at: 1,
        }
    }

    #[tokio::test]
    async fn cold_start_returns_none() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let avail: ProviderAvailability = Arc::new(|_p: &str| true);
        let recall = RoutingRecall::new(store, avail);
        let attribution = RoutingAttribution::new("s".into());
        let msg = recall.build_routing_experience_message("do X", "a", None, &attribution).await.unwrap();
        assert!(msg.is_none());
        assert!(attribution.task_emb.get().is_some()); // embed still happened
    }
}
```
- **Run:** `cargo test -p alephcore --lib routing::recall` → **PASS** (`cold_start_returns_none`).
- **Commit:** `routing: RoutingRecall run-start + cold-start None + lib availability gate (U4)`

- [ ] **Step 2: GREEN — unavailable models are MARKED, not filtered (W2/O4).** Add to `recall.rs` `mod tests`:
```rust
    #[tokio::test]
    async fn recalled_unavailable_model_is_marked() {
        let backend = Arc::new(temp_backend());
        backend.record_routing_experience(&row("1", "a", "m-dead", "deadprov"), &emb(1.0), 768).unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let avail: ProviderAvailability = Arc::new(|p: &str| p != "deadprov");
        let recall = RoutingRecall::new(store, avail);
        let attribution = RoutingAttribution::new("s".into());
        let msg = recall
            .build_routing_experience_message("do X", "a", None, &attribution)
            .await.unwrap().unwrap();
        assert!(msg.contains("UNAVAILABLE")); // O4: marked, not filtered
        assert!(msg.contains("m-dead"));       // still visible to the LLM
        assert!(msg.contains("memory-context")); // fence-wrapped
    }
```
(`backend` is `Arc<SqliteMemoryBackend>`; `Arc` derefs to `&SqliteMemoryBackend`, so `backend.record_routing_experience(...)` before wrapping into the store works.)
- **Run:** `cargo test -p alephcore --lib routing::recall` → **PASS** (both).
- **Commit:** `routing: mark recalled-unavailable models, keep visible (W2/O4)`

- [ ] **Step 3: GREEN — record/recall embedding-key symmetry (U5/D6).** Add to `recall.rs` `mod tests`:
```rust
    #[tokio::test]
    async fn record_and_recall_share_one_embedding_key() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(0.7) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let avail: ProviderAvailability = Arc::new(|_p: &str| true);
        let recall = RoutingRecall::new(store.clone(), avail);
        let attribution = RoutingAttribution::new("s".into());
        let _ = recall.build_routing_experience_message("same text", "a", None, &attribution).await.unwrap();
        let recalled_key = attribution.task_emb.get().cloned().unwrap();
        let direct = store.embed_task("same text").await.unwrap();
        assert_eq!(recalled_key, direct); // observer attributes with the key recall queried with
    }
```
- **Run:** `cargo test -p alephcore --lib routing::recall` → **PASS** (all three).
- **Commit:** `routing: assert record/recall embedding-key symmetry (U5/D6)`

---

### Task 4a: PromptBuilder run-start routing seam + emit + W1 emission test

**Files:**
- Modify `src/thinker/prompt_layer.rs` (LayerInput field + 4 ctor inits + setter)
- Modify `src/thinker/prompt_builder/mod.rs` (PromptBuilder field + default + setter)
- Modify `src/thinker/prompt_builder/cache.rs` (thread routing into LayerInput on the production cached path; W1 emission test)
- Modify `src/thinker/layers/memory_augmentation.rs` (emit routing verbatim after memory)

- [ ] **Step 1: Add the LayerInput field + setter (mirror `memory_user_message`).** In `src/thinker/prompt_layer.rs`, add the field right after `pub memory_user_message: Option<String>,` (~:97):
```rust
    /// Pre-rendered routing-experience text from `RoutingRecall`. When set,
    /// `MemoryAugmentationLayer` injects this verbatim after memory.
    pub routing_experience_user_message: Option<String>,
```
Add `routing_experience_user_message: None,` to each of the **four** `LayerInput` constructors (alongside the existing `memory_user_message: None,` at ~:169, :196, :227, :254). Add the setter directly after `with_memory_user_message` (~:323):
```rust
    /// Attach pre-rendered routing-experience text from `RoutingRecall`.
    #[must_use]
    pub fn with_routing_experience_message(mut self, text: String) -> Self {
        self.routing_experience_user_message = Some(text);
        self
    }
```
- **Run:** `cargo check -p alephcore --lib` → **PASS** (field unused for now is acceptable).
- **Commit:** `prompt: add LayerInput.routing_experience_user_message (mirror memory)`

- [ ] **Step 2: Add the PromptBuilder field + setter.** In `src/thinker/prompt_builder/mod.rs`, add the field after `memory_user_message: Option<String>,` (~:153):
```rust
    /// Pre-rendered routing-experience text. Threaded into `LayerInput` on the
    /// cached production path so `MemoryAugmentationLayer` renders it.
    routing_experience_user_message: Option<String>,
```
Add `routing_experience_user_message: None,` to the builder default block (alongside `memory_user_message: None,` at ~:207). Add the setter after `with_memory_user_message` (~:270):
```rust
    /// Attach pre-rendered routing-experience text from `RoutingRecall`.
    #[must_use]
    pub fn with_routing_experience_message(mut self, text: String) -> Self {
        self.routing_experience_user_message = Some(text);
        self
    }
```
- **Run:** `cargo check -p alephcore --lib` → **PASS**.
- **Commit:** `prompt: add PromptBuilder.with_routing_experience_message setter`

- [ ] **Step 3: Thread the field into the cached production path + emit it in the layer.** In `src/thinker/prompt_builder/cache.rs`, directly after the existing memory threading (~:68-70):
```rust
        let input = match &self.memory_user_message {
            Some(text) => input.with_memory_user_message(text.clone()),
            None => input,
        };
        let input = match &self.routing_experience_user_message {
            Some(text) => input.with_routing_experience_message(text.clone()),
            None => input,
        };
```
In `src/thinker/layers/memory_augmentation.rs`, extend `inject` (~:41) to emit routing verbatim after memory:
```rust
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(text) = &input.memory_user_message {
            if !text.trim().is_empty() {
                output.push_str(text);
            }
        }
        if let Some(text) = &input.routing_experience_user_message {
            if !text.trim().is_empty() {
                output.push_str(text);
            }
        }
    }
```
- **Run:** `cargo check -p alephcore --lib` → **PASS**.
- **Commit:** `prompt: emit routing-experience text on cached path via MemoryAugmentationLayer`

- [ ] **Step 4: W1 — routing text injected exactly once on the production path.** Add to `src/thinker/prompt_builder/cache.rs` `mod tests`:
```rust
    #[test]
    fn routing_experience_message_injected_exactly_once() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
        let marker = "ROUTING_EXP_MARKER_7f3a";
        let builder = PromptBuilder::new(PromptConfig::default())
            .with_memory_user_message("MEM_BODY".to_string())
            .with_routing_experience_message(marker.to_string());
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);
        let full: String = parts.iter().map(|p| p.text.clone()).collect::<Vec<_>>().join("");
        assert_eq!(full.matches(marker).count(), 1, "routing text must be emitted exactly once");
        assert!(full.contains("MEM_BODY"), "memory still emitted alongside routing");
    }
```
(Adapt `PromptBuilder::new` / `SystemPromptPart` field accessor — `p.text` — to the real constructor + part shape used by the other tests already in this file; those tests at `cache.rs:125/137/159` show the exact builder + parts API.)
- **Run:** `cargo test -p alephcore --lib prompt_builder::cache` → **PASS**.
- **Commit:** `prompt: W1 — routing-experience injected exactly once on cached path`

---

### Task 4b: lib availability gate already built (Task 3) + boot assembly + runner struct fields

**Files:**
- Modify `src/orchestrator/harness_bridge/mod.rs` (~:222, add two fields to `AgentHarnessRunner`)
- Modify `src/bin/aleph-server/commands/start/orchestrator_init.rs` (~:41 new param, ~:223 assemble + set fields)
- Modify `src/bin/aleph-server/commands/start/mod.rs` (~:1173 pass embedder)

**Interfaces consumed:** boot `agent_result.embedder: Option<Arc<dyn EmbeddingProvider>>` (`mod.rs:1129` already reads it); `memory_backend: Option<MemoryBackend>` where `MemoryBackend = Arc<SqliteMemoryBackend>` (`memory/store/mod.rs:89`); `config.providers: HashMap<String, ProviderConfig>`; `shared_token_mgr: Arc<SharedTokenManager>` (param at `orchestrator_init.rs:63`); the lib `alephcore::routing::{RoutingExperienceStore, RoutingRecall, provider_availability_from_config}`.

- [ ] **Step 1: Add the two fields to `AgentHarnessRunner`.** In `src/orchestrator/harness_bridge/mod.rs`, after `pub primary_context_window: Option<u32>,` (the last field, ~:221):
```rust
    /// Routing-experience store (record path). `None` when no embedder is
    /// configured. Lives on the runner, never on `HarnessDeps` (R10).
    pub routing_store: Option<Arc<crate::routing::RoutingExperienceStore>>,
    /// Run-start routing recall (read path). `None` when no embedder is
    /// configured. Invoked once per run, pre-loop.
    pub routing_recall: Option<Arc<crate::routing::RoutingRecall>>,
```
- **Run:** `cargo check -p alephcore --lib` → **FAIL** (the single struct literal in `orchestrator_init.rs` does not yet set the new fields — this is the binary, surfaced by the next step's check). Proceed to Step 2.
- **Commit:** `orchestrator: add routing_store/routing_recall fields to runner`

- [ ] **Step 2: New `embedder` param on `initialize_orchestrator`; assemble store + recall; set fields.** In `src/bin/aleph-server/commands/start/orchestrator_init.rs`, add to the `initialize_orchestrator` signature (after the `memory_backend` param, ~:58):
```rust
    embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
```
Just before the `let harness = Arc::new(AgentHarnessRunner {` literal (~:223), assemble (clone `memory_backend`/`shared_token_mgr` since the literal/`build_guardrail_registry` consume them):
```rust
    let routing_store = match (embedder.clone(), memory_backend.clone()) {
        (Some(embedder), Some(backend)) => Some(std::sync::Arc::new(
            alephcore::routing::RoutingExperienceStore::new(backend, embedder),
        )),
        _ => None,
    };
    let routing_recall = routing_store.clone().map(|store| {
        let availability = alephcore::routing::provider_availability_from_config(
            config.providers.clone(),
            Some(shared_token_mgr.clone()),
        );
        std::sync::Arc::new(alephcore::routing::RoutingRecall::new(store, availability))
    });
```
Add the two fields inside the struct literal (after `parallel_tool_concurrency: ...,`):
```rust
        routing_store,
        routing_recall,
```
- **Run:** `cargo check -p alephcore --lib` → **PASS** (lib compiles; the binary param mismatch is fixed in Step 3 — lib check alone passes because the runner struct now has its fields set wherever the lib references them; the binary call-site arity is corrected next).
- **Commit:** `server: assemble routing store + recall (gate-reused availability) at boot`

- [ ] **Step 3: Pass the embedder at the call site.** In `src/bin/aleph-server/commands/start/mod.rs`, in the `initialize_orchestrator(...)` call (~:1173), add the new argument in the same position as the signature (immediately after `agent_result.memory_backend.clone(),`):
```rust
            agent_result.memory_backend.clone(),
            agent_result.embedder.clone(),
```
- **Run:** `cargo check -p alephcore --lib` → **PASS** (lib-only; the binary arity is now consistent and will be exercised by the final whole-crate check in Task 4d Step 6).
- **Commit:** `server: thread boot embedder into initialize_orchestrator`

---

### Task 4c: per-run wiring in `runner_impl.rs` + `build_system_prompt` threading

**Files:**
- Modify `src/orchestrator/harness_bridge/runner_impl.rs` (~:124 attribution+recall+model id; ~:152 thread routing_text; ~:254 observer wrap)
- Modify `src/orchestrator/harness_bridge/prompt_build.rs` (~:140 signature; ~:349 inject)

**Interfaces consumed (verified anchors):** `provider_name: String` (`runner_impl.rs:116`); `spec.agent` (the agent id, `:124`); `spec.brain` (`BrainRef::Strict { model: Some(m), .. }`); `user_query` (`:133`); `session_id: SessionId` (`:124`); `session_pref_key` (`:86`); `trace_sink: Option<Arc<dyn TraceSink>>` (run param, `:41`); `crate::providers::session_model_handle::get_session_model(&str) -> Option<SessionModelPref{ provider, model }>`; `self.agent_registry.get(&str) -> Option<AgentDef>` with field `model_hint: Option<String>`; `build_system_prompt` (`prompt_build.rs:140`, called at `runner_impl.rs:152`).

- [ ] **Step 1: `build_system_prompt` gains `routing_text` + injects next to memory.** In `src/orchestrator/harness_bridge/prompt_build.rs`, add the param as the **last** argument of `build_system_prompt` (~:140, after `workspace: Option<&std::path::Path>,`):
```rust
        routing_text: Option<String>,
```
At the memory seam (~:349, directly after the `if let Some(text) = memory_text { builder = builder.with_memory_user_message(text); }` block):
```rust
        if let Some(text) = routing_text {
            builder = builder.with_routing_experience_message(text);
        }
```
- **Run:** `cargo check -p alephcore --lib` → **FAIL** (the call site at `runner_impl.rs:152` does not yet pass `routing_text`). Fixed in Step 2.
- **Commit:** `prompt-build: thread routing_text into build_system_prompt (next to memory)`

- [ ] **Step 2: Per-run attribution + recall + frozen model id; pass `routing_text`.** In `src/orchestrator/harness_bridge/runner_impl.rs`, **after `session_id` is built (~:124) and before the `build_system_prompt` call (~:152)**, insert:
```rust
        // Per-run routing handle: co-locates recall backfill (writer) with the
        // completion observer (reader). §6/§7. Lives outside the harness (R10).
        let routing_attribution =
            std::sync::Arc::new(crate::routing::RoutingAttribution::new(session_id.to_key_string()));

        // Frozen model id for attribution — same precedence the spawn chain uses
        // (explicit > model_hint > native). `explicit` folds the dynamic
        // select_model pick and the BrainRef::Strict model.
        let routing_explicit_model: Option<String> =
            crate::providers::session_model_handle::get_session_model(&session_pref_key)
                .map(|p| p.model)
                .or_else(|| match &spec.brain {
                    crate::orchestrator::flow_spec::BrainRef::Strict { model: Some(m), .. } => Some(m.clone()),
                    _ => None,
                });
        let routing_model_hint: Option<String> =
            self.agent_registry.get(&spec.agent).and_then(|d| d.model_hint);
        let routing_model_id = crate::routing::resolve_routing_model_id(
            routing_explicit_model.as_deref(),
            routing_model_hint.as_deref(),
            &provider_name,
        );

        // Run-start recall (ONCE, pre-loop) → fenced String for the builder;
        // also backfills routing_attribution.task_emb for the observer (symmetry).
        let routing_text: Option<String> = if let Some(recall) = self.routing_recall.as_ref() {
            recall
                .build_routing_experience_message(&user_query, &spec.agent, None, &routing_attribution)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
```
Then add `routing_text` as the **last** argument of the existing `build_system_prompt(...)` call (~:152, after `workspace_override.as_deref(),`):
```rust
                workspace_override.as_deref(),
                routing_text,
```
- **Run:** `cargo check -p alephcore --lib` → **PASS**.
- **Commit:** `orchestrator: run-start routing recall + frozen model id (Seam 1)`

- [ ] **Step 3: Wrap the per-run sink with `OutcomeObserver` (top-level only).** In `src/orchestrator/harness_bridge/runner_impl.rs`, **immediately before the `let deps = HarnessDeps {` literal (~:254)** — i.e. after `MeteringProvider` (`:110-111`) already captured the raw sink, so subagents (which got the raw sink in the gateway run loop) are never contaminated:
```rust
        // Wrap the per-run sink so this run's SessionCompleted is observed —
        // harness-external (R10). Subagents already hold the RAW sink (captured
        // before this wrap in the gateway run loop), so they are never routed
        // into this observer (v1: top-level runs only; no cross-agent leakage).
        let trace_sink = match (trace_sink, self.routing_store.as_ref()) {
            (Some(parent), Some(store)) => Some(std::sync::Arc::new(
                crate::routing::OutcomeObserver::new(
                    parent,
                    store.clone(),
                    routing_attribution.clone(),
                    routing_model_id.clone(),
                    provider_name.clone(),
                    spec.agent.clone(),
                ),
            ) as std::sync::Arc<dyn crate::harness::TraceSink>),
            (other, _) => other,
        };
```
This rebinds `trace_sink`; the existing `trace_sink: trace_sink.clone()` (`:264`) and `trace_sink.as_ref()` uses (`:325`, `:378`) now carry the observer, which forwards every event unchanged.
- **Run:** `cargo check -p alephcore --lib` → **PASS**.
- **Commit:** `orchestrator: wrap per-run sink with OutcomeObserver (top-level only)`

---

### Task 4d: e2e through the real sink (E1/E2) + W1 think.rs purity + non-regression

**Files:**
- Modify `src/routing/mod.rs` (add `#[cfg(test)] mod integration_tests`)

- [ ] **Step 1: E1 — accrual through the REAL `TraceSink::on_trace` path.** Add `#[cfg(test)] mod integration_tests` to `src/routing/mod.rs`:
```rust
#[cfg(test)]
mod integration_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::error::AlephError;
    use crate::harness::trace::{LoopTraceEvent, LoopTraceSessionOutcome};
    use crate::harness::TraceSink;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::memory::EmbeddingProvider;
    use crate::orchestrator::dispatch::{TerminateReason, TokenBreakdown, ToolInvocation};

    use super::{OutcomeObserver, RoutingAttribution, RoutingExperienceStore};

    fn emb(seed: f32) -> Vec<f32> { let mut v = vec![0.0f32; 768]; v[0] = seed; v }
    fn temp_backend() -> SqliteMemoryBackend {
        let dir = std::env::temp_dir().join(format!("aleph-routing-int-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap()
    }
    struct StubEmbedder { vec: Vec<f32> }
    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> { Ok(self.vec.clone()) }
        async fn embed_batch(&self, t: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> { Ok(t.iter().map(|_| self.vec.clone()).collect()) }
        fn dimensions(&self) -> usize { 768 }
        fn model_name(&self) -> &str { "stub" }
        fn provider_id(&self) -> &str { "stub" }
    }
    #[derive(Default)]
    struct SpySink { session_completed: AtomicUsize }
    impl TraceSink for SpySink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            if matches!(event, LoopTraceEvent::SessionCompleted { .. }) {
                self.session_completed.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn flush(&self) {}
    }
    fn session_completed() -> LoopTraceEvent {
        LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Completed,
            iterations: 2,
            tool_calls_made: 1,
            total_tokens: 30,
            hit_limit: false,
            final_text: Some("done".into()),
            terminate_reason: Some(TerminateReason::Completed),
            duration_ms: Some(123),
            token_breakdown: Some(TokenBreakdown { input: 10, output: 20, cache_read: 0, cache_creation: 0, reasoning: 0 }),
            tool_timeline: vec![ToolInvocation { id: "1".into(), name: "bash".into(), duration_ms: 5, success: true, error: None }],
        }
    }
    async fn drain_until_row(store: &RoutingExperienceStore, agent: &str) -> Vec<crate::memory::store::sqlite::routing_experience::RoutingNeighbor> {
        // `#[tokio::test]` is current-thread: yielding lets the spawned
        // fire-and-forget record task run. Bounded poll → deterministic.
        let mut got = Vec::new();
        for _ in 0..200 {
            tokio::task::yield_now().await;
            got = store.recall(agent, &emb(0.0), 5).await.unwrap();
            if !got.is_empty() { break; }
        }
        got
    }

    #[tokio::test]
    async fn observer_on_trace_records_through_real_sink() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let spy = Arc::new(SpySink::default());
        let attribution = Arc::new(RoutingAttribution::new("run".into()));
        attribution.task_emb.set(emb(1.0)).unwrap(); // recall would have set this
        let observer = OutcomeObserver::new(
            spy.clone() as Arc<dyn TraceSink>,
            store.clone(),
            attribution,
            "MODEL_X".into(), "PROV_Y".into(), "agentA".into(),
        );
        observer.on_trace(&session_completed());
        let got = drain_until_row(&store, "agentA").await;
        assert_eq!(spy.session_completed.load(Ordering::SeqCst), 1, "forwarded unchanged");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model_id, "MODEL_X");
        assert_eq!(got[0].provider_id, "PROV_Y");
        assert_eq!(got[0].agent_id, "agentA");
        assert_eq!(got[0].iterations, 2);
        assert_eq!(got[0].tool_call_total, 1);
    }
}
```
- **Run:** `cargo test -p alephcore --lib routing::integration_tests::observer_on_trace_records_through_real_sink` → **PASS**.
- **Commit:** `routing: e2e — record through real TraceSink::on_trace path (E1)`

- [ ] **Step 2: E2 — per-agent attribution isolation through real sinks.** Add to `integration_tests`:
```rust
    #[tokio::test]
    async fn parent_and_child_attribution_isolated() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));

        // Two independently-constructed observers (the per-run sink-construction
        // model: each run freezes its own model + agent + attribution).
        let attr_p = Arc::new(RoutingAttribution::new("p".into()));
        attr_p.task_emb.set(emb(1.0)).unwrap();
        let obs_p = OutcomeObserver::new(
            Arc::new(SpySink::default()) as Arc<dyn TraceSink>,
            store.clone(), attr_p, "M".into(), "P".into(), "parent".into(),
        );
        let attr_c = Arc::new(RoutingAttribution::new("c".into()));
        attr_c.task_emb.set(emb(2.0)).unwrap();
        let obs_c = OutcomeObserver::new(
            Arc::new(SpySink::default()) as Arc<dyn TraceSink>,
            store.clone(), attr_c, "N".into(), "P".into(), "child".into(),
        );

        obs_p.on_trace(&session_completed());
        obs_c.on_trace(&session_completed());
        let p = drain_until_row(&store, "parent").await;
        let c = drain_until_row(&store, "child").await;
        assert!(p.iter().all(|n| n.model_id == "M")); // parent never absorbs child
        assert!(c.iter().all(|n| n.model_id == "N")); // child never written to parent's model
    }
```
- **Run:** `cargo test -p alephcore --lib routing::integration_tests` → **PASS** (both e2e).
- **Commit:** `routing: e2e — per-agent attribution isolation through real sinks (E2)`

- [ ] **Step 3: W1(b) — `build_prompt` path never references routing recall (think.rs purity).** Add to `integration_tests`:
```rust
    #[test]
    fn build_prompt_path_never_references_routing_recall() {
        // Source-level guard: recall is run-start only; the per-turn prompt
        // assembly (`prompt.rs::build_prompt`, called by `think.rs`) must never
        // touch routing recall (R10 — loop stays dumb).
        let prompt_src = include_str!("../harness/agent/prompt.rs");
        let think_src = include_str!("../harness/agent/think.rs");
        for needle in ["RoutingRecall", "build_routing_experience_message", "routing_recall"] {
            assert!(!prompt_src.contains(needle), "prompt.rs must not reference {needle}");
            assert!(!think_src.contains(needle), "think.rs must not reference {needle}");
        }
    }
```
- **Run:** `cargo test -p alephcore --lib routing::integration_tests::build_prompt_path_never_references_routing_recall` → **PASS**.
- **Commit:** `routing: W1 — assert build_prompt path never calls recall`

- [ ] **Step 4: Non-regression verification (read-only; no cargo).**
  - **N1 (harness budget):** `git diff --stat -- src/harness/` is empty (especially `trace.rs`); `ls src/harness/*.rs src/harness/agent/*.rs | wc -l` == 12.
  - **N2 (`select_model` byte-identical):** `git diff --stat -- src/builtin_tools/select_model.rs` is empty.
  - **N3 (`list_models` purity, §5.5 deferred):** `git diff --stat -- src/builtin_tools/list_models.rs` is empty (the gate semantics are reused via the new lib helper, not by editing `list_models.rs`; no `ModelEntry` fields added).
  - **Subagent scope (verified, no code):** confirm the gateway run loop hands the **raw** sink to `SubagentTool::with_trace_sink` (`src/gateway/execution_engine/run_loop/inner.rs:760`) and to `orchestrator.run` (`:851`), and that the observer wrap in `runner_impl.rs` lands only in the top-level `HarnessDeps` — so child completions never reach the parent observer (top-level-only capture; no cross-agent leakage; no spawn-side code).
- **Commit:** `routing: N1/N2/N3 + subagent-scope verified (no diff)`

- [ ] **Step 5: Final whole-crate compile gate.**
- **Run:** `cargo check -p alephcore --lib` → **PASS** (entire crate compiles with all routing wiring, including the binary call-site arity).
- **Commit:** `routing: VESR v1 compile gate green`

---

## Notes for the executing worker

- **Crate paths:** library code (Tasks 1–3, 4a, 4c, 4d) uses `crate::routing::…`; **only** Task 4b (binary, `src/bin/aleph-server/…`) uses `alephcore::routing::…`. There is no `extern crate self as alephcore`, so `alephcore::` does not resolve inside the library.
- **Availability gate crosses the crate boundary via the lib helper only:** the binary calls `alephcore::routing::provider_availability_from_config(config.providers.clone(), Some(shared_token_mgr.clone()))`; it never calls `resolve_vault_secret` (which is `pub(crate)` and lib-internal). The helper's second arg is `Option<Arc<SharedTokenManager>>`, matching the real `resolve_vault_secret(key, vault: &SharedTokenManager)` signature.
- **Verified anchors (no guessing required):** `runner_impl.rs` — `provider_name` (`:116`), `spec.agent` (`:124`), `user_query` (`:133`), `session_id` (`:124`), `session_pref_key` (`:86`), `trace_sink` run param (`:41`), `build_system_prompt` call (`:152`), `HarnessDeps` literal (`:254`). `prompt_build.rs` — `build_system_prompt` (`:140`), memory seam (`:349`). `agent_init` boot — embedder (`:300`), `memory_db`/`memory_backend` (`MemoryBackend = Arc<SqliteMemoryBackend>`). `orchestrator_init.rs` — runner literal (`:223`), `shared_token_mgr` param (`:63`), `config.providers` (`HashMap<String, ProviderConfig>`, `ProviderConfig.api_key: Option<String>`). `mod.rs` orchestrator call (`:1173`), `agent_result.embedder` (`:1129`).
- **`UnifiedMessage` deliberately avoided:** `RoutingRecall` returns `Option<String>` (already fence-wrapped); the prompt-build seam threads `Option<String>` (`memory_text`), so no `UnifiedMessage` import in `src/routing/` (R3/P6). This is the one intentional deviation from spec §5.4's `Option<UnifiedMessage>`; W1 (Task 4a Step 4) asserts the routing text reaches the prompt exactly once regardless.
- **Three post-trace fields (`estimated_cost`/`context_tokens`/`context_window`) are `None`/`0` in v1** because `SessionCompleted` does not carry them and the observer must stay verbatim (R7). USD enrichment is v1.1 (rides with deferred §5.5).
- **§7 primary path confirmed:** model + provider are in scope at `runner_impl.rs:116/85-105` before sink construction, so attribution is injected at `OutcomeObserver::new(...)` and `src/harness/trace.rs` has zero diff. The §7 fallback (fields on `ProviderUsage`) is NOT used.
- **Subagent capture is out of scope for v1** (top-level runs only), and the wrap-after-tool-construction ordering makes this safe (no parent absorbs child) — verified, not assumed. If a future version wants subagent capture, the contingency is a per-child observer in `subagent_spawner` (its model precedence is already the tested `resolve_routing_model_id` chain) — not planned here.