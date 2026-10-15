# Task 4 — goal/loop telecom origin slice (bounded; Task 4 is NOT complete)

Branch feat/pstn-sms-thin-relay. Scope: stop autonomous goal / loop / wake runs from
taking their telecom provenance from "the most recently completed RunRequest"
(review N1: None-owner turn laundering Some, or Some narrowing None). No private
reads, approvals or tool consumers were enabled.

## What changed
- `gateway/telecom/run_origin.rs` (new): task-local run origin (Unseeded / Ordinary / Public).
  `seed_run_origin` has ONE production caller: `run_loop::with_request_scope`, from
  `request.telecom_public` (census-pinned). Reader `current_run_origin()` is used only by
  `goal set` and `loop start`. Seed future is `Box::pin`ed (un-boxed it overflowed the
  debug test-thread stack in btw_promote tests).
- `scope/carried.rs`: `CarriedAttribution` captures/re-establishes the origin
  (`capture_carry`/`reestablish_carry`, carrier-only, census-pinned), so every spawn that
  already carries scope/author (subagent, batch, team fan-out) carries origin. Unseeded stays unseeded.
- `goal/store.rs`: independent table `goal_telecom_origin(session_id PK, goal_id, marker)`;
  marker = `PersistedTelecomMarker` JSON. `put_with_origin` writes goal row + origin row in
  one lock/transaction (None recorded as Ordinary, so a new objective under None REPLACES a
  public origin). `delete` removes both in one transaction. `origin_for(&Goal)` validates the
  goal id: missing row -> LegacyUnproven (restores None, stated limitation); id mismatch,
  unknown or corrupt marker -> Refuse. Plain `put`, atomic claim helpers and
  `commit_field_update` leave the origin row untouched. `Goal` JSON/schema unchanged
  (test pins no region/binding/telecom). `PersistedTelecomMarker::capture` census now = exactly
  {busy_queue/durable.rs, goal/store.rs}.
- `builtin_tools/goal.rs`: `set` calls `put_with_origin(&goal, current_run_origin())`; `GoalArgs`
  has no origin surface (forged extra keys ignored; schema pinned).
- `looping`: in-memory `LoopState.telecom_origin` (+`with_telecom_origin`, sole prod caller
  `loop_manage start`); `commit_field_update` restores the live value; `transition`
  (pause/resume) clones live; `put` (restart) replaces.
- `execution_engine`: `goal_continuation::post_run` no longer takes the completing request's
  context; it uses the goal creator origin via `resolve_goal_origin` (Refuse/error -> warn, no
  claim, no run). Loop tick uses `state.telecom_origin`. `goal_wait::spawn_wake_run` restores the
  goal origin (Refuse -> wake dropped) instead of hardcoded `None`. A census pins that
  execute.rs production no longer reads `request.telecom_public`.

## Tests (all green unless noted)
run_origin (carry across spawn, unseeded stays unseeded, 4 censuses), goal::store origin tests (8),
builtin_tools::goal origin tests (5), looping + loop_manage origin tests (4), execution_engine
post_run tests (public goal + ordinary completing turn -> Some; ordinary goal -> None; corrupt
origin -> no claim, no run). Ran: `cargo test --lib` filters run_origin / goal::store /
builtin_tools::goal / builtin_tools::loop_manage / looping:: / persisted_marker /
gateway::execution_engine / goal_wait / scope:: / agents::subagent_tool / agents::subagent_spawner /
teams::broadcast / gateway::telecom / gateway::busy_queue / census. rustfmt --edition 2021 on touched files.
Reds seen, none touching this diff: `btw_wire_tests::no_shipped_command_word...` (skills submodules
not initialised), `run_loop::tests::project_skill_block_none_when_no_project_skills` (reads the
user's home skills dir), `gateway::visibility::every_ambient_owner_call_site_is_in_the_census`
(caller_identity.rs:375, untouched). I did not re-run them on a clean baseline.

## NOT done / residual risks
- Bare `tokio::spawn` that does not go through `CarriedAttribution` still loses the origin
  (same class as scope/author). A goal/loop created from such a task reads Unseeded -> recorded
  as ordinary. I did NOT add a deny: unseeded is also the legitimate state of non-run callers and
  of many existing tests, and I could not enumerate/prove every spawn site in this slice. This is
  fail-open in that one direction; must be closed (audit spawn sites, or make public-context
  creation deny when a spawn is detected) before any consumer trusts `None`.
- Goals created before this change (no origin row) restore as None (LegacyUnproven).
- Refused goals (corrupt/foreign origin) stay Active but never continue; no auto-block/notice.
- Loop tick reads origin from the `get_active` snapshot before `try_claim_tick`; a restart
  racing in between could pair an old origin with the new loop's tick (tiny window, in-memory).
- Same-origin Some/Some across different bindings is still not isolated (pre-existing known gap).
- Other None producers (sessions_send, announce, teams, a2a, cron, openai_api,
  resume_coordinator.rs:992, handlers/agent.rs) unchanged. Loop registry is in-memory, so a
  restart drops loops and their origin by design.
- Task 4 as a whole (private reads, approvals, tool consumers) is not complete.
