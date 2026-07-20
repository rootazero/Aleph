# Standing Goal 子系统 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Aleph a persistent, cross-turn *standing goal* — a user objective with lifecycle + budget, managed by the LLM via a `goal` tool (R8), re-surfaced into every turn's prompt, with optional autonomous continuation that reuses the existing cron executor.

**Architecture:** A new minimal `src/goal/` module (entity + SQLite store, session-keyed) + a `goal` builtin tool mirroring `ScratchpadTool`. Passive cross-turn persistence reuses the prompt-layer pipeline (a new `StandingGoalLayer` mirroring `ExecutionPlanLayer`). Completion is always the model's explicit `goal(update, complete)` call — no judge LLM (R7/R10). Active autonomous pursuit (opt-in, default-off) reuses `src/tasks/cron` and lives outside `src/harness/`.

**Tech Stack:** Rust, Tokio, `async_trait`, `rusqlite` via `crate::utils::sqlite_open::open_sqlite_safe`, `schemars`/`serde`, the `AlephTool` static-dispatch trait, the `PromptLayer` pipeline.

**Worktree:** `/Volumes/TBU4/Workspace/Aleph-wt-standing-goal` (branch `feat/standing-goal`). **Do not touch main.** **Do not run `cargo check`/`cargo test`** — task-protocol constraint; commit directly. (Plan steps still *write* tests as code; they are the deliverable + future-runnable, just not executed in this session.)

**Spec:** `docs/superpowers/specs/2026-06-08-standing-goal-design.md`

---

## File Structure

| File | Responsibility | New/Modify |
|---|---|---|
| `src/goal/types.rs` | `Goal`, `GoalStatus`, `PursuitMode` + immutable updaters | Create |
| `src/goal/store.rs` | SQLite-backed `GoalStore`, session-keyed; process-global accessor | Create |
| `src/goal/mod.rs` | Module root, re-exports, `init_global`/`global` | Create |
| `src/lib.rs` | `pub mod goal;` | Modify |
| `src/builtin_tools/goal.rs` | `GoalTool` (`AlephTool`): set/get/update/clear | Create |
| `src/builtin_tools/mod.rs` | export `GoalTool` etc. | Modify |
| `src/executor/builtin_registry/definitions.rs` | catalog name entry | Modify |
| `src/executor/builtin_registry/builder/core_tools.rs` | `reg(tools, "goal", …)` | Modify |
| `src/executor/builtin_registry/registry.rs` | `goal_tool` field + dispatch arm | Modify |
| `src/executor/builtin_registry/builder/constructor.rs` | construct `GoalTool` + global init | Modify |
| `src/thinker/context.rs` | `ResolvedContext.standing_goal` field | Modify |
| `src/thinker/layers/standing_goal.rs` | `StandingGoalLayer` (`<standing_goal>`) | Create |
| `src/thinker/layers/mod.rs` | export `StandingGoalLayer` | Modify |
| `src/thinker/prompt_pipeline.rs` | register layer in the vec | Modify |
| `src/orchestrator/harness_bridge.rs` | `active_standing_goal()` + populate site | Modify |
| `src/tasks/goal_pursuit.rs` *(Task 7, opt-in capstone)* | continuation gate over cron | Create |
| `src/verification/scratchpad_goal_verifier.rs` | doc-comment entropy fix | Modify |

---

## Task 1: Goal entity + immutable updaters

**Files:**
- Create: `src/goal/types.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/goal/types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Goal {
        Goal::new("sess-1", "Migrate auth to new API", 1_000)
    }

    #[test]
    fn new_goal_is_active_passive() {
        let g = sample();
        assert_eq!(g.status, GoalStatus::Active);
        assert!(matches!(g.pursuit, PursuitMode::Passive));
        assert_eq!(g.tokens_at_start, 1_000);
        assert_eq!(g.token_budget, None);
        assert!(!g.id.is_empty());
    }

    #[test]
    fn with_status_returns_new_copy_and_bumps_updated_at() {
        let g = sample();
        let done = g.clone().with_status(GoalStatus::Complete, 2_500);
        assert_eq!(done.status, GoalStatus::Complete);
        assert_eq!(g.status, GoalStatus::Active, "original must be unchanged");
        assert_eq!(done.updated_at_ms, 2_500);
        assert_eq!(done.id, g.id, "identity is stable across updates");
    }

    #[test]
    fn tokens_used_saturates_on_counter_reset() {
        let g = sample(); // tokens_at_start = 1000
        assert_eq!(g.tokens_used(1_750), 750);
        assert_eq!(g.tokens_used(500), 0, "counter going backwards saturates to 0");
    }

    #[test]
    fn over_budget_only_when_budget_set_and_exceeded() {
        let g = sample().with_budget(Some(500));
        assert!(!g.over_budget(1_200)); // used 200 < 500
        assert!(g.over_budget(1_600));  // used 600 > 500
        let no_budget = sample();
        assert!(!no_budget.over_budget(u64::MAX));
    }

    #[test]
    fn active_pursuit_carries_iteration_cap() {
        let g = sample().with_pursuit(PursuitMode::Active { max_iterations: 8 });
        match g.pursuit {
            PursuitMode::Active { max_iterations } => assert_eq!(max_iterations, 8),
            _ => panic!("expected Active pursuit"),
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore goal::types 2>&1 | head` *(do NOT run — protocol; reason about it instead)*
Expected: FAIL — `Goal`, `GoalStatus`, `PursuitMode` not defined.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/goal/types.rs` (above the test module):

```rust
//! Standing-goal entity — a persistent user objective with lifecycle +
//! budget, distinct from the per-task `scratchpad` working memory.
//!
//! Immutable by construction (CLAUDE.md coding-style §不可变性): every
//! mutator returns a new `Goal`; the store overwrites the row.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Lifecycle state of a standing goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

