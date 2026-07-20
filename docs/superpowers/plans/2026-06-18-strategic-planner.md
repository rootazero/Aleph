# Strategic Planner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an independent, tool-free LLM "strategic planner" at the top of `/goal`·`/loop`·`/workflow` that mints a short Strategy before any tool call, and weld that Strategy into every downstream execution node's prompt so long tasks stay on-goal (StraTA application-layer pattern).

**Architecture:** A one-shot planner runs *above* the harness loop (harness untouched, R10). It persists a structured `Strategy` to a composite-keyed `StrategyStore`. A Stable `StrategyLayer` welds the full `<strategy>` into the KV-cacheable prefix; a Dynamic `StrategyPointerLayer` echoes the guardrails verbatim near the prompt tail. Subagents and workflow DAG nodes (which bypass the main pipeline) get the Strategy through explicit threading seams. A `strategy` tool offers a dumb-write `revise` escape hatch; lifecycle clears keep the store in lockstep with the owning flow.

**Tech Stack:** Rust (crate `alephcore`, bin `aleph-server`), tokio + serde, rusqlite (mirrors `src/goal/`), the existing `src/thinker/` prompt pipeline, `AiProvider` trait.

**Spec:** `docs/superpowers/specs/2026-06-18-strategic-planner-design.md` (this plan implements it; §12 = build order = Group A→F).

## Global Constraints

*Every task's requirements implicitly include this section. Values copied verbatim from the spec.*

- **Scope:** the planner fires **only** for `/goal`, `/loop`, `/workflow`. Never ordinary chat.
- **Redlines:** R7 (LLM sovereignty — planner is real reasoning, never deterministic middleware) · R8 (everything-is-a-tool — `strategy` tool) · R9 (one extra LLM call per *task*, not per turn; planner lives **above** the loop) · R10 (`src/harness/` is **untouched**; the loop's "5 nots" intact; `revise` is dumb-write, never judges legitimacy).
- **Fail-soft everywhere:** any planner/provider failure (disabled, no provider, LLM error, timeout, self-gated) ⇒ **no Strategy stored** ⇒ the prompt is **byte-identical** to today ⇒ the command proceeds. The planner is never on a command's critical path.
- **Fire exactly once per task** with a "strategy already exists for this key?" guard; continuations only **read** via `active_strategy`, never re-plan.
- **Self-gate:** if the planner cannot produce a concrete (non-blank) guardrail, store **no** Strategy (`Strategy::is_empty()` ⇒ skip). Trivial `/loop` polls naturally yield nothing.
- **v1 simplification — `PlannerContext.tool_descriptions` is passed empty** at the fire sites (C4/C5). Spec §4 wants a curated tool shortlist for feasibility grounding, but the tool registry is not cleanly reachable from `GoalTool`/`LoopTool`; the planner prompt degrades gracefully ("capability surface not enumerated → keep guardrails about scope, not tools") and the §4 non-goal "planner must not pre-commit to specific tools" makes scope-framed guardrails the safer default anyway. Wiring real tool descriptions is a deliberate **v2** follow-up; do not block v1 on it.
- **Planner enable + provider fallback (resolve ONCE at init):** `enabled = config.strategy.map_or(true, |s| s.enabled)` (default-on; absent `[strategy]` ⇒ on). The effective planner provider = `enabled ? Some(build_strategy_planner_provider(...).unwrap_or_else(|| <executor main provider>.clone())) : None`. **`build_strategy_planner_provider` returning `None` means "use the executor's model", NOT "planner off"** — so the planner fires in the default config (no `planner_model` set). Only `enabled=false` turns it off.
- **StrategyStore composite key** (NOT bare `session_id`): `goal:{session_id}` · `loop:{session_id}` · `workflow:{run_id}`. Mirror `src/goal/store.rs` shape (SQLite `Mutex<Connection>`, `ON CONFLICT … DO UPDATE` upsert, `Ok(None)` on corrupt row, lock-poison via `.unwrap_or_else(|e| e.into_inner())`). Persistent.
- **`StrategyLayer`**: `stability()=Stable`, `priority()=70`, `paths()=[Basic,Hydration,Soul,Context,Cached]` (Cached is mandatory or it silently vanishes in production). Renders the full `<strategy>` envelope, **rendered once, injected verbatim**.
- **`StrategyPointerLayer`**: `stability()=Dynamic`, `priority()=1756`, `paths()` includes `Cached`. Renders the **guardrails verbatim** (`<strategy_reminder>`), de-duped against `StandingGoalLayer@1754` (no objective re-echo for `/goal`).
- **Both layers use the 3-guard empty inject** (`input.context` None → `ctx` field `as_deref` None → `is_empty`): a `None` Strategy leaves the prompt **byte-identical at head AND tail**.
- **`render_*` are pure and deterministic** — no timestamps, no `HashMap` iteration order (sorted/`Vec` only), no `now_ms` in the Stable body.
- **`prompt_pipeline.rs` count asserts** (will fail the build if not bumped): `layer_count` 40 → **42**; `dynamic_names.len()` 14 → **15** + `assert!(dynamic_names.contains(&"strategy_pointer"))`.
- **`ContextAggregator::resolve` literal** (`src/thinker/context.rs:258-268`) MUST get `strategy: None` and `strategy_guardrails: None` — missing a field is a hard E0063.
- **`[strategy] planner_model`**: tier-1 explicit only — **NO** `default_aux_model` fallback (don't downgrade the strategist to a flash model); keep the same-as-primary no-op guard; fail-soft to `None` ⇒ planner reuses the executor provider. `enabled` defaults **true**.
- **Workflow weld = global-frame only**: objective + cross-cutting guardrails, **no phases**; labeled `## Global Strategy (context — your specific task is below)`; the per-node task description stays authoritative (placed after, dominates).
- **Subagent weld** goes through `SpawnRequest` → inline `PromptBuilder::with_strategy` — **NOT** `context_summary` (which lands in the user turn, is `ContextMode::Summary`-gated, and is mutable transcript).
- **Lifecycle clears** pair with authoritative end-points only: goal `Clear` (`goal.rs:325`) + loop `stop` (`loop_manage.rs:194`) + optional gate-confirmed Complete (`execute.rs:699`). **Never** clear on `Blocked` (a blocked goal may resume).
- **Cargo frugality (user's hard preference):** keep test-FIRST discipline, but do NOT run cargo after every step — batch ONE scoped `cargo test -p alephcore --lib <module>::` (or `cargo check -p alephcore`) at the END of each task. Commit per task.
- **Toolchain:** MSRV 1.95, pinned stable 1.96.0 (no `cargo +<ver>`). Commit format `<scope>: <description>` — **no** Co-Authored-By (attribution disabled in this repo).

## Task map (build order)

- **Group A** (A1–A4): the `src/strategy/` foundation — everything else consumes its types/store/render/keys.
- **Group B** (B1–B4): config + provider builder.
- **Group C** (C1–C5): the planner itself + firing it once at each flow start.
- **Group D** (D1–D7): the weld for normal/loop/continuation runs (layers + context + join).
- **Group E** (E1–E2): the weld for subagents + workflow DAG nodes.
- **Group F** (F1–F4): the `strategy` tool + lifecycle.

> Within-group order is strict. Groups A→B→C→D→E→F is the recommended execution order; D depends only on A, E depends on A+D (`PromptBuilder`/`StrategyLayer`), F depends on A+C.

---


## Group A — `src/strategy/` module (types · store · render · keys/global)

### Task A1: Strategy artifact type + self-gate (`src/strategy/types.rs`)

**Files:**
- Create: `/Volumes/TBU4/Workspace/Aleph/src/strategy/types.rs`
- Test: same file, `#[cfg(test)] mod tests` (mirrors `goal/types.rs`)

**Interfaces:**
- Produces (frozen contract): `pub struct Strategy { objective, approach, phases, guardrails, success_criteria, goal_id }`, `impl Strategy { pub fn is_empty(&self) -> bool }`. All later GROUP A tasks (store, render) and GROUPS C/D/E/F consume this exact struct.

Steps:

- [ ] **Step 1: Write the failing test.** Append this `tests` module to `src/strategy/types.rs` (it will fail to compile until Step 2 defines `Strategy`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Strategy {
        Strategy {
            objective: "Migrate auth to new API".into(),
            approach: "Incremental, behind a feature flag".into(),
            phases: vec!["understand the failure".into(), "implement".into(), "verify".into()],
            guardrails: vec!["do not refactor unrelated modules".into()],
            success_criteria: "gate command passes and old callers unaffected".into(),
            goal_id: Some("goal-deadbeef".into()),
        }
    }

    #[test]
    fn is_empty_false_when_concrete_guardrail_present() {
        assert!(!sample().is_empty());
    }

    #[test]
    fn is_empty_true_when_no_guardrails() {
        let s = Strategy {
            guardrails: Vec::new(),
            ..sample()
        };
        assert!(s.is_empty(), "no guardrail at all => non-strategy");
    }

    #[test]
    fn is_empty_true_when_all_guardrails_blank() {
        // Whitespace-only guardrails carry no concrete distractor (self-gate).
        let s = Strategy {
            guardrails: vec!["   ".into(), "\t".into(), "".into()],
            ..sample()
        };
        assert!(s.is_empty(), "all-blank guardrails => non-strategy");
    }

    #[test]
    fn is_empty_false_when_one_guardrail_nonblank() {
        let s = Strategy {
            guardrails: vec!["  ".into(), "avoid touching the parser".into()],
            ..sample()
        };
        assert!(!s.is_empty());
    }

    #[test]
    fn roundtrips_through_serde_json() {
        let s = sample();
        let json = serde_json::to_string(&s).unwrap();
        let back: Strategy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn old_payload_without_goal_id_deserializes_none() {
        // goal_id is #[serde(default)] — payloads minted before the cross-ref
        // field read None.
        let json = r#"{"objective":"o","approach":"a","phases":[],
            "guardrails":["x"],"success_criteria":"s"}"#;
        let s: Strategy = serde_json::from_str(json).expect("deserialize old payload");
        assert_eq!(s.goal_id, None);
    }
}
```

- [ ] **Step 2: Implement.** Write the full file header + struct + `is_empty` above the tests:

```rust
//! Strategy artifact — a short, welded "map" produced once at the top of a
//! long task (`/goal` · `/loop` · `/workflow`) and pinned into every
//! downstream execution prompt (the StraTA application-layer pattern).
//!
//! Immutable by construction (CLAUDE.md coding-style §不可变性): the planner
//! mints a `Strategy`, the store overwrites the row; nothing mutates in place.

use serde::{Deserialize, Serialize};

/// A lightly-structured strategy. The `guardrails` field is the StraTA secret
/// sauce and carries the fine resolution; `phases` stay coarse and
/// outcome-phrased (never tool names / arg shapes).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Strategy {
    /// One-line north star — restates the user's end goal.
    pub objective: String,
    /// The chosen overall play (advisory: "initial plan, adapt as you learn").
    pub approach: String,
    /// Coarse, ordered arc (NOT a tactical TODO). Outcome-phrased.
    pub phases: Vec<String>,
    /// 1–3 concrete, named, observable distractors to avoid. If every entry is
    /// blank the strategy is non-concrete → self-gated to nothing (`is_empty`).
    pub guardrails: Vec<String>,
    /// Semantic/human success statement — references the existing objective
    /// gate, never re-implements verification.
    pub success_criteria: String,
    /// Cross-ref to the originating goal (`goal.id`, FNV of `session:objective`)
    /// so a changed objective auto-invalidates a stale strategy. `#[serde(default)]`
    /// → payloads minted before this field read `None`.
    #[serde(default)]
    pub goal_id: Option<String>,
}

impl Strategy {
    /// A strategy with no concrete guardrail is no strategy at all: the planner
    /// self-gates to `None` and the prompt stays byte-identical. True when every
    /// guardrail is blank (or there are none).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.guardrails.iter().all(|g| g.trim().is_empty())
    }
}
```

- [ ] **Step 3 (last step): Verify + commit.**
```bash
cargo test -p alephcore --lib strategy::types::
git add src/strategy/types.rs
git commit -m "strategy: add Strategy artifact type with self-gate"
```
Note: this command requires `src/strategy/` to be a module. If `mod.rs` does not yet exist, create a one-line stub `pub mod types;` in `src/strategy/mod.rs` AND add `pub mod strategy;` to `src/lib.rs` so the test compiles; Task A2 then replaces the stub `mod.rs` with the full version. (If your orchestrator runs A2 before this verify, skip the stub.)

---

### Task A2: `src/strategy/mod.rs` — composite-key helpers + process-global

**Files:**
- Create/Modify: `/Volumes/TBU4/Workspace/Aleph/src/strategy/mod.rs`
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/lib.rs` (add `pub mod strategy;` if not already present)
- Test: `src/strategy/mod.rs` `#[cfg(test)] mod tests` (mirrors `goal/mod.rs`)

**Interfaces:**
- Consumes: `StrategyStore` (declared via `pub mod store;`; defined in Task A3), `Strategy` (Task A1).
- Produces (frozen contract): `pub fn goal_key(session_id: &str) -> String` (`goal:{session_id}`), `pub fn loop_key(session_id: &str) -> String` (`loop:{session_id}`), `pub fn workflow_key(run_id: &str) -> String` (`workflow:{run_id}`), `pub fn init_global(store: StrategyStore)`, `pub fn global() -> Option<&'static StrategyStore>`.

Note: the frozen `init_global`/`global` signatures take/return `StrategyStore` (not `Arc<StrategyStore>`), so the `OnceCell` holds `StrategyStore` directly and `global()` returns `Option<&'static StrategyStore>`. This is the contract-mandated shape and differs from `goal/mod.rs`'s `Arc<GoalStore>` — the structure (OnceCell + idempotent `set` + `#[cfg(test)] set_global_for_test`) is mirrored, the wrapper type follows the contract.

Steps:

- [ ] **Step 1: Write the failing test.** This is the full `mod.rs` test block (compiles after Step 2):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_key_is_prefixed() {
        assert_eq!(goal_key("sess-1"), "goal:sess-1");
    }

    #[test]
    fn loop_key_is_prefixed() {
        assert_eq!(loop_key("sess-1"), "loop:sess-1");
    }

    #[test]
    fn workflow_key_uses_run_id() {
        assert_eq!(workflow_key("run-abc"), "workflow:run-abc");
    }

    #[test]
    fn goal_and_loop_keys_for_same_session_differ() {
        // CRITICAL: a session running /goal AND /loop concurrently must not
        // collide — composite keying is the whole point.
        assert_ne!(goal_key("sess-1"), loop_key("sess-1"));
    }

    #[test]
    fn init_then_global_returns_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = StrategyStore::open(&dir.path().join("strat.db")).unwrap();
        set_global_for_test(store);
        assert!(global().is_some());
    }
}
```

- [ ] **Step 2: Implement.** Write the full `mod.rs` (mirrors `goal/mod.rs`; `OnceCell<StrategyStore>` per the frozen contract):

```rust
//! Strategic-planner subsystem: a welded `Strategy` (the StraTA application-layer
//! pattern) minted once at the top of `/goal` · `/loop` · `/workflow`, stored
//! persistently, and pinned into every downstream execution prompt. Distinct
//! from the standing `goal` (objective) and the per-task `scratchpad`.

pub mod store;
pub mod types;

pub use store::StrategyStore;
pub use types::Strategy;

use once_cell::sync::OnceCell;

/// Composite-key prefix for a `/goal`-flow strategy, keyed by session.
#[must_use]
pub fn goal_key(session_id: &str) -> String {
    format!("goal:{session_id}")
}

/// Composite-key prefix for a `/loop`-flow strategy, keyed by session. Distinct
/// from `goal_key` so a session running both flows never clobbers either row.
#[must_use]
pub fn loop_key(session_id: &str) -> String {
    format!("loop:{session_id}")
}

/// Composite-key prefix for a `/workflow`-flow strategy, keyed by run (a
/// workflow run is run-wide, not session-wide).
#[must_use]
pub fn workflow_key(run_id: &str) -> String {
    format!("workflow:{run_id}")
}

/// Process-global strategy store. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as "no
/// strategy subsystem" and the prompt layers stay dormant.
static GLOBAL: OnceCell<StrategyStore> = OnceCell::new();

/// Install the global store at boot. Idempotent: a second call is ignored.
pub fn init_global(store: StrategyStore) {
    let _ = GLOBAL.set(store);
}

/// Read the global store, if initialized.
#[must_use]
pub fn global() -> Option<&'static StrategyStore> {
    GLOBAL.get()
}

/// Test-only override. In production `init_global` is the only writer.
#[cfg(test)]
pub fn set_global_for_test(store: StrategyStore) {
    let _ = GLOBAL.set(store);
}
```

  Then ensure `src/lib.rs` declares the module. Locate the existing `pub mod goal;` line and add immediately after it (alphabetical-ish, near goal):
```rust
pub mod strategy;
```
  (If `pub mod strategy;` was already added by the Task A1 stub note, this is a no-op — confirm it is present exactly once.)

- [ ] **Step 3 (last step): Verify + commit.**
```bash
cargo test -p alephcore --lib strategy::tests::
git add src/strategy/mod.rs src/lib.rs
git commit -m "strategy: add composite-key helpers and process-global store"
```

---

### Task A3: `StrategyStore` — composite-keyed SQLite persistence

**Files:**
- Create: `/Volumes/TBU4/Workspace/Aleph/src/strategy/store.rs`
- Test: same file, `#[cfg(test)] mod tests` (mirrors `goal/store.rs`)

**Interfaces:**
- Consumes: `Strategy` (Task A1), `crate::error::{AlephError, Result}`, `crate::utils::sqlite_open::open_sqlite_safe`, composite-key helpers from `mod.rs` (Task A2) in tests.
- Produces (frozen contract): `pub struct StrategyStore`, `impl StrategyStore { pub fn open(path: &std::path::Path) -> anyhow::Result<Self>; pub fn put(&self, key: &str, strategy: &Strategy) -> anyhow::Result<()>; pub fn get(&self, key: &str) -> anyhow::Result<Option<Strategy>>; pub fn delete(&self, key: &str) -> anyhow::Result<()> }`.

Note on the return type: `goal/store.rs` uses `crate::error::Result` (alias for `Result<T, AlephError>`); the frozen contract spells the signatures with `anyhow::Result`. Since `AlephError` converts into `anyhow::Error`, we keep the internal body using `AlephError::other(...)` exactly like the reference and let `?`/return coerce — but the **declared** signature returns `anyhow::Result<...>` to match the contract verbatim. Concretely: the function bodies build `AlephError` and use `.map_err(|e| anyhow::anyhow!(...))` so the error type is `anyhow::Error`. The store mirrors `goal/store.rs` shape (lock-poison handling, `ON CONFLICT … DO UPDATE`, `Ok(None)` on corrupt JSON) field-for-field; only the error alias differs to satisfy the contract.

Steps:

- [ ] **Step 1: Write the failing test.** Append to `src/strategy/store.rs` (mirrors `goal/store.rs` tests; exercises composite keys + corrupt-row → `Ok(None)`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{goal_key, loop_key};
    use crate::strategy::types::Strategy;

    fn temp_store() -> (StrategyStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = StrategyStore::open(&dir.path().join("strategy.db")).unwrap();
        (store, dir)
    }

    fn sample(objective: &str) -> Strategy {
        Strategy {
            objective: objective.into(),
            approach: "incremental".into(),
            phases: vec!["understand".into(), "implement".into()],
            guardrails: vec!["do not refactor unrelated modules".into()],
            success_criteria: "gate passes".into(),
            goal_id: Some("goal-abc".into()),
        }
    }

    #[test]
    fn put_get_roundtrip() {
        let (store, _d) = temp_store();
        let k = goal_key("sess-1");
        store.put(&k, &sample("Do the thing")).unwrap();
        let got = store.get(&k).unwrap().unwrap();
        assert_eq!(got.objective, "Do the thing");
        assert_eq!(got.guardrails, vec!["do not refactor unrelated modules"]);
    }

    #[test]
    fn put_replaces_existing_for_same_key() {
        let (store, _d) = temp_store();
        let k = goal_key("sess-1");
        store.put(&k, &sample("first")).unwrap();
        store.put(&k, &sample("second")).unwrap();
        let got = store.get(&k).unwrap().unwrap();
        assert_eq!(got.objective, "second", "upsert overwrites same key");
    }

    #[test]
    fn composite_keys_do_not_clobber_each_other() {
        // CRITICAL bug guard: a session running /goal AND /loop must keep two
        // independent strategies — composite keys, not bare session_id.
        let (store, _d) = temp_store();
        let gk = goal_key("sess-1");
        let lk = loop_key("sess-1");
        store.put(&gk, &sample("goal-strategy")).unwrap();
        store.put(&lk, &sample("loop-strategy")).unwrap();
        assert_eq!(store.get(&gk).unwrap().unwrap().objective, "goal-strategy");
        assert_eq!(store.get(&lk).unwrap().unwrap().objective, "loop-strategy");
    }

    #[test]
    fn get_missing_is_none() {
        let (store, _d) = temp_store();
        assert!(store.get("goal:nope").unwrap().is_none());
    }

    #[test]
    fn corrupt_row_is_none_not_error() {
        // A bad JSON blob must never wedge prompt assembly — fail-safe to None,
        // mirroring GoalStore::get.
        let (store, _d) = temp_store();
        {
            let conn = store.lock();
            conn.execute(
                "INSERT INTO strategies (key, json) VALUES (?1, ?2)",
                rusqlite::params!["goal:bad", "{not valid json"],
            )
            .unwrap();
        }
        assert!(
            store.get("goal:bad").unwrap().is_none(),
            "corrupt JSON => Ok(None), never Err"
        );
    }

    #[test]
    fn delete_removes_row() {
        let (store, _d) = temp_store();
        let k = goal_key("sess-1");
        store.put(&k, &sample("x")).unwrap();
        store.delete(&k).unwrap();
        assert!(store.get(&k).unwrap().is_none());
    }
}
```

- [ ] **Step 2: Implement.** Write the full `src/strategy/store.rs` (mirrors `goal/store.rs` exactly; `anyhow::Result` per contract, internal errors via `AlephError::other` mapped into `anyhow`):

```rust
//! `StrategyStore` — `SQLite` persistence for welded strategies, keyed by a
//! composite `{flow}:{id}` string (`goal:<sess>` / `loop:<sess>` /
//! `workflow:<run>`), so a session running several long-task flows never
//! clobbers another's strategy.
//!
//! One row per key (PK = `key`), strategy serialized as a JSON blob. Opens via
//! the process-safe helper (`open_sqlite_safe`, Spec C) so it never races the
//! daemon's other `SQLite` writers. Persistent — survives `/resume` and daemon
//! restart, matching goal/workflow.

use std::path::Path;

use anyhow::Context;

use crate::error::AlephError;
use crate::strategy::types::Strategy;

pub struct StrategyStore {
    conn: std::sync::Mutex<rusqlite::Connection>,
}

