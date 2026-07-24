# Multiagent V2 Follow-ups — Design

- **Date**: 2026-07-24
- **Baseline**: `main` @ `eae1539fb` (the multiagent V2 integration round `e7f0c3cec` + `9a507d3b4` is already merged; the memory note that called the worktree "未合 main" was stale at write time)
- **Origin**: the "刻意不做（勿重提）" deferred list recorded at the end of the multiagent V2 integration round. The owner has now elected to resolve four of those items.
- **Status**: design approved; pending spec review → implementation plan.

## Scope

Four independent items from the deferred list. Two turned out to be reframes once the code was mapped (Item 2 = no live bug; Item 3 = mostly never-built). Each is independently implementable; they share one spec because each is small-to-medium and thematically one batch.

| # | Item | Decision | Size |
|---|------|----------|------|
| 1 | `team_delegate` interrupt-while-running (documented finish-then-fence) | **Do it** — make delegate cancellable + fix a detached-member leak | M |
| 2 | Subagent spend into goal budget (double-counting risk) | **Keep rails separate** — lock the invariant with the missing regression test | S |
| 3 | residency / persistent-topology / followup_task / fork_turns (zero consumers) | **Cut the one real dead field; leave the rest unbuilt** | S |
| 4 | `timeout_seconds` naming unification (model-facing schema churn) | **Unify to `timeout_seconds` + `#[serde(alias)]`** (zero breakage) | S–M |

Suggested implementation order: **3 → 4 → 2 → 1** (trivial cut, then mechanical rename, then a test, then the meaty behavioral change with the largest review surface).

All file:line anchors are as-of `eae1539fb`; the implementation plan re-verifies them before editing.

---

## Item 1 — Make `team_delegate` interruptible while running

### Current state (mapped)

- Two cancel mechanisms exist. **(A) Engine per-run cancel** — an `mpsc::Sender<()>` per `ActiveRun` bridged to a `CancellationToken` propagated into the harness Flow; a genuine cooperative mid-turn abort (`engine.rs:396`, `execute.rs:131-135,510-534`, `helpers.rs:205-214`, surfaced `run_loop/inner.rs:1201,1223-1229`). Used by `agent.cancel`, `/stop`, `chat.abort`, the busy-input `Interrupt` branch, and `teams.chat.cancel`'s member walk. **(B) `BackgroundAgentTracker` token** — observed only if something awaits it.
- **Group-chat fan-out is already interruptible mid-turn.** `teams.chat.cancel` (`canvas.rs:429-482`) poisons the tree token (stops new spawns) then walks `running_children_of` and calls `execution_adapter().cancel(member_run_id)` — mechanism (A).
- **Only the synchronous `team_delegate` → `execute_member_task` stack is finish-then-fence.** `execute_member_task` (`runner.rs:132`) builds a `RunRequest` with a fresh engine `run_id` (`runner.rs:197`), `tokio::spawn`s `execution_adapter.execute(...)` → `handle` + `abort_handle` (`runner.rs:281-286`), and only fires `abort_handle.abort()` **on timeout** (`runner.rs:308-316`). The delegate tool's tracker registration (`delegate.rs:400-413`) uses a *separate* uuid and a **placeholder `CancellationToken::new()`** (`delegate.rs:404`) disconnected from the engine `run_id` — documented at `delegate.rs:396-398` and `FEATURE_LOCATOR.md:521` as "stack-B finish-then-fence".

### The real defect (not just a missing feature)

When the leader is cancelled (`/stop` / `chat.abort` → `cancel_session`, `engine.rs:420`), the leader's future is dropped → `execute_member_task`'s future is dropped → the `handle` (JoinHandle) is dropped → **the spawned member task is detached and keeps running** (tokio does not abort a task on JoinHandle drop; the `abort` line never runs because that path only fires on timeout). `SettleOnDrop` (`delegate.rs:366`) settles the coord-task *row* but does not reap the detached member run, which continues to spend tokens bounded only by whatever timeout the engine enforces internally. **This is a real resource leak on leader cancel**, which is what makes the fix a correctness change rather than a pure feature add.

### Approach (reuse the group-chat mechanism; no new machinery)

1. **Carry the real engine `run_id` in the tracker registration.** Surface the `run_id` minted at `runner.rs:197` so the delegate registration (`delegate.rs:400-413`) records it (replacing the placeholder token/id), making the in-flight member run addressable for cancellation.
2. **Cancel on leader-session cancel.** Extend session cancellation so cancelling a leader session also walks its tracker children and fires `execution_adapter().cancel(child_run_id)` — the same cooperative mechanism (A) that `teams.chat.cancel` already uses for group-chat members (`canvas.rs:461-472`). Prefer a shared helper over duplicating the walk.
3. **Let the normal return path settle.** A cooperatively-cancelled member makes `execute_member_task` return an outcome normally, so the existing settle path runs and defuses `SettleOnDrop` (which stays as the drop-only safety net). No abrupt `abort_handle.abort()` on the cancel path — cooperative cancel is cleaner and matches group-chat.

