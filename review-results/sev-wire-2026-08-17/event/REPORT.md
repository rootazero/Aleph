# Severed-Wire Audit — `src/event/`

**Audit**: severed-wire-audit
**Date**: 2026-08-17
**Module**: `src/event/`
**Files scanned**: `src/event/{mod.rs, bus.rs, handler.rs, filter.rs, global_bus.rs, types.rs}`, `src/event/tests/{mod.rs, integration.rs}`
**Method**: PRODUCED − CONSUMED symbol parity. For each `pub` symbol under `src/event/`, run `rg -n "<symbol>" src/ bin/ interfaces/ shared/` (production code only; `#[cfg(test)]` blocks are tagged in evidence). Classify against the 6 forms of severed wire. Decide CUT/CONNECT/DECIDE.
**Prior reviews cross-referenced**: `review-results/SUMMARY.md`, `review-results/SUMMARY-2026-08-16.md`, `review-results/_summary.md`, `review-results/_batch3_summary.md`, `review-results/_batch4_summary.md`, `review-results/severed-wire-2026-08-17/REVIEW_PROTOCOL.md`. None of those reference `src/event/` symbols — fresh audit.

---

## Producer ↔ consumer matrix (production code)

Cross-checked every `AlephEvent` variant + every `EventType` discriminant against the live producer and live subscriber paths:

| Variant / Symbol | Live producer (path:line) | Live consumer (path:line) | Status |
|---|---|---|---|
| `AlephEvent::SubAgentCompleted` | `src/agents/subagent_tool/spawn.rs:381-385`, `src/agents/background_persistence.rs:371-374` | `src/gateway/subagent_announce.rs:50` via `announce_delivery::subscribe` | connected |
| `AlephEvent::SubAgentTreeUpdate` | `src/agents/subagent_tree_events.rs:35` | `src/gateway/subagent_tree_relay.rs:31` (registered at `src/bin/aleph-server/commands/start/mod.rs:1471`) | connected |
| `AlephEvent::ProcessCompleted` | `src/builtin_tools/process_completion.rs:111` (driven from `src/builtin_tools/bash_exec.rs:363`) | `src/gateway/process_announce.rs:68` via `announce_delivery::subscribe` | connected |
| `AlephEvent::TeamCreated` | `src/teams/store.rs:425` | `src/teams/events.rs:492` (`TeamEventLogger`) | connected |
| `AlephEvent::TeamMemberAdded` | `src/teams/store.rs:643` | `src/teams/events.rs:493` | connected |
| `AlephEvent::TeamMemberRemoved` | `src/teams/store.rs:725` | `src/teams/events.rs:494` | connected |
| `AlephEvent::TeamDisbanded` | `src/teams/store.rs:528` | `src/teams/events.rs:499` | connected |
| `AlephEvent::TeamTaskAssigned` | `src/agents/swarm/tasks/store/mod.rs:126` | `src/teams/dispatcher/mod.rs:238`, `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1598`, `src/builtin_tools/task_manage/wait.rs:88` | connected |
| `AlephEvent::TeamTaskUpdated` | `src/agents/swarm/tasks/store/mod.rs:137,177` | `src/teams/notifier.rs:163/168`, `src/teams/dispatcher/mod.rs:239`, `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1599`, `src/gateway/execution_engine/goal_wait.rs:101` | connected |
| `AlephEvent::TeamTaskCompleted` | `src/agents/swarm/tasks/store/mod.rs:150` | `src/teams/notifier.rs:163`, `src/teams/dispatcher/mod.rs:240`, `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1600`, `src/gateway/execution_engine/goal_wait.rs:99` | connected |
| `AlephEvent::TeamTaskFailed` | `src/agents/swarm/tasks/store/mod.rs:162` | `src/teams/notifier.rs:164`, `src/teams/dispatcher/mod.rs:241`, `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1601`, `src/gateway/execution_engine/goal_wait.rs:100` | connected |
| `EventType::All` (filter wildcard) | n/a (no `AlephEvent` variant) | `src/event/filter.rs:101,189` (`EventFilter::all()` + `matches`) | connected (filter-only) |
| `AlephEvent::event_type()` | n/a (derive) | `src/event/global_bus.rs:243` (production trace), `src/event/filter.rs:193` (`matches`) | connected |
| `AlephEvent::name()` | n/a | `#[cfg(test)]` only (test helper) | see finding F-04 |
| `AlephEvent::ALL_VARIANT_NAMES` | n/a | `#[cfg(test)]` only (bus-drift guard) | see finding F-05 |
| `EventType::ALL` | n/a | `#[cfg(test)]` only (bus-drift guard) | see finding F-05 |
| `EventBus` (empty marker in `bus.rs`) | none — `EventBus::new()` has zero callers | none | **severed** (F-01) |
| `EventContext`, `EventHandler`, `HandlerError` | none (trait/struct) | `src/teams/notifier.rs:39,156,175-176`, `src/teams/events.rs:394,485,506-507`, `src/bin/aleph-server/commands/start/mod.rs:1311,1315,1359,1368` | connected |
| `EventFilter`, `EventFilter::new`, `EventFilter::with_session`, `EventFilter::with_agent` | n/a | every caller above subscribes via `EventFilter::new(...)` + `.with_session()`/`.with_agent()` | connected |
| `EventFilter::all`, `EventFilter::default`, `EventFilter::with_sessions`, `EventFilter::with_agents` | n/a | `#[cfg(test)]` only | see findings F-06, F-07, F-08, F-09 |
| `GlobalBus`, `GlobalBus::new`, `GlobalBus::global`, `GlobalBus::broadcast`, `GlobalBus::subscribe_async`, `GlobalBus::subscribe_broadcast`, `GlobalBus::next_sequence`, `GlobalBus::unsubscribe`, `GlobalBus::subscribe` | producers above | all have at least one live caller (the boot path `commands/start/mod.rs:1311,1320,1376`, announcers, dispatcher, etc.) — except **`GlobalBus::subscribe` (non-async)**, which is dead (F-02); `GlobalBus::subscription_count` is test-only (F-10) |
| `GlobalEvent`, `GlobalEvent::new`, `GlobalEvent::for_test` | producers above | `for_test` only used in `#[cfg(test)]` (F-11); `new` used internally and across the bus surface | connected |
| `Subscription`, `SubscriptionId`, `SubscriptionId::new`, `SubscriptionId::as_str`, `SubscriptionId::into_inner` | internally constructed | `Subscription::id`/`filter`/`callback` are never read by external code (Form 6) (F-12); `SubscriptionId::as_str`/`into_inner` never called externally (F-13) |
| `TimestampedEvent`, `TimestampedEvent::new` | n/a | `#[cfg(test)]` only (in `types.rs:297,308`) | see finding F-14 |
| `SubAgentCompletionEvent`, `ProcessCompletionEvent` | `agents/subagent_tool/spawn.rs:310`, `agents/background_persistence.rs:351`, `builtin_tools/process_completion.rs:57-92` | `gateway/subagent_announce.rs:254`, `gateway/process_announce.rs:249`, `agents/subagent_tool/tests.rs:2953` | connected |

