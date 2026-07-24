# Multiagent V2 Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve four deferred items from the multiagent V2 integration round — make `team_delegate` interruptible (fixing a detached-member leak), lock the goal-budget single-rail invariant with a regression test, cut one dead config field, and unify model-facing timeout naming without breaking old callers.

**Architecture:** Four independent changes. Item 1 reuses the existing engine per-run cooperative cancel (the same mechanism `teams.chat.cancel` uses) by making the delegate's tracker registration carry the real engine `run_id` and extending `cancel_session` to walk a leader's in-flight children. Item 2 adds a store-backed regression test + an invariant comment. Item 3 deletes a zero-consumer `serde` field. Item 4 renames timeout fields to a canonical `timeout_seconds` with `#[serde(alias)]` on every old spelling (deserialize-only, so the model-facing schema unifies while old callers still parse).

**Tech Stack:** Rust (tokio + serde + schemars), `#[tokio::test]`, `tempfile` for store-backed tests. Baseline `main` @ `eae1539fb`.

**Spec:** `docs/superpowers/specs/2026-07-24-multiagent-v2-followups-design.md`

## Global Constraints

- **Rust MSRV 1.95**, toolchain pinned `1.96.0` (`rust-toolchain.toml`) — no `cargo +<ver>`.
- **R10 harness boundary:** none of these tasks may add lines to `src/harness/`. If a change seems to need harness edits, stop and re-scope — all four items live outside it.
- **Verification commands** (memory: `--lib` skips `#[cfg(test)]`-gated bin handler registration; `cargo check --lib` skips test code):
  - Unit tests: `cargo test -p alephcore --lib <filter>`
  - Handler/bin wiring after gateway edits (Item 1): `cargo check --bin aleph-server`
  - Lint: `cargo clippy -p alephcore -- -D warnings` (run before each commit if convenient; mandatory before the final commit of each item).
- **Commit style:** English, `<scope>: <description>`. One commit per task (the last step of each task). Work directly on `main` (project runs single-branch mode) unless the executor set up a worktree.
- **`serde(alias)` is deserialize-only** and does NOT appear in the schemars-generated JSON schema — that is exactly why it gives zero-breakage renames (Item 4): the schema shows the new primary name; old wire names still parse.
- **Anchors** are as-of `eae1539fb`; re-confirm each `file:line` with a quick read before editing (surrounding code may have shifted).

---

## Item 3 — Cut the dead `QueueMode::Followup` config field

Smallest change; do it first. `QueueMode` (`config/types/general.rs:11-21`) and `GeneralConfig.queue_mode` (`:36-39`) are defined, serialized, and JsonSchema'd but read by nobody. `GeneralConfig` does **not** derive `#[serde(deny_unknown_fields)]` (confirmed at `general.rs:28`), so removing the field lets an existing `config.toml` with `queue_mode = "..."` parse fine (unknown field silently ignored) — no tombstone needed.

### Task 1: Delete `QueueMode` + `queue_mode` (and orphaned `collect_window_ms`)

**Files:**
- Modify: `src/config/types/general.rs:10-43`

**Interfaces:**
- Produces: nothing (pure deletion). Confirms `QueueMode`, `queue_mode`, and (conditionally) `collect_window_ms` have zero remaining references.

- [ ] **Step 1: Prove zero consumers repo-wide**

Run:
```bash
cd /Volumes/TBU/Workspace/Aleph
grep -rn "QueueMode\|queue_mode\|collect_window_ms" src/ | grep -v "src/config/types/general.rs"
```
Expected: **no output** (every reference is inside `general.rs`). If any consumer prints, STOP — this is no longer a pure dead-code cut; report the consumer and hold. Also check for a re-export:
```bash
grep -rn "QueueMode" src/config/types/mod.rs src/config/mod.rs src/config/types.rs 2>/dev/null
```
Expected: no `pub use ... QueueMode` re-export (if one exists, delete it in Step 2 too).

- [ ] **Step 2: Delete the enum, the field, and the orphaned window field**

Delete `general.rs:10-21` (the doc line + the entire `QueueMode` enum). Delete the `queue_mode` field and its doc (`:36-39`). Because `collect_window_ms` (`:40-43`) exists solely to parameterize `queue_mode == collect` (per its own doc) and Step 1 confirmed it has zero consumers, delete it too — it is orphaned by this cut. Resulting `GeneralConfig` (unchanged fields shown for context):

```rust
/// General configuration settings
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct GeneralConfig {
    /// Default provider to use when no routing rule matches
    #[serde(default)]
    pub default_provider: Option<String>,
    /// Preferred language override (e.g., 'en', 'zh-Hans'). If None, use system language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Browser system configuration (profiles, SSRF policy, Playwright CLI).
    #[serde(default)]
    pub browser: crate::browser::profile::BrowserSystemConfig,
    /// Global fallback provider chain.
    /// When the default provider fails with a transient error (rate limit, timeout),
    /// these providers are tried in order. Names must match keys in [providers].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_providers: Vec<String>,
    /// Session store backend: "sqlite" (default) or "file".
    #[serde(default = "default_session_store_backend")]
    pub session_store_backend: String,
}
```

- [ ] **Step 3: Verify compilation (the deletion is the test)**

