# Team StraTA Strategic Coordination (Strategy Round 2)

- **Date**: 2026-06-19
- **Status**: Design approved; spec hardened by a verification + redline red-team pass (anchors corrected, F2 re-trigger redesigned). Pending user spec review → writing-plans.
- **Lineage**: Round 2 of the StraTA-inspired strategic planner. Round 1 welded a strategy into the single agent loop / goal / loop / workflow (`2026-06-18-strategic-planner-design.md`, `2026-06-19-naked-loop-strategy-planner-design.md`). This round applies the same "plan-first, weld-forever" philosophy to **team / multi-agent group chat**.
- **Paper**: StraTA — *Incentivizing Agentic Reinforcement via Strategic Trajectory Abstraction*. App-layer takeaway (not RL): mint one global strategy up front, then weld it as a guardrail into every downstream actor's prompt so it never forgets the objective. The "self-audit" mechanism maps to a leader acceptance/verification step.

---

## 1. Problem

A team **group chat** can swarm and talk without producing work. Root cause is a conjunction of five structural absences (verified against code):

1. **No leader strategic plan at message 1.** The naked-loop planner deliberately excludes teams (`SessionKey::is_interactive()` is `false` for `Task` variants — `session_key.rs:1184`; team members use `SessionKey::task(.., "team_chat", ..)`). `handle_chat_send` does first-message auto-naming but never plans.
2. **No team-scoped strategy can be stored or read.** `src/strategy/mod.rs` has no `team_key`; `active_strategy` (`context_blocks.rs:54`) has no team branch. Both Strategy layers emit nothing for any member.
3. **No coord_task on the chat path.** `src/teams/broadcast/` imports `MessageStore`/`TeamStore` only (`mod.rs:37/38`, fields `:71/:72`) — **zero `CoordTaskStore`**. It writes only `team_messages`. Real work lands on the kanban only if a leader voluntarily calls `task_create`/`team_delegate`.
4. **The leader contract is too thin and the strong one is dead.** `member_prompt.rs:16–23` already branches on `is_leader` and appends a minimal `leader_block` (it does say "不要自己闷头做完所有事" and nudges `team_delegate`/`task_create`). But the richer, StraTA-shaped `src/teams/leader_prompt.rs::build` contract (decompose → assign → review → summarize) is **dead** — zero production callers (only `pub mod` at `teams/mod.rs:12` + a test). The gap is: the inline block is too weak, and the strong contract is unwired. Members get no obey/accept/submit framing at all.
5. **No acceptance/verification loop.** `task_submit` (`src/builtin_tools/team/task_submit.rs`) writes only to `ArtifactStore::create_artifact` (`:88–103`); it does not flip the owning `coord_task` or notify the leader.

The only anti-storm guards are quantitative (`MAX_CHAIN_DEPTH=6`, `MAX_FANOUT_WIDTH=5`, `MAX_TOTAL_ACTIVATIONS=32` — `broadcast/mod.rs:12/14/22`). They bound *how much* chatter happens, not *whether* it produces work. That is exactly StraTA's forgetting failure mode.

### Two separate execution paths (the heart of the split)

| | coord_task / kanban | welded Strategy |
|---|---|---|
| **Group-chat broadcast** `teams/broadcast/` (Path 1) | ❌ writes `team_messages` only | ❌ no key, no planner, no weld |
| **Autonomous dispatcher** `teams/dispatcher/` (Path 2) | ✅ DAG over `coord_tasks` | ✅ via `WORKFLOW_STRATEGY_KEY` in handoff (workflow-minted only) |

Round 2 makes the **group-chat path** mint, weld, decompose, and verify — mirroring the workflow→coord_task precedent — without touching the harness or adding any deterministic "chit-chat vs work" classifier.

---

## 2. Confirmed product decisions

1. **Hard gate** — on the user's first message to a team while the team strategy slot is empty, the leader runs **first and alone** (even if the user `@`-named a member). The gate is purely structural (a `slot_empty` boolean — never any Rust "is this substantive?" content inspection; that judgment stays in the leader's prompt). The discarded `@` is surfaced to the user. After a plan exists, normal equal-broadcast resumes.
2. **Explicit verification tool** — the leader actively accepts/rejects deliverables via a new `task_review` tool.
3. **Welded obey-contract + live kanban injection** — members are prompt-told to obey the leader, accept assigned tasks, complete & submit, while still free-`@`-chatting; every member also sees live kanban counts as convergence pressure. Prompt-driven, zero Rust classifier.
4. **Persistent + leader-revisable + LLM-judged re-plan** — the team strategy persists for the team's lifetime; the leader may rewrite it via the existing `strategy` tool; re-planning happens only when the leader's LLM judges the objective changed (no regex pivot detection).