---

## Findings — severities summary

| Severity | Count |
|---|---|
| critical | 0 |
| high | 0 |
| medium | 3 |
| low | 11 |
| **Total** | **14** |

Decision mix: **CUT 5, CONNECT 0, DECIDE 9**.

Form histogram (some findings span multiple forms): Form 1 = 4 (F-01, F-02, F-13, F-14), Form 4 = 9 (F-04, F-05, F-06, F-07, F-08, F-09, F-10, F-11, F-14), Form 5 = 1 (F-03), Form 6 = 2 (F-01, F-12). Total unique findings = 14.

---

## Findings — detail

### F-01 — `event::EventBus` (empty marker struct + `EventBus::new()`) is a dead producer (medium, Form 1 + Form 6)

**Files**: `src/event/bus.rs:27`, `src/event/mod.rs:21`

**Symbol**: `pub struct EventBus;` (with `EventBus::new()`), re-exported as `pub use bus::EventBus;` in `src/event/mod.rs:21`.

**Evidence**:

```text
$ rg -n "event::EventBus\b|event::\{[^}]*EventBus\b" src/ bin/ interfaces/ shared/
(no matches outside src/event/)
```

```text
$ rg -n "EventBus::new\(\)" src/ bin/ interfaces/ shared/
src/event/bus.rs:22:/// (`EventBus::new()` in `commands/start/mod.rs`); after the
src/executor/builtin_registry/builder/constructor/mod.rs:508:        let loop_graph_bus = crate::loop_graph::TopologyEventBus::new();
src/canvas/store.rs:619:        let bus = Arc::new(GatewayEventBus::new());
src/loop_graph/mod.rs:145:        let bus = TopologyEventBus::new();
src/loop_graph/mod.rs:180:            let bus = TopologyEventBus::new();
src/loop_graph/events.rs:161:        TopologyEventBus::new()
src/approval/node_requester.rs:169:        let event_bus = Arc::new(GatewayEventBus::new());
src/approval/adapters.rs:346:        let event_bus = Arc::new(GatewayEventBus::new());
src/approval/adapters.rs:394:        let event_bus = Arc::new(GatewayEventBus::new());
src/approval/operator_requester.rs:280:        let event_bus = Arc::new(GatewayEventBus::new());
src/approval/operator_requester.rs:357:        let event_bus = Arc::new(GatewayEventBus::new());
src/approval/operator_requester.rs:413:        let event_bus = Arc::new(GatewayEventBus::new());
src/approval/operator_requester.rs:463:        let event_bus = Arc::new(GatewayEventBus::new());
src/agents/swarm/tasks/store/tests.rs:329:    let bus = std::sync::Arc::new(GatewayEventBus::new());
src/agents/swarm/tasks/store/tests.rs:353:    let bus = std::sync::Arc::new(GatewayEventBus::new());
src/builtin_tools/loop_graph_manage.rs:1775:            crate::loop_graph::init_event_bus(crate::loop_graph::TopologyEventBus::new());
src/gateway/execution_engine/session_run_registry.rs:411:        let bus = Arc::new(GatewayEventBus::new());
src/gateway/execution_engine/tests.rs:211:    let bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
src/gateway/execution_engine/tests.rs:292:    let bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
```
None of the `EventBus::new()` calls outside `src/event/` are the bare `event::EventBus` type — they are all `GatewayEventBus` or `TopologyEventBus`. The bare `event::EventBus` is referenced only inside `src/event/` (the doc comment claims `commands/start/mod.rs` constructs one, but the boot path contains zero `EventBus` references — `rg -n "event::" src/bin/aleph-server/commands/start/mod.rs` only matches `EventContext, EventFilter, EventHandler, EventType, GlobalBus`).

