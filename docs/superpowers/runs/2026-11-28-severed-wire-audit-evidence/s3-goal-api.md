# S3 Slice Audit — `s3-goal-api`

- **Auditor**: `sev-auditor` (read-only)
- **Repo root**: `/home/zou/data/workspace/Aleph` (branch `main`)
- **Slice key**: `s3-goal-api`
- **Slice files (read IN FULL)**:
  - `src/goal/mod.rs` (90 lines)
  - `src/goal/types.rs` (742 lines)
  - `src/goal/pursuit.rs` (1038 lines)
- **Also read (consumer side, for parity judgement)**: `src/goal/store.rs` (the store that owns the Goal row), `src/looping/pursuit.rs` + `src/looping/types.rs` (the near twin), `src/builtin_tools/goal.rs` (the tool front-end), `src/gateway/execution_engine/goal_continuation.rs`, `src/gateway/execution_engine/goal_wait.rs`, `src/gateway/goal_budget.rs`, `src/gateway/handlers/kanban.rs`, `src/gateway/handlers/users.rs`, `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`, `src/executor/builtin_registry/builder/constructor/mod.rs`.
- **Constraints honoured**: no source file edited, no `cargo` run, no git mutation. Only `rg`/`grep`/`read`/`git log` inspection.

---

## A. Findings

| ID | Form | Severity | Title | Producer | Consumer |
|----|------|----------|-------|----------|----------|
| SW-S3-GOAL-API-1 | 1 | low | `WaitPark::Task { task_id }` payload has no production reader | `src/goal/pursuit.rs:356` / constructed `:370` | none (only `WaitPark::Timer` is matched, `src/goal/store.rs:664`) |
| SW-S3-GOAL-API-2 | 5 | low | `src/goal/mod.rs:22` claims "**21** production call sites"; there are 23 | `src/goal/mod.rs:22-23` (doc) | 23 real `crate::goal::global()` call sites |
| SW-S3-GOAL-API-3 | 5 | low | Stale comment: "neither `goal` nor `looping` persists it" — `Goal.workspace` does | `src/goal/types.rs:170` (`pub workspace`) | `src/gateway/execution_engine/execute.rs:1340` (stale claim) |
| SW-S3-GOAL-API-4 | 2 | low | `action='set'` silently drops `lesson` and `status` (the same path explicitly refuses `wait_minutes`/`wait_for_task`) | `src/builtin_tools/goal.rs:53` (`status`), `:80` (`lesson`) | no reader on the `set` arm (`src/builtin_tools/goal.rs:812-966`) |

**Severity tally**: high 0 · medium 0 · low 4 · (UNCERTAIN 0)

No high or medium finding survived the "read before write" rule. The `goal` subsystem is unusually well wired: every `pub fn` in `src/goal/pursuit.rs` has a **production** (non-`#[cfg(test)]`) caller, every builder in `src/goal/types.rs` has a production reader, every enum variant of `GoalStatus` / `PursuitMode` / `GateOutcome` is both produced and matched, both `MAX_*` constants are enforced at their single writer, and the process-global store has exactly one production writer with 23 production readers that each spell out their own `None` semantics.

---

### SW-S3-GOAL-API-1 — `WaitPark::Task { task_id }` payload has no production reader
- **Form**: 1 (no consumer) — scoped to the *payload*, not the variant
- **Producer**: `src/goal/pursuit.rs:352-358` — `pub enum WaitPark { Timer { wake_ms: u64 }, Task { task_id: String } }`; the payload is filled at `src/goal/pursuit.rs:370-372` (`return Some(WaitPark::Task { task_id: task_id.clone() })`) from `goal.waiting_on_task`
- **Consumer**: **no consumer found** for `task_id`. The only production match on the enum is `src/goal/store.rs:664` — `if let pursuit::WaitPark::Timer { wake_ms } = park {` — a *refutable* `if let`, so the `Task` arm falls through to `src/goal/store.rs:717-723`:
  ```rust
  // Task barrier: woken externally by `GoalWakeService` on the
  // task's settle event. ...
  return Ok(ContinuationDecision::Idle);
  ```
  The id itself is never read from the enum; the wake service reads `goal.waiting_on_task` **directly** instead: `src/gateway/execution_engine/goal_wait.rs:159` (`g.waiting_on_task.as_deref() == Some(task_id)`), `:185`, `:329`, `:350` (`goal.waiting_on_task.clone()`), `:386`.
- **Live caller search**:
  - `rg 'WaitPark::'` → `src/goal/pursuit.rs:370`, `:375` (constructors), `src/goal/pursuit.rs:979`, `:997` (tests), `src/goal/store.rs:664` (Timer only), `src/goal/store.rs:1835` (test, Timer only). **No production site ever destructures `WaitPark::Task { task_id }`.**
  - `rg 'waiting_on_task'` → every production read is on the `Goal` field, never on the enum payload.
