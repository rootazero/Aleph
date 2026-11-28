# Severed-wire audit — slice `s4-goal-store`

- **Slice key:** `s4-goal-store`
- **Slice file (read in full):** `src/goal/store.rs` (3399 lines)
- **Repo root:** `/home/zou/data/workspace/Aleph` (git root, branch `main`)
- **Method:** READ FIRST (whole slice file, in order) → then `rg -n --glob '*.rs' '<symbol>' .` over the whole repo for every exported symbol → then confirm each hit's production/test status by reading the surrounding lines.
- **Read-only:** no source file was modified; no `cargo` command was run.

`GoalStore` is the far end of the goal module: a `session_id`-keyed SQLite store of standing goals
(`src/goal/store.rs:155` `pub struct GoalStore { conn: crate::sync_primitives::Mutex<rusqlite::Connection> }`),
with three tables created in `open` (`src/goal/store.rs:165`): `goals(session_id PK, json)`,
`settle_notified(session_id PK, stamp)`, `goal_telecom_origin(session_id PK, goal_id, marker)`.

---

## Section A — Findings

| id | form | severity | producer (file:line) | consumer (file:line) | evidence | required outcome |
|----|------|----------|----------------------|----------------------|----------|------------------|
| SW-S4-GOAL-STORE-1 | 1 | medium | `src/goal/store.rs:216` — `pub fn put(&self, goal: &Goal) -> Result<()>` | **no consumer found** (production) | `rg -n --glob '*.rs' 'GoalStore' crates/ shared/ interfaces/ desktop/ tools/ bindings/` → 0 hits; every goal-store `.put(` hit outside `src/goal/store.rs` is inside a `#[cfg(test)] mod tests` (`src/builtin_tools/goal.rs:1920,1962,3043,3079`; `src/gateway/handlers/kanban.rs:266,485`; `src/gateway/handlers/users.rs:2882`; `src/memory/dreaming/stages/goal_lessons_promote.rs:226`; `src/gateway/execution_engine/tests/goal_origin_race.rs:246`; `src/gateway/execution_engine/tests/goal_origin_effects.rs:572`). Production write path is `put_with_origin` only: `src/builtin_tools/goal.rs:925` (pinned by `src/gateway/telecom/run_origin.rs:223` census `production_files_containing(".put_with_origin(", "goal/store.rs")` → exactly 1 hit). | CUT |
| SW-S4-GOAL-STORE-2 | 1 | medium | `src/goal/store.rs:267` — `pub fn origin_for(&self, goal: &Goal) -> Result<crate::gateway::telecom::MarkerRestore>` | **no consumer found** (production) | `rg -n --glob '*.rs' 'origin_for' .` → 16 hits total, ALL non-production: 9 inside `src/goal/store.rs` tests (`:2471,2488,2501,2507,2526,2542,2758,2775,2795`), 5 inside `src/builtin_tools/goal.rs` tests (`:1522,1536,1545,1564,1584`; `#[cfg(test)] mod tests` starts at `:1418`), 1 doc mention (`:271`), 1 unrelated test *name* (`src/approval/audit.rs:88`). The capability itself is live through the private `origin_locked` (`src/goal/store.rs:274`) / `live_origin_locked` (`:306`) used by `try_claim_continuation_bound` (`:490`), `bound_origin` (`:518`) and `live_origin_is` (`:330`, used at `:866,1364,1442`). | CUT |
| SW-S4-GOAL-STORE-3 | 1 | low | `src/goal/store.rs:440` — `pub fn try_claim_continuation(...)` (the **unbound** claim) | **no consumer found** (production) — deliberate quarantine | `rg -n --glob '*.rs' 'try_claim_continuation' .` → 16 hits, of which the only *calls* are `src/gateway/execution_engine/tests.rs:2567,2663,2965,3069,3138`; all other hits are doc comments (`src/goal/types.rs:114`, `src/goal/pursuit.rs:8,420`, `src/gateway/execution_engine/goal_wait.rs:8`, `src/gateway/execution_engine/execute.rs:1325,1651`, `src/gateway/execution_engine/goal_continuation.rs:5,171`). Deliberately enforced by census test `src/gateway/execution_engine/tests/goal_origin_race.rs:661-681` `no_production_code_outside_the_store_calls_the_unbound_claim` (uses `crate::utils::source_scan::files_whose_production_code_reads`, `src/utils/source_scan.rs:1168-1193`; word-boundary aware via `mentions_identifier`, `:1198-1208`, so `try_claim_continuation_bound` does not trip it). Private `claim_locked` (`src/goal/store.rs:534`) is reached only from `try_claim_continuation` (`:449`); `claim_goal_locked` (`:555`) is shared with the bound claim (`:494`). | DECIDE |