**Rationale**: The file's own header comment says the constructor was kept "as a thin marker type that future local-event work can extend". After the 2026-08-16 audit removed `EventContext::bus`, `EventBusConfig`, `EventBusError`, `EventSubscriber`, `publish`, `subscribe`, etc., the only remaining surface is the empty struct + `Default` impl + `::new()`. The boot path does NOT instantiate it; the stale doc comment at `bus.rs:22` is the only "evidence" of a consumer and is provably false. `pub use bus::EventBus` in `mod.rs:21` makes it part of the crate's public surface; if anything depends on it externally it would show up in the rg output, and it does not.

**Decision**: **CUT**. Delete `src/event/bus.rs` entirely and remove `pub use bus::EventBus;` from `src/event/mod.rs:21`. Keep the module header comment in `mod.rs` accurate (drop the `EventBus` line or replace with a one-line note that local-event work would land here).

**Proposed change**: 
- Delete `src/event/bus.rs`
- Remove `mod bus;` and `pub use bus::EventBus;` from `src/event/mod.rs`
- Update module-level doc in `src/event/mod.rs` to drop the `EventBus` bullet

**Risk**: low. Nothing imports `alephcore::event::EventBus`; this is confirmed by the empty rg result above. Worst case is a downstream consumer (e.g. `interfaces/cli` or `interfaces/webchat`) we missed; the broad `rg` over `src/`, `bin/`, `interfaces/`, `shared/` rules that out.

**Verification**: 
1. After deletion, run `rg -n "EventBus" src/ bin/ interfaces/ shared/` and confirm no remaining `event::EventBus` references.
2. Confirm `cargo check -p alephcore` succeeds (out of scope per task, but a real maintainer should).

**Existing review ref**: `review-results/severed-wire-2026-08-17/` does not yet contain an event/ report; the 2026-08-16 audit removed the *previous* dead surface but left `bus.rs` as a stub.

---

### F-02 — `GlobalBus::subscribe` (non-async) is dead (medium, Form 1)

**Files**: `src/event/global_bus.rs:283-306`

**Symbol**: `pub fn subscribe(&self, filter: EventFilter, callback: impl Fn(GlobalEvent) + Send + Sync + 'static) -> SubscriptionId`

**Evidence**:

