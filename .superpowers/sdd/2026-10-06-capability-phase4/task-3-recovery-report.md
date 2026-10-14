# Task 3 — Effect claim recovery adapter

## Scope

Updated only the effect-claim recovery structure and session-event wire state. Approval, `HookExecutor`, dispatch, `SessionEventStore`, and `reduction.rs` were not changed.

## Changes

- Extended `ClaimStateWire` with `Succeeded` and `Failed`, retaining the existing serde tagging and `Active`/`Unknown` spellings.
- Updated the terminal event fixture to use `Succeeded` instead of `Active`.
- Removed pointer-identity logic from the reducer; transition validation now uses explicit event index and state transitions.
- Added session-event mapping and reconciliation adapters. Unknown/incomplete carrier data remains fail-closed as `Unknown`; no effect replay is enabled.

## Verification

- `cargo test -p alephcore session::events --no-fail-fast` — passed (workspace target reported no matching tests).
- `cargo test -p alephcore capability::effect_claim --no-fail-fast` — passed (workspace target reported no matching tests).
- `cargo check -p alephcore` — passed.

The requested focused test filters did not discover tests in the current crate target; compilation was verified with `cargo check`.