### SW-S4-GOAL-STORE-1 — `GoalStore::put` is a public writer with zero production readers, and it is the *wrong* writer

**Producer.** `src/goal/store.rs:216`:

```rust
/// Upsert the goal for its session (replaces any existing one).
pub fn put(&self, goal: &Goal) -> Result<()> {
    Self::put_locked(&self.lock(), goal)
}
```

**Consumer.** None in production. The only production writer of the `goals` row is
`put_with_origin` (`src/goal/store.rs:235`), called from exactly one production site —
`src/builtin_tools/goal.rs:925` — and the repo pins that with a census
(`src/gateway/telecom/run_origin.rs:223`). Every other `.put(` in the repo that reaches a
`GoalStore` is inside a `#[cfg(test)] mod tests` block (listed in the evidence column); the
workspace contains no other crate that names `GoalStore` at all
(`rg -n --glob '*.rs' 'GoalStore' crates/ shared/ interfaces/ desktop/ tools/ bindings/` → 0 hits).

**What is missing.** The arm that would make `put` live is absent: no production path writes a
goal through it. That is harmless *today* only because `put_with_origin` happens to be the
writer.

**Why it matters.** `put` is not merely redundant, it is **semantically different from the
production writer** in a way that is invisible until someone calls it:

- `put_locked` (`src/goal/store.rs:396`) writes **only** the `goals` table. It never touches
  `goal_telecom_origin`. `put_with_origin` (`:235`) writes the goal row *and* the origin row in
  one transaction (`:255`).
- So a goal written via `put` inherits whatever origin row that session already had:
  - a session that previously had an origin row keeps the **stale** marker. `origin_locked`
    (`:274`) compares the row's `goal_id` with `goal.id` and returns
    `MarkerRestore::Refuse("goal telecom origin belongs to a different goal; refusing to run it
    as ordinary")` when they differ (`:288-293`). `live_origin_locked` (`:306`) turns that into
    `Err(reason)`, and every claim path treats `Err` as fail-closed
    (`try_claim_continuation_bound` at `:490`; `bound_origin` at `:518`;
    `live_origin_is` at `:330`). The goal would therefore be **permanently unable to fire** —
    blocked with no diagnosis beyond "untrustworthy origin".
  - a session that never had an origin row reads as `MarkerRestore::LegacyUnproven` (`:286`),
    which restores as `None` — a **public** goal silently downgraded to ordinary.

`put` is the only *public* way to write a goal row, so it is the API a future caller (or a
downstream crate) is most likely to reach for, and it is the one that produces a broken or
silently-downgraded goal. The live capability it shadows is already fully covered by the private
`origin_locked` / `live_origin_locked` helpers.

**Minimal fix (CUT).** Make `put` non-public (e.g. `fn` / `pub(crate)`) or delete it, and route
any future writer through `put_with_origin`. If it must stay public for a reason, rename it to
something that states the hazard (`put_without_origin`) and make it write `LegacyUnproven`
explicitly rather than leaving a stale row behind.

### SW-S4-GOAL-STORE-2 — `GoalStore::origin_for` is a public reader with zero production readers