Run: `cargo check -p alephcore`
Expected: compiles clean. (A dangling reference would surface here as `cannot find type QueueMode` — that would mean Step 1's grep missed a consumer; if so, revert and report.)

- [ ] **Step 4: Verify tests + lint unaffected**

Run: `cargo test -p alephcore --lib general` then `cargo clippy -p alephcore -- -D warnings`
Expected: PASS / no warnings. No test referenced `queue_mode` (Step 1 proved it).

- [ ] **Step 5: Commit**

```bash
git add src/config/types/general.rs
git commit -m "config: remove dead QueueMode/queue_mode/collect_window_ms (R10 zero-consumer)"
```

---

## Item 4 — Unify model-facing timeout naming to `timeout_seconds` + `serde` aliases

Canonical primary name = `timeout_seconds`. Every old spelling becomes a `#[serde(alias = "...")]`. `sessions_send` and `task_wait` are already `timeout_seconds` and serve as the anchor (unchanged). Untouched: the `timeout_ms` group, the `timeout_minutes` group, and `duration*` fields. For each renamed field: rename the Rust field, add the alias, update every internal `args.<old>` read, update the doc-comment to say seconds, add a deserialize test proving both spellings parse.

> **Per-field procedure (applies to Tasks 2–5).** After renaming a field `X_secs`/`timeout` → `timeout_seconds`, run `grep -n "\.<oldfield>\b" <file>` to find internal reads and update them. The Rust field name change is compile-enforced, so `cargo check` catches any missed internal use.

### Task 2: `task_manage` — align `task_create` to `task_wait` (the sharpest divergence)

**Files:**
- Modify: `src/builtin_tools/task_manage/create.rs:66` (+ internal uses)
- Test: `src/builtin_tools/task_manage/create.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Produces: `TaskCreateArgs.timeout_seconds: Option<u32>` (was `timeout_secs`), deserialization-compatible with the legacy `timeout_secs` key via alias. `task_wait.rs` already exposes `timeout_seconds` — no change there.

- [ ] **Step 1: Write the failing deserialize test**

Add to the `#[cfg(test)]` module in `create.rs` (confirm the exact `TaskCreateArgs` field type first — the survey saw `#[serde(default)]`; match it):

```rust
#[test]
fn task_create_timeout_accepts_canonical_and_legacy_alias() {
    // New canonical spelling.
    let a: TaskCreateArgs =
        serde_json::from_value(serde_json::json!({ "subject": "x", "timeout_seconds": 45 })).unwrap();
    assert_eq!(a.timeout_seconds, Some(45));
    // Legacy spelling still parses via alias (saved calls / prompts).
    let b: TaskCreateArgs =
        serde_json::from_value(serde_json::json!({ "subject": "x", "timeout_secs": 45 })).unwrap();
    assert_eq!(b.timeout_seconds, Some(45));
}
```
(Use the minimal set of other required fields for `TaskCreateArgs`; read the struct to see which are non-`Option`/non-`default`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib task_create_timeout_accepts_canonical_and_legacy_alias`
Expected: FAIL to compile (`no field timeout_seconds on TaskCreateArgs`).

- [ ] **Step 3: Rename the field + add the alias**

At `create.rs:66`, change the field. Preserve the existing `#[serde(default)]` and doc, adding the alias and clarifying seconds:

```rust
/// Per-task wall-clock timeout in seconds (defaults to the global
/// `task_timeout_secs`). Accepts the legacy `timeout_secs` spelling.
#[serde(default, alias = "timeout_secs")]
pub timeout_seconds: Option<u32>,
```
Then update every internal read: `grep -n "\.timeout_secs\b" src/builtin_tools/task_manage/create.rs` and rename each to `.timeout_seconds`.

- [ ] **Step 4: Run test + compile**

Run: `cargo test -p alephcore --lib task_create_timeout_accepts_canonical_and_legacy_alias` then `cargo check -p alephcore`
Expected: PASS, compiles clean.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/task_manage/create.rs
git commit -m "tools: task_create timeout_secs -> timeout_seconds (serde alias, aligns task_wait)"
```

### Task 3: bare `timeout` group — `bash_exec` / `code_exec` / `code_check`

**Files:**
- Modify: `src/builtin_tools/bash_exec.rs:64-66`, `src/builtin_tools/code_exec.rs:116`, `src/builtin_tools/code_check.rs:85` (+ internal uses in each)
- Test: one `#[cfg(test)]` deserialize test per file (or a shared one in `bash_exec.rs`)

**Interfaces:**
- Produces: `BashExecArgs.timeout_seconds: Option<u64>` / `CodeExecArgs.timeout_seconds` / `CodeCheckArgs.timeout_seconds`, each `#[serde(default, alias = "timeout")]`.

- [ ] **Step 1: Write the failing test (bash_exec shown; repeat pattern for the other two)**

```rust
#[test]
fn bash_exec_timeout_accepts_canonical_and_legacy_alias() {
    let a: BashExecArgs =
        serde_json::from_value(serde_json::json!({ "cmd": "true", "timeout_seconds": 30 })).unwrap();
    assert_eq!(a.timeout_seconds, Some(30));
    let b: BashExecArgs =
        serde_json::from_value(serde_json::json!({ "cmd": "true", "timeout": 30 })).unwrap();
    assert_eq!(b.timeout_seconds, Some(30));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib bash_exec_timeout_accepts`
Expected: FAIL (`no field timeout_seconds`).

- [ ] **Step 3: Rename + alias in all three files**

`bash_exec.rs:64-66`:
```rust
/// Timeout in seconds (optional, defaults to 60). Accepts the legacy `timeout` spelling.
#[serde(default, alias = "timeout")]
pub timeout_seconds: Option<u64>,
```
Apply the identical change at `code_exec.rs:116` and `code_check.rs:85` (adjust the default value wording — code_check defaults to 120). In each file, update internal reads: `grep -n "\.timeout\b" src/builtin_tools/bash_exec.rs` (and the other two) and rename `.timeout` → `.timeout_seconds`. **Be precise:** only the timeout arg field, not unrelated `.timeout` on other types.

- [ ] **Step 4: Run tests + compile**

Run: `cargo test -p alephcore --lib timeout_accepts` then `cargo check -p alephcore`
Expected: all three PASS, compiles clean.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/bash_exec.rs src/builtin_tools/code_exec.rs src/builtin_tools/code_check.rs
git commit -m "tools: bash_exec/code_exec/code_check timeout -> timeout_seconds (serde alias)"
```

### Task 4: `timeout_secs` group — `team_delegate` + workflow step

**Files:**
- Modify: `src/builtin_tools/team/delegate.rs:45` (+ internal uses), `src/workflow/def.rs:107` (+ internal uses)
- Test: deserialize test in each file

**Interfaces:**
- Produces: `TeamDelegateArgs.timeout_seconds` and `WorkflowStepDef.timeout_seconds`, each `#[serde(default, alias = "timeout_secs")]`. **`WorkflowStepDef` is persisted in saved-workflow JSON** — the alias is what keeps old saved workflows loadable, so it is mandatory here.

- [ ] **Step 1: Write failing tests**

In `delegate.rs` tests:
```rust
#[test]
fn team_delegate_timeout_accepts_canonical_and_legacy_alias() {
    let a: TeamDelegateArgs = serde_json::from_value(
        serde_json::json!({ "agent_id": "w", "task": "t", "team_id": "tm", "timeout_seconds": 90 })).unwrap();
    assert_eq!(a.timeout_seconds, 90);   // note: field type — see Step 3
    let b: TeamDelegateArgs = serde_json::from_value(
        serde_json::json!({ "agent_id": "w", "task": "t", "team_id": "tm", "timeout_secs": 90 })).unwrap();
    assert_eq!(b.timeout_seconds, 90);
}
```
In `def.rs` tests (a saved workflow written with the old key must still load):
```rust
#[test]
fn workflow_step_timeout_accepts_legacy_alias_for_saved_workflows() {
    let step: WorkflowStepDef =
        serde_json::from_value(serde_json::json!({ "timeout_secs": 300, /* other required step fields */ })).unwrap();
    assert_eq!(step.timeout_seconds, Some(300));
}
```
(Confirm `TeamDelegateArgs.timeout_secs`'s type at `delegate.rs:45` — it has `#[serde(default = "default_timeout")]`, so it is likely a non-`Option` `u64`; match the assertion to the actual type. Fill `WorkflowStepDef`'s other required fields by reading the struct.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib workflow_step_timeout_accepts_legacy_alias timeout_accepts_canonical_and_legacy_alias`
Expected: FAIL (`no field timeout_seconds`).

- [ ] **Step 3: Rename + alias**

`delegate.rs:45` — keep the existing `default = "default_timeout"`:
```rust
/// Per-task wall-clock timeout in seconds. Accepts the legacy `timeout_secs` spelling.
#[serde(default = "default_timeout", alias = "timeout_secs")]
pub timeout_seconds: u64,
```
`def.rs:107` — keep the existing `default, skip_serializing_if`:
```rust
/// Per-step wall-clock timeout (seconds). Accepts the legacy `timeout_secs` spelling.
#[serde(default, skip_serializing_if = "Option::is_none", alias = "timeout_secs")]
pub timeout_seconds: Option<u64>,
```
Update internal reads in both files: `grep -n "\.timeout_secs\b" src/builtin_tools/team/delegate.rs src/workflow/def.rs src/workflow/compile.rs` — the workflow step's timeout is consumed during materialize (`compile.rs`), so check there too; rename each `.timeout_secs` → `.timeout_seconds`.

- [ ] **Step 4: Run tests + compile (workflow is reached through the `workflow` tool's `save`)**

Run: `cargo test -p alephcore --lib timeout_accepts` then `cargo check -p alephcore`
Expected: PASS, compiles clean.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/team/delegate.rs src/workflow/def.rs src/workflow/compile.rs
git commit -m "tools: team_delegate/workflow-step timeout_secs -> timeout_seconds (serde alias)"
```

### Task 5: `moa` — suffix-only unification (`advisor_timeout_secs` → `advisor_timeout_seconds`)

**Files:**
- Modify: `src/builtin_tools/moa_manage.rs:79-81` (enum field), `:186` (hand-written schema description string), + internal uses
- Test: deserialize test for the `set_preset` variant

**Interfaces:**
- Produces: `MoaManageArgs::SetPreset { advisor_timeout_seconds: Option<u64>, .. }`, `#[serde(default, alias = "advisor_timeout_secs")]`. The meaningful `advisor_` prefix stays; only the `_secs`→`_seconds` suffix unifies.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn moa_set_preset_advisor_timeout_accepts_canonical_and_legacy_alias() {
    let a: MoaManageArgs = serde_json::from_value(serde_json::json!({
        "action": "set_preset", "name": "p", "advisor_timeout_seconds": 120
    })).unwrap();
    match a { MoaManageArgs::SetPreset { advisor_timeout_seconds, .. } => assert_eq!(advisor_timeout_seconds, Some(120)), _ => panic!("wrong variant") }
    let b: MoaManageArgs = serde_json::from_value(serde_json::json!({
        "action": "set_preset", "name": "p", "advisor_timeout_secs": 120
    })).unwrap();
    match b { MoaManageArgs::SetPreset { advisor_timeout_seconds, .. } => assert_eq!(advisor_timeout_seconds, Some(120)), _ => panic!("wrong variant") }
}
```
(Confirm the enum's serde tagging / the exact discriminant key by reading `moa_manage.rs` around the enum definition; adjust the `"action"` tag field name accordingly.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib moa_set_preset_advisor_timeout_accepts`
Expected: FAIL.

- [ ] **Step 3: Rename field + alias, and update the hand-written schema string**

`moa_manage.rs:79-81`:
```rust
/// Per-advisor wall-clock budget in seconds. Omit for 120. Accepts the legacy `advisor_timeout_secs` spelling.
#[serde(default, alias = "advisor_timeout_secs")]
advisor_timeout_seconds: Option<u64>,
```
Then at the hand-written schema (`moa_manage.rs:186`), rename the property key emitted to the model from `advisor_timeout_secs` to `advisor_timeout_seconds` (the hand-written schema does NOT get serde aliases automatically — it must name the new primary explicitly so the model sees the unified name). Update internal reads: `grep -n "advisor_timeout_secs" src/builtin_tools/moa_manage.rs src/config/types/moa.rs` — note the config key `moa.rs:49 advisor_timeout_secs` is an internal TOML key (out of scope, leave it) unless the same struct is reused; verify they are distinct.

- [ ] **Step 4: Run test + compile**

Run: `cargo test -p alephcore --lib moa_set_preset_advisor_timeout_accepts` then `cargo check -p alephcore`
Expected: PASS, compiles clean.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p alephcore -- -D warnings
git add src/builtin_tools/moa_manage.rs
git commit -m "tools: moa advisor_timeout_secs -> advisor_timeout_seconds (serde alias)"
```

---

## Item 2 — Lock the goal-budget single-rail invariant

No behavior change: in-process subagent spend deliberately stays off the goal budget. Add the missing end-to-end `tree_tokens` regression test and a one-line invariant comment. Reuse the store builder from `session_manager/tests.rs` (`test_config` + `SessionManager::new`, whose SQLite backend implements `get_total_tokens`; seed rows with `update_session_usage`).

### Task 6: End-to-end `tree_tokens` sum test + invariant comment

**Files:**
- Modify: `src/gateway/goal_budget.rs:199` (add invariant comment) and its `#[cfg(test)]` module OR add the test in `src/gateway/session_manager/tests.rs` (chosen — the store builder + `test_config` live there)
- Test: `src/gateway/session_manager/tests.rs` (new `#[tokio::test]`)

**Interfaces:**
- Consumes: `goal_budget::tree_tokens(&Arc<dyn SessionStore>, &Goal, &SessionKey) -> Option<u64>`; `Goal::new(session_id, objective, now_total_tokens, now_ms)` (`goal/types.rs:183`); `BudgetMember { session_id, tokens_at_join }` (`goal/types.rs:162`); `SessionStore::update_session_usage(&key, input_i64, output_i64, cost_f64, model, provider)` and `get_total_tokens(&key)`.
- Produces: nothing (test + comment only).

- [ ] **Step 1: Write the failing end-to-end test**

Add to `src/gateway/session_manager/tests.rs` (mirror the store setup at `tests.rs:475 test_get_total_tokens_none_then_accumulates`). This asserts `tree_tokens == own_delta_base + Σ(member_total − tokens_at_join)` AND that an unrelated (non-enrolled) session row — standing in for an in-process subagent, which is never enrolled — contributes nothing:

```rust
#[tokio::test]
async fn goal_tree_budget_sums_own_plus_member_deltas_only() {
    use std::sync::Arc;
    use crate::gateway::session_store::SessionStore;
    use crate::gateway::goal_budget::tree_tokens;
    use crate::goal::types::{Goal, BudgetMember};

    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    // Owner session: seed 100+40 = 140 cumulative tokens.
    let own_key = SessionKey::main("leader");
    manager.get_or_create(&own_key).await.unwrap();
    manager.update_session_usage(&own_key, 100, 40, 0.0, None, None).await.unwrap();

    // Enrolled member (a delegated task session): seed 200+60 = 260; joined at 60.
    let member_key = SessionKey::task("worker", "team", "task-1");
    manager.get_or_create(&member_key).await.unwrap();
    manager.update_session_usage(&member_key, 200, 60, 0.0, None, None).await.unwrap();

    // An UNENROLLED session (stands in for an in-process subagent — never a budget member).
    let stray_key = SessionKey::task("worker", "team", "stray");
    manager.get_or_create(&stray_key).await.unwrap();
    manager.update_session_usage(&stray_key, 9_999, 9_999, 0.0, None, None).await.unwrap();

    // Goal owned by own_key; one enrolled member with tokens_at_join = 60.
    let mut goal = Goal::new(&own_key.to_key_string(), "obj", 0, 0);
    goal.token_budget = Some(10_000);
    goal.budget_members = vec![BudgetMember {
        session_id: member_key.to_key_string(),
        tokens_at_join: 60,
    }];

    let store: Arc<dyn SessionStore> = Arc::new(manager);
    let total = tree_tokens(&store, &goal, &own_key).await.expect("own total readable");

    // own(140) + member_delta(260 - 60 = 200) = 340. The stray 19_998 is absent.
    assert_eq!(total, 340, "only own row + enrolled member delta count; unenrolled spend is invisible");
}
```
(Confirm `Goal`'s field names `token_budget`/`budget_members` and `BudgetMember`'s fields against `goal/types.rs:58-169`; confirm `SessionKey::main`/`SessionKey::task` constructors — both are used elsewhere in these tests / in `runner.rs:196`. If `Arc::new(manager)` cannot coerce to `Arc<dyn SessionStore>` because `tree_tokens` needs the trait object, adjust to `Arc<dyn SessionStore> = Arc::new(manager)` as written — `SessionManager: SessionStore` holds.)

- [ ] **Step 2: Run to verify it fails (or reveals the real number)**

Run: `cargo test -p alephcore --lib goal_tree_budget_sums_own_plus_member_deltas_only -- --nocapture`
Expected: FAIL — either it doesn't compile yet (fix imports) or the assertion pins the number. If it fails on the number, DO NOT change the assertion to match; first confirm the arithmetic by hand (140 + 200 = 340) — a mismatch means a real bug in `tree_tokens`, which is exactly what this test exists to catch. Only proceed once the failure is purely "test not yet present / import wiring."

- [ ] **Step 3: Make it pass (test-wiring only — no production change expected)**

`tree_tokens` already implements the summation (`goal_budget.rs:186-218`); this test should pass once imports/constructors are correct. Adjust only the test's imports and constructor calls until green. If a genuine production bug surfaces, stop and report it — that is out of scope for a "lock the invariant" task and needs its own fix.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alephcore --lib goal_tree_budget_sums_own_plus_member_deltas_only`
Expected: PASS.

- [ ] **Step 5: Add the invariant comment at the summation point**

At `src/gateway/goal_budget.rs:199` (just before `let mut total = own;`), add:

```rust
    // INVARIANT (single accounting rail): the tree total is own row + each
    // ENROLLED member's disjoint gateway row, counted exactly once. In-process
    // subagents bill a separate child-harness counter (harness/agent.rs) that
    // touches no gateway session row and is never enrolled — so their spend is
    // absent here BY DESIGN. Do not add a second accumulation of the same
    // tokens (e.g. rolling a child's total into the owner row, or enrolling one
    // run under two keys): that is the double-count this rail is built to avoid.
    // Covered by session_manager::tests::goal_tree_budget_sums_own_plus_member_deltas_only.
```

- [ ] **Step 6: Commit**

```bash
git add src/gateway/goal_budget.rs src/gateway/session_manager/tests.rs
git commit -m "goal: lock tree-budget single-rail invariant with end-to-end test + comment"
```

---

## Item 1 — Make `team_delegate` interruptible while running

The meatiest item. Reuse the engine per-run cooperative cancel. Three tasks: (7) add a session-scoped tracker walk, (8) carry the real engine `run_id` in the delegate registration, (9) extend `cancel_session` to cancel a leader's in-flight children. The `gate.rs` interrupt-semantics simplification is **explicitly deferred** (see Deferred section) — not in this plan.

### Task 7: Add `running_runs_of_session` to `BackgroundAgentTracker`

**Files:**
- Modify: `src/agents/background_tracker.rs` (new method near `running_children_of:451` / `session_has_running:648`)
- Test: `src/agents/background_tracker.rs` `#[cfg(test)]` module (mirror `session_has_running_matches_by_root_session:921`)

**Interfaces:**
- Produces: `BackgroundAgentTracker::running_runs_of_session(&self, root_session: &str) -> Vec<String>` — request-ids of still-running registrations whose `SpawnMeta.root_session == root_session`. Consumed by Task 9.

- [ ] **Step 1: Write the failing test**

Mirror the existing root-session test at `background_tracker.rs:921`:

```rust
#[test]
fn running_runs_of_session_returns_ids_for_matching_root_session() {
    let tracker = Arc::new(BackgroundAgentTracker::new());
    let leader = "agent:leader:main";
    let _r1 = RunningRegistration::register(
        Arc::clone(&tracker), "run-A".to_string(), CancellationToken::new(),
        "member A".to_string(),
        SpawnMeta { parent_id: None, depth: 1, root_session: leader.to_string(), model: None },
    );
    let _r2 = RunningRegistration::register(
        Arc::clone(&tracker), "run-B".to_string(), CancellationToken::new(),
        "unrelated".to_string(),
        SpawnMeta { parent_id: None, depth: 1, root_session: "agent:other:main".to_string(), model: None },
    );
    let mut ids = tracker.running_runs_of_session(leader);
    ids.sort();
    assert_eq!(ids, vec!["run-A".to_string()]);
    // Dropping the guard delists it.
    drop(_r1);
    assert!(tracker.running_runs_of_session(leader).is_empty());
}
```
(Confirm `BackgroundAgentTracker::new()` / `SpawnMeta` fields against the existing test at `:921`; copy its exact constructor calls.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib running_runs_of_session_returns_ids_for_matching_root_session`
Expected: FAIL (`no method running_runs_of_session`).

- [ ] **Step 3: Implement the method**

Add next to `running_children_of` (`background_tracker.rs:451`), mirroring it but filtering on `root_session`:

```rust
/// Request-ids of still-running registrations owned by `root_session`
/// (`SpawnMeta.root_session`, the top-level session key in
/// `SessionKey::to_key_string()` form). Backs the leader-cancel walk: when a
/// leader session is cancelled, its in-flight delegated member runs are
/// enumerated here and each engine per-run token is fired. O(running) scan,
/// same as `session_has_running`. Ids that are not live engine runs (e.g.
/// in-process subagents) simply yield a harmless `cancel` miss at the seam.
#[must_use]
pub fn running_runs_of_session(&self, root_session: &str) -> Vec<String> {
    self.running
        .read()
        .unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        })
        .iter()
        .filter(|(_, agent)| agent.meta.root_session == root_session)
        .map(|(id, _)| id.clone())
        .collect()
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p alephcore --lib running_runs_of_session_returns_ids_for_matching_root_session`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agents/background_tracker.rs
git commit -m "agents: add BackgroundAgentTracker::running_runs_of_session (leader-cancel walk)"
```

### Task 8: Carry the real engine `run_id` in the delegate registration

Today `execute_member_task` mints its own `run_id` internally (`runner.rs:197`) and the delegate registration uses a *different* throwaway uuid + a placeholder token (`delegate.rs:403-404`), so nothing can address the member run for cancellation. Fix: generate the `run_id` in the caller, pass it into `execute_member_task`, and register the delegate under it.

**Files:**
- Modify: `src/teams/dispatcher/runner.rs:132-142` (signature), `:197` (use param), `:248`
- Modify: `src/builtin_tools/team/delegate.rs:400-425` (register under run_id; pass it in)
- Modify: `src/teams/dispatcher/schedule/mod.rs:312` (the other caller — pass a fresh run_id)
- Test: `src/builtin_tools/team/delegate.rs` or `runner.rs` `#[cfg(test)]` — a registration-discoverability assertion

**Interfaces:**
- Consumes: `running_runs_of_session` (Task 7).
- Produces: `execute_member_task(context, target, team_id, task_id, task_text, run_id: String, timeout_secs, isolate_workspace, model_override, think_level) -> MemberRunOutcome` — new `run_id` param inserted (position: after `task_text`, before `timeout_secs`). The delegate registration now uses this `run_id` as its tracker `request_id` and keeps `root_session = caller`.

- [ ] **Step 1: Change the `execute_member_task` signature to accept `run_id`**

At `runner.rs:132`, insert the `run_id` parameter and use it instead of minting one. Replace the internal mint at `runner.rs:197` (`let run_id = uuid::Uuid::new_v4().to_string();`) — delete that line; the value now arrives as the parameter. The `RunRequest { run_id, ... }` at `:248` then uses the parameter unchanged.

```rust
pub async fn execute_member_task(
    context: &GatewayContext,
    target: &MemberDispatchTarget,
    team_id: &str,
    task_id: &str,
    task_text: String,
    run_id: String,              // NEW: caller-supplied engine run id (so the run is addressable for cancel)
    timeout_secs: u64,
    isolate_workspace: bool,
    model_override: Option<crate::gateway::model_override::ModelOverride>,
    think_level: Option<String>,
) -> MemberRunOutcome {
```
(Leave the ACP short-circuit at `:148-167` as-is — ACP members go through their own pool; passing an unused `run_id` there is harmless.)

- [ ] **Step 2: Update the autonomous-dispatcher caller (`schedule/mod.rs:312`)**

This caller does not register the run for interactive cancel (no leader awaiting), so it just supplies a fresh id:

```rust
let outcome = execute_member_task(
    context,
    &target,
    team_id,
    &task.id,
    task_text,
    uuid::Uuid::new_v4().to_string(),   // NEW positional arg
    timeout_secs,
    isolate,
    model_override,
    think_level,
)
.await;
```
(Match the exact existing argument list at `schedule/mod.rs:312`; insert the new arg in the same position as the signature.)

- [ ] **Step 3: Update the delegate caller to register under the run_id + pass it in**

In `delegate.rs`, mint the run_id before the registration block (`:400`), use it as the tracker `request_id` (replacing the throwaway uuid at `:403`), and pass it into `execute_member_task` (`:419`). Keep `root_session = caller`. The placeholder cancel token stays (the registration API requires one), but it is no longer the cancel mechanism — engine cancel by run_id is (Task 9). Replace the doc comment at `delegate.rs:390-399` accordingly:

```rust
// W12 + delegate-interrupt: register the member run under its REAL engine
// run_id and this leader's root_session, so cancelling the leader
// (cancel_session) can enumerate this in-flight delegation via
// running_runs_of_session and fire the engine per-run token. RAII delists on
// settle. Skipped when no caller session is wired (nothing to guard).
let member_run_id = uuid::Uuid::new_v4().to_string();
let running_reg = caller_session.as_deref().map(|caller| {
    crate::agents::background_tracker::RunningRegistration::register(
        crate::agents::background_tracker::BackgroundAgentTracker::global(),
        member_run_id.clone(),
        tokio_util::sync::CancellationToken::new(),
        format!("team_delegate → {}: {}", args.agent_id, args.task),
        crate::agents::background_tracker::SpawnMeta {
            parent_id: None,
            depth: 1,
            root_session: caller.to_string(),
            model: None,
        },
    )
});

let outcome = execute_member_task(
    context,
    &target,
    &args.team_id,
    &task.id,
    args.task.clone(),
    member_run_id,               // NEW: same id the tracker registered
    // ... existing trailing args (timeout, isolate=false, model_override, think_level)
)
.await;
```
(Read `delegate.rs:419-425` for the exact trailing args and preserve them.)

- [ ] **Step 4: Verify the root_session string forms match**

`cancel_session` (Task 9) will call `running_runs_of_session(&session_key.to_key_string())`. The delegate stores `root_session = caller` where `caller = crate::tools::turn_context::current_session_key()` (`delegate.rs:301`). Confirm these produce the identical string:
```bash
grep -n "fn current_session_key" -r src/tools/turn_context* 
```
Read it: it must return the session key in `SessionKey::to_key_string()` form (the same form `session_has_running`/`root_session` documents at `background_tracker.rs:638-639`). If it returns a different form, normalize one side so they match. Note the finding in the commit body.

- [ ] **Step 5: Write a registration-discoverability test**

Prove the delegate-style registration is found by the walk (a focused, harness-free assertion):

```rust
#[test]
fn delegate_style_registration_is_discoverable_by_leader_walk() {
    let tracker = crate::agents::background_tracker::BackgroundAgentTracker::global();
    let leader = "agent:leader-disc-test:main";
    let run_id = "engine-run-xyz";
    let _reg = crate::agents::background_tracker::RunningRegistration::register(
        std::sync::Arc::clone(tracker),
        run_id.to_string(),
        tokio_util::sync::CancellationToken::new(),
        "team_delegate → worker: t".to_string(),
        crate::agents::background_tracker::SpawnMeta {
            parent_id: None, depth: 1, root_session: leader.to_string(), model: None,
        },
    );
    assert!(tracker.running_runs_of_session(leader).contains(&run_id.to_string()));
}
```
(`BackgroundAgentTracker::global()` is process-global; use a unique `leader` string to avoid cross-test bleed. If `global()` returns `&Arc<...>` vs `Arc<...>`, adjust the `Arc::clone` accordingly.)

- [ ] **Step 6: Run tests + compile the bin (signature change touches call sites)**

Run: `cargo test -p alephcore --lib delegate_style_registration_is_discoverable_by_leader_walk` then `cargo check --bin aleph-server`
Expected: PASS, bin compiles (all `execute_member_task` call sites updated).

- [ ] **Step 7: Commit**

```bash
git add src/teams/dispatcher/runner.rs src/teams/dispatcher/schedule/mod.rs src/builtin_tools/team/delegate.rs
git commit -m "teams: carry real engine run_id in delegate registration (addressable for cancel)"
```

### Task 9: Extend `cancel_session` to cancel a leader's in-flight children

**Files:**
- Modify: `src/gateway/execution_engine/engine.rs:420-435` (`cancel_session`)
- Test: `src/gateway/execution_engine/tests.rs` (integration test using the existing engine harness)

**Interfaces:**
- Consumes: `running_runs_of_session` (Task 7); the delegate registration wired in Task 8; `ExecutionEngine::cancel(run_id)` (`engine.rs:396`).
- Produces: `cancel_session` unchanged signature/return (`Result<Option<String>, ExecutionError>` = the leader's own cancelled run id), but as a side effect now also fires the engine per-run token for every in-flight child registered under the leader session. Non-engine ids (in-process subagents) yield a harmless `cancel` miss.

- [ ] **Step 1: Extend `cancel_session`**

At `engine.rs:420-435`, after resolving/cancelling the leader's own run, walk and cancel its tracker children. The child cancel happens regardless of whether an own-run target existed:

```rust
pub async fn cancel_session(
    &self,
    session_key: &crate::routing::session_key::SessionKey,
) -> Result<Option<String>, ExecutionError> {
    let target = {
        let runs = self.active_runs.read().await;
        super::steering::find_steering_target_id(&runs, "", session_key)
    };
    // Cancel the leader's own run (if any).
    let own = match target {
        Some(run_id) => {
            self.cancel(&run_id).await?;
            Some(run_id)
        }
        None => None,
    };
    // Also cancel any in-flight delegated child runs owned by this session, so a
    // /stop or chat.abort actually stops the delegation instead of leaving it
    // detached (fires the same cooperative engine per-run token; a child whose
    // id is not a live engine run yields a harmless miss).
    let children = crate::agents::background_tracker::BackgroundAgentTracker::global()
        .running_runs_of_session(&session_key.to_key_string());
    for child in children {
        let _ = self.cancel(&child).await;
    }
    Ok(own)
}
```
(Confirm `SessionKey::to_key_string()` is in scope here / the correct method name; it is the canonical string form used across the tracker.)

- [ ] **Step 2: Write the integration test (mirror the existing engine harness)**

In `execution_engine/tests.rs`, model the setup on `two_sessions_same_agent_run_in_parallel` (`tests.rs:371`) / `second_message_same_session_takes_busy_path` (`:430`) — build an engine via the `ExecutionEngine::new(...)` helper (`:321`), start a member run on a `SessionKey::task` session, register it in the tracker under the leader's `root_session` with that run's id, then call `cancel_session(&leader_key)` and assert the member run is cancelled (its session returns to idle / its result is `Cancelled`). Concrete shape:

```rust
#[tokio::test]
async fn cancel_session_cancels_in_flight_delegated_child() {
    // Build the engine + a long-enough member run exactly as the busy-path
    // tests in this file do (see two_sessions_same_agent_run_in_parallel:371
    // for ExecutionEngine::new, gate_test_agent, gate_test_request).
    let temp = tempfile::tempdir().unwrap();
    let leader_key = SessionKey::main("leader-cancel-it");
    let member_key = SessionKey::task("worker", "team", "task-cancel");
    let member_run_id = "member-run-cancel-1";

    // ... start the member run through the engine on `member_key` with run id
    //     `member_run_id` (follow the neighbouring test's request/agent setup),
    //     and register it in the tracker under the leader:
    let _reg = crate::agents::background_tracker::RunningRegistration::register(
        std::sync::Arc::clone(crate::agents::background_tracker::BackgroundAgentTracker::global()),
        member_run_id.to_string(),
        tokio_util::sync::CancellationToken::new(),
        "delegated member".to_string(),
        crate::agents::background_tracker::SpawnMeta {
            parent_id: None, depth: 1, root_session: leader_key.to_key_string(), model: None,
        },
    );

    // Cancel the leader session — should fire the member's engine per-run token.
    let _ = engine.cancel_session(&leader_key).await.unwrap();

    // Assert: the member run terminated (session back to Idle / result Cancelled).
    // Use the same state/idle assertion the busy-path tests use.
}
```
**If** driving a real long-running member run proves flaky in a unit test, the guaranteed assertion is that `cancel_session` invokes `engine.cancel(member_run_id)` for a run present in `active_runs`: start any run that registers `member_run_id` in `active_runs` (the harness at `:321`/`:371` already starts real runs), register the tracker entry above, call `cancel_session`, and assert the member run's session leaves `Running`. Keep the assertion the same kind the neighbouring tests already make so it is non-flaky. Do not leave this test `#[ignore]`d — if the harness cannot express it, downgrade to asserting `running_runs_of_session(leader)` selected `member_run_id` and that a direct `engine.cancel(member_run_id)` returns `Ok` (proving the two halves the walk composes), and say so in the commit body.

- [ ] **Step 3: Run to verify failure (before Step 1 is in place) / then pass**

Run: `cargo test -p alephcore --lib cancel_session_cancels_in_flight_delegated_child`
Expected: PASS after Step 1. (If you wrote the test first per TDD, it fails before Step 1 because the child is never cancelled.)

- [ ] **Step 4: Full-item verification**

Run:
```bash
cargo test -p alephcore --lib background_tracker
cargo test -p alephcore --lib execution_engine
cargo check --bin aleph-server
cargo clippy -p alephcore -- -D warnings
```
Expected: all green, no warnings. Regression check: existing group-chat cancel tests (`teams`/`canvas`) and busy-path tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/engine.rs src/gateway/execution_engine/tests.rs
git commit -m "gateway: cancel_session cancels in-flight delegated children (fix detached-member leak)"
```

---

## Deferred / Non-goals (do NOT implement here)

- **Item 1 secondary — `gate.rs` interrupt-semantics simplification.** Once delegate is cancellable, `steering::session_is_interruptible` (`steering.rs:109-121`) / the `gate.rs:116-211` demote-to-queue guard *could* switch from "demote to queue" to "actually cancel." That changes interrupt UX and carries its own risk; it is a separate decision recorded in the spec, not this plan.
- **Item 2 — wiring subagent spend into the goal budget.** Explicitly not done; the invariant comment forbids it without a dedup design.
- **Item 3 — building residency / fork_turns / AgentPath / persistent spawn topology.** Never defined, no demand; leave unbuilt.
- **Item 4 — `timeout_ms` / `timeout_minutes` / `duration*` groups** and internal-only config keys (e.g. `[team_broadcast] member_run_timeout_secs`, `[moa] advisor_timeout_secs`): out of scope (unit difference is legitimate; config keys are not model-facing).

## Self-Review

- **Spec coverage:** Item 1 → Tasks 7–9 (+ deferred gate note). Item 2 → Task 6. Item 3 → Task 1. Item 4 → Tasks 2–5 (task_manage / bash-code / delegate-workflow / moa cover all four seconds-spellings; sessions_send + task_wait already canonical, noted). All spec sections mapped.
- **Type consistency:** `execute_member_task` gains `run_id: String` (Task 8) — both callers (`delegate.rs`, `schedule/mod.rs`) updated in the same task. `running_runs_of_session(&str) -> Vec<String>` defined in Task 7, consumed in Task 9. Renamed field is `timeout_seconds` everywhere (Tasks 2–4); `advisor_timeout_seconds` for moa (Task 5). `tree_tokens`/`Goal`/`BudgetMember` signatures quoted from source.
- **Placeholder scan:** the only conditional is Task 9 Step 2's harness fallback, which gives a concrete guaranteed-writable assertion rather than a TODO. Field-type confirmations (e.g. `TaskCreateArgs.timeout` u32 vs `Option`) are flagged as "read the struct" because the exact type must match the source — not a design gap.