- **Symptom**: none user-visible. The barrier still works because the authoritative id lives on `Goal.waiting_on_task`. The observable effect is that the enum looks like it carries the wake identity but does not, so a future maintainer who reads `WaitPark` instead of `Goal` will build a barrier with no id.
- **Hypothesis**: **CUT** (payload only). The *variant* is wired — `wait_parked` is consumed at `src/goal/store.rs:663` and the `Task` arm is explicitly handled (documented fall-through to `Idle`). Only the `task_id` field is inert.
- **Confidence**: high (the grep is exhaustive over `WaitPark::`; there are 9 hits total and 3 of them are `#[cfg(test)]`).
- **Severity**: low — a dead payload inside a handled variant, not a dark feature.

---

### SW-S3-GOAL-API-2 — `src/goal/mod.rs:22` claims "21 production call sites"; the real count is 23
- **Form**: 5 (name/path drift — a literal figure that no longer matches the code it describes)
- **Producer**: `src/goal/mod.rs:22-23` — the doc on `static GLOBAL`:
  ```
  /// `ConsumerDecides`, and it is the shape by weight of evidence: **21**
  /// production call sites, each writing its own meaning for `None`.
  ```
  This number is the stated *justification* for the slot's `MissingSemantics::ConsumerDecides` classification (`src/goal/mod.rs:29-30`), which is pinned by the census test `src/capability/census.rs:3325` `every_slot_pins_its_own_missing_semantics` and by `src/goal/mod.rs:82-89`.
- **Consumer**: the doc's own subject — the production readers of `crate::goal::global()`. Exhaustive count (`rg -n 'goal::global\(\)' --include='*.rs' src/` → 39 raw hits):
  - **23 production call sites**: `src/gateway/goal_budget.rs:81`, `:140`; `src/gateway/handlers/users.rs:1049`; `src/gateway/handlers/kanban.rs:123`; `src/gateway/continuation_lifecycle.rs:51`, `:81`; `src/gateway/execution_engine/goal_continuation.rs:164`, `:402`, `:583`, `:659`; `src/gateway/execution_engine/execute.rs:1904`, `:1977`, `:2391`; `src/gateway/execution_engine/goal_wait.rs:144`, `:169`, `:312`, `:383`, `:415`, `:558`; `src/builtin_tools/loop_graph_manage.rs:303`; `src/orchestrator/harness_bridge/context_blocks.rs:37`, `:67`, `:152`.
  - 6 test-only hits (`src/gateway/handlers/users.rs:1950`, `:1956`; `src/gateway/handlers/kanban.rs:253`, `:259`; `src/gateway/execution_engine/tests.rs:89`; `src/memory/dreaming/stages/goal_lessons_promote.rs:344`).
  - 10 prose mentions in doc comments (`src/gateway/method_visibility.rs:765`, `src/gateway/handlers/kanban.rs:12`, `:240`, `:270`, `src/gateway/handlers/mod.rs:1092`, `src/security/audit.rs:329`, `src/memory/flush/mod.rs:20`, `src/memory/dreaming/stages/goal_lessons_promote.rs:13`, `src/tasks/cron/mod.rs:68`, `src/tasks/heartbeat/mod.rs:31`).
- **Live caller search**: `rg -n 'goal::global\(\)' --include='*.rs' src/` → 39 hits; classified by hand into 23 production / 6 test / 10 prose. No census test pins the number 21 — `src/capability/census.rs:3325` pins the *variant* (`ConsumerDecides`), not the count.
- **Symptom**: a reviewer auditing the `goal/store` slot reads "21 call sites" and re-derives 23. The doc's *conclusion* (`ConsumerDecides`) is still correct and still independently pinned by the census test — this is a stale supporting figure, not a wrong classification.
- **Hypothesis**: **CUT** (the figure no longer matches the code). The classification itself remains correct.
- **Confidence**: high.
- **Severity**: low (drift in a rationale figure; the guarded invariant — the classification — is intact and test-pinned).

---