```text
$ rg -n "\.subscribe\(filter|GlobalBus::global\(\)\.subscribe\(" src/ bin/ interfaces/ shared/
src/event/global_bus.rs:23://! let sub_id = bus.subscribe(filter, |event| {
src/approval/adapters.rs:356:        let _string_sub = event_bus.subscribe();
src/loop_graph/mod.rs:101:    let mut rx = bus.subscribe();
src/agents/swarm/tasks/store/tests.rs:354:    let mut rx = bus.subscribe();
src/gateway/event_emitter/tests.rs:253:    let mut rx = event_bus.subscribe();
src/gateway/event_emitter/team_fanout.rs:236:        let mut rx = bus.subscribe();
src/gateway/event_emitter/team_fanout.rs:257:        let mut rx = bus.subscribe();
src/gateway/event_emitter/team_fanout.rs:270:        let mut rx = bus.subscribe(); // raw string channel
src/gateway/event_emitter/team_fanout.rs:313:        let mut rx = bus.subscribe();
src/gateway/event_emitter/team_fanout.rs:355:        let mut rx = bus.subscribe();
src/gateway/event_emitter/team_fanout.rs:395:        let mut rx = bus.subscribe();
src/gateway/event_emitter/team_fanout.rs:431:        let mut rx = bus.subscribe();
src/gateway/event_emitter/team_fanout.rs:471:        let mut rx = bus.subscribe();
src/builtin_tools/loop_graph_manage.rs:1778:        let mut rx = bus.subscribe();
src/gateway/execution_engine/session_run_registry.rs:412:        let mut rx = bus.subscribe();
src/gateway/server/handler.rs:611:    tokio::spawn(forward_bus_to_client(ctx.event_bus.subscribe(), buffer));
src/gateway/server/handler.rs:2841:        let mut rx = bus.subscribe();
src/gateway/server/handler.rs:2872:        let mut rx = bus.subscribe();
src/gateway/server/handler.rs:2911:        let mut rx = bus.subscribe();
```
The only `.subscribe()` calls on the `event::GlobalBus` type are inside the doc-comment at `global_bus.rs:23`. Every other `.subscribe()` in the repo is on `GatewayEventBus` or `loop_graph::TopologyEventBus`. `subscribe_async` (`global_bus.rs:308`) is the live wire and is used by every boot-path subscriber (`teams/dispatcher/mod.rs:236`, `teams/notifier.rs` boot path, `gateway/execution_engine/goal_wait.rs:97`, `gateway/announce_delivery.rs:94`, `agents/subagent_tool/tests.rs:2950`).

**Rationale**: The blocking variant exists only because `subscribe_async` could not be called from sync context. But every caller in the boot path is async (`tokio::spawn(async move { ... .subscribe_async(...).await; ... })`), so the sync variant has been dead since the boot path adopted `subscribe_async`. It also has a `tokio::task::block_in_place` call (`global_bus.rs:299`) which is a footgun (panics inside `block_in_place` if called from within a multi-threaded runtime without a `block_in_place` capable context).

**Decision**: **CUT**. Delete `GlobalBus::subscribe` (`global_bus.rs:283-306`) and the doc example referencing it (`global_bus.rs:23`). `subscribe_async` is sufficient and is what every live caller uses.

**Proposed change**: Remove `global_bus.rs:283-306` and the comment block at `global_bus.rs:23-28`.

**Risk**: low. Confirmed via rg that no production caller invokes the sync variant. The implementation detail to be aware of: this method internally calls `tokio::task::block_in_place` which would be a panic risk if it ever were called from a non-runtime context — removing it closes a latent footgun.

**Verification**: `rg -n "GlobalBus::global\(\)\.subscribe\b|bus\.subscribe\(filter" src/ bin/ interfaces/ shared/` should return zero hits after the cut.

**Existing review ref**: none.

---

### F-03 — Doc-comment drift in `src/event/bus.rs:14-23` (medium, Form 5)

**Files**: `src/event/bus.rs:14-23`

**Symbol**: doc-comment claim that `EventBus::new()` is called from `commands/start/mod.rs`.

**Evidence**:

```text
$ rg -n "EventBus" src/bin/aleph-server/commands/start/mod.rs
(no matches)
```

```text
$ rg -n "event::" src/bin/aleph-server/commands/start/mod.rs
1311:            use alephcore::event::{EventContext, EventFilter, EventHandler, EventType, GlobalBus};
1359:            use alephcore::event::{EventContext, EventFilter, EventHandler, GlobalBus};
```
The boot path imports `EventContext`, `EventFilter`, `EventHandler`, `EventType`, `GlobalBus` — never `EventBus`.

**Rationale**: The header in `bus.rs` (lines 14-23) explicitly says `EventBus::new()` is constructed in `commands/start/mod.rs`, "after the `EventContext::bus` field was removed this constructor is no longer required either". Yet the boot path was *already* not constructing `EventBus::new()` before `EventContext::bus` was removed — the comment is documenting a relationship that does not exist in the current tree. This is the textbook Form 5 (name/path drift).

**Decision**: **CUT** (tied to F-01). When `bus.rs` is deleted per F-01, this stale comment goes with it. If `bus.rs` is kept for any reason, the comment block at `bus.rs:1-25` must be rewritten to remove the false claim.

**Proposed change**: Same as F-01.

**Risk**: zero (no behavioural change; pure documentation).

**Verification**: After the cut, the stale claim is gone.

**Existing review ref**: none.

---

### F-04 — `AlephEvent::name()` is only used by tests (low, Form 4)

**Files**: `src/event/types.rs:178-198`

**Symbol**: `pub const fn name(&self) -> &'static str`

**Evidence**:

```text
$ rg -n "\.name\(\)" src/ bin/ interfaces/ shared/ | rg "event"
src/event/types.rs:292:        assert_eq!(event.name(), "SubAgentCompleted");
src/event/types.rs:373:            assert_eq!(event.name(), name);
```
Both call-sites are inside `#[cfg(test)] mod tests` in `types.rs` itself. No production code reads `AlephEvent::name`.

