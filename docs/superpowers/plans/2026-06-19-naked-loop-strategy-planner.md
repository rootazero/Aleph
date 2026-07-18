# Naked Agent-Loop Strategic Planner (StraTA Round 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the existing StraTA strategic planner so a plain complex *first message* on the naked agent loop (no `/goal`·`/loop`·`/workflow`) also gets a Strategy planned up front and welded into turn 1.

**Architecture:** The weld side already works (`StrategyLayer`/`StrategyPointerLayer` read via `active_strategy()`). Round 2 adds (a) a new `session:{key}` composite key, (b) a fail-closed origin gate (`SessionKey::is_interactive()`) so only genuine human interactive first messages trigger, (c) a fire-once planner spawn in `ExecutionEngine::execute`'s first-message path — started concurrently with first-message setup, awaited+stored before the harness dispatch — and (d) a `[strategy].plan_naked_loop` config switch gated at boot. `src/harness/` is untouched.

**Tech Stack:** Rust (tokio + serde), Aleph `alephcore` lib + `aleph-server` bin. SQLite-backed `StrategyStore` (round 1).

**Spec:** `docs/superpowers/specs/2026-06-19-naked-loop-strategy-planner-design.md`. Round 1 spec: `docs/superpowers/specs/2026-06-18-strategic-planner-design.md`.

## Global Constraints