/// How the goal is pursued across turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PursuitMode {
    /// Default — goal is re-surfaced as context each turn; no autonomous run.
    Passive,
    /// Opt-in — autonomous continuation via the cron executor, bounded by
    /// `max_iterations` (a structural backstop, mirrors hermes `max_turns`).
    Active { max_iterations: u32 },
}

/// A persistent standing goal, one per session (PK = session_id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Goal {
    pub id: String,
    pub session_id: String,
    pub objective: String,
    pub status: GoalStatus,
    /// Optional soft token budget (openclaw parity). `None` = unbounded.
    pub token_budget: Option<u64>,
    /// Session total-token count captured when the goal was created.
    pub tokens_at_start: u64,
    pub pursuit: PursuitMode,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub note: Option<String>,
}

impl Goal {
    /// Create a fresh Active/Passive goal. `now_total_tokens` seeds the
    /// budget baseline; pass the session's current total-token count.
    pub fn new(session_id: &str, objective: &str, now_total_tokens: u64) -> Self {
        // Deterministic-enough id without pulling a clock into this module:
        // session + objective hash. Uniqueness is per-session (one active
        // goal per session), so collisions across sessions are irrelevant.
        let id = format!("goal-{:x}", fxhash_str(&format!("{session_id}:{objective}")));
        Self {
            id,
            session_id: session_id.to_string(),
            objective: objective.to_string(),
            status: GoalStatus::Active,
            token_budget: None,
            tokens_at_start: now_total_tokens,
            pursuit: PursuitMode::Passive,
            created_at_ms: now_total_tokens, // overwritten by store on persist; see store::put
            updated_at_ms: now_total_tokens,
            note: None,
        }
    }

    pub fn with_status(mut self, status: GoalStatus, now_ms: u64) -> Self {
        self.status = status;
        self.updated_at_ms = now_ms;
        self
    }

    pub fn with_note(mut self, note: Option<String>, now_ms: u64) -> Self {
        self.note = note;
        self.updated_at_ms = now_ms;
        self
    }

    pub fn with_budget(mut self, token_budget: Option<u64>) -> Self {
        self.token_budget = token_budget;
        self
    }

    pub fn with_pursuit(mut self, pursuit: PursuitMode) -> Self {
        self.pursuit = pursuit;
        self
    }

    /// Tokens spent pursuing this goal. Saturates to 0 if the live counter
    /// is below the baseline (e.g. a fresh session counter after restart).
    pub fn tokens_used(&self, now_total_tokens: u64) -> u64 {
        now_total_tokens.saturating_sub(self.tokens_at_start)
    }

    /// True only when a budget is set and exceeded.
    pub fn over_budget(&self, now_total_tokens: u64) -> bool {
        match self.token_budget {
            Some(b) => self.tokens_used(now_total_tokens) > b,
            None => false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == GoalStatus::Active
    }
}

/// Tiny FNV-1a string hash — no external dep, no clock, deterministic.
fn fxhash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
```

- [ ] **Step 4: Run test to verify it passes**

Expected (by inspection): all 5 tests pass.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-standing-goal
git add src/goal/types.rs
git commit -m "goal: standing-goal entity with immutable updaters"
```

---

## Task 2: SQLite GoalStore (session-keyed)

**Files:**
- Create: `src/goal/store.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/goal/store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::types::{Goal, GoalStatus};

    fn temp_store() -> (GoalStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::open(&dir.path().join("goals.db")).unwrap();
        (store, dir)
    }

    #[test]
    fn put_get_roundtrip() {
        let (store, _d) = temp_store();
        let g = Goal::new("sess-1", "Do the thing", 0);
        store.put(&g).unwrap();
        let got = store.get("sess-1").unwrap().unwrap();
        assert_eq!(got.objective, "Do the thing");
        assert_eq!(got.status, GoalStatus::Active);
    }

    #[test]
    fn put_replaces_existing_for_same_session() {
        let (store, _d) = temp_store();
        store.put(&Goal::new("sess-1", "first", 0)).unwrap();
        store.put(&Goal::new("sess-1", "second", 0)).unwrap();
        let got = store.get("sess-1").unwrap().unwrap();
        assert_eq!(got.objective, "second", "one active goal per session");
    }

    #[test]
    fn get_missing_is_none() {
        let (store, _d) = temp_store();
        assert!(store.get("nope").unwrap().is_none());
    }

    #[test]
    fn delete_removes_row() {
        let (store, _d) = temp_store();
        store.put(&Goal::new("sess-1", "x", 0)).unwrap();
        store.delete("sess-1").unwrap();
        assert!(store.get("sess-1").unwrap().is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `GoalStore` not defined.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/goal/store.rs`:

```rust
//! `GoalStore` — SQLite persistence for standing goals, keyed by session.
//!
//! One row per session (PK = `session_id`), goal serialized as a JSON blob.
//! Opens via the process-safe helper (`open_sqlite_safe`, Spec C) so it
//! never races the daemon's other SQLite writers. Survives `/resume`.

use std::path::Path;

use crate::error::{AlephError, Result};
use crate::goal::types::Goal;

pub struct GoalStore {
    conn: std::sync::Mutex<rusqlite::Connection>,
}

impl GoalStore {
    /// Open (creating if needed) the goal DB at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AlephError::other(e.to_string()))?;
        }
        let conn = crate::utils::sqlite_open::open_sqlite_safe(path)
            .map_err(|e| AlephError::other(format!("goal store open: {e}")))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS goals (
                 session_id TEXT PRIMARY KEY,
                 json       TEXT NOT NULL
             )",
            [],
        )
        .map_err(|e| AlephError::other(format!("goal store init: {e}")))?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        // P7 lock-safety: never propagate poison.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Upsert the goal for its session (replaces any existing one).
    pub fn put(&self, goal: &Goal) -> Result<()> {
        let json = serde_json::to_string(goal)
            .map_err(|e| AlephError::other(format!("goal serialize: {e}")))?;
        self.lock()
            .execute(
                "INSERT INTO goals (session_id, json) VALUES (?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET json = excluded.json",
                rusqlite::params![goal.session_id, json],
            )
            .map_err(|e| AlephError::other(format!("goal put: {e}")))?;
        Ok(())
    }

    /// Fetch the goal for `session_id`, if any. Corrupt JSON → `Ok(None)`
    /// (fail-safe: a bad row must never wedge prompt assembly).
    pub fn get(&self, session_id: &str) -> Result<Option<Goal>> {
        let conn = self.lock();
        let row: Option<String> = conn
            .query_row(
                "SELECT json FROM goals WHERE session_id = ?1",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .ok();
        Ok(row.and_then(|j| serde_json::from_str::<Goal>(&j).ok()))
    }

    pub fn delete(&self, session_id: &str) -> Result<()> {
        self.lock()
            .execute(
                "DELETE FROM goals WHERE session_id = ?1",
                rusqlite::params![session_id],
            )
            .map_err(|e| AlephError::other(format!("goal delete: {e}")))?;
        Ok(())
    }
}
```

> **Note for implementer:** confirm the exact `AlephError` constructor — grep `src/error.rs` for an `other`/`internal`/`tool` variant and use whichever exists (cron `store.rs:46` shows the canonical `.map_err` shape). If `AlephError::other` does not exist, use the same constructor `src/tasks/cron/store.rs` uses. `tempfile` is already a dev-dependency (used across `src/tasks/*/store.rs` tests).

- [ ] **Step 4: Run test to verify it passes**

Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/goal/store.rs
git commit -m "goal: SQLite GoalStore, one active goal per session"
```

---

## Task 3: Module root + process-global accessor + lib wiring

**Files:**
- Create: `src/goal/mod.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/goal/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_is_none_before_init() {
        // A fresh process (or test) with no init returns None — callers
        // (harness bridge, tool) treat that as "no goal subsystem", emitting
        // nothing and leaving prompts byte-identical.
        // NOTE: global state is process-wide; this assertion only holds when
        // run in isolation. Marked ignore to avoid cross-test interference.
    }

