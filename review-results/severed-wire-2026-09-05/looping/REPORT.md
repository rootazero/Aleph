# Severed-Wire Audit — `src/looping`

- Audit: `severed-wire-audit` / 2026-09-05
- Module: `src/looping` (3 files, ~2,711 LOC incl. tests)
- Method: PRODUCED–CONSUMED symbol parity (`rg` across `src/`, `src/bin/`, `interfaces/`, `shared/`, `desktop/`, `crates/`), read-before-write triage. No prior audit for this module — fresh review, every claim re-verified with fresh `rg`.
- Read-only review; no source files modified, no cargo runs.

## Files scanned

- `src/looping/mod.rs` (~1,355 lines, ~890 lines of which are `#[cfg(test)]` tests)
- `src/looping/pursuit.rs` (~486 lines, ~250 lines of `#[cfg(test)]` tests)
- `src/looping/types.rs` (~871 lines, ~440 lines of `#[cfg(test)]` tests)

---

## Module wiring summary (verified, NOT severed)

The `looping` subsystem is **fully wired** into the gateway's continuation hook and the `loop` builtin tool. The producer (in-memory `LoopRegistry`) and the consumers (continuation hook in `execute.rs`, the `loop` tool in `loop_manage.rs`, the prompt bridge in `context_blocks.rs`, the user-deactivation freeze in `users.rs`, the kanban enumeration in `kanban.rs`) all read the same global slot installed at boot.

### Top-level wiring

```
src/executor/builtin_registry/builder/constructor/mod.rs:554-556   boot install
    crate::looping::LoopRegistry::default()       constructor/mod.rs:554
    crate::looping::init_global(reg)              constructor/mod.rs:555
    LoopTool::new(reg)                            constructor/mod.rs:556
```

The process-global `CapabilitySlot<Arc<LoopRegistry>>` is registered in the capability roster at boot):

```
src/capability/mod.rs:337   crate::looping::global_slot(),
```

### Production consumers, by symbol