### Verify at plan time

- The outcome→settle mapping for a cancelled member: confirm it produces a sensible terminal state (cancelled/failed) on the coord row rather than a misleading one.
- Confirm `execution_adapter().cancel(run_id)` reaches the delegate member run's `active_runs` entry (it should, since the member runs through `execution_adapter.execute()` with `request.run_id`).

### Explicitly out of core scope (secondary, plan-phase decision)

`gate.rs`'s demote-to-queue guard (`gate.rs:116-211`, `steering::session_is_interruptible` `steering.rs:109-121`) force-demotes any session with an active fan-out **because delegate was un-cancellable**. Once delegate is cancellable, that special-case *could* be simplified to a real cancel. This changes interrupt **semantics** (cancel vs queue) and carries its own UX risk, so it is **not** part of the committed core. Core delivery = "delegate can be cancelled + no detached-member leak." The gate simplification is a separate decision recorded for the plan phase.

### Tests

- Delegate member run is cancelled mid-turn when the leader session is cancelled (assert the member run terminates and the coord row settles).
- Regression: no detached member run survives a leader cancel.
- Group-chat cancel behavior unchanged.

---

## Item 2 — Goal budget: keep subagent spend on a separate rail, lock the invariant

### Current state (mapped) — the double-count risk is hypothetical, not live

- Goal budget spend is **derived, not accumulated**: `tree_tokens` (`goal_budget.rs:186-218`) = the goal owner's own `SessionStore::get_total_tokens` + `Σ(member_total − tokens_at_join)` over enrolled `budget_members`, computed on demand (`Goal.token_budget`/`tokens_at_start`/`budget_members` at `goal/types.rs:64-65,149`).
- **Exactly one accumulation path feeds the goal budget**, over mutually disjoint gateway `sessions` rows:
  - Delegation / team / workflow-step children bill to their own `SessionKey::task(agent_id,"team",task_id)` row (`runner.rs:196`, `execute.rs:611`, `session_projector.rs:189`) and are enrolled once (`schedule/mod.rs:302`, `send_tool.rs:350`, `delegate.rs:380`) → counted via the member delta.
  - **In-process subagents** bill to their **own child-harness `AtomicU64`** (`harness/agent.rs:263-264`), surfaced only as `LoopRunResult.total_tokens` / a tool-result figure — they touch **no** gateway `sessions` row and are **never** enrolled. `grep goal_budget|check_and_enroll|origin_session` over `src/agents/` = 0 hits.
- Safeguards keep the single rail honest: self-enroll guard (`store.rs:534`), first-writer-wins (`store.rs:540`), idempotent accrual / replay suppression (`session_projector.rs:151`).
- **The e7f0c3cec change** enrolled the dispatcher's task child keyed off `origin_session`; it did **not** add a second summation.

### The real gap

No test drives `tree_tokens` end-to-end to assert the numeric sum across ≥2 live gateway rows; existing `goal_budget.rs` unit tests cover only `origin_session` metadata round-tripping (`goal_budget.rs:224-262`).

### Approach

1. Add an end-to-end `tree_tokens` regression test: seed an owner row + ≥1 enrolled member row with known totals, assert the sum equals `own + Σ(member − tokens_at_join)`, and include a **negative assertion** that an in-process subagent's spend does **not** appear in the goal total.
2. Add a one-line invariant comment at the `tree_tokens` summation point documenting the single-rail rule (owner's own row + each enrolled member's disjoint row, counted once) so a future change can't silently introduce the second accumulation path.

### Explicitly not doing

Not wiring in-process subagent spend into the goal budget. Subagents are ephemeral helpers; folding them in would add noise and *re-open* the double-count risk — the opposite of the goal here.

---

## Item 3 — Zero-consumer abstractions: cut the one real dead field

### Current state (mapped) — "zero consumer" was mostly "zero definition"