**Producer.** `src/goal/store.rs:267`:

```rust
pub fn origin_for(&self, goal: &Goal) -> Result<crate::gateway::telecom::MarkerRestore> {
    Self::origin_locked(&self.lock(), goal)
}
```

**Consumer.** None in production. All 16 repo hits are non-production: 9 in the store's own
`#[cfg(test)] mod tests` (starts `src/goal/store.rs:1598`), 5 in `src/builtin_tools/goal.rs`'s
test module (starts `:1418`), one doc line (`:271`), and one unrelated test *name*
(`src/approval/audit.rs:88`). The underlying capability is live via the private
`origin_locked` (`:274`) and `live_origin_locked` (`:306`), used by `try_claim_continuation_bound`
(`:490`), `bound_origin` (`:518`) and `live_origin_is` (`:330`, used at `:866`, `:1364`, `:1442`).

**What is missing.** The arm that would make the *public* wrapper live. The wrapper's own doc
(`:271`) says it exists for "an already-held connection lock" — but that is exactly the private
`origin_locked` it delegates to, and the locked variant is what the live claim paths use.

**Why it matters.** The *feature* is not dead (origin resolution is exercised on every bound
claim), so this is a public-surface / maintenance hazard rather than a dark feature. `origin_for`
is safe on its own — it takes the lock and validates the marker against `goal.id`, so it never
launders an origin — but it reads the origin **outside** the claim's critical section. A caller
that did `get()` → (await) → `origin_for()` would reintroduce exactly the snapshot-then-read gap
the store's bound claim exists to close. The repo already recognises that class of mistake: the
unbound `try_claim_continuation` is quarantined by a census test for precisely this reason
(`src/gateway/execution_engine/tests/goal_origin_race.rs:661-681`), and there is **no** such
census for `origin_for` — the second, quieter half of the same hazard.

**Minimal fix (CUT).** Make `origin_for` private (`pub(crate)` / `fn`) so the only origin reads
are the ones taken under the claim's lock, and (optionally) extend the existing census
(`files_whose_production_code_reads`) to cover `origin_for` as well, so the hazard cannot
re-enter through a renamed copy.

### SW-S4-GOAL-STORE-3 — the unbound `try_claim_continuation` is public but deliberately quarantined (no production reader)

**Producer.** `src/goal/store.rs:440` `pub fn try_claim_continuation(&self, session_id, tokens, now_ms, gate_configured, workspace) -> Result<ContinuationDecision>`.

**Consumer.** None in production. Every repo hit is either a doc comment or a test call
(`src/gateway/execution_engine/tests.rs:2567,2663,2965,3069,3138`).

**What is missing.** Any production caller — and that is the *intent*, not an accident: the
store's doc for the bound sibling (`try_claim_continuation_bound`, `:475`) explains that the
unbound form re-opens the snapshot-then-claim gap. The quarantine is enforced mechanically by
`src/gateway/execution_engine/tests/goal_origin_race.rs:661-681`, which scans production code for
the identifier and fails if anything outside `goal/store.rs` mentions it. The scanner is
word-boundary aware (`src/utils/source_scan.rs:1198-1208`), so the live `try_claim_continuation_bound`
does not trip it.

**Why it matters (low).** Unlike findings 1 and 2, this one is *guarded*: the hazard is
documented, the dangerous form is unreachable, and a test will redden the moment a caller
appears. It is still a public API with zero production readers, and the private `claim_locked`
(`:534`) exists only to serve it, so the public surface is wider than the live behaviour. The
residual risk is that a future reader adds `#[cfg(test)]`-adjacent code, or a caller that the
scanner cannot see (e.g. a macro-generated call, or a call through a re-export), slips the
census.

**Minimal fix (DECIDE).** Either (a) shrink it to `fn` and delete the now-unreachable
`claim_locked` wrapper, keeping `claim_goal_locked` for the bound path — the census test then
becomes a tautology rather than a guard; or (b) keep it deliberately as a documented trap and
add a line to the module doc saying so explicitly, so a future reader knows the quarantine is
intentional rather than an oversight. No behavioural change is required either way.