### SW-S3-GOAL-API-3 — Stale comment: "neither `goal` nor `looping` persists it" — `Goal.workspace` does
- **Form**: 5 (name/path drift — a literal claim that disagrees with the code)
- **Producer**: `src/goal/types.rs:170` — `pub workspace: Option<String>` on `Goal`, with the setter `src/goal/types.rs:332-334` `with_workspace`. It is stamped by the claim pipeline at `src/goal/store.rs:620` (`goal = goal.with_workspace(Some(ws.to_string()));`) and read back at `src/gateway/execution_engine/goal_wait.rs:526` (`let workspace = goal.workspace.as_ref().map(std::path::PathBuf::from);`) so a hook-less wake (task-settle / boot re-arm / periodic recheck) rebuilds the continuation **in the same project**.
- **Consumer**: the stale claim lives at `src/gateway/execution_engine/execute.rs:1339-1341`:
  ```
  // …and this turn's project root. It is NOT recoverable later:
  // neither `goal` nor `looping` persists it, and
  // `projects::run_context` is already closed by the time this
  // hook runs.
  ```
  The `looping` half is still true (`src/looping/types.rs` `LoopState` has no workspace field — only `owner_user_id`/`scope_id`/`telecom_origin`). The `goal` half is **false**: `Goal.workspace` exists precisely to persist it, and `src/goal/pursuit.rs`'s own doc at `src/goal/types.rs:164-168` says so ("can rebuild the continuation in the same workspace instead of silently falling back to the agent workspace").
- **Live caller search**: `git log -S 'with_workspace' -- src/goal/types.rs` → `3dd9f5473` (2026-07-30, "goal: harden standing-goal lifecycle — status-CAS field merge, barrier-aware abandon, wake workspace inheritance"); `git log -S 'neither \`goal\` nor \`looping\` persists it' -- src/gateway/execution_engine/execute.rs` → `5b90f4a6a` (2026-07-14). `git merge-base --is-ancestor 5b90f4a6a 3dd9f5473` → true, i.e. **the comment predates the field it denies**. The field was added after the comment and the comment was never updated.
- **Symptom**: none behavioural — the hook still passes `request.workspace_path()` explicitly, which is the right thing to do at that point. The cost is a misleading rationale for a reader who would otherwise rely on `Goal.workspace` being absent.
- **Hypothesis**: **CUT** (the comment is stale). The behaviour it justifies is unchanged and correct.
- **Confidence**: high on the factual drift (git ordering + field existence proven); **UNCERTAIN** on intent — the author may have meant "the *current* turn's root is not recoverable from the ambient context at hook time" (which is true: `projects::run_context` is closed), in which case the sentence is merely ambiguous rather than false. Evidence that would settle it: the PR/issue that introduced `3dd9f5473` — whether it explicitly set out to update this comment. I did not find such an update in the commit's diff for `execute.rs`.
- **Severity**: low.

---

