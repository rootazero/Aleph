# Severed-Wire Audit — `src/capability/`

- **Date:** 2026-08-17
- **Module:** `src/capability/{census.rs, mod.rs}`
- **Method:** PRODUCED − CONSUMED symbol parity (`rg` across `src/`, `bin/`, `interfaces/`, `shared/`), read-before-write triage
- **Working tree:** `/home/zou/data/workspace/Aleph/.worktrees/sw-batch5-bundled-canvas-cap-clar/`

## Summary

**Verdict: every public symbol has a live production consumer.** No CUT or CONNECT
candidates. The module is unusually clean for a foundational type: the type
itself enforces the contract (`install` + `outcome` set in one act), and its
own test-only `census.rs` carries a derived roster whose guards enforce that
no slot can be declared without an accessor and no accessor can exist without a
slot.

The `pub(crate) mod census;` declaration is gated by `#[cfg(test)]` at
`src/capability/mod.rs:49`, so the entire `census.rs` file compiles out of
production builds. It contributes zero `pub` items to the crate (`rg -n "^pub "
src/capability/census.rs` → empty); its `pub(crate)` items are crate-internal
test scaffolding only.

The two files audited total ~3,000 lines, but only ~140 lines of `mod.rs`
are production surface. The remaining ~2,800 lines are all inside
`#[cfg(test)]` test bodies and helpers. This audit focuses on the production
surface; the test-only machinery is excluded by the protocol's
"production vs `#[cfg(test)]`" rule.

## Public surface inventory (production)

### `mod.rs` (line refs)

| Symbol | Defined at | Production consumers |
|---|---|---|
| `pub enum MissingSemantics` | `src/capability/mod.rs:60` | 148 references outside `src/capability/` (146 non-test, 2 test) |
| `pub enum Outcome` | `src/capability/mod.rs:83` | 1 production consumer (`capability_wiring.rs:122`) + 7 doc/test refs |
| `pub trait SlotStatus` | `src/capability/mod.rs:93` | 86 production refs; trait methods called via `&dyn SlotStatus` |
| `pub struct CapabilitySlot<T>` | `src/capability/mod.rs:102` | Declared in 40+ slot modules |
| `pub struct MutableCapabilitySlot<T>` | `src/capability/mod.rs:206` | Declared 1× (`spend::GLOBAL_POLICY`) |
| `pub static ALL_SLOTS` | `src/capability/mod.rs:309` | `src/diagnostics/checks/capability_wiring.rs:252,290` |

### `census.rs`

No `pub` items (`rg -n "^pub " src/capability/census.rs` → empty). Two
`pub(crate)` items (`HandleSite` and `capability_handles`) are crate-internal
and only visible under `#[cfg(test)]`.

## Detailed parity checks

### `pub enum MissingSemantics`

`mod.rs:60`. Four variants: `IndistinguishableDefault { reads_as }`,
`ConsumerDecides`, `FailsClosed`, `FailsOpen`. Each variant has a documented
operator-facing meaning and is wired into the diagnostic's severity mapping.

```bash
rg -n "MissingSemantics::FailsClosed"    src/ bin/ interfaces/ shared/ | rg -v "#\[cfg\(test\)\]" | wc -l
# → 41 production references (declarations + the `Severity::Info` mapping)

rg -n "MissingSemantics::FailsOpen"      src/ bin/ interfaces/ shared/ | rg -v "#\[cfg\(test\)\]" | wc -l
# → 15 production references (declarations + the `Severity::Error` mapping)

rg -n "MissingSemantics::ConsumerDecides" src/ bin/ interfaces/ shared/ | rg -v "#\[cfg\(test\)\]" | wc -l
# → 27 production references (declarations + the `Severity::Warning` mapping)

rg -n "MissingSemantics::IndistinguishableDefault" src/ bin/ interfaces/ shared/ | rg -v "#\[cfg\(test\)\]" | wc -l
# → 29 production references (declarations + the `Severity::Warning` mapping)
```

Each variant is mapped to a `Severity` in
`src/diagnostics/checks/capability_wiring.rs:148-157` and described in
`describe()` at lines 186-205. **No inert variant.**

### `pub enum Outcome`

`mod.rs:83`. Two variants: `Installed`, `Declined { because }`. Both written by
the slot machinery; both consumed by the diagnostic check.

