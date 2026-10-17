# Task #14 slice — one-shot A/B public approval fence

Commit: `feat(telecom): add one-shot public approval fence`

## Files
- `src/gateway/telecom/public_approval.rs` (new): `PublicApprovalLedger`, `PublicApprovalScope`, `ApprovalSource`, `PublicApprovalStage {PrivateRead, Disclosure}`, `PublicApprovalError`, `ConsumedApproval`, `PrivateDisclosureSink`, `deliver_private_disclosure` (effect fence) + 16 unit tests.
- `src/gateway/telecom/mod.rs`: `pub mod public_approval;` + re-exports.

## Design
- Existing `ExecApprovalManager` is a single-flight oneshot (no deadline / scope binding) and `OperatorApprovalRequester` publishes over the event bus; neither can carry A/B scope binding, so this is a pure, storage-free policy module. It is NOT wired into a product flow.
- A (private read) and B (disclosure) are separate records; neither stage can be consumed as the other; B can only be requested from a consumed, unexpired, same-scope, unclaimed A, and is re-checked against A's deadline at consumption.
- Deadline fixed at request creation (SMS 300 s, voice 60 s); approving never renews it; decide/consume at or after deadline refuses.
- Only `ApprovalSource::operator_control_plane()` (pub(crate)) may decide or consume; weak-telecom/unauthenticated sources are refused without burning the request.
- Bound at consumption to exact body, TelecomProvenance (channel, region, binding_ref, connection_generation), core_id, agent_id, turn_id, session_epoch; any mismatch retires the approval (fail closed). One-shot: replay refuses.
- `deliver_private_disclosure` reaches the sink only after B is consumed against the current context; all refusals leave the private canary undelivered.
- Operator view carries the complete bounded body (<= 8 KiB); Debug output of scope/view/grant redacts bodies and ids.
- Caps: 256 live records, expired/retired records reclaimed on insert.

## Verification (actually run)
- `OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2 cargo check -p alephcore --lib --tests` : passed, no warnings from public_approval.rs (pre-existing unrelated warnings remain).
- `cargo test -p alephcore --lib gateway::telecom::public_approval` : 16 passed, 0 failed.
- `rustfmt --edition 2021 --check` on public_approval.rs: clean.

## Residual integration gaps (not done)
- No operator control-plane rendering of `PublicApprovalView` nor decision wiring to `decide(..)`.
- Public run driver does not yet supply core/agent/turn/session_epoch, call `request_private_read` / `request_disclosure` / `cancel_binding` (hang-up, revoke, interruption, generation change), or route private drafts through `deliver_private_disclosure`. `public_run.rs` / `public_history.rs` untouched.
- Session epoch source (must bump on rebind/revoke/reset) is not defined yet.
- Ledger is in-memory; approvals do not survive restart (fail-closed by design).
- `ApprovalSource::operator_control_plane` is convention-gated (pub(crate)), not a cryptographic proof.
- Public default (no tools/derivations/private writeback) is unchanged; tool census, review and full verification of Task #14 remain open.