### SW-S3-GOAL-API-4 — `action='set'` silently drops `lesson` and `status`
- **Form**: 2 (stub far-end — the call succeeds, but the parameter's far end is a no-op on this path)
- **Producer**: `src/builtin_tools/goal.rs:53` — `pub status: Option<GoalStatus>` and `src/builtin_tools/goal.rs:80` — `pub lesson: Option<String>` on the single flat `GoalArgs` struct (one schema for all actions, so both are legal input for `action='set'`).
- **Consumer**: **none on the `set` path.** The `GoalAction::Set` arm spans `src/builtin_tools/goal.rs:812-966`; it reads `objective`, `note`, `token_budget`, `pursuit_max_iterations`, `gate_command`, `timeout_minutes`, `owner_scope`, `author` — and never `args.lesson` or `args.status`. Exhaustive grep:
  ```
  rg -n 'args\.lesson|args\.status' src/builtin_tools/goal.rs
  284:        if args.status != Some(GoalStatus::Paused) {          // remote_pause
  300:            args.lesson.is_some().then_some("lesson"),      // remote_pause refusal list
  1041:                let barrier_touched = args.status.is_some() // update arm
  1044:                if let Some(status) = args.status {         // update arm
  1124:                } else if matches!(args.status, Some(GoalStatus::Active))
  1134:                if let Some(lesson) = args.lesson.clone() {  // update arm
  ```
  No hit inside 812-966. `lesson` is only appended by `Goal::with_lesson_appended` (`src/goal/types.rs:404`), which the `set` arm never calls; `status` is only written by `Goal::with_status` (`src/goal/types.rs:359`), which the `set` arm never calls (the new goal is always `GoalStatus::Active` from `src/goal/types.rs:243`).
- **Live caller search**: the `set` arm reads `args.wait_minutes` / `args.wait_for_task` — but only to **refuse** them (`src/builtin_tools/goal.rs:840-846`): "wait_minutes / wait_for_task apply to action='update' — a goal is never born parked…". That explicit refusal is what makes the `lesson`/`status` silence an inconsistency rather than a design choice.
- **Symptom**: a model that calls `goal(action='set', objective='…', lesson='remember to use the staging DB')` receives `success: true` with a "Set. …" message, and the lesson is never persisted — the goal row's `lessons` stays empty and the lesson never reaches the continuation prompt. Likewise `status='paused'` on `set` is dropped and the goal is created `Active`.
- **Hypothesis**: **CUT** on the `set` arm only (the fields are live on the `update` arm: `src/builtin_tools/goal.rs:1134` for `lesson`, `:1044` for `status`). This contradicts the tool's own stated rule at `src/builtin_tools/goal.rs:268-271` — "Every other `update` field is refused rather than silently ignored — a caller that passed `pursuit_max_iterations` alongside the pause would otherwise be told 'Paused' while its cap change vanished" — and the analogous refusal for `objective` on `update` (`src/builtin_tools/goal.rs:1022-1031`, "the local path owes the same honesty").
- **Confidence**: high for the mechanism (the grep is exhaustive over `args.lesson` / `args.status`). **UNCERTAIN** on severity/impact: the schema doc strings do scope both fields ("New status — for `update`." at `src/builtin_tools/goal.rs:52`; "Optional lesson to append to the goal's state file — for `update`." at `src/builtin_tools/goal.rs:79`), so a compliant model would not send them with `action='set'` — but the schema is flat and does not make that binding, and the tool's own policy elsewhere is to *refuse* rather than silently ignore. Evidence that would settle it: whether the tool's schema generator emits per-action field constraints (it does not — `GoalArgs` is one flat struct, so no `if/then` on `action` can be expressed).
- **Severity**: low (documented field scope; no production path silently corrupts a goal — the loss is an inert model-supplied lesson).

---

## B. Clean / verified symbols (no severed wire)

### B.1 `src/goal/pursuit.rs` — every `pub fn` has a PRODUCTION caller

| Symbol | Definition | Production caller(s) |
|---|---|---|
| `should_continue` | `pursuit.rs:57` | `src/goal/store.rs:670`, `:702`, `:726`; also `pursuit.rs:459` (internal, `reopen_after_gate_failure`) |
| `fires_out_of_bounds` | `pursuit.rs:107` | `src/goal/store.rs:671`, `:1368`; `src/gateway/execution_engine/goal_wait.rs:218` |
| `continuation_prompt` | `pursuit.rs:171` | `src/goal/store.rs:727` |
| `exhausted_while_active` | `pursuit.rs:~250` | `src/goal/store.rs:744`, `:1446` |
| `cap_reached_note` | `pursuit.rs:~270` | internal only — `pursuit.rs:346` inside `stop_reason_note`. **Not severed** (the brief's Form-1 rule requires zero *real* callers; an internal caller is a real caller) |
| `deadline_reached_note` | `pursuit.rs:~280` | `src/goal/store.rs:705`, `:1369`; `pursuit.rs:344` |
| `budget_reached_note` | `pursuit.rs:~290` | internal only — `pursuit.rs:342`. **Not severed**, same reasoning |
| `transient_park_note` | `pursuit.rs:~300` | `src/gateway/execution_engine/goal_continuation.rs:597` |
| `bound_transient_park_delay_ms` | `pursuit.rs:~320` | `src/gateway/execution_engine/goal_continuation.rs:566`, `:592`; `src/gateway/execution_engine/execute.rs:2489` — the hint comes from the live provider error chain (`crate::providers::llm_retry::extract_retry_after_str`) |
| `stop_reason_note` | `pursuit.rs:340` | `src/goal/store.rs:703`, `:745`, `:1447`; `pursuit.rs:457` |
| `wait_parked` | `pursuit.rs:365` | `src/goal/store.rs:663` (Timer arm) — the `Task` arm falls through to `ContinuationDecision::Idle` at `src/goal/store.rs:717-723` (explicitly commented) |
| `wait_resume_prompt` | `pursuit.rs:384` | `src/goal/store.rs:673`; `src/gateway/execution_engine/goal_wait.rs:247`, `:406` |
| `awaiting_gate` | `pursuit.rs:411` | `src/goal/store.rs:573` |
| `gateless_terminal_complete` | `pursuit.rs:428` | `src/gateway/execution_engine/goal_continuation.rs:233` |
| `confirm_complete` | `pursuit.rs:436` | `src/gateway/execution_engine/goal_continuation.rs:427` |
| `reopen_after_gate_failure` | `pursuit.rs:452` | `src/gateway/execution_engine/goal_continuation.rs:462` |
| `gate_failure_prompt` | `pursuit.rs:483` | `src/gateway/execution_engine/goal_continuation.rs:485` |

Private helpers `truncate_chars` (`pursuit.rs:28`), `render_lessons` (`pursuit.rs:39`), `render_quota` (`pursuit.rs:118`), `objective_block` (`pursuit.rs:~165`) are all consumed inside the module. `objective_block` escapes via the single source `crate::thinker::xml_util::escape_xml` — the same escape `StandingGoalLayer` applies, so the two renders of one objective agree.

### B.2 `src/goal/types.rs` — every builder / accessor has a production reader

`new` (`types.rs:231`) → `src/builtin_tools/goal.rs:876`; `with_wait_until` (`:269`) → `goal.rs:1152`; `with_wait_on_task` (`:280`) → `goal.rs:1160`; `without_wait` (`:296`) → `goal.rs:1046`, `src/goal/store.rs` (pause paths); `has_wait_barrier` (`:306`) → `src/goal/store.rs:924`, `:1282`, `goal.rs:411`, `:460`, `goal_wait.rs:254`; `with_pending_continuation` (`:314`) → `src/goal/store.rs` (multiple); `spent_continuation` (`:322`) → `src/goal/store.rs:677`, `:736`, `:872`; `with_workspace` (`:332`) → `src/goal/store.rs:620`; `with_owner_scope` (`:342`) → `goal.rs:882`; `with_author` (`:353`) → `goal.rs:886`; `with_status` (`:359`) → `goal.rs:1044`, `src/goal/store.rs:1054`-region, `store.rs` exhaustion paths; `with_note` (`:377`) → `goal.rs:1129`, `src/goal/store.rs`; `with_gate_outcome` (`:386`) → `goal.rs:1054`, `pursuit.rs:436`/`:465`/`:472`; `with_gate_command` (`:395`) → `goal.rs:899`, `:1109`; `with_lesson_appended` (`:404`) → `goal.rs:1142`, `pursuit.rs:466`, `:473`; `with_budget` (`:417`) → `goal.rs:1069`; `with_pursuit` (`:425`) → `goal.rs:893`, `:1074`; `with_deadline_ms` (`:433`) → `goal.rs:903`, `:1078`; `with_baseline` (`:443`) → `src/goal/store.rs:610`; `tokens_used` (`:451`) → `pursuit.rs:125`, `:131`; `over_budget` (`:464`) → `pursuit.rs:67`, `:341`, `src/gateway/goal_budget.rs:97`, `:147`; `is_active` (`:472`) → `src/goal/store.rs:607`, `src/gateway/goal_budget.rs:88`; `GoalStatus::as_str` (`:26`) → `goal.rs:347`, `:363`, `:453`, `:654`, `:1208`.

**Field readers (production)** — `scope_id` → `goal_wait.rs:626`, `kanban.rs:142`; `author_user_id` → `goal_continuation.rs:335`, `goal_wait.rs:630`; `completed_at_ms` → `src/goal/store.rs:1590`; `workspace` → `goal_wait.rs:526`; `waiting_reason` → `pursuit.rs:388` + tool render; `gate_command` → `goal_continuation.rs:407`; `budget_members` → `src/gateway/goal_budget.rs:208`; `baseline_captured` / `pending_continuation_ms` / `tokens_at_start` → `src/goal/store.rs` (claim pipeline); `id` → `goal.rs:909` (`previous.filter(|p| p.id != goal.id)`), `store.rs` CAS predicates; `lessons` → `pursuit.rs` `render_lessons` + `src/memory/dreaming/stages/goal_lessons_promote.rs`.

### B.3 Variant parity (every variant produced AND handled)

| Enum | Variants | Produced at | Handled at |
|---|---|---|---|
| `GoalStatus` (`types.rs:13`) | `Active` | `types.rs:243` (default), `types.rs:359` (`with_status`) | `pursuit.rs:61`, `:232`, `:366`, `:412`, `:430`; `store.rs:771`, `:956` |
| | `Paused` | `store.rs:1054` (`pause_all_active`), `store.rs:1146` (`pause_all_owned_by`), tool `with_status` | same |
| | `Blocked` | `pursuit.rs:468` (`reopen_after_gate_failure`), `store.rs:712`, `:748`, `src/goal/store.rs` exhaustion | same |
| | `Complete` | tool `with_status` (`goal.rs:1044`), `pursuit.rs:436` (`confirm_complete`) | same |
| `PursuitMode` (`types.rs:39`) | `Passive` | `types.rs:246` (default) | `pursuit.rs:58` (`should_continue` → false), `:367`, `:412`, `:429` |
| | `Active { max_iterations }` | `goal.rs:893`, `:1074` | `pursuit.rs:58`, `:172`, `:367`, `:412`, `:429`; `store.rs:923`; `context_blocks.rs:243` |
| `GateOutcome` (`types.rs:54`) | `Unchecked` (default) | `types.rs:251`, `pursuit.rs:463`, `:470` | `pursuit.rs:413` (`awaiting_gate`), `store.rs:772` |
| | `Passed` | `pursuit.rs:436` (`confirm_complete`) | `store.rs:772` |

**Name drift check**: `GoalStatus` is `#[serde(rename_all = "snake_case")]` (`types.rs:14`) and `as_str()` returns `"active"/"paused"/"blocked"/"complete"` (`types.rs:26-35`) — the serde wire form and the model-facing string agree, and the tool's `status` arg is typed `Option<GoalStatus>` (`goal.rs:53`), so the model-facing values and the parser cannot drift. `GoalSummary` (`src/gateway/handlers/kanban.rs:90-108`) serialises the enum directly, so the RPC shape inherits the same snake_case form. `PursuitMode` uses `#[serde(tag = "mode", rename_all = "snake_case")]` (`types.rs:41`) — a tagged shape, read only inside Rust (`goal.rs:374`, `:419`, `:454`, `:641`, `:701`; `store.rs:923`; `context_blocks.rs:243`). No key mismatch found.

### B.4 Constants

- `MAX_BUDGET_MEMBERS: usize = 32` (`src/goal/types.rs:223`) — enforced at its single writer `src/goal/store.rs:1217-1222` (`register_budget_member`: cap reached → `warn!` + `Ok(false)`). `BudgetMember` (`types.rs:210`) is constructed only at `store.rs:1225`.
- `MAX_LESSONS: usize = 5` (`src/goal/types.rs:228`) — enforced at `src/goal/types.rs:406-409` inside `with_lesson_appended` (push, then `drain(0..drop)` keeps the newest). `with_lesson_appended` is the **sole** lesson appender; the three other `lessons` writers (`pursuit.rs:466`, `pursuit.rs:473`, `goal.rs:1142`) all route through it. No second, uncapped writer exists.

### B.5 Global store (`src/goal/mod.rs`)

- **One production writer**: `crate::goal::init_global(goal_store.clone())` at `src/executor/builtin_registry/builder/constructor/mod.rs:495`, the only production `GoalStore::open` (over `data_dir/goals.db`). It is reached from `BuiltinToolRegistry::with_config` at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:586` and the fallback-agents path at `:1637`; every other `with_config` caller is a test. `init_global` is idempotent (`src/goal/mod.rs:39-45`, `CapabilitySlot::install` returns false on a duplicate and logs `debug!`).
- **Slot registration**: `global_slot()` (`src/goal/mod.rs:33`) is registered in the roster at `src/capability/mod.rs:361` (id `"goal/store"`, `MissingSemantics::ConsumerDecides`), and the pin is asserted by `src/capability/census.rs:3325` and by `src/goal/mod.rs:82-89`.
- **`None` tolerance**: all 23 production readers spell out their own `None` semantics — `?`/early-return for the prompt layer (`src/orchestrator/harness_bridge/context_blocks.rs:37`, `:67`, `:152` → the standing-goal block silently vanishes), fail-open for the budget gate (`src/gateway/goal_budget.rs:81`, `:140` → the gate stops gating), `Option<Arc<GoalStore>>` for the deactivation freeze (`src/gateway/handlers/users.rs:1049` → `report.goals = 0`, which reads as "owned none"), `Option` in `BackgroundWorkHandles` (`users.rs:1036-1042`), and `Idle`/no-op for the continuation hook (`goal_continuation.rs:164`, `:402`, `:583`, `:659`). **No reader panics or silently misbehaves on `None`.**
- **`set_global_for_test`** (`src/goal/mod.rs:53-57`) is `#[cfg(test)]`-gated and its only callers are `src/gateway/execution_engine/tests.rs:86`, `src/gateway/handlers/users.rs:1950`-region, `src/gateway/handlers/kanban.rs:253`-region — no production caller, as documented.

### B.6 Twin-module check (`src/goal/pursuit.rs` vs `src/looping/pursuit.rs`)

- The **only** cross-module import from the twin is `src/goal/pursuit.rs:22` — `use crate::looping::types::fmt_duration_ms;`. `fmt_duration_ms` (`src/looping/types.rs:50`) is a pure `u64 → String` formatter with **no** `LoopState` coupling, so this is a legitimate shared utility, **not** a helper applied to the wrong state type.
- `crate::goal::RearmDecision` and `crate::looping::RearmDecision` are **two distinct enums with the same name**. The goal one is consumed at `src/gateway/execution_engine/goal_continuation.rs:31`, `:661`, `:666`, `:673`; the loop one at `src/gateway/execution_engine/execute.rs:2283`, `:2299`, `:2498`, `:2524`. Every call site qualifies the path, so there is no cross-type confusion at the call boundary.
- `deadline_reached_note` has different arity in the two modules (goal takes `&Goal`, looping takes none); all call sites are correctly qualified (`goal::pursuit::deadline_reached_note(&goal)` vs `looping::pursuit::deadline_reached_note()`). No name collision is currently mis-wired.

### B.7 Driver wiring (the far end of the slice's decisions)

- `post_run` is invoked in production at `src/gateway/execution_engine/execute.rs:1347`, guarded by `self.continuation_deps.get()` and `!owned_worktree` (`execute.rs:1332`).
- `ContinuationDeps` is constructed once in production at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1241-1248`, with `gate: alephcore::verification::stop_hooks::build_from_config(&app_config.stop_hooks)` (`:1237-1238`) — so the objective gate is real, not a stub.
- `GoalWakeService` is constructed at `agent_init/mod.rs:1256-1262`, `.subscribe()` at `:1265`, `.spawn_periodic_recheck(60)` at `:1269`, and `rearm_parked_goals()` at `:1280` on a spawned task — so the wait-barrier wake path is live.
- `goal_configured = deps.gate.is_some() || peek.gate_command.is_some()` (`src/gateway/execution_engine/goal_continuation.rs:185`); gate arbitration via `crate::verification::stop_hooks::effective_gate(deps.gate.as_ref(), goal.gate_command.as_deref())` (`goal_continuation.rs:405`); claim via `store.try_claim_continuation_bound(&peek, tokens, now, gate_configured, workspace_str.as_deref())` (`goal_continuation.rs:197`) — the `workspace` argument is the one that stamps `Goal.workspace` (`src/goal/store.rs:620`).

### B.8 RPC / tool surface

- `goal.list` is registered (`src/gateway/handlers/mod.rs:1096` → `src/gateway/handlers/kanban.rs:119`) and classified in `src/gateway/method_census.rs:249` (`Class::Open`) and `src/gateway/method_visibility.rs:774` (`Treatment::ListFiltered`). The handler is real (scope-filtered, non-empty projection) — no stub far-end. No other `goal.*` RPC method is declared anywhere, so there is no unregistered client call (no Form-4 ghost).
- The `goal` tool's `GoalAction` enum is `{Set, Get, Update, Clear, List, PauseAll}` (`src/builtin_tools/goal.rs:27-48`) and every arm is implemented with a real effect; `PauseAll` (`goal.rs:1386-1400`) is operator-gated and calls the real `store.pause_all_active` (`src/goal/store.rs:1054`).
- The `goal` tool's `set` arm explicitly **refuses** the wait parameters rather than dropping them (`src/builtin_tools/goal.rs:840-846`) — see SW-S3-GOAL-API-4 for the two parameters that are *not* refused.

---

## C. New observations (not findings — context for the other slices)

1. **`src/goal/store.rs` owns two "the tool must not roll back" field classes.** `commit_field_update` (`store.rs:1497-1543`) merges the pipeline-owned fields (`pending_continuation_ms`, `continuations_used`, `tokens_at_start`, `baseline_captured`, `budget_members`, `workspace`, `owner_user_id`, `scope_id`, `author_user_id`) from the LIVE row, and on a status-CAS mismatch also takes the live lifecycle cluster (`status`, `note`, `gate_outcome`, `completed_at_ms`, and the wait-barrier fields). This is the reason the slice's `workspace` / `owner_user_id` / `scope_id` / `author_user_id` fields survive a tool update — relevant to any slice that reads them.
2. **`GoalStatus::Paused` is produced by two production paths** — the operator kill-switch (`store.rs:1054`) and the account-deactivation freeze (`store.rs:1146`) — and the second is reached from `src/gateway/handlers/users.rs:1152+` via `BackgroundWorkHandles::from_globals()` (`users.rs:1047`). Both are live; `count_owned_by` is the read-only counterpart (`store.rs:1160`-region) used by the `users.get` dossier.
3. **The tree-budget accounting is a second, independent budget surface.** `src/gateway/goal_budget.rs` (`check_and_enroll_delegation`, `tree_budget_refusal`, `with_origin_session`) is reached from `src/builtin_tools/sessions/send_tool.rs:493`, `src/builtin_tools/team/delegate.rs:405`, `src/teams/dispatcher/schedule/mod.rs:381` — and it is the only consumer of `BudgetMember` / `budget_members` besides the store's own writer. A slice auditing the delegation seam should confirm that leg; this slice only confirmed that `MAX_BUDGET_MEMBERS` is enforced at its single writer.
4. **The gate has two independent sources and an AND relationship**: the global `config.toml [[stop_hooks]]` (`ContinuationDeps.gate`) and the per-goal `gate_command` (`Goal.gate_command`, `src/goal/types.rs:96`). `gate_configured` is the OR of the two for *arming*, but the effective gate is `verification::stop_hooks::effective_gate(deps.gate.as_ref(), goal.gate_command.as_deref())` (`goal_continuation.rs:405`) — an AND at arbitration time. A per-goal `gate_command` therefore cannot *weaken* the global gate, and the per-goal command is itself ACL-guarded (`owns_reference`, `src/builtin_tools/goal.rs:1084-1097`).
5. **`Goal.workspace` is the only project-root persistence in the goal/loop pair.** `LoopState` has no workspace field (`src/looping/types.rs` has `owner_user_id`/`scope_id`/`telecom_origin` but no `workspace`), so a hook-less *loop* wake still falls back to the agent workspace. This is the asymmetry behind SW-S3-GOAL-API-3 and may matter to a slice auditing the loop twin.

---

## D. Coverage statement

**Covered** (read in full or exhaustively grepped, production vs `#[cfg(test)]` distinguished in every case):

- All 3 slice files end-to-end: `src/goal/mod.rs`, `src/goal/types.rs`, `src/goal/pursuit.rs`.
- All 24 public items of `src/goal/types.rs` (`new`, `with_wait_until`, `with_wait_on_task`, `without_wait`, `has_wait_barrier`, `with_pending_continuation`, `spent_continuation`, `with_workspace`, `with_owner_scope`, `with_author`, `with_status`, `with_note`, `with_gate_command`, `with_gate_outcome`, `with_lesson_appended`, `with_budget`, `with_pursuit`, `with_deadline_ms`, `with_baseline`, `tokens_used`, `over_budget`, `is_active`, `GoalStatus::as_str`) — each traced to ≥1 production reader with `path:line`.
- All 17 public items of `src/goal/pursuit.rs` + `WaitPark` — each traced to ≥1 production caller; the two with no *external* caller (`cap_reached_note`, `budget_reached_note`) have in-module production callers and are explicitly **not** reported as Form 1.
- All 4 `GoalStatus` / 2 `PursuitMode` / 2 `GateOutcome` variants — produced and matched.
- Both constants (`MAX_BUDGET_MEMBERS`, `MAX_LESSONS`) — enforced at their single writer; confirmed no second, uncapped writer exists.
- The global store: writer, slot registration, all 23 production readers, all 6 test-only callers, all 10 prose mentions.
- The twin module `src/looping/pursuit.rs` / `src/looping/types.rs` — the only cross-module import (`fmt_duration_ms`) verified to be state-agnostic; the duplicate `RearmDecision` names verified to be separately and correctly consumed.
- The slice's production far-end consumers: `src/goal/store.rs` (claim/commit/pause/budget paths), `src/builtin_tools/goal.rs` (all 6 actions), `src/gateway/execution_engine/goal_continuation.rs`, `goal_wait.rs`, `execute.rs`, `src/gateway/goal_budget.rs`, `src/gateway/handlers/kanban.rs`, `users.rs`, `src/gateway/continuation_lifecycle.rs`, `src/orchestrator/harness_bridge/context_blocks.rs`, `src/builtin_tools/loop_graph_manage.rs`.
- RPC surface `goal.list` — registered, classified, real handler; no other `goal.*` method is declared, so no Form-4 ghost is possible inside this slice.
- Stub sweep over the three slice files: **no** `TODO`, `FIXME`, `todo!()`, `unimplemented!()`, empty tables, or validate-then-`Ok(success)` handlers. `rg 'cfg(feature'` over the slice and its consumers: **no hits**, so no Form-6 never-compiled far-end.

**Not covered / out of scope** (stated so another slice can pick them up):

1. `src/goal/store.rs` is **not** a slice file — I read the claim/commit/pause/budget paths it exposes, but I did **not** audit the store's own public API for Form-1/Form-5 (e.g. `list_all`, `count_owned_by`, `get`, `put_with_origin`, `try_claim_settle_notify`, `release_settle_notify`, `rearm_after_busy`, `abandon_*`). Its consumers were only checked where they intersect the slice.
2. The RPC **dispatch** table was checked only for `goal.*`. Other gateway methods (e.g. `loop.*`, `session.*`) were not audited.
3. `src/gateway/goal_budget.rs` (the tree-budget seam) was verified as a *reader* of `budget_members`, but its own refusal semantics, the `with_origin_session` / `origin_session_from_metadata` chain, and the three delegation call sites were only listed, not audited.
4. The `owns_reference` governance ACL (`loop_graph` edges) was checked for existence on the `gate_command` and `objective` paths, but the governance loop's own side (who can unlink, who arbitrates proposals) was not audited.
5. The **Panel / TSX front-end** was not searched for goal-shaped consumers (the RPC `goal.list` shape is consumed server-side only in this slice's scope).
6. `src/memory/dreaming/stages/goal_lessons_promote.rs` (the nightly lesson promotion) was read only far enough to confirm it consumes `Goal.lessons` via `store.list_all()`; its promotion-to-memory far end was not audited.
7. `src/gateway/telecom/*` origin-binding tests (`src/gateway/execution_engine/tests/goal_origin_race.rs`, `goal_origin_effects.rs`) were read as evidence of the claim path's shape, but the tests themselves were not audited for whether they assert the production path.
8. No `cargo check` / `cargo test` / `cargo clippy` was run (read-only mandate), so "compiles clean" for the whole slice is inferred from the source, not from a build.