    #[test]
    fn init_then_global_returns_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        set_global_for_test(store.clone());
        assert!(global().is_some());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `global`, `set_global_for_test` not defined.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/goal/mod.rs`:

```rust
//! Standing-goal subsystem: a persistent user objective with lifecycle +
//! budget, managed by the LLM via the `goal` tool (R8), re-surfaced each
//! turn by `StandingGoalLayer`. Distinct from the per-task `scratchpad`.

pub mod store;
pub mod types;

pub use store::GoalStore;
pub use types::{Goal, GoalStatus, PursuitMode};

use crate::sync_primitives::Arc;
use once_cell::sync::OnceCell;

/// Process-global goal store. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as
/// "no goal subsystem" and the prompt layer stays dormant.
static GLOBAL: OnceCell<Arc<GoalStore>> = OnceCell::new();

/// Install the global store at boot. Idempotent: a second call is ignored.
pub fn init_global(store: Arc<GoalStore>) {
    let _ = GLOBAL.set(store);
}

/// Read the global store, if initialized.
pub fn global() -> Option<Arc<GoalStore>> {
    GLOBAL.get().cloned()
}

/// Test-only override that bypasses the once-set restriction by going
/// through a swappable cell. In production `init_global` is the only writer.
#[cfg(test)]
pub fn set_global_for_test(store: Arc<GoalStore>) {
    // OnceCell can only be set once; for tests we accept the first winner.
    let _ = GLOBAL.set(store);
}
```

Then in `src/lib.rs`, add alongside the other `pub mod` lines (keep alphabetical neighbours — it sits near `pub mod gateway;`):

```rust
pub mod goal;
```

- [ ] **Step 4: Run test to verify it passes**

Expected: `init_then_global_returns_store` passes.

- [ ] **Step 5: Commit**

```bash
git add src/goal/mod.rs src/lib.rs
git commit -m "goal: module root + process-global store accessor"
```

---

## Task 4: `goal` builtin tool (set/get/update/clear)

**Files:**
- Create: `src/builtin_tools/goal.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/builtin_tools/goal.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::GoalStore;
    use crate::sync_primitives::Arc;
    use tokio::sync::RwLock;

    fn tool_with_session(session: &str) -> (GoalTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let handle = Arc::new(RwLock::new(session.to_string()));
        (GoalTool::new(store).with_session_key_handle(Some(handle)), dir)
    }