---

## 3. End-to-end loop

```
User's first message → team (team_key strategy slot empty)
  │
  ├─① Team planner fires once (Seam C, inside dispatch_user, atomic put_if_absent):
  │     plan_strategy(objective_with_roster, ctx) mints team Strategy
  │     {objective, approach, phases, guardrails, success_criteria}
  │     → stored under team_key(team_id). Fail-soft: None ⇒ proceed.
  │
  ├─② Hard-gate routing (Seam B-route): dispatch() reads slot_empty via strategy::global()
  │     and passes slot_empty=true into the pure resolve_targets ⇒ targets = [leader] only.
  │     The discarded user @ is surfaced via post_system ("planning first; leader will route").
  │
  ├─③ Leader runs (team Strategy welded into its prompt via Seam B):
  │     resurrected leader_prompt::build contract (now names task_create/team_delegate/task_review)
  │     → decompose → task_create(owner=member) → @mention members to start them.
  │
  ├─④ Members triggered by @ (normal broadcast recursion):
  │     team Strategy + obey-contract + live kanban welded into each member prompt
  │     → accept → work → task_submit (Seam F2):
  │         write artifact + flip owning coord_task → WaitingReview.
  │       Re-trigger (Seam F2-retrigger): run_member, holding a read-only CoordTaskStore,
  │       diffs this member's WaitingReview tasks pre/post turn; any newly-submitted task ⇒
  │       synthetic dispatch("@<leader> task <id> ready for review", sender="system",
  │       depth+1, user_triggered=true, budget) carrying the live budget+depth.
  │       (Deterministic state→routing plumbing — NOT an inert team_messages row.)
  │
  ├─⑤ Leader re-triggered → inspects artifact → calls task_review{verdict} (Seam F1):
  │     approve → coord_task→Completed (unblocks dependents via get_newly_unblocked)
  │     reject  → coord_task→InProgress + feedback into task metadata + re-notify owner.
  │
  └─⑥ Leader LLM judges success_criteria met → reports to user, done.
        Strategy persists. User pivots → leader LLM judges → rewrites via `strategy` tool.
```

**Key invariant**: the verification loop builds **no new state machine** — it reuses the `coord_task` status machine and the broadcaster's existing dispatch primitive. The planner mints the map (before ②); the leader operationalizes it into tasks (③). This is round-1's "strategist mints / soldier executes" with the soldier being a team.