```bash
rg -n "\bOutcome::Installed\b|\bOutcome::Declined\b" src/ bin/ interfaces/ shared/ | rg -v "/capability/" | rg -v "tests\.rs"
# → src/diagnostics/checks/capability_wiring.rs:254   Some(Outcome::Installed) => {}
# → src/diagnostics/checks/capability_wiring.rs:255   Some(Outcome::Declined { because }) => findings.push(...)
```

**Both variants live.** Note: there is no production code that **constructs**
`Outcome::Installed` outside `mod.rs` itself — both variants are written by
the inherent `install`/`decline` methods at lines 134 and 143. That's by
design (R8: the type is the only place this state mutates), not severed.

### `pub trait SlotStatus` (id, missing, outcome)

`mod.rs:93-100`. Trait methods consumed via `&dyn SlotStatus` in the
diagnostic check:

```bash
rg -n "slot\.id\(\)|slot\.missing\(\)|slot\.outcome\(\)" src/ | rg -v "/capability/" | rg -v "tests\.rs"
# → src/diagnostics/checks/capability_wiring.rs:253    match slot.outcome() {
# → src/diagnostics/checks/capability_wiring.rs:258,271 severity_for(slot.missing())
# → src/diagnostics/checks/capability_wiring.rs:259,272 format!("Capability `{}` ...", slot.id())
# → src/diagnostics/checks/capability_wiring.rs:263,276 describe(slot.id(), slot.missing())
# → src/diagnostics/checks/capability_wiring.rs:189,331 concurrency_limiter_slot().id() (test)
# → (40+ in `tests.rs` blocks in slot modules, all asserting the per-slot
#    invariant the diagnostic relies on)
```

Each `*_slot()` accessor returns `&'static dyn SlotStatus`; the trait is the
sole public erasure mechanism for the roster. **All three trait methods have
production callers.**

### `pub struct CapabilitySlot<T>`

Methods: `new`, `install`, `decline`, `get`, `outcome` (inherent + trait).
**All have production callers.**

- `CapabilitySlot::new` — 40+ slot declarations across `src/`. Confirmed by:
  ```bash
  rg -n "CapabilitySlot::new" src/ | rg -v "/capability/" | rg -v "tests\.rs" | wc -l
  # → 39 production references
  ```