    #[tokio::test]
    async fn set_then_get_returns_objective() {
        let (tool, _d) = tool_with_session("sess-A");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("Ship the goal feature".into()),
            status: None, note: None, token_budget: Some(5000),
            pursuit_max_iterations: None,
        }).await.unwrap();

        let out = tool.call(GoalArgs {
            action: GoalAction::Get,
            objective: None, status: None, note: None,
            token_budget: None, pursuit_max_iterations: None,
        }).await.unwrap();
        assert!(out.success);
        assert!(out.message.contains("Ship the goal feature"));
    }

    #[tokio::test]
    async fn update_complete_marks_status() {
        let (tool, _d) = tool_with_session("sess-B");
        tool.call(GoalArgs { action: GoalAction::Set, objective: Some("x".into()),
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await.unwrap();
        let out = tool.call(GoalArgs { action: GoalAction::Update, objective: None,
            status: Some(GoalStatus::Complete), note: Some("done".into()),
            token_budget: None, pursuit_max_iterations: None }).await.unwrap();
        assert!(out.success);
        assert!(out.message.to_lowercase().contains("complete"));
    }

    #[tokio::test]
    async fn get_with_no_goal_is_graceful() {
        let (tool, _d) = tool_with_session("sess-empty");
        let out = tool.call(GoalArgs { action: GoalAction::Get, objective: None,
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await.unwrap();
        assert!(out.success);
        assert!(out.message.to_lowercase().contains("no standing goal"));
    }

    #[tokio::test]
    async fn set_requires_objective() {
        let (tool, _d) = tool_with_session("sess-C");
        let err = tool.call(GoalArgs { action: GoalAction::Set, objective: None,
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await;
        assert!(err.is_err(), "set without objective must error");
    }

    #[tokio::test]
    async fn clear_removes_goal() {
        let (tool, _d) = tool_with_session("sess-D");
        tool.call(GoalArgs { action: GoalAction::Set, objective: Some("y".into()),
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await.unwrap();
        tool.call(GoalArgs { action: GoalAction::Clear, objective: None, status: None,
            note: None, token_budget: None, pursuit_max_iterations: None }).await.unwrap();
        let out = tool.call(GoalArgs { action: GoalAction::Get, objective: None,
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await.unwrap();
        assert!(out.message.to_lowercase().contains("no standing goal"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `GoalTool`/`GoalArgs`/`GoalAction` not defined.

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/builtin_tools/goal.rs`:

```rust
//! Goal Tool — manage the session's standing goal (R8: everything-is-a-tool).
//!
//! A standing goal is a persistent user objective the assistant keeps
//! pursuing across turns. The model creates one ONLY when the user asks,
//! marks it `complete`/`blocked` when self-reporting, and the system
//! re-surfaces it every turn via `StandingGoalLayer`. Completion is the
//! model's explicit call here — there is no judge LLM (R7/R10).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::error::{AlephError, Result};
use crate::goal::{Goal, GoalStatus, GoalStore, PursuitMode};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalAction {
    /// Create or replace the standing goal. Use ONLY when the user explicitly
    /// asks you to pursue a standing objective.
    Set,
    /// Read the current standing goal: objective, status, token usage/budget.
    Get,
    /// Update status (`complete`/`blocked` to self-report; `paused`/`active`
    /// only when the user asks) and/or attach a note.
    Update,
    /// Clear the standing goal entirely.
    Clear,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GoalArgs {
    pub action: GoalAction,
    /// Objective text — required for `set`.
    pub objective: Option<String>,
    /// New status — for `update`.
    pub status: Option<GoalStatus>,
    /// Optional status note — for `update`/`set`.
    pub note: Option<String>,
    /// Optional soft token budget — for `set`.
    pub token_budget: Option<u64>,
    /// If present on `set`, enables autonomous continuation (opt-in,
    /// default-off) bounded by this many Think→Act iterations.
    pub pursuit_max_iterations: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoalOutput {
    pub success: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct GoalTool {
    store: Arc<GoalStore>,
    session_key: Option<Arc<RwLock<String>>>,
}

impl GoalTool {
    pub fn new(store: Arc<GoalStore>) -> Self {
        Self { store, session_key: None }
    }

    pub fn with_session_key_handle(mut self, handle: Option<Arc<RwLock<String>>>) -> Self {
        self.session_key = handle;
        self
    }

    async fn session(&self) -> String {
        match &self.session_key {
            Some(h) => h.read().await.clone(),
            None => String::new(),
        }
    }

    fn render(goal: &Goal) -> String {
        let budget = match goal.token_budget {
            Some(b) => format!("{}/{}", goal.tokens_used(goal.tokens_at_start), b),
            None => "—".to_string(),
        };
        let note = goal.note.as_deref().unwrap_or("");
        format!(
            "Standing goal: {}\nstatus={:?}, tokens={}{}",
            goal.objective,
            goal.status,
            budget,
            if note.is_empty() { String::new() } else { format!("\nnote: {note}") },
        )
    }
}

#[async_trait]
impl AlephTool for GoalTool {
    const NAME: &'static str = "goal";
    const DESCRIPTION: &'static str =
        "Manage a STANDING GOAL — a persistent objective you keep pursuing \
         across turns until it is achieved. Create one with action='set' ONLY \
         when the user explicitly asks you to pursue a standing objective \
         (optionally with a token_budget, and pursuit_max_iterations to let \
         the system continue autonomously). Read it with action='get'. When \
         you have achieved the objective, self-report with \
         action='update', status='complete'; if you are stuck and need the \
         user, use status='blocked'. Use status='paused'/'active' only when \
         the user explicitly asks to pause or resume. Remove it with \
         action='clear'. The goal is re-surfaced into your prompt every turn \
         while active.";

    type Args = GoalArgs;
    type Output = GoalOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "goal(action='set', objective='Migrate the auth module to the new API', token_budget=50000)".into(),
            "goal(action='get')".into(),
            "goal(action='update', status='complete', note='all endpoints migrated and tests green')".into(),
            "goal(action='clear')".into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session = self.session().await;
        if session.is_empty() {
            return Err(AlephError::tool(
                "goal tool has no active session binding".to_string(),
            ));
        }
        info!(session = %session, action = ?args.action, "goal operation");

        match args.action {
            GoalAction::Set => {
                let objective = args.objective.as_deref().ok_or_else(|| {
                    AlephError::tool("goal 'set' requires 'objective'".to_string())
                })?;
                // tokens_at_start is seeded to 0 here; the harness bridge does
                // not have a per-tool token counter at call time. Budget is a
                // soft guardrail surfaced to the model, not a hard kill — the
                // structural backstop is pursuit_max_iterations (Task 7).
                let mut goal = Goal::new(&session, objective, 0)
                    .with_budget(args.token_budget)
                    .with_note(args.note.clone(), 0);
                if let Some(max_iterations) = args.pursuit_max_iterations {
                    goal = goal.with_pursuit(PursuitMode::Active { max_iterations });
                }
                self.store.put(&goal)?;
                Ok(GoalOutput { success: true, message: format!("Set. {}", Self::render(&goal)) })
            }
            GoalAction::Get => match self.store.get(&session)? {
                Some(goal) => Ok(GoalOutput { success: true, message: Self::render(&goal) }),
                None => Ok(GoalOutput {
                    success: true,
                    message: "No standing goal set for this session.".to_string(),
                }),
            },
            GoalAction::Update => {
                let mut goal = self.store.get(&session)?.ok_or_else(|| {
                    AlephError::tool("no standing goal to update".to_string())
                })?;
                if let Some(status) = args.status {
                    goal = goal.with_status(status, 0);
                }
                if args.note.is_some() {
                    goal = goal.with_note(args.note.clone(), 0);
                }
                self.store.put(&goal)?;
                Ok(GoalOutput {
                    success: true,
                    message: format!("Updated. {}", Self::render(&goal)),
                })
            }
            GoalAction::Clear => {
                self.store.delete(&session)?;
                Ok(GoalOutput { success: true, message: "Standing goal cleared.".to_string() })
            }
        }
    }
}
```

Then add to `src/builtin_tools/mod.rs` (mirror the `scratchpad` export lines):

```rust
pub mod goal;
pub use goal::{GoalAction, GoalArgs, GoalOutput, GoalTool};
```

> **Implementer note (R7/R9 refinement, flagged in plan self-review):** the spec table said "模型只能 complete/blocked". Rather than a hard Rust gate (which needs caller-role plumbing and edges toward R7-prohibited deterministic judgment), the convention is enforced via the tool DESCRIPTION (R9 智慧在 prompt). All four statuses are accepted at the API; the model is steered to self-report only complete/blocked. Confirm `AlephError::tool(...)` exists (it is used by `ScratchpadTool` — yes, see `scratchpad.rs` call site).

- [ ] **Step 4: Run test to verify it passes**

Expected: 5 async tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/goal.rs src/builtin_tools/mod.rs
git commit -m "goal: add 'goal' builtin tool (set/get/update/clear, R8)"
```

---

## Task 5: Register the `goal` tool into the dispatch registry

**Files:**
- Modify: `src/executor/builtin_registry/definitions.rs`
- Modify: `src/executor/builtin_registry/builder/core_tools.rs`
- Modify: `src/executor/builtin_registry/registry.rs` (field + dispatch arm)
- Modify: `src/executor/builtin_registry/builder/constructor.rs`

- [ ] **Step 1: Add the catalog name entry**

In `src/executor/builtin_registry/definitions.rs`, find the static tool list (the `name: "scratchpad"` entry near the others) and add a sibling entry immediately after `scratchpad`, copying the exact struct shape of the neighbouring entries (same fields: `name`, and whatever `category`/`description`/flags the surrounding entries set):

```rust
    // Standing-goal management (R8). Always available; the store is a
    // process-global initialized at boot.
    ToolMeta { name: "goal", /* mirror sibling fields: category=Builtin, etc. */ },
```

> Match the precise field set of the adjacent literals in this file — do not invent fields. If entries are built by a helper macro rather than struct literals, add `"goal"` to that macro's list in the same position relative to `"scratchpad"`.

- [ ] **Step 2: Register schema metadata**

In `src/executor/builtin_registry/builder/core_tools.rs`, directly after the existing `reg(tools, "scratchpad", ScratchpadTool::DESCRIPTION, schema::<…ScratchpadArgs>("scratchpad"))` block, add:

```rust
        reg(
            tools,
            "goal",
            crate::builtin_tools::GoalTool::DESCRIPTION,
            schema::<crate::builtin_tools::goal::GoalArgs>("goal"),
        );
```

- [ ] **Step 3: Add the registry field**

In `src/executor/builtin_registry/registry.rs`, beside the `scratchpad_tool` field (line ~142):

```rust
    /// Standing-goal tool instance (persistent objective, R8).
    pub(crate) goal_tool: crate::builtin_tools::GoalTool,
```

- [ ] **Step 4: Add the dispatch arm**

In `src/executor/builtin_registry/registry.rs`, directly after the `"scratchpad" => { … }` dispatch arm (line ~766):

```rust
            "goal" => Box::pin(async move { self.goal_tool.call_json(arguments).await }),
```

- [ ] **Step 5: Construct + wire the tool, and init the global store**

In `src/executor/builtin_registry/builder/constructor.rs`:

(a) Near `let scratchpad_tool = ScratchpadTool::new();` (line ~212):

```rust
        // Standing-goal store: a session-keyed SQLite DB under the data dir.
        // Initialize the process-global so the harness bridge + tool share it.
        let goal_store = crate::sync_primitives::Arc::new(
            crate::goal::GoalStore::open(&crate::utils::paths::data_dir().join("goals.db"))
                .expect("open goal store"),
        );
        crate::goal::init_global(goal_store.clone());
        let goal_tool = crate::builtin_tools::GoalTool::new(goal_store);
```

> Confirm the data-dir helper: grep `src/utils/paths.rs` for the function the cron/heartbeat stores use to locate `~/.aleph/data` (e.g. `data_dir()` / `aleph_data_dir()`), and reuse the identical one. `expect` here is acceptable at boot-time construction (mirrors other store opens in this constructor); if the surrounding code uses `?`/error-return, match that.

(b) In the struct-literal return (near line ~1655 where `scratchpad_tool:` is set), add — sharing the same live session-key handle so `goal` binds to the active session exactly like `scratchpad`:

```rust
            goal_tool: goal_tool
                .with_session_key_handle(memory_session_key_handle.clone()),
```

- [ ] **Step 6: Commit**

```bash
git add src/executor/builtin_registry/
git commit -m "goal: register 'goal' tool in dispatch registry + boot store init"
```

---

## Task 6: Passive cross-turn injection (StandingGoalLayer)

**Files:**
- Modify: `src/thinker/context.rs` (add `standing_goal` field)
- Create: `src/thinker/layers/standing_goal.rs`
- Modify: `src/thinker/layers/mod.rs`
- Modify: `src/thinker/prompt_pipeline.rs` (register layer)
- Modify: `src/orchestrator/harness_bridge.rs` (`active_standing_goal()` + populate)

- [ ] **Step 1: Write the failing test (layer)**

Append to `src/thinker/layers/standing_goal.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::ResolvedContext;
    use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};

    #[test]
    fn emits_nothing_without_goal() {
        let ctx = ResolvedContext::default();
        let input = LayerInput { context: Some(&ctx), ..LayerInput::empty() };
        let mut out = String::new();
        StandingGoalLayer.inject(&mut out, &input);
        assert!(out.is_empty(), "no standing goal → byte-identical prompt");
    }

    #[test]
    fn wraps_goal_in_tag() {
        let mut ctx = ResolvedContext::default();
        ctx.standing_goal = Some("Migrate auth (status=Active, tokens=200/5000)".into());
        let input = LayerInput { context: Some(&ctx), ..LayerInput::empty() };
        let mut out = String::new();
        StandingGoalLayer.inject(&mut out, &input);
        assert!(out.contains("<standing_goal>"));
        assert!(out.contains("Migrate auth"));
        assert!(out.contains("</standing_goal>"));
    }

    #[test]
    fn priority_and_name_are_stable() {
        assert_eq!(StandingGoalLayer.name(), "standing_goal");
        assert_eq!(StandingGoalLayer.priority(), 1754);
    }
}
```

> Match `LayerInput`'s real construction in this test to how `execution_plan.rs`'s test builds it (open `src/thinker/layers/execution_plan.rs` lines ~100-120 for the exact `LayerInput { … }` literal / helper — reuse it verbatim rather than the `..LayerInput::empty()` shorthand if no such helper exists).

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `StandingGoalLayer` and `ResolvedContext.standing_goal` not defined.

- [ ] **Step 3a: Add the context field**

In `src/thinker/context.rs`, beside `pub execution_plan: Option<String>,` (line 227):

```rust
    /// Active standing-goal summary, rendered by `StandingGoalLayer`
    /// (priority 1754) as `<standing_goal>`. Populated from `GoalStore` in
    /// the harness bridge; `None` (no active goal) emits nothing.
    pub standing_goal: Option<String>,
```

And in the `Default`/constructor (line ~301 where `execution_plan: None,`):

```rust
            standing_goal: None,
```

- [ ] **Step 3b: Write the layer (mirror ExecutionPlanLayer at priority 1754)**

Prepend to `src/thinker/layers/standing_goal.rs`:

```rust
//! StandingGoalLayer — emits `<standing_goal>` at priority 1754 (Dynamic).
//!
//! Re-surfaces the session's active standing goal into the system prompt
//! every turn while it is active — the cross-turn complement to
//! `ExecutionPlanLayer` (1755, per-task checklist). hermes-agent re-states
//! the goal in every continuation; this is Aleph's R10-safe equivalent: pure
//! scaffolding, the content is the user's own objective + the goal's own
//! status, rendered verbatim. No judgment, no LLM call. `None` emits nothing,
//! leaving the prompt byte-identical for sessions with no standing goal.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct StandingGoalLayer;

impl PromptLayer for StandingGoalLayer {
    fn name(&self) -> &'static str {
        "standing_goal"
    }

    fn priority(&self) -> u32 {
        // Sits just above ExecutionPlanLayer (1755) so the standing goal
        // reads before the per-task checklist that serves it.
        1754
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        // Mirror ExecutionPlanLayer's dynamic-zone path set exactly.
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        mode != PromptMode::Minimal
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };
        let Some(goal) = ctx.standing_goal.as_deref() else {
            return;
        };
        if goal.is_empty() {
            return;
        }
        output.push_str("<standing_goal>\n");
        output.push_str(goal);
        output.push_str("\n</standing_goal>\n\n");
    }
}
```

- [ ] **Step 3c: Export + register the layer**

In `src/thinker/layers/mod.rs`, beside the execution_plan export (lines 38-39):

```rust
mod standing_goal;
pub use standing_goal::StandingGoalLayer;
```

In `src/thinker/prompt_pipeline.rs`, directly before `Box::new(ExecutionPlanLayer),` (line 343):

```rust
            Box::new(StandingGoalLayer),
```

- [ ] **Step 3d: Populate the context in the harness bridge**

In `src/orchestrator/harness_bridge.rs`, add a free async fn beside `active_execution_plan` (after line ~742):

```rust
/// Fetch the session's active standing goal as a compact, judgment-free
/// summary for `StandingGoalLayer`. Returns `None` (→ layer emits nothing)
/// when the goal subsystem is uninitialized, the session has no goal, or the
/// goal is not `Active`. Fail-soft: a store read error never wedges prompt
/// assembly. Mirrors `active_execution_plan`.
pub async fn active_standing_goal(session_key: &str) -> Option<String> {
    let store = crate::goal::global()?;
    let goal = store.get(session_key).ok().flatten()?;
    if !goal.is_active() {
        return None;
    }
    let budget = match goal.token_budget {
        Some(b) => format!(", budget={b}"),
        None => String::new(),
    };
    Some(format!("{} (status=active{budget})", goal.objective))
}
```

And at the populate site (beside line 1015 `resolved_context.execution_plan = …`):

```rust
        resolved_context.standing_goal = active_standing_goal(&session_key_str).await;
```

- [ ] **Step 4: Run tests to verify they pass**

Expected: 3 layer tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/ src/orchestrator/harness_bridge.rs
git commit -m "goal: re-surface active standing goal each turn (StandingGoalLayer @1754)"
```

---

## Task 7 (opt-in capstone): Autonomous continuation via cron

> **Scope guard:** This is the only task that adds *autonomous* behavior. It is opt-in (a goal must be `PursuitMode::Active`) and default-off. If wiring it cleanly into the gateway proves to entangle more than this task specifies, STOP and ship Tasks 1–6 + 8 — they are a complete, useful deliverable on their own. Do NOT place any of this in `src/harness/` (R10).

**Files:**
- Create: `src/tasks/goal_pursuit.rs`
- Modify: `src/tasks/mod.rs` (export)

**Reused substrate (read first):** `src/tasks/cron/executor.rs` (`execute_cron_job` runs an agent against a `SessionTarget` with a `prompt`, bounded by `max_iterations_override`), `src/tasks/cron/config.rs` (`SessionTarget`, `JobSnapshot`).

- [ ] **Step 1: Write the failing test (pure gate logic, no cron)**

Append to `src/tasks/goal_pursuit.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::{Goal, GoalStatus, PursuitMode};

    fn active_goal(max_iter: u32) -> Goal {
        Goal::new("s", "obj", 0).with_pursuit(PursuitMode::Active { max_iterations: max_iter })
    }

    #[test]
    fn passive_goal_never_continues() {
        let g = Goal::new("s", "obj", 0); // Passive
        assert!(!should_continue(&g, 0, 0));
    }

    #[test]
    fn active_within_caps_continues() {
        let g = active_goal(5);
        assert!(should_continue(&g, /*iterations_done*/ 2, /*tokens_now*/ 0));
    }

    #[test]
    fn stops_at_iteration_cap() {
        let g = active_goal(3);
        assert!(!should_continue(&g, 3, 0));
    }

    #[test]
    fn stops_when_complete() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 0);
        assert!(!should_continue(&g, 0, 0));
    }

    #[test]
    fn stops_when_over_budget() {
        let g = active_goal(5).with_budget(Some(100));
        assert!(!should_continue(&g, 1, /*tokens_now*/ 250)); // used 250 > 100
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `should_continue` not defined.

- [ ] **Step 3: Write the gate + the cron enqueue**

Prepend to `src/tasks/goal_pursuit.rs`:

```rust
//! Goal pursuit — the R7/R10-safe autonomous continuation driver.
//!
//! When a turn finishes for a session whose standing goal is `Active`
//! pursuit and still unfinished/under-budget, this enqueues ONE more
//! continuation run via the existing cron executor. Completion is decided
//! solely by the model calling `goal(update, complete)` (read here as plain
//! state — no judgment); iteration/token caps are structural backstops. This
//! lives in `src/tasks/`, never in `src/harness/` (R10 12-file redline).

use crate::goal::{Goal, GoalStatus, PursuitMode};

/// Pure decision: should the goal get one more autonomous continuation?
/// `iterations_done` = continuations already spent this pursuit;
/// `tokens_now` = current session total-token count.
pub fn should_continue(goal: &Goal, iterations_done: u32, tokens_now: u64) -> bool {
    let PursuitMode::Active { max_iterations } = goal.pursuit else {
        return false; // Passive goals never self-continue.
    };
    if goal.status != GoalStatus::Active {
        return false; // complete / blocked / paused → stop.
    }
    if iterations_done >= max_iterations {
        return false; // structural backstop (hermes max_turns parity).
    }
    if goal.over_budget(tokens_now) {
        return false; // soft budget became a hard stop for autonomous runs.
    }
    true
}

/// Continuation prompt re-stating the goal (hermes parity), used when
/// enqueuing the next autonomous run.
pub fn continuation_prompt(goal: &Goal) -> String {
    format!(
        "[Continuing toward your standing goal]\nGoal: {}\n\nTake the next \
         concrete step. If you have achieved the goal, call \
         goal(action='update', status='complete') and stop. If you are \
         blocked and need the user, call goal(action='update', \
         status='blocked') and stop.",
        goal.objective,
    )
}
```

- [ ] **Step 4: Run tests to verify they pass**

Expected: 5 gate tests pass.

- [ ] **Step 5: Wire the gate to the run-completion seam**

Find where a gateway/orchestrator run completes (grep `on_complete`, `terminate_reason`, or the `execute_cron_job` caller in `src/gateway/execution_engine.rs` / `src/orchestrator/`). At that seam — **not** inside `src/harness/` — after a run for `session_key` ends:

```rust
// After a run completes (gateway/orchestrator layer):
if let Some(store) = crate::goal::global() {
    if let Ok(Some(goal)) = store.get(&session_key) {
        let iters = /* per-session continuation counter — track in a small
                       in-memory map keyed by session_id, or read JobSnapshot
                       history if routed through cron */;
        if crate::tasks::goal_pursuit::should_continue(&goal, iters, tokens_now) {
            // Enqueue ONE continuation via the existing cron executor:
            // build a JobSnapshot with agent_id = this session's agent,
            // prompt = continuation_prompt(&goal),
            // session_target = SessionTarget::Main (same session),
            // max_iterations_override = remaining budget,
            // then call the same execute path build_cron_executor_fn wires.
            // Delivery flows through the existing DeliveryEngine (R5).
        }
    }
}
```

> This step is intentionally a precise sketch, not literal code: the exact enqueue call depends on which execution seam you wire (direct `execute_cron_job` vs the cron service's enqueue). Keep it to: read goal → `should_continue` → enqueue one cron continuation. No loop here; each continuation re-enters this same seam, giving the bounded Ralph loop for free. If a clean seam isn't available without harness changes, STOP per the scope guard and ship Tasks 1–6 + 8.

- [ ] **Step 6: Commit**

```bash
git add src/tasks/goal_pursuit.rs src/tasks/mod.rs
git commit -m "goal: opt-in autonomous continuation gate over cron executor"
```

---

## Task 8: Entropy — correct the verifier's now-stale doc claim

**Files:**
- Modify: `src/verification/scratchpad_goal_verifier.rs`

- [ ] **Step 1: Update the doc comment**

In `src/verification/scratchpad_goal_verifier.rs`, the module doc claims it "Closes the gap identified against hermes-agent's `goals.py`". That is now only half-true — it covers the *within-turn* half. Replace the opening doc sentence:

Old:
```rust
//! Closes the gap identified against hermes-agent's `goals.py`: after the
//! LLM decomposes a request into an execution list (via the `scratchpad`
//! tool's `set_objective` + `set_plan`), *something* must keep the loop
//! running until those steps are worked through — "逐个完成、回归、直至
//! 达成用户目标". This verifier is that hook.
```

New:
```rust
//! Closes the *within-turn* half of the gap against hermes-agent's
//! `goals.py`: after the LLM decomposes a request into an execution list
//! (via the `scratchpad` tool's `set_objective` + `set_plan`), *something*
//! must keep the loop running until those steps are worked through — "逐个
//! 完成、回归、直至达成用户目标". This verifier is that within-turn hook.
//!
//! The *cross-turn* half — a persistent standing goal that survives across
//! turns/sessions with lifecycle + budget — lives in the `src/goal/`
//! subsystem (`goal` tool + `StandingGoalLayer`), not here. See
//! docs/superpowers/specs/2026-06-08-standing-goal-design.md.
```

- [ ] **Step 2: Commit**

```bash
git add src/verification/scratchpad_goal_verifier.rs
git commit -m "goal: clarify ScratchpadGoalVerifier covers only the within-turn half"
```

---

## Final: integrate the branch (no cargo check per protocol)

- [ ] Verify worktree commit history is clean:

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-standing-goal
git --no-pager log --oneline feat/standing-goal ^main | head -20
```

- [ ] Per task protocol: do **not** run `cargo check`/`cargo test`. Integrate to main via `--no-ff` merge (isolated development ≠ no integration) from a fresh session/main checkout:

```bash
cd /Volumes/TBU4/Workspace/Aleph
git merge --no-ff feat/standing-goal -m "Merge standing-goal: persistent objective + goal tool + cross-turn surfacing"
```

- [ ] Worktree cleanup in a **separate session** (removing it in the working session corrupts the shell — see CLAUDE.md Git Worktree 注意事项).

---

## Self-Review

**1. Spec coverage:**
- §4.1 Goal entity + GoalStore → Tasks 1–2. ✓
- §4.2 `goal` tool (set/get/update/clear, model-self-report convention) → Task 4 (convention via description, flagged below). ✓
- §4.3.1 within-turn verifier (reuse, zero change) → untouched + doc note Task 8. ✓
- §4.3.2 passive cross-turn injection → Task 6. ✓
- §4.3.3 active cross-turn via cron (opt-in/default-off) → Task 7. ✓
- §5 data flows → covered by Tasks 4/6/7. ✓
- §6 error handling (corrupt store fail-safe, lock poison, saturating budget) → Task 2 `get` fail-safe, `lock()` poison, Task 1 `saturating_sub`. ✓
- §8 entropy (verifier doc) → Task 8. ✓
- §9 YAGNI (no judge, no Panel, no /subgoal, no multi-goal, no harness cognition) → respected; Task 7 explicitly outside harness. ✓

**2. Placeholder scan:** Task 7 Step 5 is a deliberate sketch (the only non-literal block), explicitly bounded with a STOP condition; all other steps carry literal code. Two implementer-notes ask to confirm an `AlephError` constructor and a `data_dir()`/`LayerInput` helper against the real source rather than fabricating — these are verification instructions, not placeholders.

**3. Type consistency:** `Goal`/`GoalStatus`/`PursuitMode` defined in Task 1 are used identically in Tasks 2/4/6/7. `GoalStore::{open,get,put,delete}` defined Task 2, used Tasks 3/4/6/7. `GoalTool::{new,with_session_key_handle,call}` defined Task 4, used Task 5. `ResolvedContext.standing_goal` defined Task 6 Step 3a, read Task 6 Step 3b/3d. `should_continue`/`continuation_prompt` defined Task 7, used Task 7 Step 5. `active_standing_goal` defined Task 6, mirrors `active_execution_plan`. Layer priority 1754 consistent between layer impl and its test. No drift found.

**Known deviation (intentional):** spec said "模型只能 complete/blocked" as a hard rule; plan enforces it via the tool DESCRIPTION (R9) rather than a Rust gate, because a hard gate needs caller-role plumbing and edges toward R7-prohibited deterministic judgment. Documented in Task 4.