- **residency** (LRU persistent agent identity): 0 definitions. `resident`/`LruCache` hits are all unrelated (RSS, tool-schema residency, web-fetch cache). **Never built.**
- **persistent spawn topology** (agent-graph-store): 0 definitions as flagged. Adjacent real things exist but are different and **wired** — `subagent_tree`/`BackgroundAgentTracker` (live, in-memory, consumed by the panel relay) and `LoopGraphStore` (persistent, but the governance topology, consumed by the thinker/prompt). **Do not delete these.** The flagged "persistent graph of spawned agents" was never built.
- **followup_task** (reuse a prior session): literal name = 0 hits. Live follow-up behavior exists under other names and **has consumers** — harness `MAX_FOLLOWUP_CONTINUATIONS` (`harness/agent.rs:408,521,791-794`) and the gateway busy-queue lane (`gate.rs:214`, `busy_queue.rs:17`). **The one real dead artifact**: `QueueMode::Followup` config (`config/types/general.rs:11-22,39`) — defined, serialized, JsonSchema'd, **read by nobody** (0 read sites for `queue_mode`/`QueueMode::`). Classic R10 dead field.
- **fork_turns** (turn-granularity forking): 0 hits. Nearest real thing is compaction-driven `split_child` (`trait_def.rs:42`), not user-facing fork. **Never built.**
- **AgentPath** (hierarchical addressing): 0 definitions. `agent_path` = SQLite index names on `(agent_id, path)` — live, but not the flagged concept. **Never built.**

### Approach

1. Delete the `QueueMode` enum + the `queue_mode` field + its default fn in `config/types/general.rs` per R10.
2. Leave residency / fork_turns / AgentPath / persistent-topology unbuilt — no definition to remove, no demand to build.

### Verify at plan time

Before removing `queue_mode`, confirm the containing config struct's serde behavior: if `#[serde(deny_unknown_fields)]` is set, an existing `config.toml` with `queue_mode = "..."` would fail to parse after removal — in that case keep a `#[serde(alias)]` tombstone or a `#[serde(default, skip)]`-style ignore rather than a hard removal. Otherwise unknown-field-ignore makes the delete safe.

---

## Item 4 — Unify model-facing timeout naming to `timeout_seconds` + serde aliases

### Current state (mapped)

The seconds-unit timeout is spelled four ways across model-facing tool args, with **no serde alias on any of them** (so a bare rename would break old callers):

| Tool | Field | file:line |
|------|-------|-----------|
| bash_exec / code_exec / code_check | `timeout` | `bash_exec.rs:66`, `code_exec.rs:116`, `code_check.rs:85` |
| team_delegate / task_create / workflow step | `timeout_secs` | `delegate.rs:45`, `task_manage/create.rs:66`, `workflow/def.rs:107` |
| sessions_send / task_wait | `timeout_seconds` | `sessions/send_tool.rs:62`, `task_manage/wait.rs:29` |
| moa_manage (`set_preset`) | `advisor_timeout_secs` | `moa_manage.rs:81` (hand-written schema at `:186`) |

Sharpest divergence: **same subsystem** `task_manage` uses `task_create.timeout_secs` vs `task_wait.timeout_seconds`. The `timeout_ms` group (node_*/desktop/browser) and the `timeout_minutes` group (goal/loop) are each internally consistent — they diverge only by unit, which is legitimate. Prior art for the compat mechanism: `#[serde(default, alias = "timeout")] timeout_secs` at `extension/manifest/parsers.rs:74`.

### Approach

Canonical primary name = **`timeout_seconds`** (explicit; matches most internal config keys and the spelled-out `timeout_minutes`). Every old spelling becomes a `#[serde(alias)]` so old callers — including persisted saved-workflow JSON — still parse. Concretely:

- `bash_exec` / `code_exec` / `code_check`: primary `timeout_seconds`, `#[serde(alias = "timeout")]`.
- `team_delegate` / `task_create` / workflow step: primary `timeout_seconds`, `#[serde(alias = "timeout_secs")]`.
- `sessions_send` / `task_wait`: already `timeout_seconds` — become the canonical anchor, unchanged.
- `moa`: `advisor_timeout_secs` → `advisor_timeout_seconds` (suffix-only unification, keep the meaningful `advisor_` prefix), `#[serde(alias = "advisor_timeout_secs")]`; update the hand-written schema description string (`moa_manage.rs:186`).
- Update each field's doc-comment to say seconds.
- **Untouched**: the `timeout_ms` group, the `timeout_minutes` group, and `duration*` fields (different unit / different concept). Internal config-file keys are out of scope (not model-facing) except where a struct is reused as both.

### Why this is near-zero-risk

`serde(alias)` keeps every historical spelling parseable; the only observable change is the field name the model is shown going forward (the point of the unification). No saved workflow or existing caller breaks.

### Tests

- For each renamed field, a deserialize test proving the old spelling still parses via the alias and the new spelling parses.
- Snapshot/schema check (if one exists) updated to the new primary names.

---

## Non-goals (whole spec)

- No wiring of in-process subagent spend into the goal budget (Item 2).
- No building of residency / fork_turns / AgentPath / persistent spawn topology (Item 3) — no demand, nothing defined.
- No renaming of the `timeout_ms` / `timeout_minutes` groups or `duration*` fields (Item 4).
- The `gate.rs` interrupt-semantics simplification (Item 1 secondary) is deferred to a plan-phase decision, not committed here.