impl StrategyStore {
    /// Open (creating if needed) the strategy DB at `path`.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AlephError::other(e.to_string()))
                .context("strategy store mkdir")?;
        }
        let conn = crate::utils::sqlite_open::open_sqlite_safe(path)
            .map_err(|e| AlephError::other(format!("strategy store open: {e}")))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS strategies (
                 key  TEXT PRIMARY KEY,
                 json TEXT NOT NULL
             )",
            [],
        )
        .map_err(|e| AlephError::other(format!("strategy store init: {e}")))?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        // P7 lock-safety: never propagate poison.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Upsert the strategy for its composite `key` (replaces any existing one).
    pub fn put(&self, key: &str, strategy: &Strategy) -> anyhow::Result<()> {
        let json = serde_json::to_string(strategy)
            .map_err(|e| AlephError::other(format!("strategy serialize: {e}")))?;
        self.lock()
            .execute(
                "INSERT INTO strategies (key, json) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET json = excluded.json",
                rusqlite::params![key, json],
            )
            .map_err(|e| AlephError::other(format!("strategy put: {e}")))?;
        Ok(())
    }

    /// Fetch the strategy for `key`, if any. A missing row is `Ok(None)`;
    /// corrupt JSON is also `Ok(None)` (fail-safe: a bad row must never wedge
    /// prompt assembly). Real DB errors propagate via `?` rather than being
    /// silently swallowed as "not found".
    pub fn get(&self, key: &str) -> anyhow::Result<Option<Strategy>> {
        use rusqlite::OptionalExtension;
        let conn = self.lock();
        let row: Option<String> = conn
            .query_row(
                "SELECT json FROM strategies WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::other(format!("strategy get: {e}")))?;
        Ok(row.and_then(|j| serde_json::from_str::<Strategy>(&j).ok()))
    }

    /// Remove the strategy for `key` (no-op if absent).
    pub fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.lock()
            .execute(
                "DELETE FROM strategies WHERE key = ?1",
                rusqlite::params![key],
            )
            .map_err(|e| AlephError::other(format!("strategy delete: {e}")))?;
        Ok(())
    }
}
```

  Note: the test's `corrupt_row_is_none_not_error` calls `store.lock()`, so `lock()` must be reachable from the test module. It is `fn lock(&self)` (private, same-module) — the `tests` module is a child of the same module, so `super::*` brings it in scope. No visibility change needed (mirrors `goal/store.rs`).

- [ ] **Step 3 (last step): Verify + commit.**
```bash
cargo test -p alephcore --lib strategy::store::
git add src/strategy/store.rs
git commit -m "strategy: add composite-keyed StrategyStore with fail-safe get"
```

---

### Task A4: `src/strategy/render.rs` — pure, deterministic renderers

**Files:**
- Create: `/Volumes/TBU4/Workspace/Aleph/src/strategy/render.rs`
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/strategy/mod.rs` (add `pub mod render;` + re-export the three fns)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Strategy` (Task A1).
- Produces (frozen contract): `pub fn render_strategy_summary(s: &Strategy) -> String` (full body, Stable StrategyLayer), `pub fn render_guardrails_only(s: &Strategy) -> String` (guardrail lines, Dynamic tail), `pub fn render_workflow_global_frame(s: &Strategy) -> String` (objective + cross-cutting guardrails, **NO phases**). GROUP D's `render_strategy_summary`/`render_guardrails_only` callers and GROUP E's `render_workflow_global_frame` caller consume these.

**Determinism contract (spec §5):** these are PURE — no timestamps, no `now_ms`, no `HashMap` iteration (all fields are `Vec`/`String`, rendered in declaration order). Same input → identical bytes. The functions return the INNER text only; the prompt layers (GROUP D) wrap them in `<strategy>…</strategy>` / `<strategy_reminder>…</strategy_reminder>` envelopes.

Steps:

- [ ] **Step 1: Write the failing test.** Append to `src/strategy/render.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::types::Strategy;

    fn sample() -> Strategy {
        Strategy {
            objective: "Migrate auth to the new API".into(),
            approach: "Incremental, behind a feature flag".into(),
            phases: vec![
                "understand the current failure".into(),
                "implement the migration".into(),
                "verify against the gate".into(),
            ],
            guardrails: vec![
                "do not refactor the unrelated parser".into(),
                "do not add new config keys".into(),
            ],
            success_criteria: "the objective gate passes and old callers still work".into(),
            goal_id: Some("goal-deadbeef".into()),
        }
    }

    #[test]
    fn summary_contains_all_sections() {
        let out = render_strategy_summary(&sample());
        assert!(out.contains("Migrate auth to the new API"));
        assert!(out.contains("Incremental, behind a feature flag"));
        assert!(out.contains("understand the current failure"));
        assert!(out.contains("do not refactor the unrelated parser"));
        assert!(out.contains("the objective gate passes"));
    }

    #[test]
    fn summary_is_deterministic_across_two_renders() {
        // PURE + DETERMINISTIC: same input rendered twice => identical bytes.
        // No timestamps, no HashMap ordering — guards the cache-prefix invariant.
        let s = sample();
        let a = render_strategy_summary(&s);
        let b = render_strategy_summary(&s);
        assert_eq!(a, b, "render must be byte-identical for identical input");
        // No timestamp / clock leak: there must be no digits-bearing "ms" stamp.
        assert!(!a.contains("ms"), "no timestamp may appear in the stable body");
    }

    #[test]
    fn guardrails_only_lists_guardrails_and_nothing_else() {
        let out = render_guardrails_only(&sample());
        assert!(out.contains("do not refactor the unrelated parser"));
        assert!(out.contains("do not add new config keys"));
        // De-dup vs StandingGoal: the tail must NOT restate the objective.
        assert!(
            !out.contains("Migrate auth to the new API"),
            "guardrail tail omits the objective to avoid reminder-blindness"
        );
        assert!(!out.contains("understand the current failure"), "no phases in tail");
    }

    #[test]
    fn guardrails_only_skips_blank_lines() {
        let s = Strategy {
            guardrails: vec!["  ".into(), "keep the change surgical".into(), "".into()],
            ..sample()
        };
        let out = render_guardrails_only(&s);
        assert!(out.contains("keep the change surgical"));
        // Blank guardrails are dropped, not rendered as empty bullets.
        assert!(!out.contains("- \n"), "no empty bullet for a blank guardrail");
    }

    #[test]
    fn guardrails_only_is_deterministic() {
        let s = sample();
        assert_eq!(render_guardrails_only(&s), render_guardrails_only(&s));
    }

    #[test]
    fn workflow_global_frame_excludes_phases() {
        // The DAG *is* the phase structure — the per-node weld drops the phase
        // list and welds only the run-global objective + cross-cutting guardrails.
        let out = render_workflow_global_frame(&sample());
        assert!(out.contains("Migrate auth to the new API"), "objective present");
        assert!(out.contains("do not refactor the unrelated parser"), "guardrails present");
        assert!(
            !out.contains("understand the current failure"),
            "phase 1 must not leak into the workflow global frame"
        );
        assert!(
            !out.contains("implement the migration"),
            "phase 2 must not leak into the workflow global frame"
        );
        assert!(
            !out.contains("verify against the gate"),
            "phase 3 must not leak into the workflow global frame"
        );
    }

    #[test]
    fn workflow_global_frame_is_deterministic() {
        let s = sample();
        assert_eq!(render_workflow_global_frame(&s), render_workflow_global_frame(&s));
    }
}
```

- [ ] **Step 2: Implement.** Write the full `src/strategy/render.rs`. All renderers iterate `Vec` fields in order and contain zero clock/HashMap access (the determinism guarantee):

```rust
//! Pure, deterministic renderers for the welded `Strategy`. These produce the
//! INNER text only — the prompt layers wrap them in `<strategy>` /
//! `<strategy_reminder>` envelopes.
//!
//! DETERMINISM CONTRACT (spec §5): no timestamps, no `now_ms`, no `HashMap`
//! iteration order. Every field is a `Vec`/`String` rendered in declaration
//! order, so the same `Strategy` renders to byte-identical output across calls.
//! This is what lets the Stable body ride the KV-cache prefix unchanged across
//! every turn of a long task (mirrors `curated_memory_envelope`).

use crate::strategy::types::Strategy;

/// Full `<strategy>` body for the Stable `StrategyLayer` — objective, approach,
/// the coarse phase arc, the concrete guardrails, and the success statement.
/// Rendered once, injected verbatim.
#[must_use]
pub fn render_strategy_summary(s: &Strategy) -> String {
    let mut out = String::new();
    out.push_str("Objective: ");
    out.push_str(s.objective.trim());
    out.push('\n');
    out.push_str("Approach: ");
    out.push_str(s.approach.trim());
    out.push('\n');

    let phases: Vec<&str> = s
        .phases
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if !phases.is_empty() {
        out.push_str("Phases:\n");
        for (i, phase) in phases.iter().enumerate() {
            out.push_str(&format!("  {}. {phase}\n", i + 1));
        }
    }

    let guardrails: Vec<&str> = s
        .guardrails
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .collect();
    if !guardrails.is_empty() {
        out.push_str("Guardrails (advisory — stay sovereign over moment-to-moment relevance):\n");
        for g in &guardrails {
            out.push_str(&format!("  - {g}\n"));
        }
    }

    let success = s.success_criteria.trim();
    if !success.is_empty() {
        out.push_str("Success: ");
        out.push_str(success);
        out.push('\n');
    }

    // Trim the single trailing newline so the layer controls envelope spacing.
    out.truncate(out.trim_end().len());
    out
}

/// Guardrail lines only, for the Dynamic `StrategyPointerLayer` tail near the
/// read head. Deliberately omits the objective (StandingGoalLayer already
/// re-injects that every turn — restating it here would cause reminder-blindness)
/// and the phases.
#[must_use]
pub fn render_guardrails_only(s: &Strategy) -> String {
    let mut out = String::new();
    for g in &s.guardrails {
        let g = g.trim();
        if g.is_empty() {
            continue;
        }
        out.push_str("- ");
        out.push_str(g);
        out.push('\n');
    }
    out.truncate(out.trim_end().len());
    out
}

/// Workflow per-node global frame: the run-global objective + cross-cutting
/// guardrails ONLY. Drops the coarse phase list — in a heterogeneous DAG the
/// graph itself is the phase structure, and a global phase list would conflict
/// with each node's local objective.
#[must_use]
pub fn render_workflow_global_frame(s: &Strategy) -> String {
    let mut out = String::new();
    out.push_str("Objective: ");
    out.push_str(s.objective.trim());
    out.push('\n');

    let guardrails: Vec<&str> = s
        .guardrails
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .collect();
    if !guardrails.is_empty() {
        out.push_str("Cross-cutting guardrails:\n");
        for g in &guardrails {
            out.push_str(&format!("  - {g}\n"));
        }
    }
    out.truncate(out.trim_end().len());
    out
}
```

  Then wire the module into `src/strategy/mod.rs`. Add the `mod` declaration next to the others:
```rust
pub mod render;
```
  and add the re-export next to `pub use types::Strategy;`:
```rust
pub use render::{render_guardrails_only, render_strategy_summary, render_workflow_global_frame};
```

- [ ] **Step 3 (last step): Verify + commit.**
```bash
cargo test -p alephcore --lib strategy::render::
git add src/strategy/render.rs src/strategy/mod.rs
git commit -m "strategy: add pure deterministic strategy renderers"
```

---

**GROUP A notes for the orchestrator:**
- Recommended task order: A1 (types) → A2 (mod.rs + lib.rs wiring) → A3 (store) → A4 (render). A2's `mod.rs` declares `pub mod store;`/`pub mod types;`; if A2 runs before A3, the `pub mod store;` line will reference a not-yet-created file — sequence A3 immediately after A2, or have A2 create a one-line `src/strategy/store.rs` stub that A3 overwrites. Simplest: run A1→A2→A3→A4 in order; each task's verify command only compiles once all its declared sibling modules exist, so the **first green build** is at the end of A3 (store), and A4 adds render. If the orchestrator wants each task independently green, A2 must stub `store.rs` (`pub struct StrategyStore;` placeholder) — but the cleaner path is strict ordering.
- Files created by GROUP A: `src/strategy/mod.rs`, `src/strategy/types.rs`, `src/strategy/store.rs`, `src/strategy/render.rs`. One line added to `src/lib.rs` (`pub mod strategy;`).
- Frozen-contract surface produced: `Strategy` + `is_empty`; `StrategyStore::{open,put,get,delete}`; `goal_key`/`loop_key`/`workflow_key`; `init_global`/`global`; `render_strategy_summary`/`render_guardrails_only`/`render_workflow_global_frame`. GROUPS C/D/E/F consume these exact names.
- Error-type reconciliation: `goal/store.rs` uses `crate::error::Result`; the frozen contract spells `anyhow::Result`. Task A3 declares `anyhow::Result` (per contract) while reusing `AlephError::other(...)` internally (coerces into `anyhow::Error`). If the orchestrator's `cargo check` flags `AlephError`→`anyhow::Error` coercion, confirm `AlephError: std::error::Error` (it is, via the crate's error module) — the `.map_err(|e| AlephError::other(...))?` then auto-converts through `?`. The explicit `.context(...)` import (`use anyhow::Context;`) is present for the mkdir path; if unused after final wording, drop that one line to avoid an unused-import warning.


---


## Group B — `[strategy]` config + fail-soft planner provider builder

### Task B1: Add `StrategyToml` config struct

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/config/types/phase6_wiring.rs` (add struct after `ModelThresholdToml` impl block, ~line 199; add tests in the existing `#[cfg(test)] mod tests`)
- Test: `/Volumes/TBU4/Workspace/Aleph/src/config/types/phase6_wiring.rs` (same `mod tests`)

**Interfaces:**
- Consumes: existing `phase6_wiring` serde/derive conventions (`#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]`, `#[serde(default, skip_serializing_if = "Option::is_none")]`).
- Produces (frozen contract): `pub struct StrategyToml { pub enabled: bool /*serde default = true*/, pub planner_model: Option<String> }`.

**Key mirror note (the one deviation from `ContextBudgetToml`):** `ContextBudgetToml::enabled` uses bare `#[serde(default)]` → defaults to `false`. `StrategyToml::enabled` must default to **`true`** (spec §9: opt-in/off-switch where the feature is on by default). A bare `#[serde(default)]` on a `bool` yields `false`, so we cannot reuse that pattern — we need an explicit `#[serde(default = "...")]` pointing at a helper that returns `true`, and a hand-written `Default` impl (so `..StrategyToml::default()` in test literals also yields `enabled: true`). This means **do NOT** add `Default` to the derive list; implement it manually.

- [ ] **Step 1: Write the failing tests** (append to `mod tests` in `phase6_wiring.rs`; also extend the `Probe` struct + the two existing whole-file tests so the new section is exercised the same way `context_budget` is). Add a `strategy` field to `Probe`:

```rust
    // add inside `struct Probe { ... }`, after the `context_budget` field:
        #[serde(default)]
        strategy: Option<StrategyToml>,
```

Add to `empty_toml_yields_none_for_all_sections` (after the existing `context_budget` assert):

```rust
        assert!(p.strategy.is_none());
```

Then append these three new tests:

```rust
    #[test]
    fn strategy_section_defaults_to_enabled() {
        // `[strategy]` present but `enabled` omitted → enabled = true, so the
        // feature is on by default (opt-out off-switch, not opt-in). This is the
        // one deviation from `[context_budget]`, which defaults to disabled.
        let p: Probe = toml::from_str("[strategy]\n").expect("toml parses");
        let s = p.strategy.expect("section present");
        assert!(s.enabled);
        // Planner model is opt-in — unset by default → planner reuses executor.
        assert!(s.planner_model.is_none());
    }

    #[test]
    fn strategy_default_is_enabled() {
        // `StrategyToml::default()` (used by `..StrategyToml::default()` in test
        // literals and by the absent-field path) must also yield enabled = true.
        let s = StrategyToml::default();
        assert!(s.enabled);
        assert!(s.planner_model.is_none());
    }

    #[test]
    fn strategy_planner_model_parses() {
        let toml_str = "[strategy]\nenabled = true\nplanner_model = \"claude-opus-4-8\"\n";
        let p: Probe = toml::from_str(toml_str).expect("toml parses");
        let s = p.strategy.expect("section present");
        assert!(s.enabled);
        assert_eq!(s.planner_model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn strategy_enabled_false_parses() {
        // The off-switch: explicitly disable the planner without removing the
        // section (A/B + Future-Proof escape valve, spec §2/§9).
        let p: Probe = toml::from_str("[strategy]\nenabled = false\n").expect("toml parses");
        let s = p.strategy.expect("section present");
        assert!(!s.enabled);
    }
```

- [ ] **Step 2: Implement** `StrategyToml` (insert after the `impl ContextBudgetToml { ... }` block at line 199, before `#[cfg(test)]`). Note: **no `Default` in the derive** — hand-rolled `Default` plus a serde default helper, because the field defaults to `true`:

```rust
/// `[strategy]` — opt-in (default **on**) strategic-planner welding for the
/// three long-task entry points (`/goal`, `/loop`, `/workflow`). When enabled,
/// a one-shot tool-free LLM planning node mints a short `Strategy` that is
/// welded into the cacheable system-prompt prefix of every downstream turn, so
/// a long task stays anchored to its objective and a small set of concrete
/// anti-distraction guardrails. The planner fires **once per task** (above the
/// Think→Act loop), is fully fail-soft, and self-gates: a trivial task or any
/// failure stores no Strategy and leaves the prompt byte-identical.
///
/// Unlike `[context_budget]`, this section defaults to **enabled = true**: the
/// welded Strategy is a model-independent context-engineering win (KV-cache
/// prefix reuse + attention anchoring), so it ships on. `enabled = false` is the
/// one-flip A/B + Future-Proof escape valve (spec §2): if a future model shows
/// zero goal-drift on these flows, the feature retires via config, no code
/// change.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StrategyToml {
    /// Master switch. Default **true** (on). Set `false` to fully disable the
    /// planner — no extra LLM call, no welded prefix, byte-identical prompts.
    #[serde(default = "strategy_enabled_default")]
    pub enabled: bool,
    /// Optional planner model id. When set, the planning call routes to a
    /// provider built from the *primary* provider's config (same vendor / API
    /// key / endpoint / protocol) with this model substituted. Unset (default)
    /// ⇒ the planner reuses the executor's main provider — planning is
    /// reasoning-heavy, so unlike `[context_budget].summary_model` there is no
    /// cheap-tier auto-fallback (that would silently downgrade the strategist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_model: Option<String>,
}

/// serde default for `StrategyToml::enabled` — the planner is on unless an
/// operator explicitly flips it off.
fn strategy_enabled_default() -> bool {
    true
}

impl Default for StrategyToml {
    fn default() -> Self {
        Self {
            enabled: strategy_enabled_default(),
            planner_model: None,
        }
    }
}
```

- [ ] **Step 3 (last step): Verify** — one scoped command for the whole task:
  - `cargo test -p alephcore --lib config::types::phase6_wiring::`
  - Then: `git add src/config/types/phase6_wiring.rs && git commit -m "config: add [strategy] StrategyToml section (enabled defaults true)"`

---

### Task B2: Wire `Config.strategy` field + `lib.rs` re-export

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/config/structs.rs` (add field after `context_budget` at line 233)
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/lib.rs` (add `StrategyToml` to the `types::phase6_wiring::{...}` re-export at lines 153-155)
- Test: covered indirectly by B3's builder tests (a `Config { strategy: ... }` literal must compile); no standalone test needed here — this is pure plumbing that the compiler verifies, and B3's `cargo` run exercises the new field. (Per the E0063 convention, B3 tests use `..Config::default()` so adding an `Option` field is non-breaking.)

**Interfaces:**
- Consumes: `StrategyToml` (from B1).
- Produces (frozen contract): `Config.strategy: Option<StrategyToml>` field; `crate::StrategyToml` re-export so `crate::StrategyToml` resolves (mirroring `crate::ContextBudgetToml`).

- [ ] **Step 1: Implement the `Config` field** (insert in `src/config/structs.rs` immediately after the `context_budget` field block ending at line 233, mirroring its `#[serde(default, skip_serializing_if = "Option::is_none")]` shape):

```rust
    /// Opt-in (default **on**) strategic-planner welding for the three long-task
    /// flows (`/goal`, `/loop`, `/workflow`). When `Some` and `enabled = true`
    /// (the default when the section is present), the start path builds a
    /// one-shot planner that welds a short `Strategy` into the cacheable
    /// system-prompt prefix. Absent section ⇒ `None` ⇒ planner uses the executor
    /// provider with the feature defaulting on; `enabled = false` is the
    /// off-switch. Fully fail-soft: any failure leaves prompts byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<crate::config::types::phase6_wiring::StrategyToml>,
```

- [ ] **Step 2: Implement the `lib.rs` re-export** — extend the existing `types::phase6_wiring::{...}` import/re-export block at lines 153-155 to include `StrategyToml` (keep alphabetical-ish grouping consistent with the existing list):

```rust
    types::phase6_wiring::{
        ContextBudgetToml, FallbackProviderToml, GuardrailsToml, ModelThresholdToml, StabilityToml,
        StrategyToml,
    },
```

- [ ] **Step 3 (last step): Verify** — one scoped command:
  - `cargo check -p alephcore`
  - Then: `git add src/config/structs.rs src/lib.rs && git commit -m "config: thread Config.strategy field and re-export StrategyToml"`

---