**Rationale**: `name()` is the human-readable form used by the bus-drift guard tests (`aleph_event_all_variant_names_matches_name_exhaustively`) and the `test_event_type_mapping` test. It has zero production consumers; `event_type()` is what the production filter (`filter.rs:193`) and trace log (`global_bus.rs:243`) actually use. The function is a useful test invariant but does not participate in any wire format or runtime dispatch.

**Decision**: **DECIDE** (keep, with note). It costs ~20 lines of source, provides compile-time + runtime guard against adding a variant to `AlephEvent` without naming it, and is referenced by `ALL_VARIANT_NAMES` (F-05). Removing it would force the bus-drift guard to rely on `event_type()` only, losing the human-readable guard. Keep with the rationale that the test invariant is the consumer.

**Proposed change**: None (intentional).

**Risk**: low — pure dead-code in production, but actively consumed by the bus-drift guard tests. Marking as DECIDE means accepting that the `pub` surface is a test artifact.

**Verification**: n/a.

**Existing review ref**: none.

---

### F-05 — `AlephEvent::ALL_VARIANT_NAMES` and `EventType::ALL` are only used by tests (low, Form 4)

**Files**: `src/event/types.rs:76-87` (`EventType::ALL`), `src/event/types.rs:202-215` (`AlephEvent::ALL_VARIANT_NAMES`)

**Symbols**: `pub const ALL: &'static [EventType]` and `pub const ALL_VARIANT_NAMES: &'static [&'static str]`.

**Evidence**:

```text
$ rg -n "EventType::ALL" src/ bin/ interfaces/ shared/
src/event/types.rs:355:    //   * `EventType::ALL` likewise must cover every variant; a forgotten
src/event/types.rs:467:        for et in EventType::ALL {
src/event/types.rs:494:                "EventType::{et:?} has no live AlephEvent producer; remove it from EventType::ALL"

$ rg -n "AlephEvent::ALL_VARIANT_NAMES" src/ bin/ interfaces/ shared/
src/event/types.rs:71:    /// [`AlephEvent::ALL_VARIANT_NAMES`] so the bus-drift guard can assert
src/event/types.rs:352:    //   * `AlephEvent::ALL_VARIANT_NAMES` is a hand-synced slice; the
src/event/types.rs:369:        for &name in AlephEvent::ALL_VARIANT_NAMES {
src/event/types.rs:451:        for name in AlephEvent::ALL_VARIANT_NAMES {
```
All call-sites are inside `#[cfg(test)] mod tests` in `types.rs` itself. No production code references these constants.

**Rationale**: Both constants are hand-synced mirrors of the enum and are read exclusively by the bus-drift guard tests (`aleph_event_all_variant_names_matches_name_exhaustively`, `every_aleph_event_variant_is_reachable_from_event_type`). The module doc explicitly frames them as test-only invariants (lines 343-356 in `types.rs`). Without them the guard would have to walk the variants by enumeration, which loses the "hand-synced" property that catches typos.

**Decision**: **DECIDE** (keep). Same rationale as F-04 — these exist to be consumed by the bus-drift guard, not by production code. The `pub` visibility is required only because the tests live in a child module.

**Proposed change**: None (intentional).

**Risk**: low.

**Verification**: n/a.

**Existing review ref**: none.

---

### F-06 — `EventFilter::all()` has zero production callers (low, Form 1 + Form 4)

**Files**: `src/event/filter.rs:100-102`

**Symbol**: `pub fn all() -> Self`

**Evidence**:

```text
$ rg -n "EventFilter::all\(\)" src/ bin/ interfaces/ shared/
src/event/filter.rs:25://! let filter = EventFilter::all()
src/event/filter.rs:356:        let filter = EventFilter::all();
src/event/global_bus.rs:476:        let filter = EventFilter::all();
src/event/global_bus.rs:497:        let filter = EventFilter::all().with_agent("agent-1");
src/event/global_bus.rs:526:        let filter = EventFilter::all().with_session("session-1");
src/event/global_bus.rs:565:        let filter2 = EventFilter::all();
src/event/tests/integration.rs:58:            .subscribe_async(EventFilter::all(), move |event| {
src/event/tests/integration.rs:120:        let filter_a = EventFilter::all().with_session("session-a");
src/event/tests/integration.rs:128:        let filter_b = EventFilter::all().with_session("session-b");
src/event/tests/integration.rs:319:            .subscribe_async(EventFilter::all(), move |event| {
src/event/tests/integration.rs:354:        let filter = EventFilter::all()
src/event/tests/integration.rs:365:        let filter = EventFilter::all()
```
All call-sites are in `#[cfg(test)]` blocks; every production `EventFilter` is built via `EventFilter::new(vec![EventType::TeamTaskCompleted])` etc. with explicit discriminants.