| Symbol | File:line | Production call site |
|---|---|---|
| `looping::global()` | — | `execute.rs:1112, 1475, 1553, 1764, 1858, 1925`; `loop_manage.rs:983, 665` (via `self.registry`); `continuation_lifecycle.rs:292`; `context_blocks.rs:86, 108, 183`; `users.rs:929`; `kanban.rs:187, 383`; `isolation_acceptance.rs:50, 51`; `daemon.rs:362` (via `alephcore::looping::types`) |
| `LoopRegistry::put` | `src/looping/mod.rs:91` | `loop_manage.rs:390` |
| `LoopRegistry::get` | `src/looping/mod.rs:97` | `loop_manage.rs:646, 648`; `context_blocks.rs:184` |
| `LoopRegistry::get_active` | `src/looping/mod.rs:103` | `execute.rs:1114`; `context_blocks.rs:86, 109` |
| `LoopRegistry::list_all` | `src/looping/mod.rs:120` | `loop_manage.rs:417, 665` |
| `LoopRegistry::transition` | `src/looping/mod.rs:135` | `loop_manage.rs:478, 522, 579`; `execute.rs:1555, 1932`; `continuation_lifecycle.rs:298` |
| `LoopRegistry::stop_all` | `src/looping/mod.rs:202` | `loop_manage.rs:620` |
| `LoopRegistry::pause_all_owned_by` | `src/looping/mod.rs:253` | `users.rs:1027` |
| `LoopRegistry::count_owned_by` | `src/looping/mod.rs:315` | `users.rs:1114` |
| `LoopRegistry::try_claim_tick` | `src/looping/mod.rs:339` | `execute.rs:1143` |
| `LoopRegistry::rearm_after_busy` | `src/looping/mod.rs:436` | `execute.rs:1768` (via `rearm_loop_after_busy`) |
| `LoopRegistry::rearm_after_interruption` | `src/looping/mod.rs:457` | `execute.rs:1873` (via `park_loop_on_transient_failure`) |
| `LoopRegistry::commit_field_update` | `src/looping/mod.rs:526` | `loop_manage.rs:856` (via `self.registry.commit_field_update(state, reschedule)`) |
| `LoopRegistry::confirm_fire` | `src/looping/mod.rs:571` | `execute.rs:1476` (continuation fire gate) |
| `pursuit::exhausted` | `src/looping/pursuit.rs:13` | `loop_manage.rs:566` (resume-with-elapsed-deadline check) |
| `pursuit::stop_reason_note` | `src/looping/pursuit.rs:212` | `loop_manage.rs:567` |
| `pursuit::fires_out_of_bounds` | `src/looping/pursuit.rs:54` | `mod.rs:368, 466` (used internally by `try_claim_tick` and `rearm_after_interruption`) |
| `pursuit::tick_delay_ms` | `src/looping/pursuit.rs:93` | `mod.rs:382, 87` (used internally by `try_claim_tick` and `is_final_deadline_tick`) |
| `pursuit::tick_prompt` | `src/looping/pursuit.rs:113` | `mod.rs:402` (used internally by `try_claim_tick`) |
| `pursuit::is_final_deadline_tick` | `src/looping/pursuit.rs:80` | `pursuit.rs:122` (used internally by `tick_prompt`) |
| `pursuit::cap_reached_note`, `pursuit::deadline_reached_note`, `pursuit::budget_reached_note` | `pursuit.rs:176, 188, 198` | `pursuit.rs:227, 230, 232` (used internally by `stop_reason_note`) |
| `types::fmt_duration_ms` | `src/looping/types.rs:61` | `daemon.rs:362`; `execute.rs:1885`; `loop_manage.rs:748`; `goal/pursuit.rs:20` |
| `Cadence::describe` | `src/looping/types.rs:32` | `loop_manage.rs:389, 738` |
| `LoopStatus::as_str` | `src/looping/types.rs:90` | `loop_manage.rs:728`; `kanban.rs:171` |
| `LoopState::new` | `src/looping/types.rs:144` | `loop_manage.rs:352`; `execute.rs:2306` (test); `users.rs:1574, 1585, 2390`; `kanban.rs:396, 437, 448` |
| `LoopState::with_*` (`with_max_iterations`, `with_deadline_ms`, `with_token_budget`, `with_owner_scope`, `with_next_wake_ms`, `with_cadence`, `with_prompt`, `with_pending_tick`, `with_status`, `with_stop_reason`) | `src/looping/types.rs:166, 217, 225, 232, 239, 246, 257, 266, 182, 192` | `loop_manage.rs:353-358, 366, 797, 817, 820, 825, 828, 832`; `kanban.rs:232, 299, 305, 408, 445, 456`; `users.rs:1563, 1570, 1582, 1593, 2398` |
| `LoopState::spent_iteration`, `LoopState::refund_iteration` | `src/looping/types.rs:166, 172` | `mod.rs:172, 221, 289, 410, 546` (used internally by all atomic lifecycle moves) |
| `LoopState::is_active`, `is_paused`, `is_adjustable` | `src/looping/types.rs:320, 330, 340` | `loop_manage.rs:754, 775, 789, 564`; `mod.rs` (internally) |
| `LoopState::human_summary`, `stable_summary`, `live_status` | `src/looping/types.rs:354, 423, 455` | `loop_manage.rs:646, 648`; `context_blocks.rs:87, 119` |
| `LoopState::tokens_used`, `over_budget` | `src/looping/types.rs:264, 278` | `pursuit.rs:19, 156, 213` (used internally) |
| `LoopState::owner_user_id`, `scope_id` (public fields) | `src/looping/types.rs:140, 142` | `kanban.rs:194, 197`; `isolation_acceptance.rs:208` |
| `TickDecision` enum variants | `src/looping/mod.rs:33` | `execute.rs:1144, 1165, 1188` |
| `RearmDecision` enum variants | `src/looping/mod.rs:53` | `execute.rs:1769, 1774, 1785, 1874, 1891, 1900` |
| `TransitionOutcome` enum variants | `src/looping/mod.rs:69` | `loop_manage.rs:481, 494, 501, 525, 534, 545, 580, 584, 600`; `execute.rs:1560, 1937`; `continuation_lifecycle.rs:303` |
| `global_slot` (`pub(crate)`) | `src/looping/mod.rs:599` | `capability/mod.rs:337` (capability roster) |