### Task B3: `build_strategy_planner_provider` (tier-1 only, fail-soft) + orchestrator re-export

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/orchestrator/deps_builder.rs` (add fn after `build_cheap_summary_provider` at line 863; add tests in the existing `#[cfg(test)] mod tests`)
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/orchestrator/mod.rs` (add `build_strategy_planner_provider` to the `deps_builder::{...}` re-export at lines 20-23)
- Test: `/Volumes/TBU4/Workspace/Aleph/src/orchestrator/deps_builder.rs` (same `mod tests`)

**Interfaces:**
- Consumes: `StrategyToml` / `Config.strategy` (B1/B2); `crate::providers::{create_provider, AiProvider}`, `crate::config::Config`, `crate::sync_primitives::Arc` (all already imported at the top of `deps_builder.rs`); test helper `cfg_with_fallback` + `ProviderConfig::test_config` (already in this test module).
- Produces (frozen contract): `pub fn build_strategy_planner_provider(config: &Config, primary_provider_key: &str) -> Option<std::sync::Arc<dyn crate::providers::AiProvider>>`; re-exported as `crate::orchestrator::build_strategy_planner_provider`.

**Critical deviations from `build_cheap_summary_provider` (spec §4 / §9):**
1. Read `config.strategy` (not `config.context_budget`); the field is `planner_model` (not `summary_model`).
2. **Default = same model as executor.** **Omit the tier-2 `default_aux_model` fallback entirely** (`deps_builder.rs:819-824`). When `planner_model` is unset/blank → return `None` (planner reuses the executor's main provider). Do **not** call `get_preset(...).default_aux_model`.
3. Keep the same-as-primary no-op guard (lines 828-834) verbatim.
4. Fail-soft to `None` on every path (missing section, disabled, unset/empty model, primary key absent, same-as-primary, `create_provider` error → `tracing::warn` → `None`).

- [ ] **Step 1: Write the failing tests** (append to the existing `mod tests` in `deps_builder.rs`, after the cheap-summary tests block ~line 1677). Add a strategy-specific config helper that mirrors `cfg_summary_keyed`/`cfg_summary` but builds `config.strategy` instead of `context_budget`:

```rust
    // ── strategy planner provider wiring (tier-1 only, no aux fallback) ──

    /// Config with `[strategy]` enabled, an optional `planner_model`, and a
    /// single mock-protocol primary provider keyed `key` whose default model is
    /// `primary_model`. Mirrors `cfg_summary_keyed`, but the planner builder
    /// reads `config.strategy` and has NO `default_aux_model` tier-2 fallback —
    /// so the `key` preset is irrelevant to its resolution.
    fn cfg_strategy_keyed(
        key: &str,
        primary_model: &str,
        planner_model: Option<&str>,
    ) -> Config {
        let mut primary = ProviderConfig::test_config(primary_model);
        primary.protocol = Some("mock".to_string());
        let mut cfg = cfg_with_fallback(None, vec![(key, primary)]);
        cfg.strategy = Some(crate::StrategyToml {
            enabled: true,
            planner_model: planner_model.map(str::to_string),
            ..crate::StrategyToml::default()
        });
        cfg
    }

    /// `cfg_strategy_keyed` with the non-preset key `"primary"`.
    fn cfg_strategy(primary_model: &str, planner_model: Option<&str>) -> Config {
        cfg_strategy_keyed("primary", primary_model, planner_model)
    }

    #[test]
    fn strategy_planner_none_when_section_missing() {
        // No `[strategy]` at all → no separate planner provider (the planner
        // reuses the executor's main provider).
        let cfg = Config::default();
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn strategy_planner_none_when_disabled() {
        let mut cfg = cfg_strategy("main-model", Some("planner-model"));
        cfg.strategy.as_mut().unwrap().enabled = false;
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn strategy_planner_none_when_unset_or_blank() {
        // planner_model unset/blank → reuse the executor provider (None). Unlike
        // summary_model there is NO cheap-tier auto-fallback to downgrade to.
        let cfg = cfg_strategy("main-model", None);
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
        let cfg = cfg_strategy("main-model", Some("   "));
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn strategy_planner_none_even_with_preset_aux_model() {
        // A preset that declares a cheap aux model (claude → claude-haiku-4-5)
        // must NOT trigger an aux fallback here — the planner is reasoning-heavy,
        // so unset planner_model reuses the executor, never the flash sibling.
        let cfg = cfg_strategy_keyed("claude", "claude-opus-4-8", None);
        assert!(
            build_strategy_planner_provider(&cfg, "claude").is_none(),
            "unset planner_model must reuse the executor, never auto-downgrade to aux"
        );
    }

    #[test]
    fn strategy_planner_none_when_equals_primary_default() {
        // A planner model identical to the primary's default would rebuild a
        // byte-identical provider — pointless, so reuse the main LLM.
        let cfg = cfg_strategy("same-model", Some("same-model"));
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn strategy_planner_none_when_primary_key_absent() {
        // planner_model set but the named primary has no [providers.*] entry.
        let cfg = cfg_strategy("main-model", Some("planner-model"));
        assert!(build_strategy_planner_provider(&cfg, "does-not-exist").is_none());
    }

    #[test]
    fn strategy_planner_some_when_distinct_model_builds() {
        // Enabled + a distinct, buildable planner model → a planner provider
        // that targets the primary vendor (mock protocol) with the swapped model.
        let cfg = cfg_strategy("main-model", Some("planner-model"));
        assert!(
            build_strategy_planner_provider(&cfg, "primary").is_some(),
            "enabled + distinct buildable planner model must yield a planner provider"
        );
    }
```

- [ ] **Step 2: Implement `build_strategy_planner_provider`** (insert in `deps_builder.rs` immediately after `build_cheap_summary_provider`'s closing brace at line 863, before `#[cfg(test)]`). This mirrors `build_cheap_summary_provider` exactly **except** it reads `config.strategy`, uses `planner_model`, and **drops the tier-2 `default_aux_model` arm** — unset/blank ⇒ `None`:

```rust
/// Build the optional dedicated provider for the strategic-planner node
/// (`[strategy] planner_model`). Mirrors [`build_cheap_summary_provider`]'s
/// vendor-cloning approach — clone the primary provider's config, swap only the
/// `models` vec — but with **two deliberate differences** (spec §4/§9):
///
/// 1. **No tier-2 `default_aux_model` fallback.** Planning is reasoning-heavy,
///    so an unset/blank `planner_model` must NOT silently downgrade the
///    strategist to a flash-tier sibling. Unset ⇒ `None` ⇒ the planner reuses
///    the executor's main provider.
/// 2. The section defaults to enabled, but is still honoured as an off-switch.
///
/// Returns `None` (planner reuses the executor provider) when:
/// - `[strategy]` is absent, or `enabled = false`;
/// - `planner_model` is unset or blank;
/// - the primary provider key has no `[providers.*]` entry to clone;
/// - the resolved model equals the primary's configured default (a separate
///   provider would be byte-identical — pointless);
/// - `create_provider` fails (bad protocol/preset) — logged, then degraded.
///
/// Fail-soft by construction: a misconfigured model never aborts boot. Total
/// failure ⇒ the planner runs on the main provider; if the planner call itself
/// fails, no Strategy is stored and the command proceeds with a byte-identical
/// prompt.
#[must_use]
pub fn build_strategy_planner_provider(
    config: &Config,
    primary_provider_key: &str,
) -> Option<Arc<dyn AiProvider>> {
    let strategy = config.strategy.as_ref()?;
    if !strategy.enabled {
        return None;
    }

    let base = config.providers.get(primary_provider_key)?;

    // Tier 1 only: explicit operator override. No `default_aux_model` tier-2 —
    // an unset/blank planner_model reuses the executor provider (return None).
    let planner_model: String = strategy
        .planner_model
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())?
        .to_string();

    // No-op when the resolved planner model is the primary's own default model —
    // the rebuilt provider would be byte-identical, so reuse the main LLM.
    if base
        .models
        .first()
        .is_some_and(|m| m.as_str() == planner_model)
    {
        return None;
    }

    let mut planner_cfg = base.clone();
    planner_cfg.models = vec![planner_model.clone()];
    match create_provider(primary_provider_key, planner_cfg) {
        Ok(provider) => {
            tracing::info!(
                provider = %primary_provider_key,
                planner_model = %planner_model,
                "strategy: routing strategic-planner node to a dedicated model"
            );
            Some(provider)
        }
        Err(e) => {
            tracing::warn!(
                provider = %primary_provider_key,
                planner_model = %planner_model,
                error = %e,
                "strategy: planner provider build failed — planner will use the main LLM"
            );
            None
        }
    }
}
```

- [ ] **Step 3: Implement the orchestrator re-export** — add `build_strategy_planner_provider` to the existing `pub use deps_builder::{...}` block in `src/orchestrator/mod.rs` (lines 20-23):

```rust
pub use deps_builder::{
    build_cheap_summary_provider, build_context_budget_config, build_failover_chain,
    build_stability_triple, build_strategy_planner_provider, ProviderChain, StabilityTriple,
};
```

- [ ] **Step 4 (last step): Verify** — one scoped command:
  - `cargo test -p alephcore --lib orchestrator::deps_builder::tests::strategy_planner`
  - Then: `git add src/orchestrator/deps_builder.rs src/orchestrator/mod.rs && git commit -m "orchestrator: add fail-soft build_strategy_planner_provider (tier-1, no aux fallback)"`

---

### Task B4: Build the planner provider once in the start path and hand it to the strategy planner component

> **CRITICAL wiring rule (resolves the enable-gate + provider-fallback; supersedes any "process-global" mention elsewhere).** The planner provider flows to the tools via the **builder-config field** that Task C3 reads (`config.planner_provider.clone()`), NOT a process-global. The effective provider is computed **once** here with two rules the lone `build_strategy_planner_provider` call cannot express alone:
> 1. **Enable gate (default-on):** `enabled = config.strategy.as_ref().map_or(true, |s| s.enabled)`. Absent `[strategy]` ⇒ on.
> 2. **Executor fallback:** `build_strategy_planner_provider` returns `None` for BOTH "disabled" and the common "no `planner_model` set" case. `None` must mean **"use the executor's main provider"**, not "planner off" — otherwise the planner never fires in the default config. So when `enabled`, fall back to the executor provider.
>
> Net: `effective = if enabled { Some(build_strategy_planner_provider(config, primary_provider_key).unwrap_or_else(|| executor_provider.clone())) } else { None }`. Build once, above the loop (R10), set on the builder config the constructor already consumes — never a field on `AgentHarnessRunner`.

**Files:**
- Modify: the **builder-config struct** the constructor consumes (the `config:` param at `src/executor/builtin_registry/builder/constructor/mod.rs` — grep its type with `rg "struct .*Config" src/executor/builtin_registry/builder/constructor/`; it already carries `team_store`, `dispatch_signal`, `browser_profile_manager`). Add a `pub planner_provider: Option<Arc<dyn AiProvider>>` field (default `None` in its `Default`/construction).
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/commands/start/orchestrator_init.rs` (import at line 24; compute `effective` planner provider inside `initialize_orchestrator` where that builder-config is assembled and the primary provider Arc is in scope, before the constructor runs).
- Test: none added here — bin construction-site plumbing (`orchestrator_init.rs` is not covered by `-p alephcore --lib`); B3 unit-tests the builder. Compiler-verified via the scoped `cargo check --bin aleph-server`.

**Interfaces:**
- Consumes: `build_strategy_planner_provider` (B3, via `alephcore::orchestrator::{...}`); `config: &Config` + `primary_provider_key: &str` (already a param of `initialize_orchestrator` at line 43); the **executor main provider** `Arc<dyn AiProvider>` already built in this fn for the harness (grep for the primary `AiProvider` local, e.g. `let provider`/`primary_provider`/the one passed to `AgentHarnessRunner`).
- Produces: the builder-config field `planner_provider: Option<Arc<dyn AiProvider>>` populated with the `effective` value — read by C3 at `config.planner_provider.clone()`.

- [ ] **Step 1: Add the field** to the builder-config struct (grep-confirmed name). Mirror the existing `Option`-of-`Arc` fields (e.g. `team_store`); add `pub planner_provider: Option<Arc<dyn AiProvider>>` and initialise it `None` wherever that struct is constructed/`Default`-ed.

- [ ] **Step 2: Add the import** — extend the existing `alephcore::orchestrator::{...}` use block (line 24) to include `build_strategy_planner_provider` (keep the rest of the multi-line `use` unchanged):

```rust
use alephcore::orchestrator::{
    build_cheap_summary_provider, build_context_budget_config, build_sandbox_factory,
    build_strategy_planner_provider,
```

- [ ] **Step 3: Compute the effective provider once + set it on the builder config.** Where the builder-config is assembled (the primary provider Arc is in scope here — substitute its real local name for `executor_provider`):

```rust
    // Strategic planner provider — resolved ONCE here, above the Think→Act loop
    // (R10), never a field on AgentHarnessRunner. Default-on: absent [strategy]
    // ⇒ enabled. build_strategy_planner_provider returns None for BOTH "disabled"
    // and "no planner_model set" — None means "use the executor's model", so when
    // enabled we fall back to the executor provider (else the planner would never
    // fire in the default config). enabled=false is the only off switch.
    let strategy_enabled = config.strategy.as_ref().map_or(true, |s| s.enabled);
    let planner_provider = if strategy_enabled {
        Some(
            build_strategy_planner_provider(config, primary_provider_key)
                .unwrap_or_else(|| executor_provider.clone()),
        )
    } else {
        None
    };
    // ...set `planner_provider` onto the builder-config struct assembled below.
```

- [ ] **Step 4 (last step): Verify** — one scoped command:
  - `cargo check --bin aleph-server`
  - Then: `git add src/bin/aleph-server/commands/start/orchestrator_init.rs <builder-config-struct file> && git commit -m "start: resolve strategy planner provider once (enable-gated, executor fallback)"`

---

**Cross-group notes for the orchestrator assembling all groups:**
- **B4 ↔ C3 share ONE seam: the builder-config field `planner_provider`.** B4 adds the field + populates it (enable-gated, executor fallback); C3 reads `config.planner_provider.clone()` into the three tools. There is **no** process-global for the planner provider. Sequence: the builder-config field must exist before C3 compiles; B4's `cargo check --bin aleph-server` needs Group C's `src/strategy/` types to resolve, so run B4 after Group A/C exist.
- **B2** (`Config.strategy` field + `lib.rs` re-export) must land before **B3** (its tests construct `cfg.strategy = Some(...)` and reference `crate::StrategyToml`). **B1** must land before **B2**. Order within Group B: B1 → B2 → B3 → B4.
- No E0063 risk: a brand-new `Option<StrategyToml>` field on `Config` and a brand-new `StrategyToml` struct introduce no exhaustive-literal break; all new test literals use `..StrategyToml::default()` / `..Config::default()`.


---


## Group C — tool-free Planner + fire-once wiring into goal/loop/workflow

### Task C1: Planner node (`plan_strategy`, tool-free, self-gating, fail-soft)

**Files:**
- Create: `/Volumes/TBU4/Workspace/Aleph/src/strategy/planner.rs`
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/strategy/mod.rs` (add `pub mod planner;` — Group A creates this file; this task only appends the module decl)
- Test: in-file `#[cfg(test)] mod tests` in `planner.rs`

**Interfaces:**
- Consumes (Group A contract): `crate::strategy::Strategy { objective, approach, phases, guardrails, success_criteria, goal_id }`, `Strategy::is_empty()`.
- Consumes (providers): `crate::providers::{AiProvider, RequestPayload, ProviderResponse}`, `crate::providers::message::UnifiedMessage`, `crate::providers::MockProvider` (tests).
- Produces (frozen contract, later tasks rely on):
  - `pub struct PlannerContext { pub tool_descriptions: Vec<String>, pub env_summary: String, pub lessons: Vec<String> }`
  - `pub async fn plan_strategy(provider: &std::sync::Arc<dyn crate::providers::AiProvider>, objective: &str, ctx: &PlannerContext, goal_id: Option<String>) -> Option<Strategy>`

Steps:

- [ ] **Step 1: Write the failing tests** (append to a new `planner.rs`; they reference `plan_strategy`/`PlannerContext` which don't exist yet, so the module fails to compile = RED). The tests use `MockProvider` to return canned JSON — the exact tool-free call pattern from `skill_distill.rs`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{AiProvider, MockError, MockProvider};
    use std::sync::Arc;

    fn ctx() -> PlannerContext {
        PlannerContext {
            tool_descriptions: vec!["bash — run shell commands".to_string()],
            env_summary: "os=macos cwd=/tmp/work".to_string(),
            lessons: vec![],
        }
    }

    /// A well-formed plan with a concrete guardrail round-trips into a Strategy.
    #[tokio::test]
    async fn plans_strategy_with_concrete_guardrail() {
        let json = r#"{
            "objective": "Migrate auth to the new API",
            "approach": "Port endpoints one module at a time",
            "phases": ["understand current auth", "port endpoints", "verify"],
            "guardrails": ["do not touch the billing module while migrating auth"],
            "success_criteria": "all auth endpoints answer on the new API and tests pass"
        }"#;
        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider::new(json));
        let s = plan_strategy(&provider, "Migrate auth to the new API", &ctx(), None)
            .await
            .expect("a concrete-guardrail plan must yield a Strategy");
        assert_eq!(s.objective, "Migrate auth to the new API");
        assert_eq!(s.guardrails.len(), 1);
        assert!(!s.is_empty());
    }

    /// The planner self-gates: an empty / blank guardrail set yields no Strategy
    /// (the most important regression — `Strategy::is_empty()` must be enforced).
    #[tokio::test]
    async fn self_gates_to_none_on_empty_guardrails() {
        let json = r#"{
            "objective": "say hi",
            "approach": "respond",
            "phases": ["respond"],
            "guardrails": ["", "   "],
            "success_criteria": "greeted"
        }"#;
        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider::new(json));
        let s = plan_strategy(&provider, "say hi", &ctx(), None).await;
        assert!(s.is_none(), "blank-only guardrails must self-gate to None");
    }

    /// Unparseable LLM output fails soft to None (never panics, never errors out).
    #[tokio::test]
    async fn unparseable_output_is_none() {
        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider::new("I cannot help with that."));
        let s = plan_strategy(&provider, "do a thing", &ctx(), None).await;
        assert!(s.is_none());
    }

    /// A provider error fails soft to None.
    #[tokio::test]
    async fn provider_error_is_none() {
        let provider: Arc<dyn AiProvider> =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Timeout));
        let s = plan_strategy(&provider, "do a thing", &ctx(), None).await;
        assert!(s.is_none());
    }

    /// The supplied goal_id is threaded into the Strategy for cross-ref
    /// auto-invalidation (overrides whatever the LLM emitted).
    #[tokio::test]
    async fn goal_id_is_stamped_into_strategy() {
        let json = r#"{
            "objective": "Ship X",
            "approach": "build then verify",
            "phases": ["build", "verify"],
            "guardrails": ["do not refactor the unrelated logging module"],
            "success_criteria": "X ships and tests pass"
        }"#;
        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider::new(json));
        let s = plan_strategy(&provider, "Ship X", &ctx(), Some("goal:sess-1:abc".to_string()))
            .await
            .unwrap();
        assert_eq!(s.goal_id.as_deref(), Some("goal:sess-1:abc"));
    }
}
```

- [ ] **Step 2: Implement `planner.rs`** (above the test module). Mirrors the `skill_distill` tool-free call exactly: `RequestPayload::new(&msgs).with_system(Some(system))` → `provider.process(...).await` → `response.text_content()` → tolerant outermost-`{...}` extraction → `serde_json::from_str`. Every error path returns `None` (fail-soft). The system prompt encodes the §3 guardrail CONTRACT and the §4 tool-free constraints.

```rust
//! Strategic planner node (军师) — a one-shot, tool-FREE LLM call that produces
//! a short `Strategy` at the top of a long task (`/goal` · `/loop` ·
//! `/workflow`), before any tool runs. StraTA's "plan-first, then weld" move,
//! application-layer only (no RL — R7). Fully fail-soft: ANY failure (provider
//! error, unparseable output, self-gate) yields `None`, leaving the downstream
//! prompt byte-identical and the command free to proceed (R9 / P7).

use std::sync::Arc;

use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::strategy::Strategy;

/// What the planner is allowed to see (tool-FREE): the curated tool
/// *descriptions* available to this run, a light env summary (OS / cwd), and —
/// for `/goal` — the existing goal lessons. It is told these are the only
/// capabilities and must not name specific tool calls.
pub struct PlannerContext {
    pub tool_descriptions: Vec<String>,
    pub env_summary: String,
    pub lessons: Vec<String>,
}

/// System prompt enforcing the §3 content contract and §4 tool-free rules.
/// Kept as a `const` so it is a single source of truth and trivially testable.
const PLANNER_SYSTEM: &str = "You are a strategist planning a long task before any work begins. \
You produce a SHORT, high-level Strategy that an executor will keep in view for the whole task. \
You CANNOT call tools; do not name specific tool calls or argument shapes. \
Reply with ONLY a single JSON object, no prose, with these fields:\n\
  objective: one line restating the user's end goal.\n\
  approach: the overall play, advisory (an initial plan to adapt as you learn).\n\
  phases: a coarse, outcome-phrased arc (e.g. \"understand the failure\", \"implement\", \"verify\"). \
NOT a tactical TODO; never name tools.\n\
  guardrails: 1-3 CONCRETE, named, observable distractors to avoid. CONTRACT: each must name a \
specific distractor tied to this task's real capability surface and be violable by a concrete next \
action. REJECT tautologies like \"stay focused\" or \"avoid scope creep\". Prefer scope-positive, \
observable phrasing. These are advisory, not hard prohibitions.\n\
  success_criteria: a semantic statement of done; reference the task's own gate, do not re-implement \
verification.\n\
CRITICAL self-gate: if you cannot produce at least ONE concrete (non-tautological) guardrail, return \
an EMPTY guardrails array — a trivial task deserves no Strategy. Do not invent filler guardrails.";

/// Tool-free, fail-soft planner. Returns `None` when the provider errors, the
/// output cannot be parsed, or the plan self-gates (no concrete guardrail, i.e.
/// `Strategy::is_empty()`). On success the supplied `goal_id` is stamped onto
/// the Strategy for objective-change auto-invalidation.
pub async fn plan_strategy(
    provider: &Arc<dyn AiProvider>,
    objective: &str,
    ctx: &PlannerContext,
    goal_id: Option<String>,
) -> Option<Strategy> {
    let prompt = build_planner_prompt(objective, ctx);
    let msgs = [UnifiedMessage::user(&prompt)];
    let payload = RequestPayload::new(&msgs).with_system(Some(PLANNER_SYSTEM));

    let response = match provider.process(payload).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "strategy planner LLM call failed; proceeding with no Strategy");
            return None;
        }
    };

    let mut strategy = parse_strategy(&response.text_content())?;
    // Self-gate: a Strategy with no concrete guardrail is welding noise; storing
    // nothing leaves the prompt byte-identical (strictly better). This is an LLM
    // judgement surfaced as data, not a code classifier (R7).
    if strategy.is_empty() {
        return None;
    }
    // Stamp the cross-ref id (overrides whatever the LLM emitted) so an
    // objective change can auto-invalidate the stored Strategy later.
    strategy.goal_id = goal_id;
    Some(strategy)
}

/// Render the user-side planner prompt from the objective + curated context.
/// An empty `tool_descriptions` is rendered explicitly so the model knows the
/// surface is unknown rather than empty-by-omission.
fn build_planner_prompt(objective: &str, ctx: &PlannerContext) -> String {
    let mut p = format!("Task objective:\n{objective}\n\n");
    p.push_str("Environment:\n");
    p.push_str(&ctx.env_summary);
    p.push_str("\n\nAvailable capabilities (the ONLY ones; do not assume others):\n");
    if ctx.tool_descriptions.is_empty() {
        p.push_str("(capability surface not enumerated — keep guardrails about scope, not tools)\n");
    } else {
        for d in &ctx.tool_descriptions {
            p.push_str("- ");
            p.push_str(d);
            p.push('\n');
        }
    }
    if !ctx.lessons.is_empty() {
        p.push_str("\nPrior lessons from this objective:\n");
        for l in &ctx.lessons {
            p.push_str("- ");
            p.push_str(l);
            p.push('\n');
        }
    }
    p.push_str("\nReturn the Strategy JSON now.");
    p
}

/// Tolerant parse: extract the outermost `{...}` JSON object from the LLM
/// response and deserialize it as a `Strategy`. Returns `None` on any failure
/// (mirrors `skill_distill::parse_distill_response`). The `goal_id` field is
/// `#[serde(default)]` on `Strategy`, so the planner JSON need not supply it.
fn parse_strategy(text: &str) -> Option<Strategy> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str::<Strategy>(&text[start..=end]).ok()
}
```

- [ ] **Step 3: Add the module declaration** to `/Volumes/TBU4/Workspace/Aleph/src/strategy/mod.rs` (Group A owns this file; insert near the other `pub mod` lines, mirroring `src/goal/mod.rs` ordering):

```rust
pub mod planner;
```

- [ ] **Step 4 (last step — Verify):** one scoped command, then commit.
```
cargo test -p alephcore --lib strategy::planner::
```
```
git add src/strategy/planner.rs src/strategy/mod.rs
git commit -m "strategy: tool-free fail-soft planner node (plan_strategy + PlannerContext)"
```

---

### Task C2: Inject `Option<Arc<dyn AiProvider>>` into GoalTool / LoopTool / WorkflowTool

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/goal.rs` (struct fields 79-84, `new` 86-91, add builder)
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/loop_manage.rs` (struct fields 103-108, `new` 110-119, add builder)
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/workflow_tool.rs` (struct fields 181-192, `new` 194-204, add builder)
- Test: in-file test modules of all three (one builder smoke test each)

**Interfaces:**
- Consumes: `crate::providers::AiProvider`, `crate::sync_primitives::Arc`.
- Produces (later C3/C4/C5 + Group E rely on these builders + private fields):
  - `GoalTool::with_planner_provider(self, Option<Arc<dyn AiProvider>>) -> Self`
  - `LoopTool::with_planner_provider(self, Option<Arc<dyn AiProvider>>) -> Self`
  - `WorkflowTool::with_planner_provider(self, Option<Arc<dyn AiProvider>>) -> Self`
  - private field `planner_provider: Option<Arc<dyn AiProvider>>` on each.

Steps:

- [ ] **Step 1: Write the failing tests** (one per tool; each asserts a tool built with a provider compiles and the field round-trips via the existing call path — RED because the builders don't exist yet).

Append to `goal.rs` tests (`mod tests`):
```rust
#[tokio::test]
async fn with_planner_provider_builds_and_still_sets_goal() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
    let handle = Arc::new(RwLock::new("sess-planner".to_string()));
    let provider: Arc<dyn crate::providers::AiProvider> =
        Arc::new(crate::providers::MockProvider::new("not json"));
    let tool = GoalTool::new(store)
        .with_session_key_handle(Some(handle))
        .with_planner_provider(Some(provider));
    // Provider present but unparseable → planner self-fails → goal Set still OK.
    let out = tool
        .call(GoalArgs {
            objective: Some("Provider-present goal".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
    assert!(out.success);
}
```

Append to `loop_manage.rs` tests (`mod tests`):
```rust
#[tokio::test]
async fn with_planner_provider_builds_and_still_starts_loop() {
    let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
    let provider: crate::sync_primitives::Arc<dyn crate::providers::AiProvider> =
        crate::sync_primitives::Arc::new(crate::providers::MockProvider::new("not json"));
    let tool = LoopTool::new(reg.clone())
        .with_session_for_test("sess-lp")
        .with_planner_provider(Some(provider));
    let out = tool
        .run(LoopArgs {
            action: LoopAction::Start,
            interval: Some("5m".to_string()),
            prompt: Some("watch".to_string()),
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
    assert!(out.success);
}
```

Append to `workflow_tool.rs` tests (`mod tests`):
```rust
#[test]
fn with_planner_provider_builds() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::agents::swarm::tasks::SqliteCoordTaskStore::open(
        &dir.path().join("c.db"),
    )
    .unwrap();
    let provider: Arc<dyn crate::providers::AiProvider> =
        Arc::new(crate::providers::MockProvider::new("x"));
    let _tool = WorkflowTool::new(Arc::new(store), None).with_planner_provider(Some(provider));
}
```
> Note: confirm the `SqliteCoordTaskStore::open` constructor used in this test matches the one already used by the existing `fn tool(...)` test helper at `workflow_tool.rs:822`; if the helper opens differently, reuse the helper's exact open call so the test compiles.

- [ ] **Step 2: Implement the field + `new` default + builder on `GoalTool`** (`goal.rs`). Add the import and edit the struct/`new`:

Add to the `use` block at top (after the existing `use crate::tools::AlephTool;`):
```rust
use crate::providers::AiProvider;
```
Struct (replace fields block 80-83):
```rust
pub struct GoalTool {
    store: Arc<GoalStore>,
    session_key: Option<Arc<RwLock<String>>>,
    /// Tool-free planner provider; `None` → no Strategy is minted on `set`
    /// (byte-identical to today). Injected at the construction site.
    planner_provider: Option<Arc<dyn AiProvider>>,
}
```
`new` (the `const fn new` must drop `const` because `Option<Arc<dyn _>>` default init in a non-const-friendly way is fine, but to stay minimal keep it `fn`):
```rust
impl GoalTool {
    pub fn new(store: Arc<GoalStore>) -> Self {
        Self {
            store,
            session_key: None,
            planner_provider: None,
        }
    }

    #[must_use]
    pub fn with_planner_provider(mut self, provider: Option<Arc<dyn AiProvider>>) -> Self {
        self.planner_provider = provider;
        self
    }
```
> `GoalTool::new` was `pub const fn`. Adding `Option<Arc<dyn AiProvider>>: None` keeps it const-eligible in principle, but to avoid a const-trait-object hazard across toolchains, change `pub const fn new` → `pub fn new`. Verify no caller relies on `new` in a const context (the two call sites are `definitions.rs:1057` and `constructor/mod.rs:264`, both runtime).

- [ ] **Step 3: Implement the same on `LoopTool`** (`loop_manage.rs`). Add import after `use crate::tools::AlephTool;`:
```rust
use crate::providers::AiProvider;
```
Struct (add field after `session_key`):
```rust
pub struct LoopTool {
    registry: Arc<LoopRegistry>,
    session_key: Option<Arc<RwLock<String>>>,
    /// Tool-free planner provider; `None` → no Strategy on `start`.
    planner_provider: Option<Arc<dyn AiProvider>>,
    #[cfg(test)]
    test_session: Option<String>,
}
```
`new` (add `planner_provider: None`) and builder:
```rust
    #[must_use]
    pub fn new(registry: Arc<LoopRegistry>) -> Self {
        Self {
            registry,
            session_key: None,
            planner_provider: None,
            #[cfg(test)]
            test_session: None,
        }
    }

    #[must_use]
    pub fn with_planner_provider(mut self, provider: Option<Arc<dyn AiProvider>>) -> Self {
        self.planner_provider = provider;
        self
    }
```

- [ ] **Step 4: Implement the same on `WorkflowTool`** (`workflow_tool.rs`). Add import after `use crate::tools::AlephTool;`:
```rust
use crate::providers::AiProvider;
```
Struct (add field after `team_store`):
```rust
    /// Tool-free planner provider; `None` → no Strategy minted on `run`.
    planner_provider: Option<Arc<dyn AiProvider>>,
}
```
`new` (add `planner_provider: None`) and builder (place next to `with_team_store`):
```rust
    pub fn new(
        coord_store: Arc<dyn CoordTaskStore>,
        dispatch_signal: Option<Arc<tokio::sync::Notify>>,
    ) -> Self {
        Self {
            coord_store,
            dispatch_signal,
            team_store: None,
            planner_provider: None,
        }
    }

    #[must_use]
    pub fn with_planner_provider(mut self, provider: Option<Arc<dyn AiProvider>>) -> Self {
        self.planner_provider = provider;
        self
    }
```

- [ ] **Step 5 (last step — Verify):** one scoped command, then commit.
```
cargo test -p alephcore --lib builtin_tools::goal:: builtin_tools::loop_manage:: builtin_tools::workflow_tool::with_planner_provider
```
```
git add src/builtin_tools/goal.rs src/builtin_tools/loop_manage.rs src/builtin_tools/workflow_tool.rs
git commit -m "builtin_tools: inject optional strategy planner provider into goal/loop/workflow tools"
```

---

### Task C3: Wire the planner provider at the construction sites

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/executor/builtin_registry/builder/constructor/mod.rs` (struct field assignment 724-725)
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/executor/builtin_registry/builder/constructor/coord_team_tools.rs` (WorkflowTool build 383-384)
- Test: none new (covered by C2 builder tests + C4/C5 integration); this is plumbing verified by `cargo check`.

**Interfaces:**
- Consumes (Group B contract): `build_strategy_planner_provider(config: &Config, primary_provider_key: &str) -> Option<Arc<dyn AiProvider>>`, re-exported via `src/orchestrator/mod.rs`. Consumes C2's `with_planner_provider`.
- Produces: goal/loop/workflow tools carrying the planner provider in production.

> **Assumption / dependency note for the orchestrator:** `build_strategy_planner_provider` needs `config` + `primary_provider_key`. Per spec §9 these are available in the orchestrator start path, but the `constructor` builder receives a `BuilderConfig` (`config`), not necessarily the raw `Config`/`primary_provider_key`. **Before implementing, grep the builder's `config` type** (`rg "struct .*Config" src/executor/builtin_registry/builder/constructor/`) to confirm a built `Option<Arc<dyn AiProvider>>` planner handle is already threadable. If the raw `Config` is not in scope here, the cleanest seam (matching how `browser_profile_manager`/`team_store` flow in via `config.*`) is: **Group B adds `pub planner_provider: Option<Arc<dyn AiProvider>>` to the builder config struct, built once in the orchestrator init path** (`orchestrator_init.rs`, where `primary_provider_key` is already a param per §9), and this task simply reads `config.planner_provider.clone()`. This keeps the build-once-above-the-loop rule (R10) and avoids calling `build_strategy_planner_provider` inside the per-tool constructor.

Steps:

- [ ] **Step 1: Wire goal + loop** (`constructor/mod.rs`). Replace the two struct-field lines (724-725):
```rust
            goal_tool: goal_tool
                .with_session_key_handle(memory_session_key_handle.clone())
                .with_planner_provider(config.planner_provider.clone()),
            loop_tool: loop_tool
                .with_session_key_handle(memory_session_key_handle.clone())
                .with_planner_provider(config.planner_provider.clone()),
```

- [ ] **Step 2: Wire workflow** (`coord_team_tools.rs`, lines 383-384):
```rust
            let tool = WorkflowTool::new(Arc::clone(coord_store), config.dispatch_signal.clone())
                .with_team_store(config.team_store.clone())
                .with_planner_provider(config.planner_provider.clone());
```

- [ ] **Step 3 (last step — Verify):** one scoped command, then commit.
```
cargo check -p alephcore
```
```
git add src/executor/builtin_registry/builder/constructor/mod.rs src/executor/builtin_registry/builder/constructor/coord_team_tools.rs
git commit -m "executor: wire strategy planner provider into goal/loop/workflow construction"
```

---

### Task C4: Fire `plan_strategy` once at goal `Set` and loop `start`

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/goal.rs` (`GoalAction::Set` arm, after `self.store.put(&goal)?` at line 250)
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/loop_manage.rs` (`start`, after `self.registry.put(state)` at line 185) — `start` is sync, so factor the planner fire into the async `run`/`call` path; see Step 2 note.
- Test: in-file test modules (fire-once guard + provider-None still succeeds)

**Interfaces:**
- Consumes: C1 `plan_strategy` + `PlannerContext`; Group A `crate::strategy::{StrategyStore, goal_key, loop_key, global}`; C2 `self.planner_provider`.
- Produces: a stored Strategy keyed `goal_key(session)` / `loop_key(session)` when a provider yields one; byte-identical behaviour when absent.

> **Fire-once guard (spec §4):** before calling `plan_strategy`, `if strategy_store.get(key)?.is_some() { skip }`. **Fail-soft:** the whole planner block is best-effort — a `None` store, `None` provider, or any error must NOT block the command. Store via `crate::strategy::global()` (process-global `StrategyStore`, `None` until boot → planner is a no-op in tests that don't init it, which keeps existing tests byte-identical).

> **Goal `goal_id` cross-ref:** the contract's `plan_strategy` takes `goal_id: Option<String>`. Pass the goal's id so objective-change auto-invalidation works. **Grep `Goal`'s id accessor** (`rg "pub id|fn id|goal.id" src/goal/types.rs`) — spec §6 references `goal.id` (FNV of `session:objective`, `types.rs:99-102`). Pass `Some(goal.id.clone())` (or the field's real name).

Steps:

- [ ] **Step 1: Write the failing tests.**

Goal — append to `goal.rs` tests:
```rust
/// With a planner provider that returns a concrete Strategy, goal `set` mints
/// and stores it under goal_key(session); a second `set` does NOT re-plan
/// (fire-once guard: the existing row is left intact).
#[tokio::test]
async fn goal_set_fires_planner_once_and_stores_strategy() {
    use crate::strategy::{goal_key, StrategyStore};
    let sdir = tempfile::tempdir().unwrap();
    crate::strategy::set_global_for_test(StrategyStore::open(&sdir.path().join("s.db")).unwrap());

    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
    let handle = Arc::new(RwLock::new("sess-fire".to_string()));
    let json = r#"{"objective":"o","approach":"a","phases":["p"],
        "guardrails":["do not touch the cache layer"],"success_criteria":"done"}"#;
    let provider: Arc<dyn crate::providers::AiProvider> =
        Arc::new(crate::providers::MockProvider::new(json));
    let tool = GoalTool::new(store)
        .with_session_key_handle(Some(handle))
        .with_planner_provider(Some(provider));

    tool.call(GoalArgs { objective: Some("First obj".into()), ..args(GoalAction::Set) })
        .await
        .unwrap();
    let stored = crate::strategy::global()
        .unwrap()
        .get(&goal_key("sess-fire"))
        .unwrap()
        .expect("a Strategy was minted");
    assert_eq!(stored.guardrails, vec!["do not touch the cache layer".to_string()]);

    // Re-set: the fire-once guard must skip planning, leaving the first row.
    tool.call(GoalArgs { objective: Some("Second obj".into()), ..args(GoalAction::Set) })
        .await
        .unwrap();
    let after = crate::strategy::global().unwrap().get(&goal_key("sess-fire")).unwrap().unwrap();
    assert_eq!(after.guardrails, stored.guardrails, "fire-once: row not re-planned");
}

/// Provider = None → goal `set` still succeeds and stores NO Strategy.
#[tokio::test]
async fn goal_set_with_no_provider_succeeds_without_strategy() {
    use crate::strategy::{goal_key, StrategyStore};
    let sdir = tempfile::tempdir().unwrap();
    crate::strategy::set_global_for_test(StrategyStore::open(&sdir.path().join("s.db")).unwrap());

    let (tool, _d) = tool_with_session("sess-noprov"); // no planner provider injected
    let out = tool
        .call(GoalArgs { objective: Some("Plain goal".into()), ..args(GoalAction::Set) })
        .await
        .unwrap();
    assert!(out.success);
    assert!(
        crate::strategy::global().unwrap().get(&goal_key("sess-noprov")).unwrap().is_none(),
        "no provider => no Strategy"
    );
}
```
> The two tests share a process-global store; `set_global_for_test` uses a `OnceCell` so only the FIRST wins. **Make each strategy test use a distinct session key** (`sess-fire`, `sess-noprov`) and rely on the first-initialised store — OR (cleaner) have Group A expose a `#[cfg(test)]` reset. Confirm Group A's `set_global_for_test` semantics before finalizing; if `OnceCell`-once, keep the tests in the SAME store and just use distinct keys (which the above already does).

Loop — append to `loop_manage.rs` tests:
```rust
#[tokio::test]
async fn loop_start_with_no_provider_succeeds_without_strategy() {
    use crate::strategy::{loop_key, StrategyStore};
    let sdir = tempfile::tempdir().unwrap();
    crate::strategy::set_global_for_test(StrategyStore::open(&sdir.path().join("s.db")).unwrap());
    let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
    let tool = LoopTool::new(reg).with_session_for_test("sess-lp-noprov");
    let out = tool
        .run(LoopArgs {
            action: LoopAction::Start,
            interval: Some("5m".to_string()),
            prompt: Some("watch".to_string()),
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
    assert!(out.success);
    assert!(crate::strategy::global().unwrap().get(&loop_key("sess-lp-noprov")).unwrap().is_none());
}
```

- [ ] **Step 2: Implement a shared fire helper + call it from goal `Set` and loop `start`.** Add a private async method to each tool (the planner is async; goal `call` is already async, loop `run` is async but `start` is sync — so call the helper from `run` after `start` returns Ok for the `Start` action).

Goal (`goal.rs`) — add a helper on `impl GoalTool` and call after `self.store.put(&goal)?` at line 250:
```rust
    /// Fire the tool-free planner ONCE for this session's goal, fail-soft.
    /// No-op when no provider is injected, no global StrategyStore exists, a
    /// Strategy already exists for the key, or the planner self-gates/errs.
    async fn maybe_plan_strategy(&self, session: &str, goal: &Goal) {
        let Some(provider) = &self.planner_provider else {
            return;
        };
        let Some(store) = crate::strategy::global() else {
            return;
        };
        let key = crate::strategy::goal_key(session);
        // Fire-exactly-once: a continuation / re-set must not re-plan.
        if matches!(store.get(&key), Ok(Some(_))) {
            return;
        }
        let ctx = crate::strategy::planner::PlannerContext {
            tool_descriptions: Vec::new(),
            env_summary: planner_env_summary(),
            lessons: goal.lessons.clone(),
        };
        if let Some(strategy) = crate::strategy::planner::plan_strategy(
            provider,
            &goal.objective,
            &ctx,
            Some(goal.id.clone()),
        )
        .await
        {
            // Best-effort: a put failure must not fail the goal command.
            let _ = store.put(&key, &strategy);
        }
    }
```
Then in the `Set` arm, immediately after `self.store.put(&goal)?;` (line 250) and before building `GoalOutput`:
```rust
                self.store.put(&goal)?;
                self.maybe_plan_strategy(&session, &goal).await;
```
Add a small free fn (module level in `goal.rs`, near `now_ms`):
```rust
/// Light env summary for the planner (OS + cwd), never failing.
fn planner_env_summary() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    format!("os={} cwd={}", std::env::consts::OS, cwd)
}
```
> Confirm `Goal` exposes `lessons: Vec<String>` and `id: String` (used in `render` at goal.rs:131 → `goal.lessons` exists; the FNV id per spec §6 → check exact field name in `src/goal/types.rs`). If the id accessor differs, adjust `Some(goal.id.clone())`.

Loop (`loop_manage.rs`) — `start` is sync and builds the `LoopState`; fire the planner from the async `run` after a successful `Start`. Edit `run`:
```rust
    pub async fn run(&self, args: LoopArgs) -> std::result::Result<LoopOutput, String> {
        let session = self.session().await;
        info!(session = %session, action = ?args.action, "loop operation");
        match args.action {
            LoopAction::Start => {
                // Capture the watch prompt before `start` consumes `args` so the
                // planner can plan over the loop's objective.
                let objective = args.prompt.clone().unwrap_or_default();
                let out = self.start(&session, args)?;
                if out.success {
                    self.maybe_plan_strategy(&session, &objective).await;
                }
                Ok(out)
            }
            LoopAction::Stop => self.stop(&session),
            LoopAction::Status => self.status(&session),
            LoopAction::Update => self.update(&session, args),
        }
    }
```
Add the helper + env fn on `impl LoopTool` (mirror goal; loops have no lessons → empty Vec):
```rust
    async fn maybe_plan_strategy(&self, session: &str, objective: &str) {
        let Some(provider) = &self.planner_provider else {
            return;
        };
        let Some(store) = crate::strategy::global() else {
            return;
        };
        let key = crate::strategy::loop_key(session);
        if matches!(store.get(&key), Ok(Some(_))) {
            return;
        }
        let ctx = crate::strategy::planner::PlannerContext {
            tool_descriptions: Vec::new(),
            env_summary: planner_env_summary(),
            lessons: Vec::new(),
        };
        if let Some(strategy) =
            crate::strategy::planner::plan_strategy(provider, objective, &ctx, None).await
        {
            let _ = store.put(&key, &strategy);
        }
    }
```
And the same module-level `planner_env_summary()` free fn in `loop_manage.rs` (near `now_ms`).
> Note: a model-paced loop's planner self-gates to `None` for trivial polling (spec intent), so most `/loop` ticks store nothing — verified behaviourally, not asserted in unit tests.

- [ ] **Step 3 (last step — Verify):** one scoped command, then commit.
```
cargo test -p alephcore --lib builtin_tools::goal:: builtin_tools::loop_manage::
```
```
git add src/builtin_tools/goal.rs src/builtin_tools/loop_manage.rs
git commit -m "builtin_tools: fire strategy planner once at goal set and loop start (fail-soft)"
```

---

### Task C5: Fire `plan_strategy` at workflow `Run` + expose result to Group E's `materialize`

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/workflow_tool.rs` (`WorkflowArgs::Run` arm 529-592; plan before `materialize` call at 569, store after `mat.run_id` is known)
- Test: in-file test module (provider-None Run still succeeds; provider-present stores under `workflow_key(run_id)`)

**Interfaces:**
- Consumes: C1 `plan_strategy`/`PlannerContext`; Group A `crate::strategy::{StrategyStore, workflow_key, global}`; C2 `self.planner_provider`. Consumes Group E's extended `materialize` signature.
- Produces: a `Strategy` stored under `workflow_key(&mat.run_id)`, AND the same `Option<&Strategy>` passed into `materialize` so Group E can stamp `render_workflow_global_frame(&s)` into each `CoordTask` metadata.

> **Sequencing reality (verified):** `materialize` mints `run_id` internally (`compile.rs:115`), so the `workflow_key(run_id)` is unknown until materialize returns. Therefore: (1) call `plan_strategy` **before** `materialize` (it only needs `input` + `WorkflowDef`, not `run_id`); (2) pass the resulting `Option<&Strategy>` **into** `materialize` (Group E adds a trailing param) so it can stamp task metadata at creation time; (3) after `materialize` returns `mat.run_id`, `store.put(workflow_key(&mat.run_id), &strategy)` under the fire-once guard. Group C owns steps (1) and (3); Group E owns the metadata stamping inside materialize. **Coordinate the exact new `materialize` param with Group E** — agreed shape: `strategy: Option<&crate::strategy::Strategy>` appended after `models`.

Steps:

- [ ] **Step 1: Write the failing tests.**

Append to `workflow_tool.rs` tests (reuse the existing `fn tool(...)` helper at line 822):
```rust
#[tokio::test]
async fn workflow_run_with_no_provider_succeeds_without_strategy() {
    use crate::strategy::StrategyStore;
    let sdir = tempfile::tempdir().unwrap();
    crate::strategy::set_global_for_test(StrategyStore::open(&sdir.path().join("s.db")).unwrap());
    // Build a tool with no planner provider, run a minimal workflow, assert OK.
    // (Mirror the existing run-path test setup at workflow_tool.rs:1060 for the
    //  team + def fixtures; assert out.success and that no strategy row exists
    //  for the returned run_id.)
    // ... fixture identical to the existing materialize-path test ...
}
```
> **Read the existing run-path test around `workflow_tool.rs:1060`** (the `let mat = workflow::materialize(...)` helper) and clone its team/def fixture so this test compiles. Assert `crate::strategy::global().unwrap().get(&crate::strategy::workflow_key(&run_id)).unwrap().is_none()` for the None-provider case.

- [ ] **Step 2: Implement the `Run` arm changes.** In `WorkflowArgs::Run { name, team_id, input }`, after `def`/`models` are computed and before the `materialize` call (line 569):
```rust
                // Plan ONCE before materialisation: the planner sees the run
                // input + WorkflowDef and produces a run-global Strategy. It does
                // not need the run_id (minted inside materialize). Fail-soft.
                let strategy = self.plan_workflow_strategy(&def, &input).await;
                let mat = workflow::materialize(
                    &def,
                    &input,
                    &team_id,
                    self.coord_store.as_ref(),
                    clarify_ctx.as_ref(),
                    (!models.is_empty()).then_some(&models),
                    strategy.as_ref(), // Group E: stamp the global frame into tasks.
                )
                .await?;
                // Persist under the now-known run_id so continuations read it via
                // active_strategy/workflow_key. Best-effort, fire-once.
                if let Some(strategy) = &strategy {
                    if let Some(store) = crate::strategy::global() {
                        let key = crate::strategy::workflow_key(&mat.run_id);
                        if !matches!(store.get(&key), Ok(Some(_))) {
                            let _ = store.put(&key, strategy);
                        }
                    }
                }
```
Add the helper on `impl WorkflowTool`:
```rust
    /// Tool-free planner for a workflow run, fail-soft. Returns `None` when no
    /// provider is injected or the planner self-gates/errs. The objective is the
    /// run input (the user's request for this workflow execution).
    async fn plan_workflow_strategy(
        &self,
        def: &WorkflowDef,
        input: &str,
    ) -> Option<crate::strategy::Strategy> {
        let provider = self.planner_provider.as_ref()?;
        let objective = if input.trim().is_empty() {
            format!("Run workflow '{}'", def.name)
        } else {
            input.to_string()
        };
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let ctx = crate::strategy::planner::PlannerContext {
            tool_descriptions: Vec::new(),
            env_summary: format!("os={} cwd={}", std::env::consts::OS, cwd),
            lessons: Vec::new(),
        };
        crate::strategy::planner::plan_strategy(provider, &objective, &ctx, None).await
    }
```
> The `materialize` call gains a trailing `strategy.as_ref()` arg. **This will not compile until Group E lands the new param.** Coordinate ordering: either (a) Group E ships the `materialize` signature change first, or (b) this task adds the arg behind agreement and the assembled plan orders Group E's materialize change before C5. Flag this dependency explicitly in the assembled plan. The `WorkflowArgs::Run` success message and output (`task_ids`/`run_id`) are otherwise unchanged.

- [ ] **Step 3 (last step — Verify):** one scoped command, then commit.
```
cargo test -p alephcore --lib builtin_tools::workflow_tool::
```
```
git add src/builtin_tools/workflow_tool.rs
git commit -m "workflow: fire strategy planner once at run and thread Strategy into materialize"
```

---

**Cross-group dependencies (for the orchestrator assembling the full plan):**
- **C1** depends on Group A (`Strategy`, `is_empty`, `src/strategy/mod.rs` existing).
- **C3** depends on Group B (`build_strategy_planner_provider` + a `planner_provider` handle threaded onto the builder config in the orchestrator init path; see C3 note — recommend Group B owns building it once and exposing `config.planner_provider`).
- **C4** depends on Group A (`StrategyStore`, `goal_key`, `loop_key`, `global`, `set_global_for_test`) + C2.
- **C5** depends on Group A (`workflow_key`) + C2 + **Group E** (extended `materialize` signature `strategy: Option<&Strategy>`). **Order Group E's materialize-signature change before C5** or C5 will not compile.

**Relevant absolute paths:**
- `/Volumes/TBU4/Workspace/Aleph/src/strategy/planner.rs` (new), `/Volumes/TBU4/Workspace/Aleph/src/strategy/mod.rs`
- `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/goal.rs`, `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/loop_manage.rs`, `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/workflow_tool.rs`
- `/Volumes/TBU4/Workspace/Aleph/src/executor/builtin_registry/builder/constructor/mod.rs`, `/Volumes/TBU4/Workspace/Aleph/src/executor/builtin_registry/builder/constructor/coord_team_tools.rs`
- Reference (read-only): `/Volumes/TBU4/Workspace/Aleph/src/memory/dreaming/stages/skill_distill.rs` (tool-free call + tolerant JSON parse), `/Volumes/TBU4/Workspace/Aleph/src/context/compact/compactor.rs:526-533` (call_llm), `/Volumes/TBU4/Workspace/Aleph/src/providers/mod.rs:232-274` (AiProvider trait), `/Volumes/TBU4/Workspace/Aleph/src/providers/adapter.rs:90-165` (RequestPayload), `/Volumes/TBU4/Workspace/Aleph/src/goal/{store.rs,mod.rs}` (store/global shape), `/Volumes/TBU4/Workspace/Aleph/src/workflow/compile.rs:102-115` (materialize mints run_id internally).


---


## Group D — prompt layers + ResolvedContext + active_strategy + 3-way join

### Task D1: ResolvedContext gains `strategy` + `strategy_guardrails` fields

**Files:**
- Modify `/Volumes/TBU4/Workspace/Aleph/src/thinker/context.rs:177-191` (add two fields after `standing_goal`/before `voice_mode_active`) and `:258-268` (add to the exhaustive `resolve()` literal — missing this is a hard E0063)
- Test `/Volumes/TBU4/Workspace/Aleph/src/thinker/context.rs` (`#[cfg(test)]` at bottom; if none exists, add one)

**Interfaces:**
- Produces: `ResolvedContext.strategy: Option<String>` (full `<strategy>` body) and `ResolvedContext.strategy_guardrails: Option<String>` (guardrail lines), both `#[serde(skip, default)]`. Consumed by D4/D5 layers and D6 (`prompt_build`).

- [ ] **Step 1: Write the failing test** — append to `src/thinker/context.rs` `#[cfg(test)]` module (create the module if absent). This pins the default-`None` invariant so the empty-path byte-identity holds:

```rust
#[cfg(test)]
mod strategy_field_tests {
    use super::*;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    #[test]
    fn resolve_defaults_strategy_fields_to_none() {
        let ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        // Both strategy surfaces default to absent so the prompt is
        // byte-identical for sessions with no planned Strategy.
        assert!(ctx.strategy.is_none());
        assert!(ctx.strategy_guardrails.is_none());
    }
}
```

- [ ] **Step 2: Implement** — add the two fields to `ResolvedContext` immediately after the `standing_goal` field (after `:183`), mirroring its `#[serde(skip, default)]` + doc-comment style:

```rust
    /// Full `<strategy>` body for `StrategyLayer` (priority 70, Stable),
    /// rendered once from the session's active `Strategy` via
    /// `render_strategy_summary`. Populated in the harness bridge from
    /// `active_strategy`; `None` (no planned Strategy) emits nothing, leaving
    /// the cacheable stable prefix byte-identical.
    #[serde(skip, default)]
    pub strategy: Option<String>,
    /// Guardrail lines for `StrategyPointerLayer` (priority 1756, Dynamic),
    /// rendered from the same `Strategy` via `render_guardrails_only` and
    /// echoed near the read head every turn to fight goal-drift. Populated in
    /// the harness bridge; `None` emits nothing (byte-identical tail).
    #[serde(skip, default)]
    pub strategy_guardrails: Option<String>,
```

  Then extend the single exhaustive struct literal in `ContextAggregator::resolve` (currently ending at `:267-268`) — add the two fields after `standing_goal: None,`:

```rust
            execution_plan: None,
            standing_goal: None,
            strategy: None,
            strategy_guardrails: None,
            voice_mode_active: false,
        }
```

- [ ] **Step 3: Verify** — `cargo test -p alephcore --lib thinker::context::strategy_field_tests`. Then `git add src/thinker/context.rs && git commit -m "context: add strategy + strategy_guardrails ResolvedContext fields"`.

---

### Task D2: `StrategyLayer` (Stable, priority 70, `<strategy>` envelope)

**Files:**
- Create `/Volumes/TBU4/Workspace/Aleph/src/thinker/layers/strategy.rs`
- Test: inline `#[cfg(test)]` in the same file (mirrors `standing_goal.rs:59-124`)

**Interfaces:**
- Consumes: `ResolvedContext.strategy: Option<String>` (D1)
- Produces: `pub struct StrategyLayer` implementing `PromptLayer` — `stability()=Stable`, `priority()=70`, `paths()=[Basic,Hydration,Soul,Context,Cached]`. Registered by D4.

- [ ] **Step 1: Write the failing test** — these go inside the file created in Step 2; they are the empty-path-first regression guards (§11). Written before the `inject` logic in the sense that they assert byte-identity for `None`/empty/missing-context:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerInput;
    use crate::thinker::security_context::SecurityContext;

    fn ctx_with_strategy(strategy: Option<&str>) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.strategy = strategy.map(|s| s.to_string());
        ctx
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        StrategyLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn no_strategy_emits_nothing() {
        let out = render(&ctx_with_strategy(None));
        assert!(out.is_empty());
    }

    #[test]
    fn empty_strategy_emits_nothing() {
        // present-but-empty body must still leave the prompt byte-identical.
        let out = render(&ctx_with_strategy(Some("")));
        assert!(out.is_empty());
    }

    #[test]
    fn missing_context_emits_nothing() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        StrategyLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn strategy_renders_inside_tag() {
        let body = "Objective: ship the planner\nApproach: plan-first\nGuardrails:\n- don't refactor unrelated modules";
        let out = render(&ctx_with_strategy(Some(body)));
        assert!(out.starts_with("<strategy>\n"));
        assert!(out.contains("Objective: ship the planner"));
        assert!(out.contains("don't refactor unrelated modules"));
        assert!(out.trim_end().ends_with("</strategy>"));
    }

    #[test]
    fn name_priority_stability() {
        assert_eq!(StrategyLayer.name(), "strategy");
        assert_eq!(StrategyLayer.priority(), 70);
        assert_eq!(StrategyLayer.stability(), LayerStability::Stable);
        assert!(StrategyLayer.paths().contains(&AssemblyPath::Cached));
    }
}
```

- [ ] **Step 2: Implement** — write the full file, mirroring `curated_memory.rs` (Stable verbatim envelope) + `standing_goal.rs` (3-guard inject). Note: `stability()` defaults to `Stable` in the trait, but we declare it explicitly for clarity, matching `curated_memory.rs:24-26`:

```rust
//! `StrategyLayer` — emits the welded `<strategy>` envelope at priority 70
//! (Stable, cacheable prefix).
//!
//! The StraTA-pattern strategic plan, minted once per long task by the
//! planner node and pinned into the stable, prefix-cacheable head of the
//! system prompt so its KV-cache is reused across every turn ("开始前先画
//! 地图，过程中不忘初心"). Sits between `CuratedMemoryLayer` (60) and
//! `ProfileLayer` (75) in the Stable zone.
//!
//! R10-safe: pure scaffolding. The body is the planner LLM's own rendered
//! `Strategy`, injected verbatim — the harness makes no judgment, runs no
//! extra LLM call here, and applies no relevance scoring. The content is
//! rendered once (deterministically, no timestamps) by `render_strategy_summary`
//! and stored in `ResolvedContext.strategy`. `None` emits nothing, leaving
//! the cacheable prefix byte-identical for sessions with no Strategy.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};