---

## Section B — Clean (verified no wire)

Exported symbols of `src/goal/store.rs`, each confirmed to have a **production** consumer
(non-`#[cfg(test)]`) elsewhere in the repo:

- `GoalStore::open` — `src/executor/builtin_registry/builder/constructor/mod.rs:488` (the only production `GoalStore::open`; `init_global` at `:495`)
- `GoalStore::put_with_origin` — `src/builtin_tools/goal.rs:925` (only production goal-row writer; census-pinned at `src/gateway/telecom/run_origin.rs:223`)
- `GoalStore::get` — `src/orchestrator/harness_bridge/context_blocks.rs:37,67,152`; `src/gateway/goal_budget.rs:84,141`; `src/gateway/execution_engine/goal_continuation.rs:172`; `src/builtin_tools/goal.rs:233,855,967,1280`; `src/builtin_tools/loop_graph_manage.rs:331`
- `GoalStore::delete` — `src/builtin_tools/goal.rs:1302` (`goal` action=`clear`); deletes goal + origin + settle rows in one tx (`src/goal/store.rs:347-370`)
- `GoalStore::list_all` — `src/gateway/handlers/kanban.rs:127` (RPC `goal.list`, handler registered `src/gateway/handlers/mod.rs:1096`, Panel client `interfaces/webchat/src/api/kanban.rs:60` — no client ghost); `src/gateway/execution_engine/goal_wait.rs:147,173,315`; `src/memory/dreaming/stages/goal_lessons_promote.rs:133` (stage registered in production at `src/memory/dreaming/mod.rs:302`)
- `GoalStore::try_claim_continuation_bound` — `src/gateway/execution_engine/goal_wait.rs:443`; `src/gateway/execution_engine/goal_continuation.rs:197`
- `GoalStore::bound_origin` — `src/gateway/execution_engine/goal_wait.rs:227`
- `GoalStore::commit_gate_pass` — `src/gateway/execution_engine/goal_continuation.rs:428,459`
- `GoalStore::try_claim_settle_notify` — `src/gateway/execution_engine/goal_continuation.rs:242,441`; `src/builtin_tools/goal.rs:1247`
- `GoalStore::release_settle_notify` — `src/builtin_tools/goal.rs:1255`; `src/gateway/execution_engine/goal_continuation.rs:249`
- `GoalStore::claim_after_gate_veto` — `src/gateway/execution_engine/goal_continuation.rs:486`
- `GoalStore::block_if_active` — `src/gateway/execution_engine/execute.rs:1983,2405`; `src/gateway/execution_engine/goal_continuation.rs:622`
- `GoalStore::block_if_abandonable` — `src/gateway/continuation_lifecycle.rs:87`
- `GoalStore::block_if_not_terminal` — `src/gateway/continuation_lifecycle.rs:57`
- `GoalStore::pause_if_active` — `src/builtin_tools/goal.rs:328` (the `false` arm reports the real status, `goal.rs:345-355` — not a silent no-op)
- `GoalStore::park_if_active` — `src/gateway/execution_engine/goal_continuation.rs:598`
- `GoalStore::pause_all_active` — `src/builtin_tools/goal.rs:1396` (operator-gated at `goal.rs:1387`)
- `GoalStore::pause_all_owned_by` — `src/gateway/handlers/users.rs:1155`
- `GoalStore::count_owned_by` — `src/gateway/handlers/users.rs:1267`
- `GoalStore::register_budget_member` — `src/gateway/goal_budget.rs:106`
- `GoalStore::supersede_wait_timer` — `src/builtin_tools/goal.rs:1221`
- `GoalStore::clear_wait_barrier` — `src/gateway/execution_engine/goal_wait.rs:384`
- `GoalStore::confirm_fire` — `src/gateway/execution_engine/goal_wait.rs:564` (the *blocking* arm `arbitrate_out_of_bounds`; no run is spawned, so no origin is needed)
- `GoalStore::confirm_fire_for_origin` — `src/gateway/execution_engine/execute.rs:1905` (census-pinned at `src/gateway/execution_engine/tests/goal_origin_race.rs:684` — the goal arm in `execute.rs` must use the origin-bound form and must NOT contain `store.confirm_fire(`)
- `GoalStore::rearm_after_busy` — `src/gateway/execution_engine/goal_continuation.rs:660`
- `GoalStore::commit_field_update` — `src/builtin_tools/goal.rs:1191` (all three outcomes — `Committed` / `Gone` / `StatusSuperseded` — consumed at `goal.rs:1192-1216`)