**`grep -n` for `looping` across the workspace returns 52 hits; 50 are production call sites or module-level documentation; the remaining 2 are tests and the `alephcore::looping` re-export test.** No `#[allow(dead_code)]` and no `#[deprecated]` items in the module (`rg -n '#\[allow\(dead_code\)\]|#\[deprecated\]' src/looping/` → 0 hits). No `#[cfg(feature=…)]`-gated code paths.

The "goal::pursuit" module (`src/goal/pursuit.rs`) defines its OWN `fires_out_of_bounds`, `exhausted_while_active`, `stop_reason_note`, `cap_reached_note`, `deadline_reached_note`, `budget_reached_note`, `tick_delay_ms` parallel functions; this is **not a missing wire**. The `looping::pursuit::*` references in `src/goal/pursuit.rs` are doc-comment cross-references (`/// (looping::pursuit::fires_out_of_bounds)`), not call sites — confirmed by `rg -n 'looping::pursuit::' src/goal/pursuit.rs` returning only comment hits.

---

## Findings

### sw-looping-1 — `pursuit::should_fire` (pursuit.rs:33) has zero production callers — DECIDE

**Form 1** (producer with zero callers, visible to `dead_code` lint as `pub` but unused). **Severity: low.**

A `pub fn` exported from the module, never called in production. The function is one-liner sugar over `state.is_active() && !exhausted(...)` (pursuit.rs:33-35) and exists at the same API surface as the rest of `pursuit::*`, but nothing reaches for it.

- **Produced:** `pub fn should_fire(state: &LoopState, tokens_now: u64, now_ms: u64) -> bool` at `src/looping/pursuit.rs:33`.
- **Production consumers:** **0**. Searched both by name and by full path.
- **Test consumers (only):** the symbol is referenced 8 times, all inside the `#[cfg(test)] mod tests` block in `pursuit.rs` itself (`pursuit.rs:240, 246, 253, 260, 267, 274, 281, 438, 440`).

**`rg` evidence:**

```
$ rg -n '\bshould_fire\b' --type rust
src/gateway/execution_engine/execute.rs:23:fn naked_loop_planner_should_fire(
src/gateway/execution_engine/execute.rs:131:        if !naked_loop_planner_should_fire(
src/gateway/execution_engine/execute.rs:2021-2093:               # 12 hits — entirely the LOCAL helper `naked_loop_planner_should_fire` (a different function)
src/looping/pursuit.rs:33:pub fn should_fire(state: &LoopState, tokens_now: u64, now_ms: u64) -> bool {
src/looping/pursuit.rs:240,246,253,260,267,274,281,438,440: # 9 hits — all inside the #[cfg(test)] mod tests in pursuit.rs
src/looping/mod.rs:385:// `is_active()` was established above, so `should_fire` here is exactly
src/looping/mod.rs:385: # 1 hit — docstring comment, no call

$ rg -n 'pursuit::should_fire|looping::pursuit::should_fire' --type rust
(no output)
```

The `should_fire` text in `src/looping/mod.rs:385` is a **comment** (`// `is_active()` was established above, so `should_fire` here is exactly…`), not a call. The `naked_loop_planner_should_fire` matches are a completely unrelated function in `execute.rs` (it gates the LoopGraph "naked loop" planner, see `execute.rs:23-131`), not a caller of `pursuit::should_fire`.