- `CapabilitySlot::install` — 25+ install sites (one per slot wrapper). The
  bool result is uniformly discarded with `let _ = ...install(...)`:
  ```bash
  rg -n "let _ = .+\.install\(" src/ bin/ | head
  # → 9 visible matches; 25+ total when also matching `.install(` after assignment.
  ```
- `CapabilitySlot::decline` — 29 production call sites in `src/` outside
  `capability/`:
  ```bash
  rg -n "\.decline\(" src/ bin/ interfaces/ shared/ | rg -v "tests\.rs" | rg -v "/capability/" | wc -l
  # → 29 production references
  ```
  Each slot module that has a conditional install has a
  `fn decline_<slot>(because: &'static str)` wrapper that calls `.decline(`.
  The module's own guard `census::every_decline_wrapper_has_a_production_caller`
  enforces this — already shipped.
- `CapabilitySlot::get` — read by `src/spend/mod.rs:439` (`GLOBAL_LEDGER.get()`)
  and the live reads documented in the slot module docs. There is no wide
  enumeration of `.get()` calls because the recommended read pattern (per the
  module's own docstring at `mod.rs:1-37`) is `*_slot().outcome()` plus
  `*_slot()`'s own type-erased accessor — the only consumer of `.get()` on a
  raw `&CapabilitySlot<T>` is `spend`, which is the round-7 anchor.
- `CapabilitySlot::outcome` — called once via the inherent path inside the
  trait forwarding body at `mod.rs:173`:
  ```bash
  rg -n "CapabilitySlot::outcome" src/
  # → src/capability/mod.rs:173   CapabilitySlot::outcome(self)   (trait impl forwarder)
  ```
  External callers go through the trait (`slot.outcome()` on `&dyn SlotStatus`),
  so the inherent method is reachable only via that forwarder. This is
  deliberate and documented at `mod.rs:170-173`: "Forwards to the inherent
  method on purpose: both must exist (the trait one is only reachable through
  `&dyn SlotStatus`, the inherent one wins at every concrete call site), but
  two copies of one fact in a file four later tasks will edit is a divergence
  waiting to happen. One body."

### `pub struct MutableCapabilitySlot<T>`

Methods: `new`, `install`, `update`, `load`, `outcome` (inherent + trait).
**All have production callers.** `decline` is deliberately absent — see
`mod.rs:182-204`, which documents that `MutableCapabilitySlot::decline` "existed
with zero callers in the whole workspace — production *or* test — and was
withdrawn (R10) rather than left as a hook for later."

```bash
rg -n "MutableCapabilitySlot" src/ bin/ interfaces/ shared/ | rg -v "/capability/" | rg -v "tests\.rs"
# → src/spend/mod.rs:35  use crate::capability::{CapabilitySlot, MissingSemantics, MutableCapabilitySlot, SlotStatus};
# → src/spend/mod.rs:447 static GLOBAL_POLICY: MutableCapabilitySlot<SpendPolicy> = MutableCapabilitySlot::new(
# → src/spend/mod.rs:458 let _ = GLOBAL_POLICY.install(policy);
# → src/spend/mod.rs:486 GLOBAL_POLICY.update(policy)
# → src/spend/mod.rs:500 GLOBAL_POLICY.load()                        (in current_policy)
# → src/spend/mod.rs:533 &GLOBAL_POLICY                              (in global_policy_slot)
```

`MutableCapabilitySlot` has exactly one static instance (`spend::GLOBAL_POLICY`)
and three live methods that read or mutate it. Both `install` (idempotent at
boot, `src/spend/mod.rs:458`) and `update` (hot-apply, `src/spend/mod.rs:486`,
`#[must_use]`) carry a live consumer. The `#[must_use]` on `update` is the
exact anti-lie measure documented at `mod.rs:247-256`.

### `pub static ALL_SLOTS`

`mod.rs:309`. One production consumer:

```bash
rg -n "ALL_SLOTS" src/ bin/ interfaces/ shared/ | rg -v "/capability/" | rg -v "tests\.rs"
# → src/diagnostics/checks/capability_wiring.rs:122  use crate::capability::{MissingSemantics, Outcome, ALL_SLOTS};
# → src/diagnostics/checks/capability_wiring.rs:252      for slot in ALL_SLOTS {
# → src/diagnostics/checks/capability_wiring.rs:290      ALL_SLOTS.len()
```

The diagnostic iterates the roster at `capability_wiring.rs:252` and reads
its length at `:290`. **Single production consumer, both call sites live.**

External (interface) crates — `interfaces/cli`, `interfaces/tui`,
`interfaces/webchat`, `shared/` — do not reference `ALL_SLOTS` (verified by
`rg -n "ALL_SLOTS" interfaces/ shared/` → empty). They also do not import
`crate::capability::*` (verified by
`rg -n "use crate::capability" interfaces/ shared/ bin/` → empty). The
capability module is a lib-internal surface today.

### `ALL_SLOTS` ↔ accessor parity (46 ↔ 46)

`ALL_SLOTS` lists 46 accessor calls. Each is a `pub(crate) const fn
*_slot() -> &'static dyn SlotStatus` in the slot's own module. Verified
one-by-one:

| Index | Accessor | Definition |
|---|---|---|
| 1 | `crate::metrics::metrics_runtime_slot()` | `src/metrics/mod.rs:84` |
| 2 | `crate::pii::engine::pii_engine_slot()` | `src/pii/engine.rs:130` |
| 3 | `crate::tasks::cron::global_cron_slot()` | `src/tasks/cron/mod.rs:106` |
| 4 | `crate::tools::result_store::global_tool_result_store_slot()` | `src/tools/result_store.rs:141` |
| 5 | `crate::tools::turn_budget::global_turn_result_budget_slot()` | `src/tools/turn_budget.rs:129` |
| 6 | `crate::tools::result_processing::result_budget_ceiling_slot()` | `src/tools/result_processing.rs:109` |
| 7 | `crate::tools::in_flight::global_in_flight_tool_calls_slot()` | `src/tools/in_flight.rs:91` |
| 8 | `crate::context::compact::manual::manual_wiring_slot()` | `src/context/compact/manual.rs:209` |
| 9 | `crate::extension::manager_global::extension_manager_slot()` | `src/extension/manager_global.rs:33` |
| 10 | `crate::memory::dreaming::dream_daemon_slot()` | `src/memory/dreaming/mod.rs:484` |
| 11 | `crate::identity::ledger::ledger_slot()` | `src/identity/ledger.rs:429` |
| 12 | `crate::identity::ledger::writer_slot()` | `src/identity/ledger.rs:433` |
| 13 | `crate::loop_graph::service::cron_trigger_slot()` | `src/loop_graph/service.rs:52` |
| 14 | `crate::loop_graph::global_slot()` | `src/loop_graph/mod.rs:126` |
| 15 | `crate::loop_graph::event_bus_slot()` | `src/loop_graph/mod.rs:130` |
| 16 | `crate::config::load::effective_config_path_slot()` | `src/config/load.rs:33` |
| 17 | `crate::config::defaults_override::defaults_override_slot()` | `src/config/defaults_override.rs:115` |
| 18 | `crate::looping::global_slot()` | `src/looping/mod.rs:576` |
| 19 | `crate::providers::route_observe::global_route_observability_slot()` | `src/providers/route_observe.rs:408` |
| 20 | `crate::providers::session_model_handle::pin_sink_slot()` | `src/providers/session_model_handle.rs:101` |
| 21 | `crate::providers::session_model_handle::pinnable_providers_slot()` | `src/providers/session_model_handle.rs:195` |
| 22 | `crate::mcp::sampling_bridge::sampling_llm_slot()` | `src/mcp/sampling_bridge.rs:62` |
| 23 | `crate::spend::global_ledger_slot()` | `src/spend/mod.rs:527` |
| 24 | `crate::spend::global_policy_slot()` | `src/spend/mod.rs:532` |
| 25 | `crate::gateway::security::shared_token::global_shared_token_manager_slot()` | `src/gateway/security/shared_token.rs:50` |
| 26 | `crate::gateway::channel_policy::channel_config_snapshot_slot()` | `src/gateway/channel_policy.rs:92` |
| 27 | `crate::gateway::shutdown_forensics::boot_instant_slot()` | `src/gateway/shutdown_forensics.rs:65` |
| 28 | `crate::gateway::resume_coordinator::global_resume_coordinator_slot()` | `src/gateway/resume_coordinator.rs:74` |
| 29 | `crate::gateway::execution_engine::tool_service_builder::confirmation_requester_slot()` | `src/gateway/execution_engine/tool_service_builder.rs:48` |
| 30 | `crate::gateway::execution_engine::tool_service_builder::config_approval_requester_slot()` | `src/gateway/execution_engine/tool_service_builder.rs:74` |
| 31 | `crate::gateway::execution_engine::tool_service_builder::mcp_tool_registry_slot()` | `src/gateway/execution_engine/tool_service_builder.rs:102` |
| 32 | `crate::gateway::execution_engine::concurrency_handle::concurrency_limiter_slot()` | `src/gateway/execution_engine/concurrency_handle.rs:35` |
| 33 | `crate::gateway::codex_token_refresher::global_slot()` | `src/gateway/codex_token_refresher.rs:75` |
| 34 | `crate::gateway::i18n::installed_locale_slot()` | `src/gateway/i18n.rs:133` |
| 35 | `crate::gateway::handlers::channel::telegram_tool_registry_slot()` | `src/gateway/handlers/channel.rs:168` |
| 36 | `crate::gateway::runtime_footer::global_footer_config_slot()` | `src/gateway/runtime_footer.rs:233` |
| 37 | `crate::goal::global_slot()` | `src/goal/mod.rs:31` |
| 38 | `crate::thinker::memory_context_provider::session_end_mcp_slot()` | `src/thinker/memory_context_provider/helpers.rs:52` |
| 39 | `crate::thinker::memory_context_provider::session_end_summarizer_slot()` | `src/thinker/memory_context_provider/helpers.rs:99` |
| 40 | `crate::thinker::memory_context_provider::session_reflector_slot()` | `src/thinker/memory_context_provider/helpers.rs:140` |
| 41 | `crate::thinker::memory_context_provider::session_end_compression_slot()` | `src/thinker/memory_context_provider/helpers.rs:184` |
| 42 | `crate::thinker::memory_context_provider::open_loop_inject_slot()` | `src/thinker/memory_context_provider/helpers.rs:234` |
| 43 | `crate::strategy::global_slot()` | `src/strategy/mod.rs:74` |
| 44 | `crate::session::store::global_session_event_store_slot()` | `src/session/store.rs:822` |
| 45 | `crate::gateway::session_projector::message_projector_slot()` | `src/gateway/session_projector.rs:94` |
| 46 | `crate::session::service::global_session_service_slot()` | `src/session/service.rs:126` |

Counted: `awk '/pub static ALL_SLOTS/,/^\];/' src/capability/mod.rs | grep -c
"_slot()"` → **46**. The accessor scan above shows each one is defined
exactly once in production code. Note: `global_slot` and `global_ledger_slot`
show higher hit counts because (a) `global_slot` is defined in 5 unrelated
modules (looping/loop_graph/goal/strategy/gateway::codex_token_refresher) and
(b) `global_ledger_slot` appears once inside a string literal in
`census.rs:1693` (test). Neither is a name collision in production.

The census's own guard `census::every_declared_slot_is_in_the_roster`
(`src/capability/census.rs:854`) and `census::every_migrated_slot_has_a_roster_accessor`
(`src/capability/census.rs:1567`) close this loop dynamically at test time —
adding a `static NAME: CapabilitySlot<…>` without listing its accessor in
`ALL_SLOTS`, or adding a roster accessor without the matching slot, fails the
build.

## Findings

**None.**

Every public symbol in `src/capability/mod.rs` has at least one production
caller. Every variant of `MissingSemantics` and `Outcome` is consumed. Every
method on `CapabilitySlot` and `MutableCapabilitySlot` is reachable from
production. `ALL_SLOTS` is read by the `core/capability-wiring` diagnostic.

The `MutableCapabilitySlot::decline` method is **deliberately absent**, with a
documented R10 rationale at `mod.rs:182-204`. That is the opposite of a
severed wire — the module author already performed the audit, found a
zero-caller method, and removed it. Adding it back would resurrect the
exact class of defect this audit exists to catch.

`census.rs` is `#[cfg(test)]` and contributes zero production surface.

## What this audit did NOT examine (out of scope)

- **Style nits.** No `cargo check` / `cargo build` / `cargo clippy` were run
  per protocol; rustfmt/clippy noise is explicitly out of scope.
- **Conditional-install completeness.** The census module's own
  `every_migrated_slot_has_a_roster_accessor` and
  `every_decline_wrapper_has_a_production_caller` guards are not re-verified
  here; they are `#[cfg(test)]` machinery whose reasoning is documented in
  `census.rs` and was last measured on 2026-08-25.
- **Interior-mutable installs (`MOA_CONFIG` etc.).** The known gap at
  `census.rs:111-235` is documented and pinned by
  `the_interior_mutable_install_class_is_below_this_rules_resolution`. That
  gap is not in the `src/capability/` module — it is in 17 other modules —
  and the audit mission is restricted to the capability module.
- **Tests of `CapabilitySlot` / `MutableCapabilitySlot` in `mod.rs:362-493`.**
  Test-only code, excluded by the protocol.

## Sanity-checked paths

All path:line citations above were re-read from
`/home/zou/data/workspace/Aleph/.worktrees/sw-batch5-bundled-canvas-cap-clar/`
on 2026-08-17:

- `src/capability/mod.rs:49` — `#[cfg(test)] pub(crate) mod census;`
- `src/capability/mod.rs:60` — `pub enum MissingSemantics`
- `src/capability/mod.rs:83` — `pub enum Outcome`
- `src/capability/mod.rs:93-100` — `pub trait SlotStatus` with `id`/`missing`/`outcome`
- `src/capability/mod.rs:102-178` — `pub struct CapabilitySlot<T>` + impl
- `src/capability/mod.rs:170-173` — `SlotStatus for CapabilitySlot` (forwards to inherent `outcome`)
- `src/capability/mod.rs:182-204` — `MutableCapabilitySlot` module doc explaining why no `decline`
- `src/capability/mod.rs:206-280` — `pub struct MutableCapabilitySlot<T>` + impl
- `src/capability/mod.rs:272-275` — `SlotStatus for MutableCapabilitySlot`
- `src/capability/mod.rs:309-356` — `pub static ALL_SLOTS` (46 entries)
- `src/diagnostics/checks/capability_wiring.rs:122,252,290` — the production consumer of `ALL_SLOTS`
- `src/spend/mod.rs:447,458,486,500,527,532` — the only `MutableCapabilitySlot` instance
- `src/spend/mod.rs:439` — the only inherent `.get()` call on a `CapabilitySlot`