Wires verified clean beyond the symbol sweep:

- **Session-key determinism (no Form-5 drift).** Write side: `GoalTool::session()` (`src/builtin_tools/goal.rs:190-202`) → `crate::tools::turn_context::current_session_key()` → `t.session_key.to_key_string()` (`src/tools/turn_context.rs:254-258`). Read side: `src/gateway/execution_engine/goal_continuation.rs:161` → `session_key.to_key_string()` (`src/routing/session_key.rs:515`, `Main` ⇒ `format!("agent:{agent_id}:{main_key}")` + epoch). Same derivation on both ends ⇒ the store's `session_id` PK is the same string on both sides.
- **Loop-graph node id ↔ store key.** `goal_node_id(session)` ⇒ `format!("goal:{session}")` (`src/loop_graph/service.rs:845-847`) vs `n.id.trim_start_matches("goal:")` (`src/builtin_tools/loop_graph_manage.rs:331`) — same literal shape.
- **Cross-session target key.** `goal(action='list')` prints `goal.session_id` verbatim (`src/builtin_tools/goal.rs:447-452`) and `session=` is resolved by `store.get(target)` (`goal.rs:231-240`) ⇒ the printed key is the lookup key.
- **Event emit-vs-subscribe parity.** The store emits **no** bus events (no `Goal*` event variant exists anywhere in the repo). Its event analogue is the settle-notify CAS, and that chain is fully wired: `try_claim_settle_notify` → `crate::loop_graph::service::notify_goal_settled` (`src/loop_graph/service.rs:488`) → `notify_node_settled` → watcher cron poke; release path `notify_goal_settled == false` → `release_settle_notify` (`goal_continuation.rs:249`, `goal.rs:1255`). The task-settle subscriber the wake service uses (`src/gateway/execution_engine/goal_wait.rs:91-137`) matches real emitted verbs (`TeamTaskCompleted` / `TeamTaskFailed` / `TeamTaskUpdated` with `cancelled`/`skipped`), and `Unsatisfiable` is explicitly derived-not-emitted with the periodic recheck as the backstop (`goal_wait.rs:283-297,344-372`).
- **Config-reader parity.** The store reads **no** config at all — the only knobs are hardcoded consts (`PENDING_STALE_GRACE_MS = 60_000`, `BUSY_RETRY_DELAY_MS = 30_000`, `GATE_STALE_GRACE_MS = 900_000` at the top of `src/goal/store.rs`; `MAX_BUDGET_MEMBERS: usize = 32` at `src/goal/types.rs:223`). The one config-shaped input, `gate_configured: bool`, is passed in by the callers (`goal_continuation.rs:186`, `goal_wait.rs:427`) and the global gate it depends on **is** populated in production: `alephcore::verification::stop_hooks::build_from_config(&app_config.stop_hooks)` → `ContinuationDeps { gate: goal_gate, .. }` (`src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1240-1247`). ⇒ no inert config (Form 3) in this slice.
- **No SQL injection / unsafe query construction.** 20 SQL executions in the file; 0 use `format!` interpolation (`rg -n 'execute\(&format!|query_row\(&format!|prepare\(&format!' src/goal/store.rs` → 0 hits). All 38 `format!` uses are error strings or the settle stamp (`src/goal/store.rs:1589-1595`).
- **No lock-across-await, no lock-ordering hazard.** The store holds exactly one `std::sync::Mutex` (`crate::sync_primitives::Mutex` is `std::sync::Mutex`, `src/sync_primitives.rs:49-51`), the guard is private (`fn lock`, `src/goal/store.rs:205`), and **every** store method is synchronous — so no guard can be held across an `.await`, and no external caller can hold one. The store never calls out while holding the lock, and its consumers hold no other lock across a store call (`src/gateway/goal_budget.rs:106` — no lock; `src/gateway/handlers/users.rs:1155` — inside `freeze_owned_background_work_with`, no lock held; the `cron.lock().await` at `users.rs:1187` is a different store's lock). Poison is swallowed by design (`src/goal/store.rs:205-207`).
- **No stub sweep hits.** `rg -n 'todo!|unimplemented!|TODO|FIXME|XXX|HACK' src/goal/store.rs` → 0 hits. Every `Ok(false)` / `Ok(None)` arm has a consuming branch that either acts or reports honestly (e.g. `goal.rs:345-355`, `goal_continuation.rs:607,628`).
- **Form 6 impossible.** `src/goal/store.rs` contains no `#[cfg(feature = ...)]` block; the only `#[cfg]` gates in the file are `#[cfg(test)]` (`:336` `corrupt_origin_marker_for_test`, and the test module at `:1598`).
- **Owner / namespace isolation.** `pause_all_owned_by` (`:1113`) and `count_owned_by` (`:1169`) use `aleph_protocol::users::owned_by(goal.owner_user_id.as_deref(), user_id)` (`shared/protocol/src/users.rs:191` ⇒ `owner_user_id == Some(user_id)`); a `None`-owner (unscoped) goal is owned by nobody and is therefore never frozen by the deactivation path — documented as deliberate at `src/goal/store.rs:1093-1105`.

---

## Section C — New observations

Adjacent items noticed but **not** classified as severed wires in this slice.

- `[severity-guess: low]` `src/goal/mod.rs:26` — the `GLOBAL` slot doc claims "**21** production call sites" for `crate::goal::global()`. The actual production count is **23** (`src/builtin_tools/loop_graph_manage.rs:303`; `src/orchestrator/harness_bridge/context_blocks.rs:37,67,152`; `src/gateway/execution_engine/goal_wait.rs:144,169,312,383,415,558`; `src/gateway/execution_engine/execute.rs:1904,1977,2391`; `src/gateway/goal_budget.rs:81,140`; `src/gateway/execution_engine/goal_continuation.rs:164,402,583,659`; `src/gateway/continuation_lifecycle.rs:51,81`; `src/gateway/handlers/kanban.rs:123`; `src/gateway/handlers/users.rs:1049`). The count has drifted since the line was written (`git log -L 24,28:src/goal/mod.rs` → `979461165`, 2026-08-25). The *argument* the count supports is unaffected; the number itself is stale.
- `[possible] [severity-guess: low]` `GoalTool::with_session_key_handle` (`src/builtin_tools/goal.rs:152`) is never called on a `GoalTool`: the production construction at `src/executor/builtin_registry/builder/constructor/mod.rs:503` is `GoalTool::new(goal_store).with_lesson_indexer(...)` with no session-key handle (the `.with_session_key_handle(...)` calls at `constructor/mod.rs:1259,1273,1276,1279` are for other tools). ⇒ `GoalTool.session_key` is always `None` in production, so the `Some(h)` arm of `GoalTool::session()` (`src/builtin_tools/goal.rs:197-200`) is dead, and the only reachable fallback is `None => String::new()` — i.e. a `goal` tool call dispatched **without** a turn context (a raw `tools.invoke` compatibility call, which per `src/tools/turn_context.rs:8-9` does not install the context) would key its goal row on the empty string `""`. Not a severed wire in this slice (the store's key handling is agnostic), but worth a look by whoever owns `builtin_tools/goal.rs`.
- `[possible] [severity-guess: low]` The loop registry (`src/looping/mod.rs`) exposes an identically-named method family (`commit_field_update`, `rearm_after_busy`, `pause_all_owned_by`, `count_owned_by`, `confirm_fire`) on a *different* type. Grep-by-name across the two registries is therefore ambiguous — the goal-store readers are distinguished only by receiver type (`src/looping/mod.rs` hits are `reg.` / `registry.`; the goal-store hits are `store.` / `goal_store.` / `self.store.`). A future name-collision between the two stores would compile fine.
- `MAX_BUDGET_MEMBERS: usize = 32` (`src/goal/types.rs:223`) and the three grace constants in `src/goal/store.rs` are hardcoded, not config knobs. There is therefore no inert-config candidate (Form 3) to report for this slice — but equally no way to tune them without a code change.
- `GoalStore::get` maps a corrupt JSON row to `Ok(None)` (`src/goal/store.rs:373-395`) and `list_all` skips corrupt rows. This is a deliberate fail-safe, and it is *recoverable*: the next `put_with_origin` overwrites the row via `INSERT … ON CONFLICT DO UPDATE` (`:404-405`).
- `GoalStore::delete` removes the `goals`, `goal_telecom_origin` and `settle_notified` rows in a single transaction with an explicit `tx.commit()` (`:347-370`) — no write path in the file ends without a commit.

---

## Section D — Coverage statement

**Slice files read in full**
- `src/goal/store.rs` — 3399 lines, read end to end in order (enums `ContinuationDecision`/`BoundClaim`/`BoundOrigin`/`RearmDecision`/`FireDecision`/`FieldUpdate`; `GoalStore` + all 30 public methods; the private helpers `lock`/`origin_locked`/`live_origin_locked`/`live_origin_is`/`get_locked`/`put_locked`/`claim_locked`/`claim_goal_locked`/`same_goal`/`still_awaiting_gate`/`confirm_fire_inner`/`settle_stamp`; the `#[cfg(test)] mod tests` block at `:1598-3399`).

**Files read (in the parts that matter) while tracing the wires**
- `src/goal/mod.rs` (full), `src/goal/types.rs` (owner/scope fields, `MAX_BUDGET_MEMBERS`), `src/goal/pursuit.rs` (gate/`gateless_terminal_complete` doc references)
- `src/builtin_tools/goal.rs` (tool struct, `session()`, `resolve_target`, `set`/`update`/`clear`/`list`/`pause_all` arms, test module boundary at `:1418`)
- `src/gateway/execution_engine/goal_continuation.rs` (`post_run` `:157-`, Idle/Fire/Exhausted/AwaitingGate arms, `arbitrate_gate` `:389-510`, `block_goal_on_failure` `:577-645`, `rearm_goal_after_busy` `:655-680`)
- `src/gateway/execution_engine/goal_wait.rs` (`GoalWakeService`, `subscribe` `:91-137`, `wake_task_barriers` `:141-166`, `rearm_parked_goals` `:168-`, `recheck_task_barrier` `:348-376`, `wake` `:379-410`, `arbitrate_out_of_bounds` `:551-600`)
- `src/gateway/execution_engine/execute.rs` (`confirm_fire_for_origin` `:1905`, `block_if_active` `:1983,2405`)
- `src/gateway/continuation_lifecycle.rs` (`:41-95`), `src/gateway/goal_budget.rs` (`:81-116,140`), `src/gateway/handlers/users.rs` (`:1049,1155,1267`), `src/gateway/handlers/kanban.rs` (`:112-152`), `src/gateway/handlers/mod.rs:1096` (RPC registration)
- `src/gateway/telecom/run_origin.rs:223` (the `put_with_origin` census), `src/gateway/execution_engine/tests/goal_origin_race.rs:650-700` (the unbound-claim and origin-bound-confirm censuses)
- `src/executor/builtin_registry/builder/constructor/mod.rs:480-520` (production `GoalStore::open` + `GoalTool` construction), `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1230-1270` (`ContinuationDeps.gate` wiring + `GoalWakeService` boot)
- `src/memory/dreaming/stages/goal_lessons_promote.rs` (stage + `promote_one`), `src/memory/dreaming/mod.rs:302` (stage registration)
- `src/routing/session_key.rs:470-540` (`to_key_string`), `src/tools/turn_context.rs:1-120,250-260`, `src/loop_graph/service.rs:840-860` (`goal_node_id`), `src/loop_graph/service.rs:488` (`notify_goal_settled`)
- `src/utils/source_scan.rs:1150-1220` (the census helper's production-code and word-boundary semantics), `src/sync_primitives.rs:49-51` (mutex type), `shared/protocol/src/users.rs:191` (`owned_by`)

**Lenses applied to this slice**
1. **Registration parity** — the goal subsystem has one RPC (`goal.list`, registered `src/gateway/handlers/mod.rs:1096`, client `interfaces/webchat/src/api/kanban.rs:60`) and one tool (`goal`). Both ends present; no ghost.
2. **Call-vs-handler parity** — every store method call site was matched to its defining method; every consumer arm (`Ok(true)` / `Ok(false)` / `Ok(None)` / `Err`) was read to confirm it is acted upon rather than dropped.
3. **Classifier-vs-handler parity** — the store contains no rate-limiter / lane / permission classifier; the nearest analogue is the permission filter on the `goal` tool (`caller_is_operator`, `src/builtin_tools/goal.rs:211-214`) gating the two cross-session verbs (`session=`, `pause_all`). Both gates are present and reach the store only after the check (`goal.rs:224-241`, `:1387-1396`). No destructive method lands in a loose bucket.
4. **Event emit-vs-subscribe parity** — the store emits no bus events; the settle-notify CAS chain and the `GoalWakeService` task-settle subscription were both traced end to end (see Section B). No subscriber waits on an event the store never produces.
5. **Config-reader parity** — the store reads no config; the `gate_configured` input and `deps.gate` were traced back to `app_config.stop_hooks` in production. No dead knob.
6. **Path / route parity** — the session-key derivation, the loop-graph node id (`goal:<session>`) and the `goal.list` session id were checked for literal agreement on both sides of every boundary. No drift found.
7. **Stub sweep** — `todo!` / `unimplemented!` / `TODO` / `FIXME`: 0 hits in the slice. `Ok(false)` / `Ok(None)` fallbacks: each read and found to have a consuming branch. DB queries whose result is never read: none (`list_all` is consumed at `goal.rs:1342`, `kanban.rs:127`, `goal_wait.rs:147,173,315`, `goal_lessons_promote.rs:133`).

**Seams NOT covered (out of slice / out of budget)**
- The full bodies of `src/gateway/execution_engine/execute.rs` (~2400 lines) and `src/builtin_tools/goal.rs` (~3300 lines) were read only at the store call sites and their immediate arms; their internal logic was not audited for severed wires of their own.
- `src/gateway/telecom/` (`MarkerRestore`, `PersistedTelecomMarker::restore`, `run_origin`) was read only where the store's origin rows are produced/consumed; the telecom origin marker format itself was not audited.
- `src/looping/mod.rs` (the loop registry, which mirrors many of the goal store's method names) was grepped for name collisions only, not read.
- `src/memory/dreaming/` was checked for the goal-lessons stage registration only; the rest of the dreaming pipeline was not audited.
- The Panel/TSX side was checked only for the `goal.list` RPC call (`interfaces/webchat/src/api/kanban.rs:60`); other Panel consumers of goal state were not audited.
- No `cargo check` / `cargo test` / `clippy` was run (forbidden by the brief); "dead code" was established by `rg` + reading, not by the compiler.
