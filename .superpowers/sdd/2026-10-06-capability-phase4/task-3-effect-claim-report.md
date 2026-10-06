# Task 3 — EffectClaim closure: Unknown transitions (Gate C)

**Status:** done
**Commit:** `fix(capability): close effect claim unknown transitions`
**Scope:** `src/capability/effect_claim.rs` only (Gate C allowed file). No changes to `SessionEventStore` / `ReplayPermit` / `ResumeCoordinator`, no changes to committed hook/approval implementations.

## 1. Reducer fix

`src/capability/effect_claim.rs` → `reconcile_effect_claim`

Before: only `Prepared→Claimed→Invoking→Succeeded/Failed` was legal; the spec §7.5 legal
`Prepared→Unknown`, `Claimed→Unknown`, `Invoking→Unknown` closures were missing (they fell
through to the catch-all and returned fail-closed `Unknown`, which was indistinguishable in
value but not expressed as an *accepted* terminal transition).

After: added the arm

```rust
(_, EffectClaimState::Unknown) if matches!(current, EffectClaimState::Prepared | EffectClaimState::Claimed | EffectClaimState::Invoking) => { current = EffectClaimState::Unknown; terminal = Some(EffectClaimState::Unknown); }
```

so the reducer now accepts the full §7.5 migration set:

- `Prepared → Claimed`
- `Prepared → Unknown`
- `Claimed → Invoking`
- `Claimed → Unknown`
- `Invoking → Succeeded`
- `Invoking → Failed`
- `Invoking → Unknown`

Everything else stays fail-closed. `Unknown` is terminal: the existing `terminal.is_some()`
guard in the loop head makes any event after `Succeeded` / `Failed` / `Unknown` return
fail-closed `Unknown`.

Preserved fail-closed guards (unchanged): empty `request_id`, empty `owner`,
`owner_generation == 0`, `fence == 0`, identity mismatch (`request_id` / `owner` /
`owner_generation` / `fence`) on any event, first event not `Prepared`, non-first
`Prepared`, duplicate active claim, terminal-after-terminal.

## 2. Tests added (all in `effect_claim.rs` `#[cfg(test)] mod tests`)

- three active→Unknown: `prepared_to_unknown`, `claimed_to_unknown`, `invoking_to_unknown`
- duplicate terminal / terminal-after-event: `duplicate_terminal_is_unknown`
  (`...Succeeded, Succeeded`), `terminal_after_succeeded_is_unknown`
  (`...Succeeded, Unknown`), `event_after_unknown_is_unknown` (`Prepared, Unknown, Claimed`)
- empty fields / zero fence / zero generation: `empty_or_zero_identity_fail_closed`
  (empty request_id / empty owner / gen 0 / fence 0 on the first event),
  `zero_fence_on_later_event_fail_closed` (fence 0 on a later event)
- legal chain still passes: existing `legal_chain` (Prepared→Claimed→Invoking→Succeeded → Succeeded)

Result: `cargo test -p alephcore capability::effect_claim --no-fail-fast`
→ `16 passed; 0 failed`.

## 3. Adapter review (`effect_claim_event_from_session` / `reconcile_session_events`)

`EffectClaimTerminal { state: ClaimStateWire::Unknown }` is **already** mapped to
`EffectClaimState::Unknown` by the existing `ClaimStateWire::Unknown => EffectClaimState::Unknown`
arm in `effect_claim_event_from_session`. No fix needed; this was locked by a new
`session_terminal_unknown_maps_to_unknown` test that constructs
`SessionEvent::EffectClaimPrepared` + `SessionEvent::EffectClaimTerminal { state: Unknown }`
and asserts `reconcile_session_events` returns `terminal: Unknown`.

## 4. Producer gap (honest, deferred — no dead constructor added)

`effect_claim.rs` is a **pure reducer** (no store I/O). Verified: the only places the
`EffectClaimPrepared` / `EffectClaimClaimed` / `EffectClaimInvoking` / `EffectClaimTerminal`
`SessionEvent` variants appear in `src/` are:

- `src/session/events.rs` — definition + serde + `is_marker` reduction
- `src/session/store.rs:2086-2142` — `reduction` matches them as non-marker (`=> None`)
- `src/agents/subagent_spawner/fork.rs:248-251` — `is_fork_relevant`-style match (`=> false`)
- `src/capability/effect_claim.rs` — the consumer (adapter + reducer)

**No runtime producer emits these four variants.** The task brief's "durable memo producer
wiring" (`call_log.rs` / `approval/policy.rs` / `hooks/executor.rs` emit via
`SessionService::emit_event`/`emit_batch`) is the separate, committed hook/approval work and
does not construct effect-claim events. Because the brief for this task says effect_claim.rs
is pure reducer with no store I/O, and there is no safe producer entry point in scope, the
producer wiring is **deferred**: no dead `SessionEvent::EffectClaim*` constructor was added.
The reducer + adapter are correct and fully tested against the §7.5 migration set and will
consume real events the moment a producer emits them.

## 5. Verification

- `cargo test -p alephcore capability::effect_claim --no-fail-fast` → PASS (16/16)
- `cargo check -p alephcore` → PASS (no warnings/errors)
- `git diff --stat` → only `src/capability/effect_claim.rs` (+ report file)

## What I did not do

- Did not touch `SessionEventStore` / `ReplayPermit` / `ResumeCoordinator`.
- Did not modify committed hook/approval implementations.
- Did not add a producer for the effect-claim event variants (deferred; see §4).
- Did not implement property/crash fuzz tests beyond the deterministic reducer cases
  (out of scope for this task; brief lists them under the wider Task 3 durable-memo work).