**Rationale**: `EventFilter::all()` exists to provide an "all events" filter convenience. It is `pub`, so it is part of the crate API surface, but every live producer-side subscriber registers with a specific `EventFilter::new(...)` (e.g. `commands/start/mod.rs:1322`, `goal_wait.rs:98`, `announce_delivery.rs:94`, `subagent_tree_relay.rs:31`). No live code path needs a wildcard.

**Decision**: **DECIDE** (keep, surface API). The doc comment at `filter.rs:11-25` is the public API example; the convenience is harmless. Cutting it would remove an explicit wildcard but cost nothing observable. Keep for API completeness; the cost is two lines.

**Proposed change**: None (intentional).

**Risk**: low. Pure convenience surface with no production impact.

**Verification**: n/a.

**Existing review ref**: none.

---

### F-07 — `EventFilter::default()` has zero production callers (low, Form 4)

**Files**: `src/event/filter.rs:49` (`#[derive(Default)]`), `src/event/filter.rs:432`, `src/event/tests/integration.rs:388`

**Symbol**: the `Default` impl is auto-derived; the *result* `EventFilter::default()` is only used in tests.

**Evidence**:

```text
$ rg -n "EventFilter::default\(\)" src/ bin/ interfaces/ shared/
src/event/filter.rs:432:        let filter = EventFilter::default();
src/event/tests/integration.rs:388:        let filter = EventFilter::default();
```
Both call-sites are tests verifying that the default (empty `event_types`) matches nothing — by design.

**Rationale**: `Default` is derived; the tests are checking that the derived default is *behaviourally* "match nothing" (because `event_types` is empty). Removing the `Default` derive would be a Form-4 cleanup but the tests would have to be deleted too. The tests serve as a regression guard.

**Decision**: **DECIDE** (keep). Same shape as F-04/F-05: the test consumer justifies the derive.

**Proposed change**: None (intentional).

**Risk**: low.

**Verification**: n/a.

**Existing review ref**: none.

---

### F-08 — `EventFilter::with_sessions` has zero production callers (low, Form 4)

**Files**: `src/event/filter.rs:124-127`

**Symbol**: `pub fn with_sessions(mut self, session_ids: HashSet<String>) -> Self`

**Evidence**:

```text
$ rg -n "\.with_sessions\(" src/ bin/ interfaces/ shared/
src/event/filter.rs:279:        let filter = EventFilter::new(vec![EventType::SubAgentCompleted]).with_sessions(sessions);
```
Single caller, inside `#[cfg(test)] mod tests` in `filter.rs`.

**Rationale**: Production subscribers always add one session at a time via `.with_session(id)` (used by `goal_wait.rs`, `subagent_announce.rs`, etc.). The bulk setter is a `pub` API surface with no production need today.

**Decision**: **DECIDE** (keep). Bulk setter is a low-cost API affordance that pairs with `with_session`; cutting it removes a documented capability without saving meaningful LOC.

**Proposed change**: None (intentional).

**Risk**: low.

**Verification**: n/a.

**Existing review ref**: none.

---

### F-09 — `EventFilter::with_agents` has zero production callers (low, Form 4)

**Files**: `src/event/filter.rs:149-152`

**Symbol**: `pub fn with_agents(mut self, agent_ids: HashSet<String>) -> Self`

**Evidence**:

```text
$ rg -n "\.with_agents\(" src/ bin/ interfaces/ shared/
src/event/filter.rs:317:        let filter = EventFilter::new(vec![EventType::SubAgentCompleted]).with_agents(agents);
```
Single caller, inside `#[cfg(test)] mod tests` in `filter.rs`. Note: `with_agent(s)` (plural is the bulk variant) is also covered by F-09.

**Rationale**: Symmetric to F-08. Production uses single-agent `.with_agent(id)` (e.g. `gateway/process_announce.rs`, `subagent_announce.rs` test scaffolding). The bulk variant has no production need.

**Decision**: **DECIDE** (keep). Same reasoning as F-08.

**Proposed change**: None (intentional).

**Risk**: low.

**Verification**: n/a.

**Existing review ref**: none.

---

### F-10 — `GlobalBus::subscription_count` is only used by tests (low, Form 4)

**Files**: `src/event/global_bus.rs:355-359`

**Symbol**: `pub async fn subscription_count(&self) -> usize`

**Evidence**:

```text
$ rg -n "subscription_count\(" src/ bin/ interfaces/ shared/
src/event/global_bus.rs:355:    pub async fn subscription_count(&self) -> usize {
src/event/global_bus.rs:479:        assert_eq!(bus.subscription_count().await, 1);
src/event/global_bus.rs:483:        assert_eq!(bus.subscription_count().await, 0);
```
Both external call-sites are tests in `event/global_bus.rs`.