pub struct StrategyLayer;

impl PromptLayer for StrategyLayer {
    fn name(&self) -> &'static str {
        "strategy"
    }

    fn priority(&self) -> u32 {
        70
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn stability(&self) -> LayerStability {
        // The welded Strategy is minted once per task and held verbatim across
        // every turn — Stable so it rides the cached stable prefix.
        LayerStability::Stable
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };
        let Some(strategy) = ctx.strategy.as_deref() else {
            return;
        };
        if strategy.is_empty() {
            return;
        }
        output.push_str("<strategy>\n");
        output.push_str(strategy);
        output.push_str("\n</strategy>\n\n");
    }
}
```

  Then append the `#[cfg(test)]` module from Step 1.

- [ ] **Step 3: Verify** — `cargo test -p alephcore --lib thinker::layers::strategy::`. Then `git add src/thinker/layers/strategy.rs && git commit -m "thinker: add StrategyLayer (Stable, prio 70) <strategy> envelope"`.

---

### Task D3: `StrategyPointerLayer` (Dynamic, priority 1756, `<strategy_reminder>`)

**Files:**
- Create `/Volumes/TBU4/Workspace/Aleph/src/thinker/layers/strategy_pointer.rs`
- Test: inline `#[cfg(test)]` (mirrors `execution_plan.rs:83-165`)

**Interfaces:**
- Consumes: `ResolvedContext.strategy_guardrails: Option<String>` (D1)
- Produces: `pub struct StrategyPointerLayer` implementing `PromptLayer` — `stability()=Dynamic`, `priority()=1756`, `paths()=[Basic,Hydration,Soul,Context,Cached]`, `supports_mode = mode != Minimal`. Registered by D4.

- [ ] **Step 1: Write the failing test** — empty-path-first guards plus the guardrail-echo render check, inside the Step-2 file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerInput;
    use crate::thinker::prompt_mode::PromptMode;
    use crate::thinker::security_context::SecurityContext;

    fn ctx_with_guardrails(guardrails: Option<&str>) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.strategy_guardrails = guardrails.map(|s| s.to_string());
        ctx
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        StrategyPointerLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn no_strategy_emits_nothing() {
        let out = render(&ctx_with_guardrails(None));
        assert!(out.is_empty());
    }

    #[test]
    fn empty_strategy_emits_nothing() {
        let out = render(&ctx_with_guardrails(Some("")));
        assert!(out.is_empty());
    }

    #[test]
    fn missing_context_emits_nothing() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        StrategyPointerLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn guardrails_render_inside_tag() {
        let guardrails = "- don't refactor unrelated modules\n- don't add config beyond what's asked";
        let out = render(&ctx_with_guardrails(Some(guardrails)));
        assert!(out.starts_with("<strategy_reminder>\n"));
        assert!(out.contains("don't refactor unrelated modules"));
        assert!(out.trim_end().ends_with("</strategy_reminder>"));
    }

    #[test]
    fn excluded_from_minimal_mode() {
        assert!(!StrategyPointerLayer.supports_mode(PromptMode::Minimal));
        assert!(StrategyPointerLayer.supports_mode(PromptMode::Full));
    }

    #[test]
    fn name_priority_stability() {
        assert_eq!(StrategyPointerLayer.name(), "strategy_pointer");
        assert_eq!(StrategyPointerLayer.priority(), 1756);
        assert_eq!(StrategyPointerLayer.stability(), LayerStability::Dynamic);
        assert!(StrategyPointerLayer.paths().contains(&AssemblyPath::Cached));
    }
}
```

- [ ] **Step 2: Implement** — full file mirroring `standing_goal.rs` (Dynamic 3-guard) but emitting guardrails-only near the read head (§5: tail = guardrails verbatim, no objective, to dodge reminder-blindness vs StandingGoal):

```rust
//! `StrategyPointerLayer` — re-echoes the Strategy's guardrails verbatim as
//! `<strategy_reminder>` at priority 1756 (Dynamic), near the read head.
//!
//! The Stable `StrategyLayer` (70) pins the full plan in the cacheable head,
//! but on a long horizon the head scrolls far from the model's read position.
//! This layer restates **only** the 1-3 concrete guardrails near the prompt
//! tail every turn — the operation drift already fails at — so the concrete
//! anti-distraction constraints stay salient. It deliberately omits the
//! objective: `StandingGoalLayer` (1754) already re-injects that for `/goal`,
//! and three near-identical end-of-prompt reminders breed reminder-blindness.
//!
//! R10-safe: pure scaffolding, guardrails injected verbatim, no judgment.
//! `Dynamic` keeps it out of the cached stable prefix; `None` (no Strategy or
//! no guardrails) emits nothing, leaving the dynamic tail byte-identical.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct StrategyPointerLayer;

impl PromptLayer for StrategyPointerLayer {
    fn name(&self) -> &'static str {
        "strategy_pointer"
    }

    fn priority(&self) -> u32 {
        1756
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn stability(&self) -> LayerStability {
        // The guardrail echo rides the per-turn dynamic suffix so it never
        // invalidates the cached stable prefix (which already holds the full
        // `<strategy>` via StrategyLayer).
        LayerStability::Dynamic
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        // Operational steering, not chrome — drop only from the bare Minimal
        // prompt, matching StandingGoal / ExecutionPlan.
        mode != PromptMode::Minimal
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };
        let Some(guardrails) = ctx.strategy_guardrails.as_deref() else {
            return;
        };
        if guardrails.is_empty() {
            return;
        }
        output.push_str("<strategy_reminder>\n");
        output.push_str(guardrails);
        output.push_str("\n</strategy_reminder>\n\n");
    }
}
```

  Then append the Step-1 `#[cfg(test)]` module.

- [ ] **Step 3: Verify** — `cargo test -p alephcore --lib thinker::layers::strategy_pointer::`. Then `git add src/thinker/layers/strategy_pointer.rs && git commit -m "thinker: add StrategyPointerLayer (Dynamic, prio 1756) <strategy_reminder>"`.

---

### Task D4: Register both layers in the pipeline + bump count asserts

**Files:**
- Modify `/Volumes/TBU4/Workspace/Aleph/src/thinker/layers/mod.rs` (add `mod` + `pub use` near the standing_goal block, `:37-43`)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/thinker/prompt_pipeline.rs` — import block (`:6-17`), `default_layers()` (`:319-365`), priority doc (`:274-317`), `layer_count` assert (`:557`), `dynamic_names.len()` assert (`:930-934`)
- Test: the two existing assert tests in `prompt_pipeline.rs` (`test_default_layers_count`, `dynamic_layers_are_correctly_classified`)

**Interfaces:**
- Consumes: `StrategyLayer` (D2), `StrategyPointerLayer` (D3)
- Produces: both layers live on the default pipeline; `layer_count()==42`, dynamic count `15`. D7's production-path test depends on these being registered.

- [ ] **Step 1: Update the failing asserts first** — bump `test_default_layers_count` (`:557`) `40` → `42`, append a count-rationale comment line:

```rust
        // → 40 (ExtraFilesLayer renders `[prompt.extra_files]`, 2026-06-10)
        // → 42 (StrategyLayer @70 Stable + StrategyPointerLayer @1756 Dynamic
        // weld the StraTA plan into the cacheable head + per-turn tail,
        // 2026-06-18). See `default_layers`.
        assert_eq!(pipeline.layer_count(), 42);
```

  And in `dynamic_layers_are_correctly_classified` (`:929-934`) add the `strategy_pointer` membership assert and bump the count `14` → `15`. `StrategyLayer` is **Stable**, so only `strategy_pointer` is added to the dynamic set:

```rust
        assert!(dynamic_names.contains(&"extra_files"));
        // StrategyPointerLayer echoes the Strategy guardrails near the read
        // head per turn — Dynamic. (StrategyLayer @70 is Stable, not counted.)
        assert!(dynamic_names.contains(&"strategy_pointer"));
        assert_eq!(
            dynamic_names.len(),
            15,
            "Exactly 15 dynamic layers expected"
        );
```

- [ ] **Step 2: Implement registration** —

  In `src/thinker/layers/mod.rs`, after the standing_goal block (`:41-43`), add:

```rust
// --- Strategy layers (StraTA welded plan + per-turn guardrail echo) ---
mod strategy;
mod strategy_pointer;
pub use strategy::StrategyLayer;
pub use strategy_pointer::StrategyPointerLayer;
```

  In `src/thinker/prompt_pipeline.rs`, add both to the `use super::layers::{…}` import block (`:6-17`) — insert `StrategyLayer, StrategyPointerLayer,` keeping the alphabetic-ish grouping (e.g. after `StandingGoalLayer,` on `:15`):

```rust
    StandingGoalLayer, StrategyLayer, StrategyPointerLayer, ThinkingGuidanceLayer,
    ToolRuntimeStateLayer, ToolUsageGrammarLayer, ToolsLayer, VoiceModeLayer,
```

  In `default_layers()` (the `vec![…]`), add both boxes (order is cosmetic — sorted by `priority()` at `:67`). Insert `Box::new(StrategyLayer),` after `Box::new(CuratedMemoryLayer),` (`:323`) and `Box::new(StrategyPointerLayer),` after `Box::new(StandingGoalLayer),` (`:360`):

```rust
            Box::new(CuratedMemoryLayer),
            Box::new(StrategyLayer),
```

  and

```rust
            Box::new(StandingGoalLayer),
            Box::new(StrategyPointerLayer),
            Box::new(ExecutionPlanLayer),
```

  Update the priority doc-comment (`:274-317`): add `70  StrategyLayer` to the Stable zone (after `60  CuratedMemoryLayer`, before `75  ProfileLayer`) and `1756  StrategyPointerLayer` to the Dynamic zone (after `1755  ExecutionPlanLayer`, before `1760  SessionResumeLayer`):

```rust
    ///   60  `CuratedMemoryLayer`
    ///   70  `StrategyLayer`
    ///   75  `ProfileLayer`
```

  and

```rust
    /// 1755  `ExecutionPlanLayer`
    /// 1756  `StrategyPointerLayer`
    /// 1760  `SessionResumeLayer`
```

  (Optional: update the stale `33 default layers` lead-in on `:274` if you touch it — leave it if minimizing diff; the count there is already drifted and not asserted.)

  No change needed to the Compact/Minimal mode tests (`:599-669`): `strategy` (Stable, default `supports_mode=true`) implicitly participates in Compact/Minimal like `curated_memory`; `strategy_pointer` declares `supports_mode = mode != Minimal` like `execution_plan`/`standing_goal`, which are not in those tests' explicit lists, so neither new name needs adding to the Compact `excluded_in_compact` or Minimal `included_in_minimal` arrays — verify they still pass in Step 3.

- [ ] **Step 3: Verify** — `cargo test -p alephcore --lib thinker::prompt_pipeline::`. Then `git add src/thinker/layers/mod.rs src/thinker/prompt_pipeline.rs && git commit -m "thinker: register Strategy layers + bump pipeline count asserts (40->42, dyn 14->15)"`.

---

### Task D5: `active_strategy` fetch (goal_key first, else loop_key) + re-export

**Files:**
- Modify `/Volumes/TBU4/Workspace/Aleph/src/orchestrator/harness_bridge/context_blocks.rs` (add `active_strategy`, mirroring `active_standing_goal` `:34-44`)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/orchestrator/harness_bridge/mod.rs:44-46` (add to the `pub use context_blocks::{…}` re-export)
- Test: inline `#[cfg(test)]` in `context_blocks.rs`

**Interfaces:**
- Consumes: `crate::strategy::{Strategy, StrategyStore, goal_key, loop_key, global}` (Group A frozen contract)
- Produces: `pub async fn active_strategy(session_key: &str) -> Option<crate::strategy::Strategy>` — resolution: try `goal_key(session)` first, else `loop_key(session)`; goal precedence when both exist. Consumed by D6 (`prompt_build`).