**The F2 re-trigger is the load-bearing fix** (the original "post a system message, the recursion picks it up" was verified non-functional: broadcast recursion fires only on an agent's *own* reply text at `broadcast/mod.rs:298`; a tool-written `team_messages` row is inert, and no-`@` agent replies are dropped at `targets.rs:46–52`). The re-dispatch must therefore live in `run_member` (which holds the live `AtomicUsize` budget + `chain_depth`), not in the tool.

---

## 4. Components & seams

Mirrors the workflow→coord_task precedent: **fire planner once → store → weld every turn**. Anchors below are verified.

### Strategy weld (the cheap half — mostly reuse)

| # | Seam | File:line | Change |
|---|------|-----------|--------|
| A | Team strategy key | `src/strategy/mod.rs` (add after the key cluster `:47`; mirror `workflow_key` `:35`) | `pub fn team_key(team_id: &str) -> String { format!("team:{}", normalize(team_id)) }`. **Normalize the team_id identically on write (here) and read (Seam B)** so the key always round-trips (see §9 risk on `SessionKey::task` normalization). |
| B | Resolve + weld | `src/orchestrator/harness_bridge/context_blocks.rs:54` `active_strategy(session_key: &str)` | Add a team branch between loop and session: when the key parses as `task(_, "team_chat", team_id)`, try `team_key(team_id)`. Precedence **goal → loop → team → session**. Parse the **typed `SessionKey`** (not a raw colon-split) to recover the normalized `team_id`. Both Strategy layers then weld the team plan into every member with **zero new layer code** (verified: `strategy.rs` prio 70 Stable reads `ctx.strategy`; `strategy_pointer.rs` prio 1756 Dynamic reads `ctx.strategy_guardrails`; neither inspects provenance). |
| C | Planner fire-once (atomic) | `src/teams/broadcast/mod.rs::dispatch_user` (`:93`) | Fire-once via a new **atomic** `StrategyStore::put_if_absent` (`put` already does `ON CONFLICT DO UPDATE`; add an `INSERT … ON CONFLICT DO NOTHING` + `changes()`/`RETURNING` so only one winner among concurrent first messages pays for `plan_strategy`). Build `PlannerContext{tool_descriptions, env_summary, lessons}` (**no `roster` field — fold the roster into the `objective`/`env_summary` string** passed to `plan_strategy`). Plumbing (4 sites): (a) `planner_provider` field on `GroupChatBroadcaster` + `new()` arg (`mod.rs:75–87` today takes only `ctx, team_store, msg_store`); (b) new `handle_chat_send` param (`canvas.rs:271`); (c) `team_planner_provider = planner_provider.clone()` taken in `agent_init/mod.rs` **before `:466`** (where `planner_provider` is moved into `tool_config`), gated `&& plan_team`, mirroring the naked-loop clone at `agent_init/mod.rs:395` (resolution is `:371`); (d) thread to `canvas.rs:369`. |
| B-route | Hard gate | `src/teams/broadcast/dispatch` (`mod.rs:109/137`) + `targets.rs:19` (signature) | Keep `resolve_targets` **pure**: add a `slot_empty: bool` param; the store read (`strategy::global()` → `team_key`) happens in `dispatch()`, which already has the team_id. `slot_empty && user_triggered ⇒ targets = [leader_id]` (ignore `@`). `global() == None` (pre-boot) **fails open** = today's behavior. Surface the discarded `@` to the user via the existing `post_system` path. |
| E | Render | reuse `src/strategy/render.rs::render_strategy_summary` (`:17`) | **Name corrected** (`render_strategy` does not exist). Keep phases for chat teams (the phase-dropping `render_workflow_global_frame` at `:89` / `render_guardrails_only` at `:69` are *not* reused). Team semantics carried by the D contracts. |

### Decomposition + verification (the heavier half — real plumbing)

| # | Seam | File:line | Change |
|---|------|-----------|--------|
| D | Prompt contracts | `src/teams/broadcast/member_prompt.rs::build_member_input` | **Leader frame**: **replace** the thin inline `leader_block` (`:16–23`) with the resurrected `leader_prompt::build` — but that requires `(team_id, team_name, roster, protocol, user_request)` (`leader_prompt.rs:15`), so **enlarge `build_member_input`'s signature** to thread `team_name` + `user_request` (+ `protocol`) from the team + triggering message at the `run_member` dispatch site (only `transcript` is present today). Add `task_review`/`task_submit`/`task_create` by name to the leader text or the leader never calls them. **Member frame**: obey-contract (accept assigned task / complete / `task_submit` / free `@`-chat but converge on the deliverable). Pure function, host-testable. |
| D2 | Live kanban | new `coord_task::global()` accessor + `ResolvedContext.team_board` + `prompt_build.rs:389` (4th fetch) + new `TeamBoardLayer` (Dynamic) | **There is no `CoordTaskStore` reachable in `harness_bridge` and no process-global today** (unlike `strategy::global()`). Add a process-global `CoordTaskStore` accessor mirroring `strategy::global()`, init at boot. `prompt_build.rs:389`'s `tokio::join!` gains a 4th future, gated on a `team_chat` session, fetching a board summary (todo / in-progress / waiting-review / accepted counts + this member's open assigned tasks). New `team_board` field on `ResolvedContext` (mirrors `strategy`/`strategy_guardrails` at `context.rs:146/190/196`). `TeamBoardLayer` is **Dynamic, near the read head** (counts change per turn → must not enter the cacheable Stable prefix). Render obeys the no-timestamp determinism contract. |
| F1 | Verification tool | new `src/builtin_tools/team/task_review.rs` **+ 4 registration sites** | `task_review { task_id, verdict: approve\|reject, feedback?: String }`. `approve` → `CoordTaskStore::update_task` (`:430`) with `CoordTaskUpdate.status = Completed` (`:213`) → `get_newly_unblocked` (`:442`) satisfies dependents (`satisfies_dependency` = `Completed\|Skipped`, `tasks/mod.rs:152`). `reject` → `InProgress` + `feedback` into task metadata + re-notify owner. **Registration (without these the leader's LLM never sees the tool)**: (1) `definitions.rs` schema entry + result-budget match arm (`task_submit` template at `:702` / `:987`); (2) construction + `UnifiedTool` insert in `executor/builtin_registry/builder/constructor/collab_session_tools.rs` (`task_submit` template `:221–245`); (3) group membership `groups.rs:201`; (4) `Option<…Tool>` field in `registry/struct_def.rs`. **Authz**: leader-only ownership check inside `call()` (the registry has no per-team-role gating). |
| F2 | Submit wiring | `src/builtin_tools/team/task_submit.rs` + its constructor | Thread `CoordTaskStore` into `TaskSubmitTool::new` (`:53–59` holds only `ArtifactStore` today; both stores exist in `BuiltinToolConfig` — `coord_task_store:70`, `message_store:102` — and the sibling `delegate.rs:233/144` already uses both). On submit: also flip the owning `coord_task` → `WaitingReview`. **Confirm and document `task_submit.task_id == coord_task.id`** (`delegate.rs:145/233` share the id-space — cite it) or add a lookup. The leader re-trigger is **not** done here (an inert row) — see F2-retrigger. |
| F2-retrigger | Leader re-dispatch | `src/teams/broadcast/mod.rs::run_member` (`:197`) + read-only `CoordTaskStore` on the broadcaster | After the member's `execute()` returns, diff this member's tasks now in `WaitingReview` against a pre-turn snapshot; any newly-submitted task ⇒ synthetic `dispatch("@{leader} task {id} ready for review", sender="system", chain_depth+1, user_triggered=true, budget)` carrying the live budget + depth. Deterministic state→routing plumbing; the accept/reject **judgment** stays in the leader's LLM `task_review` turn. The broadcaster gains a **read-only** `CoordTaskStore` (available at `canvas.rs:369` construction). |

### Config

| # | Seam | File:line | Change |
|---|------|-----------|--------|
| Cfg | Switch | `src/config/types/phase6_wiring.rs:217` `StrategyToml` (struct at `:217`, fields `enabled:221`/`planner_model:229`/`plan_naked_loop:236`) | Add `plan_team: bool` (default `true`), copying the `plan_naked_loop` default-fn (`:247`) + Default-impl (`:256`) verbatim. **No E0063 risk**: the only non-decl literal (`deps_builder.rs:1804`) uses `..StrategyToml::default()`; only the Default impl needs the new field. Master gate adds its **own** `&& plan_team` at clone time in `agent_init` (mirroring `:395`) so `plan_team=false` ⇒ `team_planner_provider = None` ⇒ Seams C/B-route are no-ops. |

### Resolved micro-decisions
- **Two LLM roles kept separate** (planner vs leader): the cheap one-shot `plan_strategy` produces the welded artifact (survives compaction, welds into *all* members); the leader operationalizes into coord_tasks. Not collapsed into "leader posts strategy as a chat message" (that would not weld into members).
- **Single `task_review` tool** with a `verdict` field (not split `team_accept` + `task_review`).
- **Strategy key precedence via b1** (parse the typed `SessionKey` in `active_strategy`, one read-side edit), not b2 (per-member session-row writes). A member's own `/goal` overrides the team frame **only if that `/goal` was minted under the same `team_chat` composite key** (see §9) — team beats bare session.

---

## 5. Redline compliance

- **R7 / R9 (LLM Sovereignty / Intelligence in the prompt)**: no "chit-chat vs work" or "is it done" classifier. Convergence pressure = welded `success_criteria` + live kanban + the leader's LLM `task_review` verdict. The hard gate inspects only a `slot_empty` boolean (state), never content. All judgment lives in prompts and LLM calls.
- **R10 (Thin Harness / Dumb Loop)**: `src/harness/` is untouched (no new LOC). Hard-gate routing, F2 submit-flip, and the F2-retrigger re-dispatch are deterministic **plumbing (state→routing)**, not cognition. `TeamBoardLayer` + the 4th prompt fetch live in the thinker/orchestrator layer system, not the harness; the new `coord_task::global()` is a store accessor exactly like `strategy::global()`.
- **R8 (Everything is a Tool)**: `task_review` is a tool; the leader manages acceptance through natural-language tool use.
- **R4 (I/O-only interfaces)**: planner fire lives in the broadcaster (`teams/broadcast/`, core logic), not the RPC handler.
- **R1 / R3**: no platform APIs, no heavy new deps; reuses `StrategyStore`, `CoordTaskStore`, existing layers.

---

## 6. Lifecycle & config

- **SET**: team planner fires once per team via **atomic** `put_if_absent` on `team_key` (fixes the non-atomic check-then-act race: `dispatch_user` runs detached via `tokio::spawn` at `canvas.rs:377`, so two near-simultaneous first messages would otherwise both pay for `plan_strategy`; the sibling auto-naming path already guards with an atomic `take_auto_name_flag`).
- **PERSIST**: team strategy lives for the team's lifetime (like the naked-loop `session_key` strategy — no auto-clear per message).
- **REVISE / RE-PLAN**: leader rewrites via the existing `strategy` tool when its LLM judges the objective changed. No `goal_id` cross-ref (decision 4 chose LLM-judged re-plan, not auto-invalidation).
- **CLEAR**: hook the existing disband cascade — `crud.rs:155 handle_delete` already calls `store.delete_team` (`:174`) then best-effort `coord_store.delete_team_tasks` (`:200`) + 4 more stores (`:197/203/206/209`), each `warn!`-on-error. Add `strategy_store.delete(&team_key(&team_id))` alongside `:200–211` (do not re-invent the cascade).
- **CONFIG**: `[strategy].plan_team = true` (default). Disabled ⇒ `team_planner_provider = None` ⇒ Seams C / B-route no-op; group chat behaves exactly as today.

---

## 7. Non-goals (YAGNI)

- ❌ Farthest-point diversity sampling / hierarchical GRPO (RL training, irrelevant to app layer).
- ❌ Per-step self-audit penalty (RL); self-audit maps to the leader's `task_review` verdict.
- ❌ Dynamically raising `MAX_TOTAL_ACTIVATIONS` (would be a strategy choice — R10). Durable `coord_tasks` survive across activation windows: if the budget is exhausted mid-work, the next user message / leader re-trigger resumes from the kanban.
- ❌ Regex pivot detection; objective change is leader-LLM-judged.
- ❌ New strategy render variant; reuse `render_strategy_summary`.
- ❌ Auto-creating coord_tasks inside the broadcast loop; only the leader's LLM (via `task_create`/`team_delegate`) creates tasks. The broadcaster gains only a **read-only** `CoordTaskStore`.

---

## 8. Test plan

- **Unit (host)**:
  - `strategy::team_key` formatting + normalization round-trip (write-key == read-key for a `team_chat` `SessionKey`).
  - `active_strategy` team-branch precedence (goal > loop > team > session) and typed-`SessionKey` team_id recovery; non-team session ⇒ no team lookup.
  - `StrategyStore::put_if_absent` returns winner/loser correctly under a simulated double-insert.
  - `resolve_targets` purity + hard gate: `slot_empty + @member ⇒ [leader]`; `!slot_empty ⇒ existing behavior`. Extend the existing `targets.rs` tests.
  - `build_member_input` leader frame (contract present, names `task_review`/`task_submit`) vs member frame (obey-contract present).
  - `TeamBoardLayer` render determinism (no timestamps; stable for fixed counts) + dormant for non-team sessions.
  - `task_review` status transitions (approve→Completed→`get_newly_unblocked`; reject→InProgress+feedback) + leader-only authz rejects a non-leader caller.
  - `task_submit` flips owning coord_task→WaitingReview; `task_id == coord_task.id` invariant asserted.
- **Integration**: first user message to a 3-member team → exactly one leader run first, a Strategy under `team_key`, ≥1 `coord_task` created; after a member `task_submit`, assert the **F2-retrigger** synthetic dispatch re-activates the leader and `task_review` flips the task to Completed. Cover a deliverable spanning two activation windows (budget-exhaustion resume from kanban).
- **Determinism**: Stable prefix (StrategyLayer body) byte-identical across two consecutive member turns.
- **Compile gate**: one `cargo check -p alephcore --lib` after the slice (project cargo-frugality rule).

---

## 9. Open risks / follow-ups

- **F2-retrigger snapshot-diff cost**: `run_member` reads `CoordTaskStore` before+after each member turn. Acceptable (one team-scoped query); the plan may instead surface a submit signal via the run emitter if cheaper. The **judgment** (approve/reject) must remain LLM.
- **`team_id` normalization invariant (b1)**: `SessionKey::task` runs `normalize_agent_id` on the team_id (`session_key.rs:183/652` — lowercase, non-`[a-z0-9-_]`→`-`). For current UUIDs (`uuid::Uuid::new_v4()`, lowercase hex+hyphens, no colon — `store.rs:303`) write-key == read-key (verified safe). **Invariant to hold**: normalize identically on write (Seam A/C) and read (Seam B), or team strategy silently never welds if id formats ever change. Recommend parsing the typed `SessionKey` rather than raw colon-split.
- **Precedence b1 caveat**: `active_strategy` tries `goal_key→loop_key→…` on the **same** session_key string, which for a team member is `agent:<id>:team_chat:<uuid>`. A member's `/goal` overrides the team frame **only if minted under that exact composite key**, not the member's main session. State this so the claim isn't overstated.
- **Leader identity** is `team.leader_id` (`broadcast/mod.rs:137/172`; `is_leader = agent_id == leader_id` at `:222`); single-leader assumption — confirm always populated.
- **`task_submit.task_id` id-space**: assumed equal to `coord_task.id` (`delegate.rs:145/233` support this). If a member submits an artifact whose `task_id` is not a coord_task, F2 must no-op gracefully (no flip, no retrigger).

---

## 10. Suggested implementation phasing (for writing-plans)

The verified surface is larger than the strategy-weld alone. Phasing lets value land incrementally and keeps each `cargo check` slice small:

- **Phase 1 — Weld + gate + contracts** (the cheap, high-value half): Seams A, B, C (with `put_if_absent`), B-route, D, E, Cfg. Outcome: leader plans first on message 1; the plan is welded into every member; members get the obey-contract. No coord_task changes yet.
- **Phase 2 — Decomposition + verification loop**: Seams F1 (+registration), F2 (+constructor store threading), F2-retrigger. Outcome: leader decomposes to coord_tasks, members submit, leader is re-triggered and accepts/rejects.
- **Phase 3 — Live kanban pressure**: Seam D2 (`coord_task::global()`, `ResolvedContext.team_board`, `TeamBoardLayer`). Outcome: every member sees live board counts as convergence pressure.

---

## 11. File index (verified anchors)

`src/strategy/mod.rs` (key cluster `:21/28/35/45`; add `team_key`, `put_if_absent`) · `src/strategy/planner.rs:20/58` · `src/strategy/render.rs:17` (`render_strategy_summary`) · `src/orchestrator/harness_bridge/context_blocks.rs:54` · `src/orchestrator/harness_bridge/prompt_build.rs:389` · `src/thinker/layers/strategy.rs` · `src/thinker/layers/strategy_pointer.rs` · `src/thinker/context.rs:146/190/196` (+ `team_board`) · `src/teams/broadcast/mod.rs:75/93/137/197/222/298` · `src/teams/broadcast/targets.rs:19/46` · `src/teams/broadcast/member_prompt.rs:16` · `src/teams/leader_prompt.rs:15` (dead, to wire) · `src/teams/dispatcher/schedule.rs:52` · `src/agents/swarm/tasks/mod.rs:152/179/213/430/442` (`CoordTaskStatus::WaitingReview:91`) · `src/builtin_tools/team/task_submit.rs:53/88` · `src/builtin_tools/team/task_review.rs` (new) · `src/builtin_tools/team/delegate.rs:145/233` · `…/builtin_registry/.../collab_session_tools.rs:221` · `…/definitions.rs:702/987` · `…/groups.rs:201` · `…/registry/struct_def.rs` · `src/gateway/handlers/teams/canvas.rs:271/369/377` · `src/teams/crud.rs:155/200` · `src/routing/session_key.rs:183/652/1184` · `src/config/types/phase6_wiring.rs:217` · `src/gateway/.../agent_init/mod.rs:371/395/466` · `src/runtime/.../deps_builder.rs:1804`