- **Rationale (read-before-write):** the `dead_code` lint does NOT currently fire because the function is `pub`; it is invisible as "unused" to the lint. But the function was written as a small convenience predicate and the production call sites went a different way — `try_claim_tick` (mod.rs:339) does the same predicate inline (`is_active()` check is gated at the top of `try_claim_tick`, the cap check uses `exhausted` directly, mod.rs:368-385). The function is not exported through any FFI/binding/wasm surface either (`alephcore` is a pure-Rust lib with `aleph_protocol` etc. as siblings — no C/python/node binding). So the orphan is real.
- **Why DECIDE rather than CUT:** this is a public API on a `pub fn` in a lib crate (`alephcore`) with external dependents (`shared/client`, `interfaces/cli`, `interfaces/tui`, `interfaces/webchat` per their Cargo.tomls). None of them currently import `looping::pursuit::should_fire`, but the change is a public-API removal. The codebase's prior audit pattern (e.g. sw-md-2 in `markdown`, archived DECIDE) treats these lib-crate pub-but-unused items as DECIDE rather than blanket CUT — the cost of keeping is one line and 6 lines of test (or, with the test removed, zero cost). The cost of removing is risk to any external importer that may have materialized in a workspace I haven't audited here.
- **Proposed change:** nothing in this audit. Document the orphan, mark low/DECIDE. If a follow-up sweep across the four external lib consumers confirms zero importers, then CUT the 3-line function and the 9 test lines that exercise only it (or move them to exercise `exhausted` + `state.is_active()` directly, which is what production does anyway).
- **Risk:** keeping is zero-risk; removing is low-risk conditional on the external-consumer sweep above.
- **Verification:** `rg -n '\bshould_fire\b' --type rust` (still 9 hits inside `pursuit.rs` tests + the unrelated `naked_loop_planner_should_fire` in `execute.rs` + 1 comment in `mod.rs`); `rg -n 'pursuit::should_fire|looping::pursuit::should_fire' --type rust` (0 hits). Re-run after any change + `cargo test -p alephcore --lib looping`.
- **`pursuit.rs:33` line is the orphan.** Cross-checked against the sibling `goal::pursuit` module — `goal::pursuit` does NOT export a `should_fire` predicate, so there is no cross-module wiring expectation for the name.

---

## Items checked and found NOT severed (no finding, listed for completeness)

- `init_global` / `global` / `global_slot` / `CapabilitySlot<…, MissingSemantics::ConsumerDecides>` — wired (constructor at mod.rs:554-555; capability roster at capability/mod.rs:337; consumer correctness pinned by the test at looping/mod.rs:1347-1351).
- All 13 `LoopRegistry::*` methods — see table above.
- All 9 `pursuit::*` functions except `should_fire` — see table above.
- All 10 `LoopState::with_*` builders + `spent_iteration` + `refund_iteration` + `tokens_used` + `over_budget` + `is_active` + `is_paused` + `is_adjustable` + `human_summary` + `stable_summary` + `live_status` + 16 public fields — all wired (see table).
- `Cadence` (enum + `describe`), `LoopStatus` (enum + `as_str`) — wired.
- `TickDecision`, `RearmDecision`, `TransitionOutcome` enums — wired (consumed by `execute.rs`, `loop_manage.rs`, `continuation_lifecycle.rs`).
- `fmt_duration_ms` — used by `daemon.rs:362` (status output), `execute.rs:1885` (retry-message formatting), `loop_manage.rs:748` (list-line deadline countdown), `goal/pursuit.rs:20, 102, 286` (the goal-side sibling shares the same vocabulary — see `fmt_duration_ms`'s module doc).
- No `#[cfg(feature=…)]` code paths in the module → form 6 (never-compiled far-end) does not apply.
- No `#[allow(dead_code)]` / `#[deprecated]` items → no inert-but-intentional surface to flag.

---

## Files written

- `review-results/severed-wire-2026-09-05/looping/REPORT.md` (this file)
- `review-results/severed-wire-2026-09-05/looping/summary.json`