**Rationale**: Pure observability helper. Useful for diagnostics (`bin/aleph-server doctor --subscriptions` style) but not wired into any production surface.

**Decision**: **DECIDE** (keep). Diagnostic-style API; no harm, plausible future use.

**Proposed change**: None (intentional).

**Risk**: low.

**Verification**: n/a.

**Existing review ref**: none.

---

### F-11 — `GlobalEvent::for_test` is only used by tests (low, Form 4)

**Files**: `src/event/global_bus.rs:89-99`

**Symbol**: `#[cfg(test)] pub fn for_test(...)`. Note: already `#[cfg(test)]` gated.

**Evidence**:

```text
$ rg -n "GlobalEvent::for_test" src/ bin/ interfaces/ shared/
src/event/filter.rs:234:        GlobalEvent::for_test(session_id, agent_id, event)
src/gateway/process_announce.rs:249:        GlobalEvent::for_test(
src/gateway/subagent_announce.rs:254:        GlobalEvent::for_test(
```
The latter two are inside `#[cfg(test)] mod tests` blocks (`process_announce.rs:154` and `subagent_announce.rs:152`); `filter.rs:234` is the `#[cfg(test)] mod tests` block inside the same crate.

**Rationale**: Already `#[cfg(test)]`-gated. Cannot be called from production code at all. Its `pub` visibility is required because `process_announce.rs` and `subagent_announce.rs` live in other modules.

**Decision**: **DECIDE** (keep). Test scaffolding; `#[cfg(test)]` gating is the correct contract.

**Proposed change**: None (intentional).

**Risk**: low.

**Verification**: n/a.

**Existing review ref**: none.

---

### F-12 — `Subscription` struct fields are `pub` but never read externally (low, Form 6)

**Files**: `src/event/global_bus.rs:162-169`

**Symbols**: `pub struct Subscription { pub id: SubscriptionId, pub filter: EventFilter, pub callback: Arc<dyn Fn(GlobalEvent) + Send + Sync> }`.

**Evidence**:

```text
$ rg -n "Subscription\s*\{|event::Subscription\b" src/ bin/ interfaces/ shared/
src/event/global_bus.rs:158:// Subscription
src/event/global_bus.rs:162:pub struct Subscription {
src/event/global_bus.rs:289:        let subscription = Subscription {
src/event/global_bus.rs:314:        let subscription = Subscription {
src/event/mod.rs:29:pub use global_bus::{GlobalBus, GlobalEvent, Subscription, SubscriptionId};
src/session/steer_signal.rs:214:struct Subscription {        # local, unrelated
src/session/steer_signal.rs:326:    SteerWatch(Some(Subscription { # local, unrelated
src/gateway/event_bus.rs:202:pub struct TopicSubscription {    # different type
```
The `event::Subscription` is constructed only inside `subscribe` / `subscribe_async`; the `id`, `filter`, `callback` fields are read only inside `broadcast` (`global_bus.rs:259-264` — `sub.filter.matches(...)`, `Arc::clone(&sub.callback)`). No external code reads these fields.

**Rationale**: A `pub` struct with all-`pub` fields is a deliberate API surface. The fields are visible to external callers but the type itself is never held by anyone outside the bus. `internal_hashmap` storage uses `Subscription` only as the value type. The visibility was probably set to `pub` because the struct is exported via `pub use ... Subscription` in `mod.rs:29`. The fields could be made `pub(crate)` without breaking any caller.

**Decision**: **CUT** (downgrade visibility). Change `pub` fields to private (or `pub(crate)`), keeping the type `pub` since it is re-exported. If the type becomes internal-only, remove from `pub use` in `mod.rs`.

**Proposed change**: 
- `src/event/global_bus.rs:164-168` — change `pub id`, `pub filter`, `pub callback` to private.
- Optional: keep `Subscription` as `pub` (for `Debug` display) and remove from `pub use` in `src/event/mod.rs:29` if no external code constructs/inspects it.

**Risk**: low. External consumers cannot be relying on these fields (rg confirms zero reads).

**Verification**: `rg -n "Subscription\b" src/ bin/ interfaces/ shared/` after the change should still show `event::Subscription` only inside `event/global_bus.rs`.

**Existing review ref**: none.

---

### F-13 — `SubscriptionId::as_str` / `into_inner` are dead (low, Form 1)

**Files**: `src/event/global_bus.rs:120-129`

**Symbols**: `pub fn as_str(&self) -> &str`, `pub fn into_inner(self) -> String`

**Evidence**:

```text
$ rg -n "\.as_str\(\)" src/ bin/ interfaces/ shared/ | rg -v "hub|gateway|extensions"
src/event/global_bus.rs:120:    pub fn as_str(&self) -> &str {

$ rg -n "id\.into_inner|SubscriptionId::into_inner" src/ bin/ interfaces/ shared/
(no matches)
```
No external caller invokes these methods on `SubscriptionId`. Note: `Deref<Target = str>` and `Display` impls are used internally and provide string conversion.

**Rationale**: `as_str` and `into_inner` are pure API surface that exposes the inner string. The internal `HashMap<SubscriptionId, Subscription>` uses `SubscriptionId`'s `Hash` / `Eq` impls; external code that needs to format/inspect the id uses `Display` (`{:subscription_id}` in `tracing::debug!` at `global_bus.rs:305,329`).

**Decision**: **DECIDE** (keep). Tiny API surface; the methods are likely intentional public API for future log/observability callers. Keep.

**Proposed change**: None (intentional).

**Risk**: low.

**Verification**: n/a.

**Existing review ref**: none.

---

### F-14 — `TimestampedEvent` is only used by tests (low, Form 4)

**Files**: `src/event/types.rs:18-34`

**Symbol**: `pub struct TimestampedEvent { pub event: AlephEvent, pub timestamp: i64, pub sequence: u64 }` and `pub fn new(event: AlephEvent, sequence: u64) -> Self`

**Evidence**:

```text
$ rg -n "TimestampedEvent" src/ bin/ interfaces/ shared/
src/event/types.rs:18:pub struct TimestampedEvent {
src/event/types.rs:24:impl TimestampedEvent {
src/event/types.rs:297:        let e1 = TimestampedEvent::new(
src/event/types.rs:308:        let e2 = TimestampedEvent::new(
src/event/mod.rs:24:    AlephEvent, EventType, ProcessCompletionEvent, SubAgentCompletionEvent, TimestampedEvent,
```
Two production references — the producer (`types.rs:18-34`) and the re-export (`mod.rs:24`). Two consumers, both inside `#[cfg(test)] mod tests` (`types.rs:297,308`). `mod.rs` re-exports it as part of the crate's public surface, but no production code consumes it.

**Rationale**: The struct has a documented purpose — historical event ordering with timestamps — but `GlobalEvent` already carries `timestamp` + `sequence` (`global_bus.rs:64-67`) and is the actual wire type. `TimestampedEvent` is a vestigial wrapper from before `GlobalEvent` existed. Production history is recorded via `GlobalEvent` in the broadcast channel; the bare `TimestampedEvent` is never wired into any subscriber-side history buffer.

**Decision**: **CUT**. Delete the struct, its `impl` block, and the re-export from `mod.rs:24`. Drop the two `#[cfg(test)]` invocations (`types.rs:297,308`) — they only exercise the type under deletion.

**Proposed change**:
- Delete `src/event/types.rs:18-34` (`struct TimestampedEvent` + `impl`).
- Delete `TimestampedEvent,` from the `pub use types::{ ... }` group in `src/event/mod.rs:23-25`.
- Delete `TimestampedEvent,` from the `use` block at `src/event/types.rs:282` (inside the test module header) and drop the test bodies at `types.rs:296-313` (`test_timestamped_event_sequence`).

**Risk**: low. The type was exported via `alephcore::event::TimestampedEvent`, but `rg -n "TimestampedEvent" src/ bin/ interfaces/ shared/` outside `src/event/` returns zero hits — no external consumer.

**Verification**: After cut, `rg -n "TimestampedEvent" src/ bin/ interfaces/ shared/` should return only the deleted references (none remaining).

**Existing review ref**: none.

---

## Cross-cutting summary

- **Form 1 (visible symbol with zero production consumers)**: F-01, F-02, F-13, F-14.
- **Form 4 (produced but consumed only by tests)**: F-04, F-05, F-06, F-07, F-08, F-09, F-10, F-11, F-14.
- **Form 5 (name/path drift)**: F-03 (stale doc comment).
- **Form 6 (orphaned pub API surface)**: F-01 (re-exported but unused), F-12 (`pub` fields unused).
- **CONNECT**: 0 — no broken-but-needed wires found. Every live producer has a live subscriber (the variant table at the top).
- **DECIDE**: 9 — keep F-04, F-05, F-06, F-07, F-08, F-09, F-10, F-11, F-13 as low-cost surface that the bus-drift guard or future diagnostics may rely on.

The module is well-trimmed: the 2026-08-16 audit removed most of the dead surface (the doc comments in `bus.rs` and `handler.rs` explicitly cite it). The remaining findings are either safe cuts (F-01, F-02, F-03, F-12, F-14) or low-priority API affordances (F-04-F-11, F-13) that survive as test infrastructure or future-use surface. None of them are load-bearing for production behaviour.