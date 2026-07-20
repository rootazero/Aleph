# Team StraTA Strategic Coordination — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On the user's first message to a team, the leader runs first and plans, the leader-minted team strategy is welded into every member's prompt, and members carry an obey/accept/submit contract — the StraTA "plan-first, weld-forever" half, with NO coord_task/verification work (that is Phase 2).

**Architecture:** Mirror the existing round-1 strategy weld. A new `team_key(team_id)` row in the existing `StrategyStore`; a fire-once team planner inside `GroupChatBroadcaster::dispatch_user` (atomic `put_if_absent`); a team branch in `active_strategy` so the two existing weld layers pick the team plan up with zero new layer code; a structural hard gate in the pure `resolve_targets`; and a stronger member/leader prompt contract (resurrecting the dead `leader_prompt::build`). All routing is deterministic plumbing; all judgment stays in the LLMs (R7/R9/R10).

**Tech Stack:** Rust, tokio, rusqlite (SQLite), serde, schemars. No new dependencies.

## Global Constraints

- **Redlines:** R7/R9 (no deterministic "chit-chat vs work / is-it-done" classifier — the hard gate inspects only a boolean; all judgment is LLM). R10 (`src/harness/` is NOT touched; routing/state are plumbing). R4 (planner fires in the broadcaster, not the RPC handler).
- **Cargo frugality (project rule — overrides standard per-step TDD):** Do **NOT** run `cargo` per step. Author each test test-first (it documents intent and runs in CI), write the implementation, commit. Run exactly **one** `cargo check -p alephcore --lib` at the very end (Task 8). The "verify it fails / passes" loop is deferred to that single gate. This is a deliberate deviation from default TDD, mandated by the project.
- **Arc aliasing:** `src/teams/broadcast/mod.rs` and `src/strategy/mod.rs` import `crate::sync_primitives::Arc`, which aliases `std::sync::Arc` in every non-`loom` build (the only build that runs the planner). `planner.rs` uses `std::sync::Arc`. They are the same type in production — passing a broadcaster-held provider to `plan_strategy` matches the existing `ExecutionEngine` precedent. Do not "fix" this with conversions.
- **Style:** rustfmt (4-space, 100 col), `snake_case`/`PascalCase`/`SCREAMING_SNAKE_CASE`, no `unwrap()` outside tests, preserve the existing Chinese prompt strings byte-for-byte when editing.
- **Branch:** project does single-branch dev on `main`. Commit per task. (The plan file lives under `docs/superpowers/` which is git-ignored — code commits are separate.)
- **Config gate folds into the provider:** when `[strategy].enabled=false` OR `[strategy].plan_team=false`, the broadcaster receives `planner_provider = None`, so Seam C and the hard gate are dormant and group chat behaves exactly as today.

---

## Task 1: Config — `StrategyToml.plan_team` switch (Seam Cfg)

**Files:**
- Modify: `src/config/types/phase6_wiring.rs:217-258` (struct + default fns + Default impl)

**Interfaces:**
- Produces: `StrategyToml.plan_team: bool` (serde default `true`); `fn strategy_plan_team_default() -> bool`.

- [ ] **Step 1: Add the failing test** — append to the `#[cfg(test)]` module in `src/config/types/phase6_wiring.rs` (create one at end of file if none exists):

```rust
#[cfg(test)]
mod plan_team_tests {
    use super::StrategyToml;

    #[test]
    fn plan_team_defaults_on() {
        assert!(StrategyToml::default().plan_team, "Default plan_team must be true");
        let from_empty: StrategyToml = serde_json::from_str("{}").unwrap();
        assert!(from_empty.plan_team, "missing plan_team deserializes to true");
    }
}
```

- [ ] **Step 2: Add the field** — in `pub struct StrategyToml`, immediately after the `plan_naked_loop` field (the verbatim current block ends at the `pub plan_naked_loop: bool,` line), insert:

```rust
    /// Whether the strategic planner also fires for a TEAM group-chat first
    /// message (strategy round 2 → teams). Default **true** (gated under
    /// `enabled`). Set `false` to keep team group chat exactly as before — no
    /// leader-first planning, no welded team strategy, byte-identical prompts.
    #[serde(default = "strategy_plan_team_default")]
    pub plan_team: bool,
```

- [ ] **Step 3: Add the serde default fn** — immediately after the existing `fn strategy_plan_naked_loop_default() -> bool { true }`:

```rust
/// serde default for `StrategyToml::plan_team` — team planning is on unless an
/// operator explicitly flips it off.
fn strategy_plan_team_default() -> bool {
    true
}
```

- [ ] **Step 4: Extend the Default impl** — in `impl Default for StrategyToml`, add the field after `plan_naked_loop`:

```rust
            plan_naked_loop: strategy_plan_naked_loop_default(),
            plan_team: strategy_plan_team_default(),
```

- [ ] **Step 5: Commit**

```bash
git add src/config/types/phase6_wiring.rs
git commit -m "strategy: add [strategy] plan_team config switch (default on)"
```

---

## Task 2: Strategy key — `team_key` (Seam A.1)

**Files:**
- Modify: `src/strategy/mod.rs` (add fn after `session_key` at ~:47; add test to the `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub fn team_key(team_id: &str) -> String` → `"team:{team_id}"`.

- [ ] **Step 1: Add the failing test** — in `src/strategy/mod.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn team_key_is_team_prefixed() {
        assert_eq!(super::team_key("squad-1"), "team:squad-1");
    }
```

- [ ] **Step 2: Add the constructor** — directly after the `session_key` fn (closing brace at ~:47):

```rust
/// Composite-key prefix for a TEAM group-chat strategy, keyed by team (a team
/// strategy is team-wide, not per-member-session — mirrors `workflow_key`).
/// Resolved in `active_strategy` BETWEEN `loop_key` and `session_key`: a
/// member's own `/goal` or `/loop` strategy still wins, but the leader's team
/// frame beats a bare session strategy. Callers MUST pass the NORMALIZED team
/// id (the form `SessionKey::task` stores in a `team_chat` key) so the planner
/// write and the weld read hit the same row.
#[must_use]
pub fn team_key(team_id: &str) -> String {
    format!("team:{team_id}")
}
```

- [ ] **Step 3: Commit**

```bash
git add src/strategy/mod.rs
git commit -m "strategy: add team_key(team_id) composite key for team-chat strategy"
```

---

## Task 3: Strategy store — atomic `put_if_absent` (Seam A.2)

**Files:**
- Modify: `src/strategy/store.rs` (add method after `put`; add test to its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `StrategyStore::open`, `lock()`, `Strategy` (all existing).
- Produces: `pub fn put_if_absent(&self, key: &str, strategy: &Strategy) -> anyhow::Result<bool>` — `true` iff this call inserted.

- [ ] **Step 1: Add the failing test** — in `src/strategy/store.rs`'s `#[cfg(test)] mod tests` (it already uses `tempfile`/`StrategyStore`; mirror the existing setup), add:

```rust
    #[test]
    fn put_if_absent_inserts_once_then_no_ops() {
        let dir = tempfile::tempdir().unwrap();
        let store = StrategyStore::open(&dir.path().join("s.db")).unwrap();
        let s1 = Strategy {
            objective: "first".into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["avoid X".into()],
            success_criteria: "done".into(),
            goal_id: None,
        };
        let s2 = Strategy { objective: "second".into(), ..s1.clone() };
        assert!(store.put_if_absent("team:t1", &s1).unwrap(), "first call inserts");
        assert!(!store.put_if_absent("team:t1", &s2).unwrap(), "second call is a no-op");
        assert_eq!(
            store.get("team:t1").unwrap().unwrap().objective,
            "first",
            "the original row is preserved (NOT upserted)"
        );
    }
```

- [ ] **Step 2: Add the method** — in `impl StrategyStore`, directly after the `put` method (after its closing brace at ~:62):

```rust
    /// Insert the strategy for `key` ONLY if no row exists yet, atomically.
    /// Returns `true` when this call inserted the row, `false` when a row was
    /// already present (left untouched). Unlike `put` (which upserts), this is
    /// the race-safe fire-once primitive for the team planner: two concurrent
    /// first messages both reach here, but exactly one inserts — `put`'s
    /// last-write-wins would otherwise let both pay for + store a plan.
    pub fn put_if_absent(&self, key: &str, strategy: &Strategy) -> anyhow::Result<bool> {
        let json = serde_json::to_string(strategy)
            .map_err(|e| AlephError::other(format!("strategy serialize: {e}")))?;
        let rows = self
            .lock()
            .execute(
                "INSERT INTO strategies (key, json) VALUES (?1, ?2)
                 ON CONFLICT(key) DO NOTHING",
                rusqlite::params![key, json],
            )
            .map_err(|e| AlephError::other(format!("strategy put_if_absent: {e}")))?;
        Ok(rows == 1)
    }
```

- [ ] **Step 3: Commit**

```bash
git add src/strategy/store.rs
git commit -m "strategy: add atomic put_if_absent for race-safe team-planner fire-once"
```

---

## Task 4: Weld resolution — team tier in `active_strategy` (Seam B)

**Files:**
- Modify: `src/orchestrator/harness_bridge/context_blocks.rs:54-70` (extract a sync resolver + add team tier; the existing `#[cfg(test)]` block starts at :178)

**Interfaces:**
- Consumes: `crate::strategy::{global, goal_key, loop_key, session_key, team_key, StrategyStore, Strategy}`, `crate::routing::session_key::SessionKey`.
- Produces: unchanged public `pub async fn active_strategy(session_key: &str) -> Option<Strategy>`; new private `fn resolve_active_strategy(store: &StrategyStore, session_key: &str) -> Option<Strategy>` (unit-testable without the process-global).

- [ ] **Step 1: Add the failing test** — in the `#[cfg(test)]` module at `context_blocks.rs:178`, add (uses `tempfile`; if the module lacks `use super::*;` add it):

```rust
    fn mk_strategy(objective: &str) -> crate::strategy::Strategy {
        crate::strategy::Strategy {
            objective: objective.into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["g".into()],
            success_criteria: "s".into(),
            goal_id: None,
        }
    }

    #[test]
    fn resolve_active_strategy_team_tier_and_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::strategy::StrategyStore::open(&dir.path().join("s.db")).unwrap();
        // A team-chat member session key: agent:alice:team_chat:squad
        let sk = crate::routing::session_key::SessionKey::task("alice", "team_chat", "squad")
            .to_key_string();

        // team tier resolves the team-wide row
        store.put(&crate::strategy::team_key("squad"), &mk_strategy("team-obj")).unwrap();
        assert_eq!(
            resolve_active_strategy(&store, &sk).map(|s| s.objective),
            Some("team-obj".to_string())
        );

        // a member's own /goal still wins over the team frame
        store.put(&crate::strategy::goal_key(&sk), &mk_strategy("goal-obj")).unwrap();
        assert_eq!(
            resolve_active_strategy(&store, &sk).map(|s| s.objective),
            Some("goal-obj".to_string())
        );

        // a non-team session never hits the team tier
        assert!(resolve_active_strategy(&store, "agent:bob:main").is_none());
    }
```

- [ ] **Step 2: Replace `active_strategy`** — replace the entire current `pub async fn active_strategy(...) { ... }` body (`context_blocks.rs:54-70`) with the thin async wrapper plus the new sync resolver carrying the team tier:

```rust
pub async fn active_strategy(session_key: &str) -> Option<crate::strategy::Strategy> {
    let store = crate::strategy::global()?;
    resolve_active_strategy(&store, session_key)
}

/// Resolve the welded Strategy for a session key against an explicit store
/// (sync; the global accessor lives in `active_strategy`, keeping this
/// unit-testable). Precedence: goal → loop → **team** → session. The team tier
/// fires only for a `team_chat` Task key — it recovers the (already-normalized)
/// team id and reads the leader-minted team-wide row, so every member welds the
/// same plan (strategy round 2). Above `session_key` so a member's own
/// `/goal`/`/loop` still wins; below `loop_key` for the same reason.
fn resolve_active_strategy(
    store: &crate::strategy::StrategyStore,
    session_key: &str,
) -> Option<crate::strategy::Strategy> {
    if let Some(s) = store.get(&crate::strategy::goal_key(session_key)).ok().flatten() {
        return Some(s);
    }
    if let Some(s) = store.get(&crate::strategy::loop_key(session_key)).ok().flatten() {
        return Some(s);
    }
    if let Some(crate::routing::session_key::SessionKey::Task { task_type, task_id, .. }) =
        crate::routing::session_key::SessionKey::parse(session_key)
    {
        if task_type == "team_chat" {
            if let Some(s) = store.get(&crate::strategy::team_key(&task_id)).ok().flatten() {
                return Some(s);
            }
        }
    }
    store
        .get(&crate::strategy::session_key(session_key))
        .ok()
        .flatten()
}
```

- [ ] **Step 3: Commit**

```bash
git add src/orchestrator/harness_bridge/context_blocks.rs
git commit -m "harness_bridge: resolve team-tier strategy in active_strategy (goal>loop>team>session)"
```

---

## Task 5: Hard gate — `resolve_targets` slot + dispatch threading (Seam B-route, mechanism only)

This task lands the structural hard gate and threads a `leader_first` flag through `dispatch`, but leaves it **off** (`dispatch_user` passes `false`). Task 7 activates it by computing the real value from the planner fire. This keeps each task a clean compile unit.

**Files:**
- Modify: `src/teams/broadcast/targets.rs:19-66` (signature + top-of-fn gate; update 6 existing tests + add 2)
- Modify: `src/teams/broadcast/mod.rs:107-117,134-140` (`dispatch` gains a `leader_first: bool` param; `dispatch_user` passes `false`; the `run_member` recursion passes `false`)

**Interfaces:**
- Produces: `pub fn resolve_targets(content, sender, leader_id, roster, user_triggered, leader_first: bool) -> Vec<String>`; `dispatch(self, team_id, content, sender, chain_depth, user_triggered, leader_first, budget)`.

- [ ] **Step 1: Update + add the failing tests** — in `src/teams/broadcast/targets.rs`'s `#[cfg(test)] mod tests`, append a `false` argument to all 6 existing `resolve_targets(...)` calls (lines 72/78/89/95/104/110 — the new last arg `leader_first=false` preserves their current expectations), then add:

```rust
    #[test]
    fn leader_first_overrides_explicit_mention() {
        // hard gate ON + user message that @-named alice → still routes to leader only
        let t = resolve_targets("@alice 看下", "user", "leader", &roster(), true, true);
        assert_eq!(t, vec!["leader".to_string()], "leader_first ignores the user @");
    }

    #[test]
    fn leader_first_inactive_keeps_normal_routing() {
        // hard gate OFF → existing behavior (alice gets it)
        let t = resolve_targets("@alice 看下", "user", "leader", &roster(), true, false);
        assert_eq!(t, vec!["alice".to_string()]);
    }
```

- [ ] **Step 2: Add the gate to `resolve_targets`** — change the signature and insert the gate as the first statement of the body:

```rust
pub fn resolve_targets(
    content: &str,
    sender: &str,
    leader_id: &str,
    roster: &[String],
    user_triggered: bool,
    leader_first: bool,
) -> Vec<String> {
    // Hard gate (strategy round 2): on the user's first message to a team while
    // the leader has just minted a plan, route ONLY to the leader so it
    // decomposes + assigns first — even if the user @-named a member. Purely
    // structural (a boolean), zero content inspection (R7). Once a plan exists
    // `leader_first` is false and the equal-broadcast below resumes.
    if user_triggered && leader_first {
        let leader = leader_id.to_string();
        return if leader != sender { vec![leader] } else { Vec::new() };
    }

    let mentions = extract_mentions(content);
    // ... rest of the existing body unchanged ...
```

(Leave everything from `let mentions = extract_mentions(content);` onward exactly as it is today.)

- [ ] **Step 3: Thread `leader_first` through `dispatch`** — in `src/teams/broadcast/mod.rs`:

  (a) `dispatch` signature (after `user_triggered: bool,`):

```rust
        user_triggered: bool,
        leader_first: bool,
        budget: Arc<AtomicUsize>,
```

  (b) the `resolve_targets` call at :134 — add `leader_first`:

```rust
            let targets = targets::resolve_targets(
                &content,
                &sender,
                &team.leader_id,
                &roster_ids,
                user_triggered,
                leader_first,
            );
```

  (c) surface the discarded `@` once when the gate overrides an explicit mention — immediately after the `let targets = ...` block and before `if targets.is_empty()`, add:

```rust
            if leader_first && !targets::extract_mention_present(&content) {
                // no-op marker; see (d)
            }
```

  Actually use this exact block (so the user learns why their `@` was bypassed), placed right after the `resolve_targets` call:

```rust
            if leader_first && user_triggered && content.contains('@') {
                self.post_system(
                    &team_id,
                    "已交由 leader 统筹:先规划任务分配,再分派给成员。",
                )
                .await;
            }
```

  (d) the recursion call inside `run_member` (broadcast/mod.rs:298) passes `false`:

```rust
        self.dispatch(team_id, reply, agent_id, chain_depth + 1, false, false, budget)
            .await;
```

  (e) `dispatch_user` (:95) passes `false` for now (Task 7 replaces this):

```rust
        self.clone()
            .dispatch(
                team_id,
                content,
                RESERVED_USER_HANDLE.to_string(),
                0,
                true,
                false,
                budget,
            )
            .await;
```

- [ ] **Step 4: Commit**

```bash
git add src/teams/broadcast/targets.rs src/teams/broadcast/mod.rs
git commit -m "teams: add structural leader-first hard gate to resolve_targets (dormant until wired)"
```

---

## Task 6: Prompt contracts — leader frame + member obey-contract (Seam D)

**Files:**
- Modify: `src/teams/broadcast/member_prompt.rs:8-79` (enlarge `build_member_input`; resurrect `leader_prompt::build`; add member obey-contract; update its 2 tests)
- Modify: `src/teams/broadcast/mod.rs:197-230,174-182` (thread `team_name`/`protocol`/`user_request` into `run_member` and its spawn)

**Interfaces:**
- Consumes: `crate::teams::leader_prompt::build(team_id, team_name, roster, protocol: Option<&str>, user_request) -> String`; `Team.name`, `Team.protocol`.
- Produces: `pub fn build_member_input(team_id, agent_id, role, roster, transcript, is_leader, team_name: &str, protocol: Option<&str>, user_request: &str) -> String`.

- [ ] **Step 1: Update + add the failing tests** — replace the two existing tests in `member_prompt.rs` (they call `build_member_input` with 6 args) with versions passing the 3 new args, and assert the new contracts:

```rust
    #[test]
    fn member_prompt_has_identity_and_obey_contract() {
        let out = build_member_input(
            "team-xyz", "alice", "researcher",
            "bob (writer), leader (leader)",
            "[user]: @alice 查下 X",
            false,
            "Squad", None, "查下 X",
        );
        assert!(out.contains("alice"));
        assert!(out.contains("team-xyz"));
        assert!(out.contains("[user]: @alice 查下 X"));
        assert!(out.contains("团队纪律"), "member gets the obey contract");
        assert!(!out.contains("你是团队「Squad」的 leader"), "member has no leader contract");
    }

    #[test]
    fn leader_prompt_uses_strong_orchestration_contract() {
        let out = build_member_input(
            "team-xyz", "leader", "leader",
            "alice (researcher)",
            "[user]: 这事谁跟进",
            true,
            "Squad", Some("Be concise"), "做个调研",
        );
        assert!(out.contains("你是团队「Squad」的 leader"), "leader contract present");
        assert!(out.contains("task_create"), "leader told to decompose with task_create");
        assert!(out.contains("不要自己闷头做完"), "anti-pattern present");
        assert!(out.contains("做个调研"), "user request surfaced to leader");
    }
```

- [ ] **Step 2: Enlarge `build_member_input`** — replace the current fn (member_prompt.rs:8-40) with:

```rust
/// 组装被唤醒 agent 的 run 输入。leader 用强编排契约(`leader_prompt::build`),
/// 普通成员用服从契约(接单/完成/交回 leader,而非只闲聊)。R7/R9:领导力与
/// 收敛压力都在 prompt 身份里,不靠代码强制。无 IO,host 可测。
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_member_input(
    team_id: &str,
    agent_id: &str,
    role: &str,
    roster: &str,
    transcript: &str,
    is_leader: bool,
    team_name: &str,
    protocol: Option<&str>,
    user_request: &str,
) -> String {
    let leader_block = if is_leader {
        format!(
            "\n\n{}",
            crate::teams::leader_prompt::build(team_id, team_name, roster, protocol, user_request)
        )
    } else {
        "\n\n团队纪律:你在 leader 的统筹下工作。当 leader 通过 @ 或任务把活派给你时,\
         优先接下并尽力完成,把产出交回 leader,而不是只在群里闲聊。你仍可自由 @ 其他\
         成员协作,但讨论要服务于把任务做完。"
            .to_string()
    };
    format!(
        "你是团队群聊里的成员 `{agent_id}`({role}),team_id: `{team_id}`。\n\
         群成员名册:{roster}。{leader_block}\n\n\
         下面是群聊记录(每行 `[发言人]: 内容`):\n{transcript}\n\n\
         请以你的身份在群里回应。约定:\n\
         - 要不要发言、说什么由你判断;与你无关可以简短跳过。\n\
         - 想让某成员接话,在回复里 `@<agent_id>`(用名册里的 id);`@all` 叫全员。\n\
         - 调任何团队工具(task_create / team_delegate / team_status 等)时,team_id 必须填 `{team_id}`。\n\
         - 不要 @ 自己,也不要 @ user。"
    )
}
```

- [ ] **Step 3: Thread the 3 new args through `run_member`** — in `src/teams/broadcast/mod.rs`:

  (a) `run_member` signature (after `roster_label: String,`):

```rust
        roster_label: String,
        team_name: String,
        protocol: Option<String>,
        user_request: String,
        chain_depth: u32,
        budget: Arc<AtomicUsize>,
```

  (b) the `build_member_input` call at :223:

```rust
        let input = member_prompt::build_member_input(
            &team_id,
            &agent_id,
            &role,
            &roster_label,
            &transcript,
            is_leader,
            &team_name,
            protocol.as_deref(),
            &user_request,
        );
```

  (c) the `tokio::spawn(this.run_member(...))` at :174-182 — add the three new args, cloning team fields per-iteration (right where `let leader_id = team.leader_id.clone();` already is, add `let team_name = team.name.clone();` and `let protocol = team.protocol.clone();` and `let user_request = content.clone();`), then:

```rust
                handles.push(tokio::spawn(this.run_member(
                    team_id_spawn,
                    agent_id,
                    role,
                    leader_id,
                    roster_label,
                    team_name,
                    protocol,
                    user_request,
                    chain_depth,
                    budget.clone(),
                )));
```

  (Note: `user_request = content.clone()` is the current trigger message; on the leader-first turn it is the user's message — exactly what the leader contract wants. On later agent-reply recursions it is that reply; acceptable for Phase 1.)

- [ ] **Step 4: Commit**

```bash
git add src/teams/broadcast/member_prompt.rs src/teams/broadcast/mod.rs
git commit -m "teams: wire leader_prompt::build leader frame + member obey-contract into group chat"
```

---

## Task 7: Team planner fire + provider plumbing (Seam C — activates the gate)

This is one atomic compile unit: the broadcaster gains a planner provider, fires a fire-once team planner in `dispatch_user`, and the provider is threaded handler→broadcaster and gated at boot. Activating the planner also activates the Task-5 hard gate (`dispatch_user` now passes the real `leader_first`).

**Files:**
- Modify: `src/teams/broadcast/mod.rs` (struct field + `new` + `dispatch_user` + new `maybe_plan_team_strategy` + pure `build_team_planner_objective`)
- Modify: `src/gateway/handlers/teams/canvas.rs:266-272,369-373` (new `handle_chat_send` param; pass to `GroupChatBroadcaster::new`)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:~403,1399-1423` (build `team_planner_provider`; capture + pass)

**Interfaces:**
- Consumes: `crate::strategy::{global, team_key}`, `crate::strategy::store::StrategyStore::put_if_absent` (Task 3), `crate::strategy::planner::{PlannerContext, plan_strategy, env_summary}`, `crate::routing::session_key::normalize_agent_id`.
- Produces: `GroupChatBroadcaster::new(ctx, team_store, msg_store, planner_provider: Option<Arc<dyn crate::providers::AiProvider>>)`; `handle_chat_send(..., team_planner_provider: Option<Arc<dyn AiProvider>>, event_bus)`.

- [ ] **Step 1: Add the failing test for the pure objective helper** — in `src/teams/broadcast/mod.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn team_planner_objective_includes_request_and_roster() {
        let o = super::build_team_planner_objective("做个市场调研", "alice (researcher), bob (writer)");
        assert!(o.contains("做个市场调研"));
        assert!(o.contains("alice (researcher), bob (writer)"));
    }
```

- [ ] **Step 2: Add the pure helper** — near the top-level helpers in `src/teams/broadcast/mod.rs` (e.g. just below `member_run_metadata`):

```rust
/// Fold the user's request + the team roster into the planner objective. The
/// planner is tool-free; the "capability surface" it reasons about for a team
/// is the member roster, so we surface it here (PlannerContext has no roster
/// field). Pure / host-testable.
#[must_use]
fn build_team_planner_objective(user_request: &str, roster_label: &str) -> String {
    format!(
        "你在为一个 agent 团队制定整体战略,团队要完成下面的用户请求。\n\
         团队成员名册(agent_id (role)):{roster_label}\n\
         用户请求:{user_request}"
    )
}
```

- [ ] **Step 3: Add the planner-provider field + constructor arg** — change the struct and `new` (broadcast/mod.rs:68-87):

```rust
#[derive(Clone)]
pub struct GroupChatBroadcaster {
    ctx: Arc<GatewayContext>,
    team_store: Arc<dyn TeamStore>,
    msg_store: Arc<dyn MessageStore>,
    /// Strategic-planner provider for the StraTA team planner (round 2). `None`
    /// (default config off, or `[strategy].plan_team=false`) keeps the
    /// first-message team planner + leader-first hard gate dormant — group chat
    /// behaves exactly as before. Gated to `Some` at boot in agent_init.
    planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
}

impl GroupChatBroadcaster {
    #[must_use]
    pub fn new(
        ctx: Arc<GatewayContext>,
        team_store: Arc<dyn TeamStore>,
        msg_store: Arc<dyn MessageStore>,
        planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    ) -> Self {
        Self {
            ctx,
            team_store,
            msg_store,
            planner_provider,
        }
    }
```

- [ ] **Step 4: Add `maybe_plan_team_strategy` + wire `dispatch_user`** — add the method (e.g. directly above `dispatch_user`) and replace `dispatch_user`'s body:

```rust
    /// Fire-once team planner. Returns `true` iff THIS call minted + stored a
    /// non-empty team Strategy (⇒ the leader should run first this turn). Returns
    /// `false` when the planner is disabled, the slot is already filled, or the
    /// plan self-gated to nothing. Fail-soft throughout (P7): any miss ⇒ false ⇒
    /// today's equal-broadcast. Plumbing only — the strategy CONTENT is the LLM's.
    async fn maybe_plan_team_strategy(&self, team_id: &str, content: &str) -> bool {
        let Some(provider) = self.planner_provider.clone() else {
            return false;
        };
        let Some(store) = crate::strategy::global() else {
            return false;
        };
        let norm = crate::routing::session_key::normalize_agent_id(team_id);
        let key = crate::strategy::team_key(&norm);
        // Cheap fast-path: already planned ⇒ no paid LLM call, no leader-first.
        if store.get(&key).ok().flatten().is_some() {
            return false;
        }
        let members = self
            .team_store
            .get_members(team_id)
            .await
            .unwrap_or_default();
        let roster_label = members
            .iter()
            .map(|m| format!("{} ({})", m.agent_id, m.role))
            .collect::<Vec<_>>()
            .join(", ");
        let objective = build_team_planner_objective(content, &roster_label);
        let ctx = crate::strategy::planner::PlannerContext {
            tool_descriptions: Vec::new(),
            env_summary: crate::strategy::planner::env_summary(),
            lessons: Vec::new(),
        };
        let Some(strategy) =
            crate::strategy::planner::plan_strategy(&provider, &objective, &ctx, None).await
        else {
            return false;
        };
        // Atomic guard against concurrent first messages: only the winner stores
        // (and reports leader_first); the loser's plan is discarded.
        store.put_if_absent(&key, &strategy).unwrap_or(false)
    }

    /// 入口:用户消息触发(没@时 leader 兜底)。假定 user 消息已由调用方存进 `msg_store`。
    pub async fn dispatch_user(&self, team_id: String, content: String) {
        let leader_first = self.maybe_plan_team_strategy(&team_id, &content).await;
        let budget = Arc::new(AtomicUsize::new(0));
        self.clone()
            .dispatch(
                team_id,
                content,
                RESERVED_USER_HANDLE.to_string(),
                0,
                true,
                leader_first,
                budget,
            )
            .await;
    }
```

- [ ] **Step 5: Add the `handle_chat_send` param + pass it to `new`** — in `src/gateway/handlers/teams/canvas.rs`:

  (a) signature (insert after the `topic_provider` param at :271):

```rust
    topic_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    team_planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
```

  (b) the `GroupChatBroadcaster::new(...)` call at :369-373:

```rust
    let broadcaster = crate::teams::broadcast::GroupChatBroadcaster::new(
        Arc::clone(&context),
        Arc::clone(&store),
        Arc::clone(&msg_store),
        team_planner_provider,
    );
```

- [ ] **Step 6: Build + thread `team_planner_provider` at boot** — in `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`:

  (a) directly after the `naked_loop_planner_provider` let (ends ~:403, BEFORE `planner_provider` is moved into `tool_config` at :466), add:

```rust
        // Round 2 (teams): the team-chat planner provider, gated additionally on
        // [strategy].plan_team (default true). `None` ⇒ the first-message team
        // planner + leader-first hard gate in the broadcaster stay dormant.
        // Cloned BEFORE planner_provider is moved into tool_config (E0382).
        // `enabled` already folded in (planner_provider is None when disabled).
        let team_planner_provider = if app_config
            .strategy
            .as_ref()
            .map_or(true, |s| s.plan_team)
        {
            planner_provider.clone()
        } else {
            None
        };
```

  (b) at the `teams.chat.send` registration (:1394-1423), add a capture beside `chat_topic_provider` and pass it into `handle_chat_send`:

```rust
                let chat_topic_provider: Option<Arc<dyn alephcore::providers::AiProvider>> =
                    topic_provider_registry
                        .get("haiku")
                        .or_else(|| Some(topic_provider_registry.default_provider()));
                let chat_team_planner = team_planner_provider.clone();
                let chat_event_bus = event_bus.clone();
                server
                    .handlers_mut()
                    .register("teams.chat.send", move |req| {
                        let store = ts.clone();
                        let ctx = chat_ctx.clone();
                        let msg_store = chat_msg_store.clone();
                        let provider = chat_topic_provider.clone();
                        let planner = chat_team_planner.clone();
                        let bus = chat_event_bus.clone();
                        async move {
                            alephcore::gateway::handlers::teams::handle_chat_send(
                                req,
                                store,
                                msg_store,
                                ctx,
                                provider,
                                planner,
                                Some(bus),
                            )
                            .await
                        }
                    });
```

- [ ] **Step 7: Commit**

```bash
git add src/teams/broadcast/mod.rs src/gateway/handlers/teams/canvas.rs \
        src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "teams: fire fire-once StraTA team planner in dispatch_user + thread planner provider (activates leader-first gate)"
```

---

## Task 8: Compile gate + wrap-up

**Files:** none (verification only)

- [ ] **Step 1: Single compile gate** — run exactly once:

```bash
cargo check -p alephcore --lib
```

Expected: clean (exit 0). If errors, fix them inline (common suspects, all pre-identified): the 6 `resolve_targets` test-call arities (Task 5 Step 1), the 2 `build_member_input` test-call arities (Task 6 Step 1), `tempfile` in-scope in the new tests, the `#[allow(clippy::too_many_arguments)]` on `build_member_input` and `run_member` if clippy is on.

- [ ] **Step 2: Note bin crate** — `handle_chat_send` and `agent_init` live partly in the `aleph-server` bin. If the lib check is clean but you want the bin verified too, run once more (still within budget — this is the wrap-up):

```bash
cargo check --bin aleph-server
```

- [ ] **Step 3: Final commit (if any fixes were made)**

```bash
git add -A
git commit -m "teams: phase-1 strata team coordination compile fixes"
```

---

## Self-Review — spec coverage

| Spec §10 Phase-1 seam | Task |
|---|---|
| Cfg `StrategyToml.plan_team` | Task 1 |
| A.1 `team_key` | Task 2 |
| A.2 `put_if_absent` (atomic fire-once) | Task 3 |
| B `active_strategy` team tier (typed `SessionKey::parse`, goal→loop→team→session) | Task 4 |
| B-route hard gate (pure `resolve_targets` + `leader_first`, discarded-`@` surfaced) | Task 5 (mechanism) + Task 7 (activation) |
| D leader frame (`leader_prompt::build`) + member obey-contract + threading | Task 6 |
| C team planner fire-once in `dispatch_user` + provider plumbing (broadcaster→handler→boot, gated `&& plan_team`) | Task 7 |
| E reuse `render_strategy_summary` | No code: the existing weld renders the team strategy unchanged via `prompt_build.rs:403` once Task 4 makes `active_strategy` return it. Verified, nothing to do. |

**Out of scope (Phase 2/3, per spec):** F1 `task_review`, F2 submit-wiring + F2-retrigger, D2 live kanban (`coord_task::global()` + `TeamBoardLayer`). Not in this plan.

**Type-consistency check:** `team_key` (Task 2) is consumed identically in Tasks 4 & 7. `put_if_absent` (Task 3) is consumed in Task 7. `leader_first: bool` threads consistently through `resolve_targets`/`dispatch`/`dispatch_user` (Tasks 5 & 7). `build_member_input`'s 9-arg signature (Task 6) matches its sole caller update (Task 6 Step 3b). `GroupChatBroadcaster::new`'s 4-arg signature (Task 7) matches its sole caller (Task 7 Step 5b). `handle_chat_send`'s new param (Task 7 Step 5a) matches its sole caller (Task 7 Step 6b).