- **MSRV = 1.95**; repo pins stable `1.96.0` via `rust-toolchain.toml`. No new dependencies.
- **极度节制 cargo (HARD — overrides the skill's per-step TDD run cadence):** Do **NOT** run `cargo` after each step. Each task writes its test code (red) and implementation (green) as artifacts and commits **without** compiling. ALL compile/test verification is **batched into the final Task 8** (≤3 cargo invocations total). This matches the user's established workflow ("默认不跑全量测试，高风险合并至多一次 cargo check").
- **Redlines:** R10 — `src/harness/` MUST NOT be touched; the trigger lives in `src/gateway/execution_engine/` (the orchestration seam, where goal continuation already lives). R7 — the only complexity decision is the planner's own self-gate; code-side gating uses **origin/plumbing facts only** (SessionKey variant, resume flag, empty input), **never** message-content heuristics (no regex/length/keyword). R4 — no business logic in the WS I/O boundary (`server::handler`/`connect`); `execute.rs` is the orchestration engine, not that boundary.
- **P7 fail-soft:** every new path degrades to "no Strategy stored ⇒ byte-identical prompt ⇒ run proceeds". Planner failure, store error, disabled config — all silent no-ops.
- **Commits:** English, `<scope>: <description>`. Attribution is disabled globally — **no** `Co-Authored-By` line. Work directly on `main` (single-branch). Commit **only** the listed `src/` files per task; never `git add -A` (the working tree has unrelated WIP in `interfaces/webchat/` and the gitignored `docs/superpowers/`).
- **Determinism / immutability:** prefer `let`; new objects over mutation; `matches!` not `==` on `anyhow::Result`.

---

### Task 1: `SessionKey::is_interactive()` — the origin gate primitive

**Files:**
- Modify: `src/routing/session_key.rs` (add a method to `impl SessionKey`, and tests to its `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub fn SessionKey::is_interactive(&self) -> bool` — `true` for `Main` / `DirectMessage` / `Group`; `false` for `Task` / `Subagent` / `Ephemeral`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/routing/session_key.rs`:

```rust
#[test]
fn is_interactive_true_for_human_variants() {
    // Main + DirectMessage (via the peer alias) are genuine human sessions.
    assert!(SessionKey::main("a").is_interactive());
    assert!(SessionKey::peer("a", "peer-1").is_interactive());
}

#[test]
fn is_interactive_false_for_automated_variants() {
    // cron + group-chat member runs use Task keys; subagents/ephemerals too.
    // These must never trip the naked-loop planner gate.
    assert!(!SessionKey::task("a", "cron", "job-1").is_interactive());
    assert!(!SessionKey::task("a", "team_chat", "team-1").is_interactive());
    assert!(!SessionKey::ephemeral("a").is_interactive());
    assert!(!SessionKey::subagent(SessionKey::main("a"), "sub-1").is_interactive());
}
```

- [ ] **Step 2: Implement the method**

Add inside `impl SessionKey { ... }` (near `agent_id`, around `src/routing/session_key.rs:227`):

```rust
/// True for genuine human-interactive session variants (`Main`,
/// `DirectMessage`, `Group`). False for automated/internal origins (`Task`
/// = cron/webhook/team_chat, `Subagent`, `Ephemeral`). Used by the naked
/// agent-loop strategic-planner gate so a cron job / group-chat member /
/// subagent's first turn never trips the planner (R7: an origin fact, not a
/// message-content heuristic). Fail-closed: any future internal variant
/// defaults to non-interactive.
#[must_use]
pub fn is_interactive(&self) -> bool {
    matches!(
        self,
        Self::Main { .. } | Self::DirectMessage { .. } | Self::Group { .. }
    )
}
```

- [ ] **Step 3: Commit** (no cargo — batched in Task 8)

```bash
git add src/routing/session_key.rs
git commit -m "strategy: add SessionKey::is_interactive() for naked-loop planner gate"
```

---

### Task 2: `strategy::session_key()` composite key

**Files:**
- Modify: `src/strategy/mod.rs` (add fn beside `goal_key`/`loop_key`/`workflow_key`, and tests)

**Interfaces:**
- Produces: `pub fn strategy::session_key(session_id: &str) -> String` → `"session:{session_id}"`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/strategy/mod.rs`:

```rust
#[test]
fn session_key_is_prefixed() {
    assert_eq!(session_key("sess-1"), "session:sess-1");
}

#[test]
fn session_key_distinct_from_goal_and_loop() {
    // Naked-loop key must not collide with the explicit-flow keys.
    assert_ne!(session_key("s"), goal_key("s"));
    assert_ne!(session_key("s"), loop_key("s"));
}
```

- [ ] **Step 2: Implement the function**

Add after `workflow_key` (around `src/strategy/mod.rs:37`):

```rust
/// Composite-key prefix for a NAKED-loop (plain interactive chat) strategy,
/// keyed by session. Lowest precedence in `active_strategy` (goal > loop >
/// session) so an explicit `/goal` or `/loop` strategy in a reused session
/// always wins. Pass the canonical `SessionKey::to_key_string()` form so the
/// weld layers and the subagent weld read the same row.
#[must_use]
pub fn session_key(session_id: &str) -> String {
    format!("session:{session_id}")
}
```

- [ ] **Step 3: Commit**

```bash
git add src/strategy/mod.rs
git commit -m "strategy: add session_key composite key for naked-loop strategy"
```

---

### Task 3: Read the session-keyed Strategy (`active_strategy` + `resolve_key`)

**Files:**
- Modify: `src/orchestrator/harness_bridge/context_blocks.rs:54-60` (`active_strategy` — production weld read path)
- Modify: `src/builtin_tools/strategy_manage.rs:18` (import) and `:95-105` (`resolve_key` — the `strategy` tool's read/write path) + tests

**Interfaces:**
- Consumes: `strategy::session_key` (Task 2).
- Produces: `active_strategy` and `resolve_key` both resolve in order **goal > loop > session**.

> Precedence is unit-tested on `resolve_key` (it takes an **injected** store — isolated, no process-global `OnceCell`). `active_strategy`'s arm is byte-identical in ordering and its global-path positive behaviour is covered by the E2E (Task 8 / user-run).

- [ ] **Step 1: Write the failing tests** (in `strategy_manage.rs`'s `#[cfg(test)] mod tests`)

```rust
#[tokio::test]
async fn resolve_key_returns_session_when_only_session_exists() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(StrategyStore::open(&dir.path().join("s.db")).unwrap());
    let strat = Strategy {
        objective: "obj".into(),
        approach: "appr".into(),
        phases: vec![],
        guardrails: vec!["avoid X".into()],
        success_criteria: "done when Y".into(),
        goal_id: None,
    };
    store.put(&crate::strategy::session_key("sess-1"), &strat).unwrap();
    let tool = StrategyTool::new(store).with_session_for_test("sess-1");
    let key = tool.resolve_key("sess-1").unwrap();
    assert_eq!(key.as_deref(), Some("session:sess-1"));
}

#[tokio::test]
async fn resolve_key_goal_beats_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(StrategyStore::open(&dir.path().join("s.db")).unwrap());
    let strat = Strategy {
        objective: "obj".into(),
        approach: "appr".into(),
        phases: vec![],
        guardrails: vec!["avoid X".into()],
        success_criteria: "done when Y".into(),
        goal_id: None,
    };
    store.put(&crate::strategy::session_key("sess-1"), &strat).unwrap();
    store.put(&goal_key("sess-1"), &strat).unwrap();
    let tool = StrategyTool::new(store).with_session_for_test("sess-1");
    let key = tool.resolve_key("sess-1").unwrap();
    assert_eq!(key.as_deref(), Some("goal:sess-1"));
}
```

- [ ] **Step 2: Add the import** in `src/builtin_tools/strategy_manage.rs:18`

Change:
```rust
use crate::strategy::{goal_key, loop_key, Strategy, StrategyStore};
```
to:
```rust
use crate::strategy::{goal_key, loop_key, session_key, Strategy, StrategyStore};
```

- [ ] **Step 3: Extend `resolve_key`** (`src/builtin_tools/strategy_manage.rs:95-105`)

Add the session tier as the final fallback (before `Ok(None)`):

```rust
    fn resolve_key(&self, session: &str) -> std::result::Result<Option<String>, String> {
        let gk = goal_key(session);
        if self.store.get(&gk).map_err(|e| e.to_string())?.is_some() {
            return Ok(Some(gk));
        }
        let lk = loop_key(session);
        if self.store.get(&lk).map_err(|e| e.to_string())?.is_some() {
            return Ok(Some(lk));
        }
        // Naked-loop (plain interactive chat) strategy — lowest precedence so a
        // /goal or /loop strategy in a reused session always wins. Lets
        // `strategy show`/`revise` operate in a naked-loop session.
        let sk = session_key(session);
        if self.store.get(&sk).map_err(|e| e.to_string())?.is_some() {
            return Ok(Some(sk));
        }
        Ok(None)
    }
```

> Leave `revise`'s `unwrap_or_else(|| goal_key(session))` fallback (`:139-141`) **unchanged**: a revise before any planner row defaults to `goal_key`, which is harmless — `active_strategy` reads `goal_key` first, so the revised strategy is still picked up.

- [ ] **Step 4: Extend `active_strategy`** (`src/orchestrator/harness_bridge/context_blocks.rs:54-60`)

```rust
pub async fn active_strategy(session_key: &str) -> Option<crate::strategy::Strategy> {
    let store = crate::strategy::global()?;
    if let Some(s) = store.get(&crate::strategy::goal_key(session_key)).ok().flatten() {
        return Some(s);
    }
    if let Some(s) = store.get(&crate::strategy::loop_key(session_key)).ok().flatten() {
        return Some(s);
    }
    // Naked-loop (plain interactive chat) strategy — lowest precedence so an
    // explicit /goal or /loop strategy in a reused session always wins. This
    // is also the read used by the subagent weld (run_loop/inner.rs), so a
    // naked-loop session's subagents inherit the session Strategy (intended).
    store
        .get(&crate::strategy::session_key(session_key))
        .ok()
        .flatten()
}
```

- [ ] **Step 5: Commit**

```bash
git add src/orchestrator/harness_bridge/context_blocks.rs src/builtin_tools/strategy_manage.rs
git commit -m "strategy: read session-keyed strategy in active_strategy + resolve_key"
```

---

### Task 4: `[strategy].plan_naked_loop` config switch

**Files:**
- Modify: `src/config/types/phase6_wiring.rs:216-245` (`StrategyToml` struct + `Default` impl + new default fn) + its `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `StrategyToml.plan_naked_loop: bool` (serde default `true`); read at boot as `app_config.strategy.as_ref().map_or(true, |s| s.plan_naked_loop)`.

- [ ] **Step 1: Write the failing test** (in `phase6_wiring.rs`'s `#[cfg(test)] mod tests`)

```rust
#[test]
fn strategy_plan_naked_loop_defaults_true() {
    // Section present but field omitted ⇒ default true.
    let s: StrategyToml = toml::from_str("enabled = true").unwrap();
    assert!(s.plan_naked_loop);
    // Default impl ⇒ true.
    assert!(StrategyToml::default().plan_naked_loop);
}

#[test]
fn strategy_plan_naked_loop_parses_false() {
    let s: StrategyToml = toml::from_str("plan_naked_loop = false").unwrap();
    assert!(!s.plan_naked_loop);
}
```

- [ ] **Step 2: Add the field** to `StrategyToml` (`src/config/types/phase6_wiring.rs`, after `planner_model`, `:229`)

```rust
    /// Whether the strategic planner also fires for a NAKED agent-loop first
    /// message (a plain complex request that did NOT use /goal·/loop·/workflow).
    /// Default **true** (gated under `enabled`). Set `false` to keep planning
    /// only on the three explicit long-task flows and spare ordinary chat the
    /// turn-1 planner latency. The planner still self-gates trivial messages.
    #[serde(default = "strategy_plan_naked_loop_default")]
    pub plan_naked_loop: bool,
```

- [ ] **Step 3: Add the default fn** (next to `strategy_enabled_default`, `:234`)

```rust
/// serde default for `StrategyToml::plan_naked_loop` — naked-loop planning is
/// on unless an operator explicitly flips it off.
fn strategy_plan_naked_loop_default() -> bool {
    true
}
```

- [ ] **Step 4: Update the manual `Default` impl** (`:238-245`)

> The manual `Default` is an exhaustive `Self { ... }` literal — omitting the new field is a hard E0063 **inside `Default` itself** (and `grep "StrategyToml {"` misses it because it reads `Self {`).

```rust
impl Default for StrategyToml {
    fn default() -> Self {
        Self {
            enabled: strategy_enabled_default(),
            planner_model: None,
            plan_naked_loop: strategy_plan_naked_loop_default(),
        }
    }
}
```

- [ ] **Step 5: Commit**

```bash
git add src/config/types/phase6_wiring.rs
git commit -m "strategy: add [strategy] plan_naked_loop config switch (default on)"
```

---

### Task 5: `ExecutionEngine.planner_provider` field + builder

**Files:**
- Modify: `src/gateway/execution_engine/engine.rs` (struct field `:42-101`, `new()` initializer `:112-138`, new builder near `:172`)

**Interfaces:**
- Produces: field `pub(super) planner_provider: Option<Arc<dyn crate::providers::AiProvider>>` and `pub fn with_planner_provider(self, Option<Arc<dyn crate::providers::AiProvider>>) -> Self`. (`Arc` = the `crate::sync_primitives::Arc` alias already imported at `engine.rs:9`.) Consumed by Task 7 (`self.planner_provider`).

> No standalone test — pure plumbing, compile-verified in Task 8 and exercised by Task 7. `engine.rs` has no `use crate::providers::AiProvider`; the type is written fully-qualified to avoid touching imports.

- [ ] **Step 1: Add the struct field** — inside `pub struct ExecutionEngine`, after `channel_registry` (`src/gateway/execution_engine/engine.rs:100`):

```rust
    /// Strategic-planner provider for the naked agent-loop StraTA planner
    /// (round 2). The same provider the goal/loop/workflow tools use, but
    /// gated additionally on `[strategy].plan_naked_loop`: `None` when either
    /// `enabled` or `plan_naked_loop` is off, so the first-message planner
    /// trigger in `execute.rs` stays dormant (fail-soft, P7).
    pub(super) planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
```

- [ ] **Step 2: Default it in `new()`** — add to the `Self { ... }` literal (`:112-137`), after `channel_registry`:

```rust
            planner_provider: None,
```

- [ ] **Step 3: Add the builder** — after `with_memory_context_provider` (`src/gateway/execution_engine/engine.rs:186`):

```rust
    /// Inject the strategic-planner provider for the naked agent-loop planner.
    /// `None` (the default) keeps the first-message planner trigger dormant.
    #[must_use]
    pub fn with_planner_provider(
        mut self,
        provider: Option<Arc<dyn crate::providers::AiProvider>>,
    ) -> Self {
        self.planner_provider = provider;
        self
    }
```

- [ ] **Step 4: Commit**

```bash
git add src/gateway/execution_engine/engine.rs
git commit -m "gateway: add planner_provider field + builder to ExecutionEngine"
```

---

### Task 6: Wire the naked-loop planner provider at boot

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` (after `planner_provider` is built, `:386`; and the `ExecutionEngine` builder chain, `:704`)

**Interfaces:**
- Consumes: `StrategyToml.plan_naked_loop` (Task 4), `ExecutionEngine::with_planner_provider` (Task 5), the existing `planner_provider: Option<Arc<dyn AiProvider>>` (`:371-386`).

> `planner_provider` is **moved** into `tool_config` at `:449`; the engine's copy MUST be `.clone()`d **before** that move or it's an E0382 use-after-move.

- [ ] **Step 1: Compute the gated clone** — insert immediately after the `planner_provider` block (`src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:386`, before the `tool_config` literal at `:389`):

```rust
        // Round 2: the engine's copy of the planner provider is additionally
        // gated on [strategy].plan_naked_loop (default true). `None` here ⇒ the
        // naked agent-loop first-message planner trigger in execute.rs stays
        // dormant, while goal/loop/workflow tools keep their (enabled-gated)
        // planner_provider. Cloned BEFORE planner_provider is moved into
        // tool_config below (E0382 guard). `enabled` is already folded in:
        // planner_provider is None when [strategy].enabled = false.
        let naked_loop_planner_provider = if app_config
            .strategy
            .as_ref()
            .map_or(true, |s| s.plan_naked_loop)
        {
            planner_provider.clone()
        } else {
            None
        };
```

- [ ] **Step 2: Pass it to the engine** — immediately after the `let mut engine = ExecutionEngine::new(...);` binding (`:704`), add:

```rust
        engine = engine.with_planner_provider(naked_loop_planner_provider);
```

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "gateway: wire naked-loop planner_provider into ExecutionEngine at boot"
```

---

### Task 7: Fire the planner on the naked agent-loop first message

**Files:**
- Modify: `src/gateway/execution_engine/execute.rs` (two module-level free fns; one method on the `impl ExecutionEngine`; a spawn call after the first-message block `~:236`; an await+store before dispatch `~:388`; a `#[cfg(test)] mod` at end of file)

**Interfaces:**
- Consumes: `SessionKey::is_interactive` (T1), `strategy::session_key` (T2), `self.planner_provider` (T5), `strategy::planner::{plan_strategy, env_summary, PlannerContext}` and `strategy::global()` (round 1).
- Produces: behaviour only — a `session:{key}` Strategy stored before harness dispatch on a genuine human first message.

- [ ] **Step 1: Write the failing tests** — add a test module at the end of `src/gateway/execution_engine/execute.rs`:

```rust
#[cfg(test)]
mod naked_loop_planner_tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    #[test]
    fn gate_fires_for_human_first_message() {
        let k = SessionKey::main("a");
        assert!(naked_loop_planner_should_fire(&k, true, false, "research X and email Bob"));
    }

    #[test]
    fn gate_excludes_non_first_message() {
        let k = SessionKey::main("a");
        assert!(!naked_loop_planner_should_fire(&k, false, false, "hi"));
    }

    #[test]
    fn gate_excludes_automated_origins() {
        // cron + group-chat member runs use Task keys; subagent/ephemeral too.
        let cron = SessionKey::task("a", "cron", "job-1");
        let team = SessionKey::task("a", "team_chat", "team-1");
        let eph = SessionKey::ephemeral("a");
        let sub = SessionKey::subagent(SessionKey::main("a"), "s");
        assert!(!naked_loop_planner_should_fire(&cron, true, false, "do a thing"));
        assert!(!naked_loop_planner_should_fire(&team, true, false, "do a thing"));
        assert!(!naked_loop_planner_should_fire(&eph, true, false, "do a thing"));
        assert!(!naked_loop_planner_should_fire(&sub, true, false, "do a thing"));
    }

    #[test]
    fn gate_excludes_resume_and_empty() {
        let k = SessionKey::main("a");
        assert!(!naked_loop_planner_should_fire(&k, true, true, "x")); // resume
        assert!(!naked_loop_planner_should_fire(&k, true, false, "   ")); // whitespace
    }

    #[test]
    fn bounded_objective_caps_utf8_safely() {
        let long = "字".repeat(5000); // multi-byte chars
        let out = bounded_objective(&long);
        assert_eq!(out.chars().count(), 4000);
        assert!(out.is_char_boundary(out.len()));
    }
}
```

- [ ] **Step 2: Add the pure gate + bounding helpers** — at module level in `src/gateway/execution_engine/execute.rs` (near the other free fns / top of the file body):

```rust
/// Pure gate for the naked agent-loop strategic planner. ORIGIN/PLUMBING FACTS
/// ONLY (R7 — never a message-content heuristic): fire only for a genuine human
/// interactive first message. Excludes cron / group-chat member / subagent /
/// ephemeral runs (non-interactive `SessionKey`), resume runs, and empty input.
/// Pure ⇒ host-testable without an LLM or a live gateway.
fn naked_loop_planner_should_fire(
    session_key: &crate::routing::session_key::SessionKey,
    is_first_message: bool,
    is_resume: bool,
    input: &str,
) -> bool {
    is_first_message
        && session_key.is_interactive()
        && !is_resume
        && !input.trim().is_empty()
}

/// Cap the planner objective at a generous UTF-8 char boundary. Naked-loop input
/// is raw channel text (goal/loop objectives are already tool-bounded), so a
/// multi-KB paste must not bloat the planner prompt.
fn bounded_objective(input: &str) -> String {
    const MAX_OBJECTIVE_CHARS: usize = 4000;
    match input.char_indices().nth(MAX_OBJECTIVE_CHARS) {
        Some((byte_idx, _)) => input[..byte_idx].to_string(),
        None => input.to_string(),
    }
}
```

- [ ] **Step 3: Add the spawn method** — inside the `impl<P, R> ExecutionEngine<P, R>` block in `execute.rs` (beside the other run helpers):

```rust
    /// Fire the tool-free strategic planner ONCE for a genuine human first
    /// message on the naked agent loop, concurrently with the remaining
    /// first-message setup. Returns `(store_key, handle)` the caller awaits and
    /// stores BEFORE dispatching the harness run, so the Strategy is welded on
    /// turn 1. Returns `None` (a no-op) when the planner is disabled
    /// (`planner_provider` is `None` ⇒ `[strategy].enabled`/`plan_naked_loop`
    /// off), the origin gate rejects the run, the strategy subsystem is
    /// uninitialized, or a Strategy already exists for this session (fire-once).
    fn spawn_naked_loop_planner(
        &self,
        request: &RunRequest,
        is_first_message: bool,
    ) -> Option<(String, tokio::task::JoinHandle<Option<crate::strategy::Strategy>>)> {
        let provider = self.planner_provider.clone()?;
        let is_resume = request.metadata.get("resume").map(String::as_str) == Some("true");
        if !naked_loop_planner_should_fire(
            &request.session_key,
            is_first_message,
            is_resume,
            &request.input,
        ) {
            return None;
        }
        let store = crate::strategy::global()?;
        let session_key_str = request.session_key.to_key_string();
        let key = crate::strategy::session_key(&session_key_str);
        // Fire-exactly-once: only when the slot is provably empty. `matches!`,
        // NOT `==` (anyhow::Result<Option<_>> is not Eq); a transient Err skips
        // so a read failure never risks a double-write (P7).
        if !matches!(store.get(&key), Ok(None)) {
            return None;
        }
        let objective = bounded_objective(&request.input);
        let handle = tokio::spawn(async move {
            let ctx = crate::strategy::planner::PlannerContext {
                tool_descriptions: Vec::new(),
                env_summary: crate::strategy::planner::env_summary(),
                lessons: Vec::new(),
            };
            crate::strategy::planner::plan_strategy(&provider, &objective, &ctx, None).await
        });
        Some((key, handle))
    }
```

- [ ] **Step 4: Start the planner after the first-message block** — in `ExecutionEngine::execute`, immediately after the `if is_first_message { self.publish_session_updated(...) }` block (`src/gateway/execution_engine/execute.rs:236`):

```rust
        // Naked agent-loop strategic planner (StraTA round 2): on a genuine
        // human first message, plan a Strategy concurrently with the rest of
        // first-message setup. Awaited + stored just before harness dispatch
        // (below) so StrategyLayer welds it on turn 1. A no-op otherwise.
        let naked_loop_planner = self.spawn_naked_loop_planner(&request, is_first_message);
```

- [ ] **Step 5: Await + store before dispatch** — immediately before the `// Execute the run` comment (`src/gateway/execution_engine/execute.rs:388`, before `let active_runs = ...`):

```rust
        // Join the naked-loop planner (started above, overlapped with setup) and
        // store its Strategy before harness dispatch so the weld is present on
        // turn 1. Best-effort: any failure (planner error / put error / join
        // error) leaves the prompt byte-identical and the run proceeds (P7).
        if let Some((key, handle)) = naked_loop_planner {
            if let Ok(Some(strategy)) = handle.await {
                if let Some(store) = crate::strategy::global() {
                    let _ = store.put(&key, &strategy);
                }
            }
        }
```

- [ ] **Step 6: Commit**

```bash
git add src/gateway/execution_engine/execute.rs
git commit -m "gateway: fire strategic planner on naked agent-loop first message"
```

---

### Task 8: Batched verification (the ONLY cargo step)

**Files:** none new — verifies Tasks 1-7.

> This is the single point where cargo runs (≤3 invocations), per the global cargo-frugality constraint. Fix any error inline and re-run only the failing command.

- [ ] **Step 1: Lib compile check** (covers all `alephcore` changes — Tasks 1-5, 7)

Run: `cargo check -p alephcore --lib`
Expected: `Finished` with 0 errors. Common pitfalls to check if it fails: `matches!` vs `==`; the `session_key` import in `strategy_manage.rs`; the `Default for StrategyToml` literal.

- [ ] **Step 2: Bin compile check** (covers the boot wiring — Task 6)

Run: `cargo check --bin aleph-server`
Expected: `Finished` with 0 errors. If E0382 (use-after-move): confirm `naked_loop_planner_provider` is cloned **before** `planner_provider` moves into `tool_config`.

- [ ] **Step 3: Run only the new unit tests** (filtered — not the full suite)

Run: `cargo test -p alephcore --lib is_interactive session_key resolve_key naked_loop plan_naked_loop bounded_objective`
Expected: all matched tests PASS (`is_interactive_*`, `session_key_*`, `resolve_key_*`, `gate_*`, `bounded_objective_*`, `strategy_plan_naked_loop_*`).

- [ ] **Step 4: Commit any fixes** (only if Steps 1-3 required edits)

```bash
git add -u src/
git commit -m "strategy: fix compile/test issues from naked-loop planner batch"
```

- [ ] **Step 5: Hand off E2E to the user** (no cargo run / deploy by the agent)

Report: lib + bin check clean, new unit tests green. Recommend the user run the gateway E2E from spec §10: a naked first message with a distractor (the "买酱油遇打折薯片" pattern) verifying (a) a session-keyed Strategy is welded on turn 1, (b) a trivial "hi" self-gates to no Strategy, (c) a cron/team first run does NOT plan.

---

## Self-Review

**1. Spec coverage:**
- §4 trigger gate → T1 (`is_interactive`) + T7 (`naked_loop_planner_should_fire`, origin-exclusion tests). ✓
- §5 change sites #1-#8 → T1/T2/T3(active_strategy+resolve_key)/T5/T7/T6/T4 respectively. ✓
- §6 config + boot folding → T4 + T6. ✓
- §7 lifecycle (persist, no clear), UTF-8 objective cap, double-fire tolerance → T7 (`bounded_objective`, fire-once `matches!`, no clear path added). ✓
- §8 latency (spawn-concurrent, await-before-dispatch) → T7 Steps 4-5. ✓
- §10 tests → T1/T2/T3/T4/T7 unit tests + T8 filtered run + E2E handoff. ✓
- §11 build order → Tasks 1-8 follow it. ✓
- §12 corrections (matches! not ==, to_key_string key, clone-before-move, Default impl literal, request.input objective, is_interactive whitelist) → all encoded in T3/T6/T7/T4. ✓

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; exact file:line targets; exact cargo commands with expected output. ✓

**3. Type consistency:** `is_interactive(&self) -> bool` (T1) used in T7's gate; `session_key(&str) -> String` (T2) used in T3/T7; `with_planner_provider(Option<Arc<dyn crate::providers::AiProvider>>)` (T5) called in T6; `planner_provider` field (T5) read in T7; `spawn_naked_loop_planner -> Option<(String, JoinHandle<Option<Strategy>>)>` produced/consumed within T7 Steps 3-5; `plan_strategy(&provider, &objective, &ctx, None)` matches round 1's `(&Arc<dyn AiProvider>, &str, &PlannerContext, Option<String>)`. Consistent. ✓