- [ ] **Step 1: Write the failing test** — a fail-soft test that does not depend on a live global store. The uninitialized-store path must return `None` (mirrors `active_standing_goal`'s `crate::goal::global()?` early return). Append to `context_blocks.rs`:

```rust
#[cfg(test)]
mod active_strategy_tests {
    use super::*;

    #[tokio::test]
    async fn returns_none_when_store_uninitialized() {
        // No `strategy::init_global` in this test process → `global()` is
        // None → fail-soft to None, leaving the prompt byte-identical.
        let out = active_strategy("session-with-no-store").await;
        assert!(out.is_none());
    }
}
```

- [ ] **Step 2: Implement** — add `active_strategy` after `active_standing_goal` (`:44`). It returns the **struct** (the contract: `prompt_build` renders both fields from it). Goal precedence, then loop, fail-soft like `active_standing_goal`:

```rust
/// Fetch the session's welded Strategy for the prompt weld. Returns the
/// `Strategy` struct (the caller renders both the `<strategy>` body and the
/// guardrail echo from it). Resolution mirrors the StraTA composite key: try
/// `goal_key(session)` first (a `/goal` Strategy takes precedence), else
/// `loop_key(session)` (a `/loop` Strategy). Returns `None` (→ both Strategy
/// layers emit nothing) when the strategy subsystem is uninitialized or no
/// Strategy is stored for either key. Fail-soft on store error. Mirrors
/// `active_standing_goal`.
pub async fn active_strategy(session_key: &str) -> Option<crate::strategy::Strategy> {
    let store = crate::strategy::global()?;
    if let Some(s) = store.get(&crate::strategy::goal_key(session_key)).ok().flatten() {
        return Some(s);
    }
    store.get(&crate::strategy::loop_key(session_key)).ok().flatten()
}
```

  In `src/orchestrator/harness_bridge/mod.rs`, extend the re-export (`:44-46`):

```rust
pub use context_blocks::{
    active_execution_plan, active_standing_goal, active_strategy, compute_runtime_state_blocks,
};
```

- [ ] **Step 3: Verify** — `cargo test -p alephcore --lib harness_bridge::context_blocks::active_strategy_tests` (or `cargo check -p alephcore` if the test name path is hard to scope before Group A lands the `strategy` module). Then `git add src/orchestrator/harness_bridge/context_blocks.rs src/orchestrator/harness_bridge/mod.rs && git commit -m "harness_bridge: add active_strategy fetch (goal_key then loop_key)"`.

> **Cross-group dependency note:** this task consumes `crate::strategy::{Strategy, StrategyStore, goal_key, loop_key, global}` from Group A. The orchestrator must sequence D5 after Group A's `src/strategy/` module exists, or D5 will not compile. If executed before Group A merges, stop at Step 2 (write the code) and defer the cargo verify to the integration point.

---

### Task D6: 3-way `tokio::join!` in `prompt_build` + render both strategy surfaces

**Files:**
- Modify `/Volumes/TBU4/Workspace/Aleph/src/orchestrator/harness_bridge/prompt_build.rs:388-393` (extend the existing 2-way `join!` to 3-way; render both `ResolvedContext` strategy fields)

**Interfaces:**
- Consumes: `active_strategy(&str) -> Option<Strategy>` (D5); `crate::strategy::{render_strategy_summary, render_guardrails_only}` (Group A render.rs); `ResolvedContext.strategy` / `.strategy_guardrails` (D1)
- Produces: populated `resolved_context.strategy` (full `<strategy>` body) + `resolved_context.strategy_guardrails` (guardrail lines) on the hot per-turn path.

- [ ] **Step 1: Identify the change site (no new test — covered end-to-end by D7).** The existing `join!` at `:388-391` polls `active_execution_plan` + `active_standing_goal` and assigns at `:392-393`. The contract's empty-path invariant (`None` ⇒ byte-identical) is already locked by D2/D3 layer tests and D7's production-path test; this is a wiring change with no independently testable surface, so per cargo-frugality there is no separate failing test here — D7 is its regression guard.

- [ ] **Step 2: Implement** — extend the `join!` to 3-way and render both fields. Replace `:388-393`:

```rust
        // The execution-plan, standing-goal, and strategy lookups are
        // independent session-keyed reads (a scratchpad file read, a goal-store
        // read with a wall-clock stamp, and a strategy-store read). Run them
        // concurrently with `tokio::join!` so prompt assembly — on the hot
        // per-turn path — pays the max of the three latencies, not their sum.
        // `join!` polls all on the current task, so there is no spawn cost and
        // no extra `Send` bound; all futures take a shared `&session_key_str`
        // borrow, which co-exist fine.
        let (exec_plan, standing, strategy) = tokio::join!(
            active_execution_plan(&session_key_str),
            active_standing_goal(&session_key_str),
            active_strategy(&session_key_str),
        );
        resolved_context.execution_plan = exec_plan;
        resolved_context.standing_goal = standing;
        // Render the welded Strategy into its two prompt surfaces: the full
        // `<strategy>` body for the Stable `StrategyLayer` (cacheable head) and
        // the guardrail-only echo for the Dynamic `StrategyPointerLayer` (per-
        // turn tail near the read head). Both renders are pure/deterministic
        // (no timestamps). `None` Strategy leaves both fields `None`, so both
        // layers emit nothing and the prompt is byte-identical.
        if let Some(s) = strategy {
            resolved_context.strategy = Some(crate::strategy::render_strategy_summary(&s));
            resolved_context.strategy_guardrails =
                Some(crate::strategy::render_guardrails_only(&s));
        }
```

  Confirm `active_strategy` is in scope: it is re-exported from `harness_bridge/mod.rs` (D5) and `active_execution_plan`/`active_standing_goal` are already called unqualified here, so add `active_strategy` to the same `use` import in `prompt_build.rs` if the siblings are imported (check the file's `use super::context_blocks::{…}` / `use super::{…}` head; add `active_strategy` alongside `active_execution_plan, active_standing_goal`).

- [ ] **Step 3: Verify** — `cargo check -p alephcore` (this site only compiles once Group A's `render_strategy_summary` / `render_guardrails_only` exist; scope to `check` rather than a unit test since the seam has no isolated test). Then `git add src/orchestrator/harness_bridge/prompt_build.rs && git commit -m "harness_bridge: wire active_strategy into 3-way join + render both strategy surfaces"`.

> **Cross-group dependency note:** consumes Group A `render_strategy_summary` / `render_guardrails_only` and D5's `active_strategy`. Sequence after Group A + D5.

---

### Task D7: Production-path integration test (Strategy welds into stable prefix + dynamic tail)

**Files:**
- Modify `/Volumes/TBU4/Workspace/Aleph/src/thinker/prompt_builder/cache.rs` (`#[cfg(test)] mod tests`, after `cached_full_prompt_carries_role_and_citation_standards` at `:187-206`)

**Interfaces:**
- Consumes: `StrategyLayer` + `StrategyPointerLayer` on the default pipeline (D4); `ResolvedContext.strategy` / `.strategy_guardrails` (D1). Catches both the missing-`Cached` vanish bug and wrong-stability in one test (§11).

- [ ] **Step 1: Write the failing test** — mirror `cached_full_prompt_carries_role_and_citation_standards` (`:188-206`) but drive a `ResolvedContext` carrying both strategy surfaces through the production `Cached` path, asserting the body lands in `parts[0]` (Stable cacheable) and the guardrail echo in `parts[1]` (Dynamic). This requires building the prompt with a resolved context; mirror how the `PromptBuilder` is fed a context. Append to the `tests` module in `cache.rs`:

```rust
    #[test]
    fn cached_full_prompt_welds_strategy_into_stable_and_dynamic() {
        // Regression guard mirroring the role/citation vanish test: with a
        // Strategy present, the full `<strategy>` body MUST land in the stable
        // cacheable prefix (StrategyLayer @70, Stable) and the guardrail echo
        // in the dynamic suffix (StrategyPointerLayer @1756, Dynamic). Catches
        // both a missing `Cached` path (silent vanish) and a wrong `stability()`.
        use crate::thinker::context::{ContextAggregator, ResolvedContext};
        use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
        use crate::thinker::security_context::SecurityContext;

        let mut ctx: ResolvedContext = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.strategy = Some(
            "Objective: ship the strategic planner\nApproach: plan-first, adapt as you learn"
                .to_string(),
        );
        ctx.strategy_guardrails =
            Some("- don't refactor unrelated modules\n- don't add speculative config".to_string());

        let builder =
            PromptBuilder::new(PromptConfig::default()).with_resolved_context(ctx);
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);

        // Stable prefix (part 0) carries the full `<strategy>` body.
        assert!(
            parts[0].content.contains("<strategy>"),
            "cached Full prompt must weld the <strategy> body into the stable prefix"
        );
        assert!(parts[0].content.contains("ship the strategic planner"));
        // Dynamic suffix (part 1) carries the guardrail echo.
        assert!(
            parts[1].content.contains("<strategy_reminder>"),
            "cached Full prompt must echo guardrails in the dynamic suffix"
        );
        assert!(parts[1].content.contains("don't refactor unrelated modules"));
        // The `<strategy>` body must NOT leak into the dynamic suffix, and the
        // reminder must NOT leak into the stable prefix.
        assert!(!parts[1].content.contains("<strategy>"));
        assert!(!parts[0].content.contains("<strategy_reminder>"));
    }
```

  > **API check before writing:** confirm `PromptBuilder::with_resolved_context(ResolvedContext) -> Self` is the exact setter used at `prompt_build.rs:401` (`builder = builder.with_resolved_context(resolved_context);`). If the builder setter has a different name/signature, match it exactly. Read `src/thinker/prompt_builder/mod.rs` (or wherever `PromptBuilder` is defined) before finalizing this test.

- [ ] **Step 2: Implement** — no production code in this task; D2/D3/D4/D1 already provide the layers and fields. This task only adds the integration test. If Step 1 reveals the test does not pass, the failure points to a missing `Cached` path or wrong stability in D2/D3 — fix there, not here.

- [ ] **Step 3: Verify** — `cargo test -p alephcore --lib thinker::prompt_builder::cache::tests::cached_full_prompt_welds_strategy_into_stable_and_dynamic`. Then `git add src/thinker/prompt_builder/cache.rs && git commit -m "thinker: production-path test for Strategy weld (stable body + dynamic guardrail echo)"`.

---

**Group D notes for the orchestrator (sequencing + cargo frugality):**
- Internal order: **D1 → D2/D3 (parallel) → D4 → D5 → D6 → D7**. D2/D3 are independent of each other (different files) and depend only on D1's fields.
- D5 and D6 consume Group A's frozen `src/strategy/` (`Strategy`, `StrategyStore`, `goal_key`, `loop_key`, `global`, `render_strategy_summary`, `render_guardrails_only`). They must be sequenced **after Group A merges**; if run earlier, complete code edits but defer the cargo verify to the integration point. D1–D4 + D7 are self-contained within Group D and `thinker` and can land independently.
- Each task carries exactly **one** scoped cargo command at its end (test-first discipline preserved by writing the failing test before the impl, batched verification per the user's hard cargo-frugality preference).
- Asserts bumped exactly as the contract demands: `layer_count` 40→42 (D4), `dynamic_names.len()` 14→15 + `contains("strategy_pointer")` (D4). Only `StrategyPointerLayer` is Dynamic; `StrategyLayer` is Stable and does not touch the dynamic count.


---


## Group E — propagation seams (subagent inline prompt · workflow metadata→handoff)

### Task E1: Thread `strategy` through the subagent SpawnRequest into the inline prompt

**Files:**
- Modify `/Volumes/TBU4/Workspace/Aleph/src/thinker/prompt_builder/mod.rs` (add `strategy` field + `with_strategy` builder + append in `build_system_prompt`)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/agents/runtime.rs` (`AgentRuntimeConfig.strategy` field 49-60; new `AgentRuntime.strategy` field + `with_strategy` builder; pass into `SpawnRequest` at 437-445)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_spawner/mod.rs` (`SpawnRequest.strategy` field 99-119; `PromptBuilder::with_strategy` call at 269)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_tool/mod.rs` (`SubagentTool.strategy` field + `with_strategy` builder)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_tool/spawn.rs` (`build_runtime` applies `with_strategy`; `spawn_background`'s `AgentRuntimeConfig` carries strategy)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_tool/loop_tool.rs` (3 `AgentRuntimeConfig {}` sites carry `strategy`)
- Test `/Volumes/TBU4/Workspace/Aleph/src/thinker/prompt_builder/tests/build_tests.rs` (unit test on `with_strategy`)
- Test `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_spawner/tests.rs` (end-to-end capture test)

**Interfaces:**
- Consumes: `SpawnRequest` struct fields (subagent_spawner/mod.rs:99-119); `AgentRuntimeConfig` (runtime.rs:49-60); `PromptBuilder::new(PromptConfig)` + `build_system_prompt(&[ToolInfo])`.
- Produces: `PromptBuilder::with_strategy(String) -> Self`; `AgentRuntimeConfig.strategy: Option<String>`; `AgentRuntime::with_strategy(String) -> Self`; `SpawnRequest.strategy: Option<&'a str>`; `SubagentTool::with_strategy(String) -> Self`. (Group C/D wire the parent's active rendered `<strategy>` body into `SubagentTool::with_strategy` at inner.rs:785.)

Note on design: the subagent inline prompt builds on the `Basic` path with **no `ResolvedContext`**, so Group D's `StrategyLayer` (which reads `ResolvedContext.strategy`) never fires here. `PromptBuilder::with_strategy` therefore appends the wrapped `<strategy>` block **post-pipeline**, byte-for-byte matching the StrategyLayer wrap (`<strategy>\n{body}\n</strategy>\n\n`). When `strategy` is `None`, nothing is appended → byte-identical to today.

- [ ] **Step 1: Write the failing unit test** for `PromptBuilder::with_strategy` in `/Volumes/TBU4/Workspace/Aleph/src/thinker/prompt_builder/tests/build_tests.rs` (append at the end of the existing `#[cfg(test)] mod` — match the file's existing `use super::super::*;` style; read the file's existing imports first and reuse them):

```rust
#[test]
fn with_strategy_appends_welded_strategy_block() {
    let agent = crate::agents::AgentDef::new("explore", crate::agents::AgentMode::SubAgent);
    let body = "Objective: ship the parser.\nGuardrails:\n- no network calls";

    let builder = PromptBuilder::new(PromptConfig {
        native_tools_enabled: true,
        ..PromptConfig::default()
    })
    .with_agent(agent.clone());
    let without = builder.build_system_prompt(&[]);

    let builder_s = PromptBuilder::new(PromptConfig {
        native_tools_enabled: true,
        ..PromptConfig::default()
    })
    .with_agent(agent)
    .with_strategy(body.to_string());
    let with = builder_s.build_system_prompt(&[]);

    // The welded block is present and wrapped exactly like StrategyLayer.
    assert!(with.contains("<strategy>\n"));
    assert!(with.contains("</strategy>\n"));
    assert!(with.contains("ship the parser."));
    // The strategy block is appended; the body without it is a strict prefix.
    assert!(with.starts_with(&without));
    assert_eq!(
        &with[without.len()..],
        &format!("<strategy>\n{body}\n</strategy>\n\n")
    );
}

#[test]
fn without_strategy_is_byte_identical() {
    let agent = crate::agents::AgentDef::new("explore", crate::agents::AgentMode::SubAgent);
    let a = PromptBuilder::new(PromptConfig {
        native_tools_enabled: true,
        ..PromptConfig::default()
    })
    .with_agent(agent.clone())
    .build_system_prompt(&[]);
    let b = PromptBuilder::new(PromptConfig {
        native_tools_enabled: true,
        ..PromptConfig::default()
    })
    .with_agent(agent)
    .build_system_prompt(&[]);
    assert_eq!(a, b);
}
```

- [ ] **Step 2: Add the `strategy` field + `with_strategy` builder to `PromptBuilder`** in `/Volumes/TBU4/Workspace/Aleph/src/thinker/prompt_builder/mod.rs`.

Add the field to the struct (after the `extra_files` field at line 191):

```rust
    /// Welded strategy `<strategy>` body for the subagent inline prompt seam.
    /// The subagent prompt builds on the `Basic` path with no `ResolvedContext`,
    /// so `StrategyLayer` (which reads `ResolvedContext.strategy`) never fires
    /// here. When set, `build_system_prompt` appends the wrapped block
    /// post-pipeline, byte-for-byte matching the `StrategyLayer` wrap. `None`
    /// leaves the prompt byte-identical to the pre-strategy build.
    strategy: Option<String>,
```

Initialize it to `None` in `PromptBuilder::new` (in the `Self { … }` literal, after `extra_files: None,`):

```rust
            strategy: None,
```

Add the builder (place it right after `with_iteration_cap` ends, before `build_system_prompt` at line 329):

```rust
    /// Attach a welded strategy `<strategy>` body for the subagent inline
    /// prompt. The body is the inner text (no tags); `build_system_prompt`
    /// wraps it in `<strategy> … </strategy>` exactly like `StrategyLayer`.
    /// Threaded in by `subagent_spawner::spawn` from `SpawnRequest.strategy`.
    #[must_use]
    pub fn with_strategy(mut self, strategy: String) -> Self {
        self.strategy = Some(strategy);
        self
    }
```

Append the block at the end of `build_system_prompt`. Replace the final line of that method (currently `self.pipeline.execute_cached(path, &input)` at line 359):

```rust
        maybe_trace_prompt_size(&self.pipeline, path, &input);
        let mut prompt = self.pipeline.execute_cached(path, &input);
        // Subagent strategy weld: appended post-pipeline because the Basic-path
        // inline prompt threads no `ResolvedContext` for `StrategyLayer` to read.
        // Wrap mirrors `StrategyLayer` byte-for-byte: `<strategy>\n{body}\n</strategy>\n\n`.
        if let Some(body) = self.strategy.as_deref() {
            prompt.push_str("<strategy>\n");
            prompt.push_str(body);
            prompt.push_str("\n</strategy>\n\n");
        }
        prompt
```

- [ ] **Step 3: Add `strategy` to `AgentRuntimeConfig` + `AgentRuntime`** in `/Volumes/TBU4/Workspace/Aleph/src/agents/runtime.rs`.

In `AgentRuntimeConfig` (after `timeout_secs` at line 59):

```rust
    /// Welded strategy `<strategy>` body inherited from the parent run.
    /// Threaded into `SpawnRequest.strategy` → the child's inline prompt.
    /// `None` leaves the child prompt byte-identical to the pre-strategy build.
    pub strategy: Option<String>,
```

Add the field to `AgentRuntime` (after `provider_overrides` at line 143):

```rust
    /// Welded strategy `<strategy>` body applied to every spawn's
    /// `AgentRuntimeConfig`. `None` (the `new()` default) keeps the legacy
    /// no-strategy path.
    strategy: Option<String>,
```

Initialize in `AgentRuntime::new` (after `provider_overrides: HashMap::new(),` at line 175):

```rust
            strategy: None,
```

Add the builder (after `with_provider_overrides` ends, near line 188):

```rust
    /// Wire the welded strategy `<strategy>` body inherited by every spawn.
    #[must_use]
    pub fn with_strategy(mut self, strategy: String) -> Self {
        self.strategy = Some(strategy);
        self
    }
```

Thread it into `SpawnRequest` at the `let req = SpawnRequest { … }` literal (line 437-445), adding the field after `isolation`:

```rust
            isolation: config.agent_def.isolation.clone(),
            strategy: config.strategy.as_deref(),
```

- [ ] **Step 4: Add `strategy` to `SpawnRequest` and inject it at mod.rs:269** in `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_spawner/mod.rs`.

Add the field to `SpawnRequest<'a>` (after `isolation` at line 118):

```rust
    /// Welded strategy `<strategy>` body inherited from the parent run.
    /// Injected into the inline `PromptBuilder` so the spawned agent shares
    /// the run-global strategy. `None` keeps the child prompt byte-identical.
    pub strategy: Option<&'a str>,
```

Thread it into the inline prompt build (the `PromptBuilder::new(…)…build_system_prompt(&[])` chain at lines 269-275). Insert `.with_strategy(...)` before `.build_system_prompt(&[])`:

```rust
        let mut builder = PromptBuilder::new(PromptConfig {
            native_tools_enabled: true,
            ..PromptConfig::default()
        })
        .with_agent(req.agent_def.clone())
        .with_chain_context(child_chain.clone());
        if let Some(strategy) = req.strategy {
            builder = builder.with_strategy(strategy.to_string());
        }
        let system_prompt = builder.build_system_prompt(&[]);
```

- [ ] **Step 5: Add `strategy` field + builder to `SubagentTool`** in `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_tool/mod.rs`.

Add the field to the struct (after `guardrails` at line 103):

```rust
    /// Welded strategy `<strategy>` body for the parent run. Threaded into
    /// every child `AgentRuntime` via `build_runtime` so spawned subagents
    /// share the run-global strategy. `None` (the `new()` default) keeps
    /// subagents strategy-free, byte-identical to the pre-strategy build.
    pub(super) strategy: Option<String>,
```

Initialize in `new` (after `guardrails: None,` at line 148):

```rust
            strategy: None,
```

Add the builder (after `with_cancel_token` ends, near line 264):

```rust
    /// Wire the parent run's welded strategy `<strategy>` body so every
    /// spawned subagent's inline prompt carries it. `None` keeps subagents
    /// strategy-free.
    #[must_use]
    pub fn with_strategy(mut self, strategy: String) -> Self {
        self.strategy = Some(strategy);
        self
    }
```

- [ ] **Step 6: Apply the strategy in `build_runtime` and carry it in every `AgentRuntimeConfig`.**

In `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_tool/spawn.rs`, in `build_runtime`, add after the `with_guardrails` block (after line 225, before `runtime`):

```rust
        if let Some(s) = self.strategy.clone() {
            runtime = runtime.with_strategy(s);
        }
        runtime
```

(Replace the existing trailing `runtime` line with the block above so the `if let` precedes it.)

In `spawn_background` (spawn.rs), the `AgentRuntimeConfig { … }` literal at lines 98-104 — add `strategy: None,` after `timeout_secs,` (the strategy is carried by the `AgentRuntime` built via `build_runtime`, so the config's own field stays `None`; the runtime's `with_strategy` is authoritative):

```rust
            let runtime_config = AgentRuntimeConfig {
                agent_def,
                task,
                context_summary,
                model,
                timeout_secs,
                strategy: None,
            };
```

In `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_tool/loop_tool.rs`, add `strategy: None,` to all three `AgentRuntimeConfig { … }` literals (sync batch at 523-529, MoA aggregator at 636-642, foreground at 780-786), each after its `timeout_secs` field. Example for the foreground site:

```rust
            let runtime_config = AgentRuntimeConfig {
                agent_def,
                task: args.task.clone(),
                context_summary: args.context_summary,
                model: args.model,
                timeout_secs: args.timeout_secs,
                strategy: None,
            };
```

Rationale for `strategy: None` at every `AgentRuntimeConfig` site: the run-global strategy is applied once on the `AgentRuntime` (via `build_runtime` → `with_strategy`), and `execute_via_harness` reads `config.strategy`. To make the runtime-level strategy authoritative at those call sites, change `execute_via_harness`'s `SpawnRequest.strategy` source from `config.strategy.as_deref()` to prefer the runtime field. **Correction to Step 3's SpawnRequest line** — use the runtime's strategy as the source of truth (the config field is the public contract surface but the runtime carries the inherited value):

```rust
            isolation: config.agent_def.isolation.clone(),
            strategy: self.strategy.as_deref().or(config.strategy.as_deref()),
```

This keeps the contract's `AgentRuntimeConfig.strategy` honored (an explicit per-config strategy still works) while letting the `SubagentTool::with_strategy` builder set it run-wide without touching call sites beyond a `None` default.

- [ ] **Step 7: Write the end-to-end capture test** in `/Volumes/TBU4/Workspace/Aleph/src/agents/subagent_spawner/tests.rs`. Add a capturing provider + test inside the existing `mod tests` (reuse existing imports: `RequestPayload`, `ProviderResponse`, `StopReason`, `AgentDef`, `AgentMode`, `ChainContext`, `Arc`, `Mutex`, the in-process session service, and `agent_with_allowed`). Mirror `SpawnerBase` construction from an existing spawn test in this file (read one near line 475 to copy the exact `SpawnerBase { … }` field set and session/tool wiring):

```rust
    /// Provider that records the system prompt of the first request, then
    /// returns a terminal text response so the harness stops after one turn.
    struct SystemPromptCapture(Mutex<Option<String>>);

    impl AiProvider for SystemPromptCapture {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            {
                let mut g = self.0.lock().unwrap();
                if g.is_none() {
                    *g = Some(payload.system_prompt.unwrap_or_default().to_string());
                }
            }
            Box::pin(async move { Ok(ProviderResponse::text_only("done".to_string())) })
        }
        fn name(&self) -> &str {
            "capture"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    #[tokio::test]
    async fn spawn_request_strategy_reaches_inline_system_prompt() {
        let provider = Arc::new(SystemPromptCapture(Mutex::new(None)));
        let base = test_base(provider.clone()).await; // helper mirroring the
        // SpawnerBase used by existing spawn tests in this file (session +
        // parent_tools + sandbox + root chain; all optional fields None).
        let agent = agent_with_allowed("explore", vec![]);

        let req = SpawnRequest {
            agent_def: &agent,
            task: "do the thing",
            context_summary: None,
            model: None,
            timeout_secs: 30,
            cancel: CancellationToken::new(),
            isolation: None,
            strategy: Some("Objective: weld it.\nGuardrails:\n- no shortcuts"),
        };
        let _ = spawn(&base, req).await.expect("spawn ok");

        let captured = provider.0.lock().unwrap().clone().expect("captured prompt");
        assert!(captured.contains("<strategy>"));
        assert!(captured.contains("weld it."));
    }

    #[tokio::test]
    async fn spawn_request_without_strategy_omits_block() {
        let provider = Arc::new(SystemPromptCapture(Mutex::new(None)));
        let base = test_base(provider.clone()).await;
        let agent = agent_with_allowed("explore", vec![]);

        let req = SpawnRequest {
            agent_def: &agent,
            task: "do the thing",
            context_summary: None,
            model: None,
            timeout_secs: 30,
            cancel: CancellationToken::new(),
            isolation: None,
            strategy: None,
        };
        let _ = spawn(&base, req).await.expect("spawn ok");

        let captured = provider.0.lock().unwrap().clone().expect("captured prompt");
        assert!(!captured.contains("<strategy>"));
    }
```

> Implementation note for Step 7: if the existing tests build `SpawnerBase` inline rather than via a `test_base` helper, copy that exact inline construction into both tests instead of introducing `test_base` — do not invent a helper that does not match the file's established pattern. Read the `SpawnerBase { … }` literal from the test near line 475 and reproduce its fields verbatim (every optional field is `None`, session is `InProcessActorSessionService`, parent_tools is the file's always-succeed `ToolService`, sandbox is the file's mock).

- [ ] **Step 8 (last step — Verify + commit):** Run the one scoped check for this task, then commit:

```
cargo test -p alephcore --lib -- prompt_builder::tests::build_tests::with_strategy subagent_spawner::tests::spawn_request
git add src/thinker/prompt_builder/mod.rs src/thinker/prompt_builder/tests/build_tests.rs src/agents/runtime.rs src/agents/subagent_spawner/mod.rs src/agents/subagent_spawner/tests.rs src/agents/subagent_tool/mod.rs src/agents/subagent_tool/spawn.rs src/agents/subagent_tool/loop_tool.rs
git commit -m "subagent: thread welded strategy through SpawnRequest into inline prompt"
```

---

### Task E2: Stamp `WORKFLOW_STRATEGY_KEY` into CoordTask metadata and render `## Global Strategy` in handoff

**Files:**
- Modify `/Volumes/TBU4/Workspace/Aleph/src/workflow/compile.rs` (add `WORKFLOW_STRATEGY_KEY` const near 41; add `strategy: Option<&Strategy>` param to `materialize` at 102-109; stamp into agent-step metadata at 171-196; update test call sites)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/workflow/mod.rs` (re-export `WORKFLOW_STRATEGY_KEY` in the `pub use` at 30-31)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/workflow_tool.rs` (pass `None` at the `materialize(...)` call, line 569-577)
- Modify `/Volumes/TBU4/Workspace/Aleph/src/teams/dispatcher/handoff.rs` (render `## Global Strategy` after the `## Task` block, ~line 179)
- Test `/Volumes/TBU4/Workspace/Aleph/src/teams/dispatcher/handoff.rs` (handoff render test, in the existing `mod tests`)
- Test `/Volumes/TBU4/Workspace/Aleph/src/workflow/compile.rs` (materialize stamp test, in the existing `mod tests`)

**Interfaces:**
- Consumes: `crate::strategy::Strategy` + `crate::strategy::render_workflow_global_frame(&Strategy) -> String` (Group A); `WORKFLOW_MODEL_KEY`/`WORKFLOW_RUN_ID_KEY` (compile.rs:35,41); `build_handoff_context(coord_store, team_store, inbox, task)` (handoff.rs:164); `task.metadata` (`serde_json::Value`).
- Produces: `pub const WORKFLOW_STRATEGY_KEY: &str = "workflow_strategy"`; `materialize(def, input, team_id, store, clarify_ctx, models, strategy: Option<&Strategy>)`. (Group C passes the planned `Strategy` here and writes it to `StrategyStore` under `workflow_key(mat.run_id)`; the dispatcher needs no change — `build_handoff_context` reads the per-task metadata stamp directly, so concurrent DAG nodes never share a store.)

Design: weld **GLOBAL-FRAME ONLY** (objective + cross-cutting guardrails, no phases) via `render_workflow_global_frame`. The section is labeled `## Global Strategy (context — your specific task is below)` and placed **after** the `## Task` block so the per-node task description (already assembled from `step.prompt` into `task.description`) stays authoritative — it is read first and dominates.

- [ ] **Step 1: Write the failing handoff render test** in `/Volumes/TBU4/Workspace/Aleph/src/teams/dispatcher/handoff.rs` `mod tests` (reuse the existing `coord_store`, `team_store`, `plain_task` helpers; add `WORKFLOW_STRATEGY_KEY` to the test imports from `crate::workflow`):

```rust
    #[tokio::test]
    async fn global_strategy_section_renders_after_task_when_stamped() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let mut nt = plain_task("Implement the parser");
        nt.description = "Write the recursive-descent parser for the grammar.".into();
        nt.metadata = serde_json::json!({
            crate::workflow::WORKFLOW_STRATEGY_KEY:
                "Objective: ship a correct parser.\nGuardrails:\n- no panics on malformed input",
        });
        let task = cs.create_task(nt).await.unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, &task).await;

        // Labeled global-frame block is present...
        assert!(ctx.contains("## Global Strategy (context — your specific task is below)"));
        assert!(ctx.contains("ship a correct parser."));
        // ...and it comes AFTER the ## Task block (task stays authoritative).
        let task_pos = ctx.find("## Task").unwrap();
        let strat_pos = ctx.find("## Global Strategy").unwrap();
        assert!(task_pos < strat_pos, "strategy must follow the task block");
        // The per-node description is present and precedes the strategy.
        let desc_pos = ctx.find("recursive-descent parser").unwrap();
        assert!(desc_pos < strat_pos);
    }

    #[tokio::test]
    async fn no_global_strategy_section_when_metadata_absent() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let task = cs.create_task(plain_task("Plain task")).await.unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, &task).await;
        // Byte-identical to the legacy envelope: no global-strategy heading.
        assert!(!ctx.contains("## Global Strategy"));
    }
```

- [ ] **Step 2: Add the const, the `materialize` param, and the metadata stamp** in `/Volumes/TBU4/Workspace/Aleph/src/workflow/compile.rs`.

Add the const after `WORKFLOW_MODEL_KEY` (after line 41):

```rust
/// Metadata key carrying the run-global welded strategy frame on every
/// materialised **agent** step. Stamped once per run (beside [`WORKFLOW_RUN_ID_KEY`])
/// from the planned [`Strategy`](crate::strategy::Strategy) via
/// [`render_workflow_global_frame`](crate::strategy::render_workflow_global_frame);
/// `build_handoff_context` renders it as a `## Global Strategy` section after the
/// task block. Absent when no strategy was planned (byte-identical legacy rows).
/// Clarify steps run no agent, so they are never stamped.
pub const WORKFLOW_STRATEGY_KEY: &str = "workflow_strategy";
```

Add the import for `Strategy` near the top imports (after line 26):

```rust
use crate::strategy::{render_workflow_global_frame, Strategy};
```

Add the `strategy` parameter to `materialize` (after `models` at line 108):

```rust
    models: Option<&std::collections::HashMap<String, String>>,
    strategy: Option<&Strategy>,
) -> Result<MaterializedWorkflow> {
```

Pre-render the frame once, beside `run_id` (after line 115 where `run_id` is minted):

```rust
    // Run-wide welded strategy frame, rendered once and stamped onto every
    // agent step's metadata. `None` (no planned strategy) leaves rows
    // byte-identical to the legacy materialisation.
    let strategy_frame: Option<String> = strategy.map(render_workflow_global_frame);
```

Stamp it in the agent-step `else` branch, right after the model-override stamp (after line 196, before the `(step.agent.clone(), meta)` tuple at line 197):

```rust
            // Run-global strategy frame: the same welded objective + cross-cutting
            // guardrails on every agent step (the DAG itself is the phase
            // structure, so no phase list). Absent when no strategy was planned.
            if let Some(frame) = strategy_frame.as_deref() {
                if let Some(obj) = meta.as_object_mut() {
                    obj.insert(WORKFLOW_STRATEGY_KEY.to_string(), json!(frame));
                }
            }
            (step.agent.clone(), meta)
```

- [ ] **Step 3: Update all `materialize` call sites to pass the new arg.**

Production caller in `/Volumes/TBU4/Workspace/Aleph/src/builtin_tools/workflow_tool.rs` (line 569-577) — append `None` after the `models` arg (Group C later replaces this `None` with the planned `Some(&strategy)`):

```rust
                let mat = workflow::materialize(
                    &def,
                    &input,
                    &team_id,
                    self.coord_store.as_ref(),
                    clarify_ctx.as_ref(),
                    (!models.is_empty()).then_some(&models),
                    None,
                )
                .await?;
```

Every test call site in `compile.rs` `mod tests` (lines 299, 308-block, 336, 351, 360, 401, 415, 433, 464, 490, and any others) — append `, None` as the final argument. For the multi-line `materialize(` at ~308 and ~464, add `None,` on its own line before the closing `)`. (Mechanical: each existing `materialize(…, None)` / `materialize(…, Some(&ctx), None)` gains one more trailing `None`.)

Re-export the const in `/Volumes/TBU4/Workspace/Aleph/src/workflow/mod.rs` — add `WORKFLOW_STRATEGY_KEY` to the existing `pub use` list (lines 30-31):

```rust
    materialize, workflow_model_override, MaterializedWorkflow, WORKFLOW_MODEL_KEY,
    WORKFLOW_NAME_KEY, WORKFLOW_RUN_ID_KEY, WORKFLOW_STEP_KEY, WORKFLOW_STRATEGY_KEY,
```

(Preserve the existing trailing items on the next line of that `pub use` block; only add the new identifier in alpha-adjacent order.)

- [ ] **Step 4: Write the failing materialize stamp test** in `/Volumes/TBU4/Workspace/Aleph/src/workflow/compile.rs` `mod tests` (reuse `setup_store`, `linear_def`, `clarify_step`; build a `Strategy` inline):

```rust
    #[tokio::test]
    async fn materialize_stamps_strategy_frame_on_agent_steps_only() {
        let store = setup_store().await;
        let strategy = crate::strategy::Strategy {
            objective: "ship the pipeline".into(),
            approach: "incremental".into(),
            phases: vec!["phase a".into(), "phase b".into()],
            guardrails: vec!["no network in tests".into()],
            success_criteria: "all green".into(),
            goal_id: None,
        };
        let mut def = linear_def();
        def.steps.push(clarify_step("ask", "which mode?", &["A", "B"], &["gather"]));

        let mat = materialize(&def, "x", "team-1", &store, None, None, Some(&strategy))
            .await
            .unwrap();

        let mut saw_agent_stamp = false;
        let mut saw_clarify_stamp = false;
        for id in &mat.task_ids {
            let task = store.get_task(id).await.unwrap().unwrap();
            let stamped = task
                .metadata
                .get(WORKFLOW_STRATEGY_KEY)
                .and_then(|v| v.as_str());
            if task.owner.as_deref() == Some(CLARIFY_OWNER) {
                saw_clarify_stamp = stamped.is_some();
            } else if let Some(frame) = stamped {
                saw_agent_stamp = true;
                // Global frame = objective + guardrails, NO phase list.
                assert!(frame.contains("ship the pipeline"));
                assert!(!frame.contains("phase a"));
            }
        }
        assert!(saw_agent_stamp, "agent steps must carry the strategy frame");
        assert!(!saw_clarify_stamp, "clarify steps must NOT be stamped");
    }

    #[tokio::test]
    async fn materialize_without_strategy_is_byte_identical() {
        let store = setup_store().await;
        let mat = materialize(&linear_def(), "x", "team-1", &store, None, None, None)
            .await
            .unwrap();
        for id in &mat.task_ids {
            let task = store.get_task(id).await.unwrap().unwrap();
            assert!(task.metadata.get(WORKFLOW_STRATEGY_KEY).is_none());
        }
    }
```

> Note: confirm the `CoordTaskStore` read method name when reading back (`get_task` is used elsewhere in this repo; if the store exposes a different accessor in this test module, mirror the one the sibling `materialize_*` tests already use — read lines 296-340 of compile.rs first and copy their read pattern). If sibling tests don't read tasks back, they fetch via the store; use the same `store.get_task(id)` the dispatcher path uses.

- [ ] **Step 5: Render the `## Global Strategy` section after the `## Task` block** in `/Volumes/TBU4/Workspace/Aleph/src/teams/dispatcher/handoff.rs`.

Insert immediately after the Task block (after line 179, `out.push('\n');`, before the acceptance-criteria section at line 181):

```rust
    // --- Global Strategy (workflow run-global frame) ---
    // Stamped by `workflow::materialize` under `WORKFLOW_STRATEGY_KEY`: the
    // run-global objective + cross-cutting guardrails (no phases — the DAG is
    // the phase structure). Placed AFTER the task so the per-node task
    // description stays the authoritative local instruction; this is context.
    // Absent for plain team tasks and pre-strategy runs (byte-identical).
    if let Some(frame) = task
        .metadata
        .get(crate::workflow::WORKFLOW_STRATEGY_KEY)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str("\n## Global Strategy (context — your specific task is below)\n");
        out.push_str(&truncate_utf8(frame, MAX_SECTION_BYTES));
        out.push('\n');
    }
```

- [ ] **Step 6 (last step — Verify + commit):** Run the one scoped check for this task, then commit:

```
cargo test -p alephcore --lib -- workflow::compile::tests::materialize_stamps_strategy workflow::compile::tests::materialize_without_strategy teams::dispatcher::handoff::tests::global_strategy teams::dispatcher::handoff::tests::no_global_strategy
git add src/workflow/compile.rs src/workflow/mod.rs src/builtin_tools/workflow_tool.rs src/teams/dispatcher/handoff.rs
git commit -m "workflow: stamp run-global strategy frame into CoordTask metadata and render in handoff"
```

---

**Cross-group dependency notes (for the orchestrator):**
- E1 and E2 both consume `crate::strategy::*` (Group A): E2 hard-requires `Strategy` + `render_workflow_global_frame` to compile; sequence E2 after Group A's `src/strategy/` lands. E1 does **not** depend on Group A/D to compile or pass its tests (the `<strategy>` weld is self-contained in `PromptBuilder`); only the *production wiring* of the parent's active strategy into `SubagentTool::with_strategy` at `inner.rs:785` (and into `workflow_tool.rs:569`'s `materialize(..., Some(&strategy))`) is deferred to Group C, which reads `crate::strategy::active_strategy` / runs the planner. Those producer-side hookups are explicitly **out of Group E's scope** per the seam ownership in the contract.
- The `materialize` 7th-parameter addition is the only signature change with call-site fan-out (1 production + ~10 tests, all mechanical `None` appends), handled entirely within E2 Step 3.


---


## Group F — `strategy` tool (revise/show) + lifecycle clears + objective-change invalidation

### Task F1: `strategy` tool (revise/show, dumb-write) — `src/builtin_tools/strategy_manage.rs`

**Files:**
- Create: `src/builtin_tools/strategy_manage.rs`
- Modify: `src/builtin_tools/mod.rs` (`pub mod strategy_manage;` + `pub use`)
- Test: `src/builtin_tools/strategy_manage.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (Group A contract): `crate::strategy::{Strategy, StrategyStore, goal_key, loop_key}`; `Strategy::is_empty()`; `StrategyStore::{put, get}`; `crate::tools::AlephTool`; mirrors `LoopTool` shape (`registry`/`session_key` → here `store`/`session_key`).
- Produces (later tasks rely on): `StrategyTool`, `StrategyAction { Revise, Show }`, `StrategyArgs`, `StrategyOutput`, `StrategyTool::{new, with_session_key_handle, run}`, `<StrategyTool as AlephTool>::{NAME="strategy", DESCRIPTION}`.

Notes carried from the frozen contract / spec §8:
- **DUMB WRITE ONLY:** `revise` validates `reason` non-empty + rejects `new_strategy.is_empty()`, then `store.put`. **No** legitimacy logic, no counters, no similarity scoring, no accept/reject classifier.
- Key resolution mirrors `active_strategy`: try `goal_key(session)` if a goal-keyed strategy exists, else `loop_key(session)`; goal precedence when both exist. `revise` writes back to whichever key it resolved (so it overwrites the in-force strategy without clobbering the other flow).
- DESCRIPTION carries the high-friction discourse (only-on-genuine-environment-shock, tactical-to-scratchpad).

- [ ] **Step 1: Write the failing test** — append this `#[cfg(test)] mod tests` block (will fail: file/type don't exist yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{goal_key, loop_key, Strategy, StrategyStore};
    use crate::sync_primitives::Arc;
    use tokio::sync::RwLock;

    fn concrete_strategy(objective: &str) -> Strategy {
        Strategy {
            objective: objective.to_string(),
            approach: "incremental, verify each step".to_string(),
            phases: vec!["understand".to_string(), "implement".to_string()],
            guardrails: vec!["do not refactor unrelated modules".to_string()],
            success_criteria: "cargo test green".to_string(),
            goal_id: None,
        }
    }

    fn tool_with_session(session: &str) -> (StrategyTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(StrategyStore::open(&dir.path().join("s.db")).unwrap());
        let handle = Arc::new(RwLock::new(session.to_string()));
        (
            StrategyTool::new(store).with_session_key_handle(Some(handle)),
            dir,
        )
    }

    #[tokio::test]
    async fn revise_rejects_empty_reason() {
        let (tool, _d) = tool_with_session("sess-empty-reason");
        let out = tool
            .run(StrategyArgs {
                action: StrategyAction::Revise,
                reason: Some("   ".to_string()),
                new_strategy: Some(concrete_strategy("ship the feature")),
            })
            .await;
        assert!(out.is_err(), "empty/whitespace reason must be rejected");
    }

    #[tokio::test]
    async fn revise_rejects_empty_guardrails() {
        let (tool, _d) = tool_with_session("sess-empty-guards");
        let mut s = concrete_strategy("ship the feature");
        s.guardrails = vec!["   ".to_string()]; // no concrete guardrail => is_empty() true
        let out = tool
            .run(StrategyArgs {
                action: StrategyAction::Revise,
                reason: Some("environment changed".to_string()),
                new_strategy: Some(s),
            })
            .await;
        assert!(out.is_err(), "a strategy with no concrete guardrail must be rejected");
    }

    #[tokio::test]
    async fn revise_overwrites_in_force_strategy() {
        let (tool, _d) = tool_with_session("sess-overwrite");
        // Seed a goal-keyed strategy directly in the store.
        let store = tool.store.clone();
        store
            .put(&goal_key("sess-overwrite"), &concrete_strategy("old objective"))
            .unwrap();
        let mut revised = concrete_strategy("new objective after shock");
        revised.approach = "pivot to the new approach".to_string();
        tool.run(StrategyArgs {
            action: StrategyAction::Revise,
            reason: Some("the API we relied on was removed".to_string()),
            new_strategy: Some(revised.clone()),
        })
        .await
        .unwrap();
        // The goal-keyed strategy is overwritten (revise resolves to it via precedence).
        let stored = store.get(&goal_key("sess-overwrite")).unwrap().unwrap();
        assert_eq!(stored.objective, "new objective after shock");
        assert_eq!(stored.approach, "pivot to the new approach");
    }

    #[tokio::test]
    async fn revise_writes_loop_key_when_only_loop_strategy_exists() {
        let (tool, _d) = tool_with_session("sess-loop-only");
        let store = tool.store.clone();
        store
            .put(&loop_key("sess-loop-only"), &concrete_strategy("loop objective"))
            .unwrap();
        let mut revised = concrete_strategy("loop objective revised");
        revised.guardrails = vec!["stay on the watch target".to_string()];
        tool.run(StrategyArgs {
            action: StrategyAction::Revise,
            reason: Some("the watch target moved".to_string()),
            new_strategy: Some(revised),
        })
        .await
        .unwrap();
        // No goal-keyed strategy exists -> revise falls back to the loop key.
        assert!(store.get(&goal_key("sess-loop-only")).unwrap().is_none());
        let stored = store.get(&loop_key("sess-loop-only")).unwrap().unwrap();
        assert_eq!(stored.objective, "loop objective revised");
    }

    #[tokio::test]
    async fn show_returns_current_strategy() {
        let (tool, _d) = tool_with_session("sess-show");
        let store = tool.store.clone();
        store
            .put(&goal_key("sess-show"), &concrete_strategy("show me"))
            .unwrap();
        let out = tool
            .run(StrategyArgs {
                action: StrategyAction::Show,
                reason: None,
                new_strategy: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.message.contains("show me"), "got: {}", out.message);
    }

    #[tokio::test]
    async fn show_with_no_strategy_is_graceful() {
        let (tool, _d) = tool_with_session("sess-none");
        let out = tool
            .run(StrategyArgs {
                action: StrategyAction::Show,
                reason: None,
                new_strategy: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(
            out.message.to_lowercase().contains("no strategy"),
            "got: {}",
            out.message
        );
    }
}
```

- [ ] **Step 2: Implement** — write the tool above the test module (mirrors `LoopTool`: `#[derive(Clone)]`, `session_key: Option<Arc<RwLock<String>>>`, `#[cfg(test)] test_session`, public `run` for direct test calls, `AlephTool` impl with session-binding guard in `call`). Full file:

```rust
//! `strategy` builtin tool (R8): the LLM revises or reads the welded Strategy
//! for a long task. Sibling of `goal`/`loop` — but unlike them it does NOT
//! create or schedule anything; the Strategy is minted by the planner node
//! above the loop. This tool is the rare escape-hatch: a DUMB schema-validated
//! overwrite (`revise`) and a read (`show`).
//!
//! "High-friction" lives entirely in the DESCRIPTION discourse (R9: intelligence
//! in the prompt), NEVER as a Rust gate / counter / similarity score / classifier
//! (spec §8 non-goal: this tool must not evaluate the legitimacy of a revision).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::error::{AlephError, Result};
use crate::strategy::{goal_key, loop_key, Strategy, StrategyStore};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StrategyAction {
    /// Overwrite the in-force Strategy for this task. Reserve for genuine
    /// environment shock that invalidates the high-level approach.
    Revise,
    /// Read the current Strategy for this task.
    Show,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StrategyArgs {
    pub action: StrategyAction,
    /// Why the revision is warranted — required (non-empty) for `revise`.
    pub reason: Option<String>,
    /// The full replacement Strategy — required for `revise`. Must carry at
    /// least one concrete guardrail (a blank-guardrail object is rejected).
    pub new_strategy: Option<Strategy>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategyOutput {
    pub success: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct StrategyTool {
    store: Arc<StrategyStore>,
    session_key: Option<Arc<RwLock<String>>>,
    #[cfg(test)]
    test_session: Option<String>,
}

impl StrategyTool {
    #[must_use]
    pub fn new(store: Arc<StrategyStore>) -> Self {
        Self {
            store,
            session_key: None,
            #[cfg(test)]
            test_session: None,
        }
    }

    #[must_use]
    pub fn with_session_key_handle(mut self, handle: Option<Arc<RwLock<String>>>) -> Self {
        self.session_key = handle;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_session_for_test(mut self, sess: &str) -> Self {
        self.test_session = Some(sess.to_string());
        self
    }

    async fn session(&self) -> String {
        #[cfg(test)]
        if let Some(s) = &self.test_session {
            return s.clone();
        }
        match &self.session_key {
            Some(h) => h.read().await.clone(),
            None => String::new(),
        }
    }

    /// Resolve which composite key holds the in-force Strategy for this session.
    /// Goal precedence (mirrors `active_strategy`): a goal-keyed Strategy wins
    /// over a co-existing loop-keyed one. Returns the key to read/overwrite, or
    /// `None` if neither exists.
    fn resolve_key(&self, session: &str) -> std::result::Result<Option<String>, String> {
        let gk = goal_key(session);
        if self.store.get(&gk).map_err(|e| e.to_string())?.is_some() {
            return Ok(Some(gk));
        }
        let lk = loop_key(session);
        if self.store.get(&lk).map_err(|e| e.to_string())?.is_some() {
            return Ok(Some(lk));
        }
        Ok(None)
    }

    /// Core dispatch — public so tests call it directly without the trait.
    pub async fn run(&self, args: StrategyArgs) -> std::result::Result<StrategyOutput, String> {
        let session = self.session().await;
        info!(session = %session, action = ?args.action, "strategy operation");
        match args.action {
            StrategyAction::Revise => self.revise(&session, args),
            StrategyAction::Show => self.show(&session),
        }
    }

    fn revise(
        &self,
        session: &str,
        args: StrategyArgs,
    ) -> std::result::Result<StrategyOutput, String> {
        // DUMB WRITE: schema validation only.
        let reason = args
            .reason
            .filter(|r| !r.trim().is_empty())
            .ok_or_else(|| "revise requires a non-empty reason".to_string())?;
        let new_strategy = args
            .new_strategy
            .ok_or_else(|| "revise requires a new_strategy".to_string())?;
        // Reject a non-strategy (no concrete guardrail) — mirrors the planner's
        // self-gate so the welded prefix never carries noise.
        if new_strategy.is_empty() {
            return Err(
                "new_strategy must carry at least one concrete guardrail".to_string(),
            );
        }
        // Overwrite the in-force Strategy. If none exists yet (a revise before
        // the planner ran), default to the goal key — the dominant flow.
        let key = self
            .resolve_key(session)?
            .unwrap_or_else(|| goal_key(session));
        self.store
            .put(&key, &new_strategy)
            .map_err(|e| e.to_string())?;
        info!(session = %session, reason = %reason, "strategy revised");
        Ok(StrategyOutput {
            success: true,
            message: "Strategy revised. The new high-level plan is welded into \
                 every following turn of this task."
                .to_string(),
        })
    }

    fn show(&self, session: &str) -> std::result::Result<StrategyOutput, String> {
        let Some(key) = self.resolve_key(session)? else {
            return Ok(StrategyOutput {
                success: false,
                message: "No strategy set for this task.".to_string(),
            });
        };
        match self.store.get(&key).map_err(|e| e.to_string())? {
            Some(s) => Ok(StrategyOutput {
                success: true,
                message: render_for_show(&s),
            }),
            None => Ok(StrategyOutput {
                success: false,
                message: "No strategy set for this task.".to_string(),
            }),
        }
    }
}

/// Human-readable single-object dump for `show`. Deterministic — no timestamps,
/// no HashMap iteration (fields are `Vec`/`String`).
fn render_for_show(s: &Strategy) -> String {
    let mut out = format!(
        "objective: {}\napproach: {}",
        s.objective, s.approach
    );
    if !s.phases.is_empty() {
        out.push_str(&format!("\nphases: {}", s.phases.join(" -> ")));
    }
    if !s.guardrails.is_empty() {
        out.push_str("\nguardrails:");
        for g in &s.guardrails {
            out.push_str(&format!("\n  - {g}"));
        }
    }
    if !s.success_criteria.is_empty() {
        out.push_str(&format!("\nsuccess_criteria: {}", s.success_criteria));
    }
    out
}

#[async_trait]
impl AlephTool for StrategyTool {
    const NAME: &'static str = "strategy";
    const DESCRIPTION: &'static str =
        "Read or REVISE the high-level Strategy welded into this long task. A \
         Strategy is the map you drew before starting — objective, approach, \
         coarse phases, and a small set of concrete guardrails — and it rides \
         in your system prompt every turn so you do not drift. \
         action='show' reads it. \
         action='revise' OVERWRITES it (reason + new_strategy required) and is \
         HIGH-FRICTION BY DESIGN: default to HOLDING the Strategy. Revise ONLY \
         on a genuine ENVIRONMENT SHOCK that invalidates the high-level \
         approach itself (the chosen tool/library is gone, the objective was \
         misread, a hard external constraint appeared). Do NOT revise for \
         ordinary tactical changes — a different file to edit, a reordered \
         step, a new sub-task — those belong in your scratchpad, not here. A \
         revise costs a prompt-cache miss and resets the map the whole task \
         leans on; keep it rare. The new_strategy must carry at least one \
         concrete, observable guardrail or it is rejected.";

    type Args = StrategyArgs;
    type Output = StrategyOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "strategy(action='show')".into(),
            "strategy(action='revise', reason='the auth library we planned around was removed upstream', new_strategy={objective:'...', approach:'...', phases:['...'], guardrails:['...'], success_criteria:'...'})".into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session = self.session().await;
        if session.is_empty() {
            return Err(AlephError::tool(
                "strategy tool has no active session binding".to_string(),
            ));
        }
        self.run(args).await.map_err(AlephError::tool)
    }
}
```

  Then wire the module into `src/builtin_tools/mod.rs` next to `loop_manage` (line 60) and its `pub use` (line 179):

```rust
pub mod strategy_manage;
```
```rust
pub use strategy_manage::{StrategyAction, StrategyArgs, StrategyOutput, StrategyTool};
```

- [ ] **Step 3 (last): Verify** — `cargo test -p alephcore --lib builtin_tools::strategy_manage::` then `git add src/builtin_tools/strategy_manage.rs src/builtin_tools/mod.rs && git commit -m "strategy: add strategy builtin tool (revise/show, dumb-write)"`.

---

### Task F2: register `strategy` tool so it is discoverable

**Files:**
- Modify: `src/executor/builtin_registry/registry/struct_def.rs` (add `strategy_tool` field)
- Modify: `src/executor/builtin_registry/builder/constructor/mod.rs` (construct + bind session handle + assign field)
- Modify: `src/executor/builtin_registry/registry/tool_registry_impl.rs` (dispatch arm)
- Modify: `src/executor/builtin_registry/builder/core_tools.rs` (schema/description registration)
- Modify: `src/executor/builtin_registry/groups.rs` (add `"strategy"` to the goal/loop category)
- Modify: `src/executor/builtin_registry/definitions.rs` (`make_tool` factory fallback arm)
- Test: `src/executor/builtin_registry/builder/core_tools.rs` is exercised by existing registry tests; add a focused name-presence test in `definitions.rs` tests.

**Interfaces:**
- Consumes: `StrategyTool`, `StrategyArgs`, `<StrategyTool as AlephTool>::DESCRIPTION`, `StrategyTool::{new, with_session_key_handle}` (Task F1); `crate::strategy::{StrategyStore, init_global, global}` (Group A contract); existing `memory_session_key_handle` (constructor) + `schema::<…>()` helper (core_tools).
- Produces: the `strategy` tool is resolvable by name in the registry (`call_json` dispatch) and listed for the LLM (core_tools/groups), so later lifecycle tasks and the planner have a live, session-bound tool.

The strategy store is process-global (mirror goal). Construct/open it in `constructor/mod.rs` right after the goal store block (lines 253-264) and `init_global` it, so `make_tool` fallback and lifecycle clears can reach it via `crate::strategy::global()`.

- [ ] **Step 1: Write the failing test** — in the `#[cfg(test)] mod tests` of `definitions.rs` (near the existing `test_all_tools_defined`), add a guard that the dynamic factory knows the name (will fail until the `make_tool` arm + groups entry exist):

```rust
    #[test]
    fn strategy_tool_is_listed_in_a_group() {
        // The `strategy` builtin must be discoverable via the category groups
        // (same surface as goal/loop), or the LLM never sees it.
        let listed = crate::executor::builtin_registry::groups::TOOL_CATEGORIES
            .iter()
            .any(|cat| cat.tools.contains(&"strategy"));
        assert!(listed, "strategy tool must appear in a tool category group");
    }
```

  (If `TOOL_CATEGORIES` has a different exact name/path, match the existing `groups.rs` const — verify with the symbol already used by group tests.)

- [ ] **Step 2: Implement** — six surgical edits mirroring goal/loop exactly:

  1. `struct_def.rs` — add the field after `loop_tool` (line 111):
```rust
    pub(crate) strategy_tool: crate::builtin_tools::StrategyTool,
```

  2. `constructor/mod.rs` — after the goal-store block (after line 264), open + globalize the strategy store and build the tool:
```rust
        // Strategy store: session-keyed SQLite DB under the data dir (mirrors
        // the goal store). Globalized so the planner node, the harness bridge,
        // and the lifecycle clears in execute.rs share one store.
        let strategy_store = Arc::new(
            crate::strategy::StrategyStore::open(
                &crate::utils::paths::get_data_dir()
                    .map_err(|e| AlephError::other(format!("strategy store data dir: {e}")))?
                    .join("strategy.db"),
            )
            .map_err(|e| AlephError::other(format!("strategy store open: {e}")))?,
        );
        crate::strategy::init_global(strategy_store.clone());
        let strategy_tool = crate::builtin_tools::StrategyTool::new(strategy_store);
```
  and in the struct literal, after `loop_tool` (line 725):
```rust
            strategy_tool: strategy_tool
                .with_session_key_handle(memory_session_key_handle.clone()),
```

  3. `tool_registry_impl.rs` — after the `"loop"` arm (line 267):
```rust
            "strategy" => Box::pin(async move { self.strategy_tool.call_json(arguments).await }),
```

  4. `core_tools.rs` — after the `"loop"` reg (line 200):
```rust
        reg(
            tools,
            "strategy",
            crate::builtin_tools::StrategyTool::DESCRIPTION,
            schema::<crate::builtin_tools::strategy_manage::StrategyArgs>("strategy"),
        );
```

  5. `groups.rs` — add `"strategy"` after `"loop"` (line 112):
```rust
            "loop",
            "strategy",
```

  6. `definitions.rs` `make_tool` factory — after the `"loop"` arm (line 1062), so a standalone (non-constructor) build still resolves it from the global store:
```rust
        // Strategy tool — backed by the process-global StrategyStore
        // (init_global at boot). None before boot, same as goal/loop.
        "strategy" => crate::strategy::global().map(|store| {
            Box::new(crate::builtin_tools::StrategyTool::new(store)) as Box<dyn AlephToolDyn>
        }),
```

- [ ] **Step 3 (last): Verify** — `cargo check -p alephcore --lib` then `cargo test -p alephcore --lib builtin_registry::definitions::tests::strategy_tool_is_listed_in_a_group`. Then `git add src/executor/builtin_registry/registry/struct_def.rs src/executor/builtin_registry/builder/constructor/mod.rs src/executor/builtin_registry/registry/tool_registry_impl.rs src/executor/builtin_registry/builder/core_tools.rs src/executor/builtin_registry/groups.rs src/executor/builtin_registry/definitions.rs && git commit -m "executor: register strategy tool in builtin registry"`.

---

### Task F3: lifecycle clears — goal `Clear`, loop `stop`, gate-confirmed Complete; NOT on Blocked

**Files:**
- Modify: `src/builtin_tools/goal.rs` (`GoalAction::Clear` arm, line 324-330)
- Modify: `src/builtin_tools/loop_manage.rs` (`stop` method, line 194-221)
- Modify: `src/gateway/execution_engine/execute.rs` (gate-confirmed complete, line 699 region)
- Test: `src/builtin_tools/goal.rs` tests + `src/builtin_tools/loop_manage.rs` tests

**Interfaces:**
- Consumes: `crate::strategy::{global, goal_key, loop_key}` + `StrategyStore::delete` (Group A contract). Reuses the existing `self.session()` already in scope at both tool sites and `session_key_str` at the execute.rs site.
- Produces: strategy rows are deleted in lockstep with the authoritative end-points; a co-existing strategy keyed by the *other* flow is left intact (composite-key isolation).

Design decision (mirrors how `execute.rs` already reaches `crate::looping::global()` / `crate::goal::global()` at lifecycle points): clear via the **process-global** strategy store, fail-soft. The tools do not hold a strategy-store handle, and threading one through every constructor would be over-engineering — the global is the established pattern for these cross-cutting lifecycle touches. Each clear is best-effort: a missing global (early boot / tests without a booted daemon) is a no-op.

- [ ] **Step 1: Write the failing tests** —

  In `goal.rs` tests, add (the existing `tool_with_session` uses an isolated temp store; for strategy we set the global once, keyed by the same session):

```rust
    #[tokio::test]
    async fn clear_deletes_goal_keyed_strategy_but_not_loop_keyed() {
        use crate::strategy::{goal_key, loop_key, Strategy, StrategyStore};
        use crate::sync_primitives::Arc;

        let sdir = tempfile::tempdir().unwrap();
        let sstore = Arc::new(StrategyStore::open(&sdir.path().join("s.db")).unwrap());
        crate::strategy::set_global_for_test(sstore.clone());

        let concrete = Strategy {
            objective: "o".into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["do not touch unrelated code".into()],
            success_criteria: "ok".into(),
            goal_id: None,
        };
        sstore.put(&goal_key("sess-clear-strat"), &concrete).unwrap();
        sstore.put(&loop_key("sess-clear-strat"), &concrete).unwrap();

        let (tool, _d) = tool_with_session("sess-clear-strat");
        tool.call(GoalArgs {
            objective: Some("x".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        tool.call(GoalArgs { ..args(GoalAction::Clear) })
            .await
            .unwrap();

        // Goal Clear removes the goal-keyed strategy...
        assert!(sstore.get(&goal_key("sess-clear-strat")).unwrap().is_none());
        // ...but leaves a co-existing loop-keyed strategy untouched.
        assert!(sstore.get(&loop_key("sess-clear-strat")).unwrap().is_some());
    }
```

  In `loop_manage.rs` tests, add:

```rust
    #[tokio::test]
    async fn stop_deletes_loop_keyed_strategy_but_not_goal_keyed() {
        use crate::strategy::{goal_key, loop_key, Strategy, StrategyStore};

        let sdir = tempfile::tempdir().unwrap();
        let sstore = std::sync::Arc::new(StrategyStore::open(&sdir.path().join("s.db")).unwrap());
        crate::strategy::set_global_for_test(sstore.clone());

        let concrete = Strategy {
            objective: "o".into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["stay on the watch target".into()],
            success_criteria: "ok".into(),
            goal_id: None,
        };
        sstore.put(&loop_key("sess-loop-stop").as_str(), &concrete).unwrap();
        sstore.put(&goal_key("sess-loop-stop").as_str(), &concrete).unwrap();

        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "sess-loop-stop",
            "p",
            crate::looping::Cadence::Fixed { interval_ms: 1000 },
            0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("sess-loop-stop");
        tool.run(LoopArgs {
            action: LoopAction::Stop,
            interval: None,
            prompt: None,
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();

        // Loop stop removes the loop-keyed strategy...
        assert!(sstore.get(&loop_key("sess-loop-stop")).unwrap().is_none());
        // ...but leaves a co-existing goal-keyed strategy untouched.
        assert!(sstore.get(&goal_key("sess-loop-stop")).unwrap().is_some());
    }
```

  > Note: `set_global_for_test` is the `#[cfg(test)]` global override Group A must expose (the spec's `init_global`/`global` mirror of `goal/mod.rs`). It is idempotent (`OnceCell::set`); if a prior test in the same binary already set it, the test still passes because both tests seed disjoint session keys and assert only on their own rows. If global-test contention is observed, Group A should expose a `reset_global_for_test`; flag if so.

- [ ] **Step 2: Implement** —

  `goal.rs` `GoalAction::Clear` arm (line 324-330) — clear the goal-keyed strategy in lockstep with the authoritative goal deletion:
```rust
            GoalAction::Clear => {
                self.store.delete(&session)?;
                // Clear the goal-welded Strategy in lockstep with the
                // authoritative goal deletion (spec §6 lifecycle). Best-effort:
                // a missing global / corrupt row is a no-op, never fails the
                // user's clear. The loop-keyed Strategy (if any) is untouched.
                if let Some(strat) = crate::strategy::global() {
                    if let Err(e) = strat.delete(&crate::strategy::goal_key(&session)) {
                        info!(session = %session, error = %e,
                            "goal clear: failed to delete welded strategy (ignored)");
                    }
                }
                Ok(GoalOutput {
                    success: true,
                    message: "Standing goal cleared.".to_string(),
                })
            }
```

  `loop_manage.rs` `stop` method — when a live loop is actually stopped (the `Some(state)` arm at lines 205-215), delete the loop-keyed strategy. Do NOT delete on the already-stopped / no-loop arms (no fresh stop happened):
```rust
            Some(state) => {
                self.registry.put(
                    state
                        .with_status(LoopStatus::Stopped)
                        .with_stop_reason(Some("Stopped by user request.".to_string())),
                );
                // Clear the loop-welded Strategy in lockstep with the
                // authoritative loop stop (spec §6 lifecycle). Best-effort; the
                // goal-keyed Strategy (if any) is untouched.
                if let Some(strat) = crate::strategy::global() {
                    if let Err(e) = strat.delete(&crate::strategy::loop_key(session)) {
                        info!(session = %session, error = %e,
                            "loop stop: failed to delete welded strategy (ignored)");
                    }
                }
                Ok(LoopOutput {
                    success: true,
                    message: "Loop stopped.".to_string(),
                })
            }
```
  (Add `use tracing::info;` is already present in `loop_manage.rs` line 12; goal.rs already imports `info` at line 13.)

  `execute.rs` gate-confirmed complete — in the `None =>` branch (gate passed, line 693-706), after the successful `store.put(&confirmed)` log, optionally clear the goal-keyed strategy for this session:
```rust
                                        None => {
                                            // 闸门通过 → 确认完成，循环终止。
                                            let confirmed =
                                                crate::tasks::goal_pursuit::confirm_complete(
                                                    &goal, now_ms,
                                                );
                                            if let Err(e) = store.put(&confirmed) {
                                                warn!(error = %e, session = %session_key_str,
                                                    "goal pursuit: failed to persist gate confirmation");
                                            } else {
                                                info!(session = %session_key_str,
                                                    "goal pursuit: objective gate passed, goal verified complete");
                                                // Gate-confirmed complete is an
                                                // authoritative end-point: clear
                                                // the welded Strategy so it does
                                                // not bleed into a later plain
                                                // turn in this reused session
                                                // (spec §6). Best-effort.
                                                if let Some(strat) = crate::strategy::global() {
                                                    if let Err(e) = strat.delete(
                                                        &crate::strategy::goal_key(
                                                            &session_key_str,
                                                        ),
                                                    ) {
                                                        warn!(error = %e, session = %session_key_str,
                                                            "goal pursuit: failed to clear welded strategy on complete (ignored)");
                                                    }
                                                }
                                            }
                                        }
```
  > Critically, do **NOT** add any strategy delete in the `exhausted_while_active` → `Blocked` branch (execute.rs:787-790), nor in `block_goal_on_failure` (the `:1139-1148` region), nor in `stop_loop_on_failure` (`:1179-1213`). A `Blocked` goal can resume via `goal(update, status='active')`, so its welded Strategy must survive (spec §6). Loop error-halt is a transient failure, not a user stop — leaving its Strategy in place is harmless because the loop is no longer Active and `active_strategy` reads are governed by goal precedence + the next authoritative stop.

  > `session_key_str` is a `String` at this point in `execute.rs`; `goal_key(&session_key_str)` deref-coerces to `&str` — matches the contract `goal_key(session_id: &str)`.

- [ ] **Step 3 (last): Verify** — `cargo test -p alephcore --lib builtin_tools::goal:: builtin_tools::loop_manage::` then `cargo check -p alephcore --lib` (covers the execute.rs edit, which is in the gateway crate path but compiled by the lib check). Then `git add src/builtin_tools/goal.rs src/builtin_tools/loop_manage.rs src/gateway/execution_engine/execute.rs && git commit -m "lifecycle: clear welded strategy on goal clear / loop stop / gate-confirmed complete"`.

---

### Task F4: objective-change auto-invalidation on goal `Set`

**Files:**
- Modify: `src/builtin_tools/goal.rs` (`GoalAction::Set` arm, after `self.store.put(&goal)?` at line 250)
- Test: `src/builtin_tools/goal.rs` tests

**Interfaces:**
- Consumes: `crate::strategy::{global, goal_key}` + `StrategyStore::{get, delete}` (Group A contract); `Strategy.goal_id: Option<String>` cross-ref field; the freshly-built `goal.id` (`Goal::new` → `"goal-{fxhash(session:objective)}"`, `src/goal/types.rs:99`).
- Produces: when a goal `Set` replaces an existing goal whose objective changed, the stale goal-keyed Strategy (which referenced the old `goal_id`) is removed, so a later `active_strategy` read does not weld a map for a different objective. The planner (Group C) mints a fresh Strategy with the new `goal.id`.

Mechanism (spec §6 "Auto-invalidate when the objective string changes — compare stored `goal.id`"): on every `Set`, after persisting the new goal, look up the existing goal-keyed Strategy. If it exists and its `goal_id` is `Some(old)` and `old != goal.id` (the new goal's id, which is a hash of `session:objective`), delete it. If the stored Strategy has `goal_id == None` (a pre-cross-ref or workflow/loop-style strategy) leave it — the contract only auto-invalidates on a *changed* concrete cross-ref. A matching `goal_id` (objective unchanged → identical hash) is left intact (re-setting the same objective keeps the map).

- [ ] **Step 1: Write the failing test** — in `goal.rs` tests:

```rust
    #[tokio::test]
    async fn set_with_changed_objective_invalidates_stale_strategy() {
        use crate::strategy::{goal_key, Strategy, StrategyStore};
        use crate::sync_primitives::Arc;

        let sdir = tempfile::tempdir().unwrap();
        let sstore = Arc::new(StrategyStore::open(&sdir.path().join("s.db")).unwrap());
        crate::strategy::set_global_for_test(sstore.clone());

        let (tool, _d) = tool_with_session("sess-objchg");
        // First goal -> compute its id the same way Goal::new does, then seed a
        // strategy cross-referencing it.
        tool.call(GoalArgs {
            objective: Some("Migrate auth to v2".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let first = sstore
            .get(&goal_key("sess-objchg"))
            .unwrap(); // not yet seeded
        assert!(first.is_none());
        // Seed a strategy that carries the FIRST goal's id.
        let first_goal_id = tool
            .store
            .get("sess-objchg")
            .unwrap()
            .unwrap()
            .id
            .clone();
        let strat = Strategy {
            objective: "Migrate auth to v2".into(),
            approach: "incremental".into(),
            phases: vec![],
            guardrails: vec!["do not break existing sessions".into()],
            success_criteria: "tests green".into(),
            goal_id: Some(first_goal_id),
        };
        sstore.put(&goal_key("sess-objchg"), &strat).unwrap();

        // Re-set with a DIFFERENT objective -> new goal.id -> stale strategy gone.
        tool.call(GoalArgs {
            objective: Some("Rewrite the billing pipeline".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        assert!(
            sstore.get(&goal_key("sess-objchg")).unwrap().is_none(),
            "changed objective must invalidate the stale welded strategy"
        );
    }

    #[tokio::test]
    async fn set_with_same_objective_keeps_strategy() {
        use crate::strategy::{goal_key, Strategy, StrategyStore};
        use crate::sync_primitives::Arc;

        let sdir = tempfile::tempdir().unwrap();
        let sstore = Arc::new(StrategyStore::open(&sdir.path().join("s.db")).unwrap());
        crate::strategy::set_global_for_test(sstore.clone());

        let (tool, _d) = tool_with_session("sess-same");
        tool.call(GoalArgs {
            objective: Some("Keep me".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let gid = tool.store.get("sess-same").unwrap().unwrap().id.clone();
        let strat = Strategy {
            objective: "Keep me".into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["one concrete guardrail".into()],
            success_criteria: "ok".into(),
            goal_id: Some(gid),
        };
        sstore.put(&goal_key("sess-same"), &strat).unwrap();

        // Same objective => same goal.id => strategy preserved.
        tool.call(GoalArgs {
            objective: Some("Keep me".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        assert!(
            sstore.get(&goal_key("sess-same")).unwrap().is_some(),
            "unchanged objective must keep the welded strategy"
        );
    }
```

  > `tool.store` is the `GoalTool`'s private `store` field — accessible because the test module is in-file (`super::*`). The contract guarantees `Goal::new` derives `id` from `session:objective`, so a re-`Set` with the same objective yields an identical id.

- [ ] **Step 2: Implement** — in `goal.rs` `GoalAction::Set`, right after `self.store.put(&goal)?;` (line 250), before building `GoalOutput`:

```rust
                self.store.put(&goal)?;
                // Objective-change auto-invalidation (spec §6): if a welded
                // Strategy exists for this session and it cross-references a
                // DIFFERENT goal id than the one we just minted, the objective
                // changed under it — drop the stale map so a later turn does not
                // weld a plan for a different objective. A Strategy with no
                // cross-ref (goal_id None) or a matching id is left intact.
                if let Some(strat_store) = crate::strategy::global() {
                    let key = crate::strategy::goal_key(&session);
                    match strat_store.get(&key) {
                        Ok(Some(existing)) => {
                            if existing
                                .goal_id
                                .as_deref()
                                .is_some_and(|old| old != goal.id)
                            {
                                if let Err(e) = strat_store.delete(&key) {
                                    info!(session = %session, error = %e,
                                        "goal set: failed to invalidate stale strategy (ignored)");
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            info!(session = %session, error = %e,
                                "goal set: failed to read strategy for invalidation (ignored)");
                        }
                    }
                }
                Ok(GoalOutput {
                    success: true,
                    message: format!("Set. {}", Self::render(&goal)),
                })
```

  `goal.id` is the field on the freshly-built `goal: Goal` (in scope at this point). `goal_id` on the stored `Strategy` is the contract field (`Option<String>`). No new imports needed (`info` already imported, `crate::strategy::*` referenced by full path).

- [ ] **Step 3 (last): Verify** — `cargo test -p alephcore --lib builtin_tools::goal::` then `git add src/builtin_tools/goal.rs && git commit -m "goal: auto-invalidate welded strategy on objective change"`.

---

Group F notes for the orchestrator:
- **Hard dependency on Group A** (`src/strategy/` contract: `Strategy`, `StrategyStore`, `goal_key`/`loop_key`, `init_global`/`global`, plus a `#[cfg(test)] set_global_for_test` mirror of `goal/mod.rs`). Group F's tests reference `set_global_for_test` — confirm Group A exposes it; if Group A omits it, F3/F4 tests must instead inject the store via a different test seam (flag to orchestrator).
- F1 and F2 are sequential (F2 registers the tool F1 defines). F3 and F4 both edit `goal.rs` (F3 the `Clear` arm, F4 the `Set` arm) — non-overlapping hunks, but if run by parallel subagents they touch the same file; serialize the commit of F3 before F4 to avoid a merge of the same file in flight.
- All lifecycle clears reach the strategy store via `crate::strategy::global()` (the established `execute.rs` pattern for `looping::global()`/`goal::global()`), not via a threaded handle — keeps tool constructors unchanged and honors composite-key isolation.
- No `cargo` runs mid-task; one scoped verify + commit per task.


---
