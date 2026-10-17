# Task 19 Live SMS Lease and Submission Fence Report

## Scope

This slice closes the Core-side live-witness seam for the owner-memory private A/B flow. It is not a carrier adapter, supplier proof, or production-line acceptance. The three pre-existing dirty documents were intentionally excluded:

- `task-3c-report.md`
- `task-4-loop-origin-fix-report.md`
- `docs/superpowers/plans/2026-10-09-pstn-sms-thin-relay-implementation.md`

## Implementation

- `PrivateSmsLease` is opaque and can only be obtained from an attached, configured `SmsRelayTransport`.
- Acquisition checks typed SMS context, region, binding, caller digest, and the active connection generation under the transport state lock.
- One cancellation epoch is shared by the active relay lease. Connect reservation, disconnect, and current-generation heartbeat failure revoke it; a late old-heartbeat failure cannot revoke a replacement epoch.
- Public selection, A/read, private draft, B/send, and fixed checking/incomplete notices retain the same witness. A new generation is never silently acquired for an old turn.
- Ordinary and private submission use the same guarded submission path. After authorizer and final state-lock waits, the path rechecks cancellation, epoch identity, exact body/scope callback, and expiry before entering `Submitting`; the peer receives the original generation.
- Private sending does not fall back to ordinary channel send, queue, retry, or owner hooks. Once the peer submission boundary is crossed, a later cancellation/reconnect does not rewrite an acknowledged or unknown outcome and unknown outcomes are not retried.
- Configuration can construct the owner-memory wiring only when explicitly enabled and the trusted operator/memory backend are present; configuration alone never grants a relay witness. Without a live witness, public isolated chat remains available while lookup/A/read is denied.
- `PrivateLookupToolService` is registered as a forwarding-census leaf because it owns only a turn-local signal and wraps no downstream service; it does not expand recovery tools.

## Validation

All commands ran in `/home/zou/data/workspace/Aleph-sms-pstn-sdd` with `OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2`, serialized by the parent process:

- `cargo check -p alephcore --lib --tests --bins --message-format short`: passed; `/tmp/aleph-telecom-task19/check-fixed.log`.
- `cargo test -p alephcore --lib gateway::telecom -- --test-threads=1`: **157 passed, 0 failed**; `/tmp/aleph-telecom-task19/telecom.log`.
- `cargo test -p alephcore --lib gateway::interfaces::sms -- --test-threads=1`: **28 passed, 0 failed**; `/tmp/aleph-telecom-task19/sms-unit.log`.
- `cargo test -p alephcore --lib gateway::channel_registry -- --test-threads=1`: **26 passed, 0 failed**; `/tmp/aleph-telecom-task19/registry.log`.
- `cargo test -p alephcore --lib gateway::inbound_router -- --test-threads=1`: **93 passed, 0 failed**; `/tmp/aleph-telecom-task19/router.log`.
- `cargo test -p alephcore --lib orchestrator::harness_bridge -- --test-threads=1`: **142 passed, 0 failed**; `/tmp/aleph-telecom-task19/harness-bridge.log`.
- `cargo test -p alephcore --lib providers::forwarding_census -- --test-threads=1`: **2 passed, 0 failed**; `/tmp/aleph-telecom-task19/forwarding-census.log`.
- `cargo test -p alephcore --test resume_coordinator_integration --features test-helpers -- --test-threads=1`: **45 passed, 0 failed**; `/tmp/aleph-telecom-task19/resume_coordinator_integration.log`.
- `cargo test -p alephcore --test sms_contract_test --features test-helpers -- --test-threads=1`: **30 passed, 0 failed**; `/tmp/aleph-telecom-task19/sms_contract_test.log`.
- `cargo test -p alephcore --test sms_delivery_policy_test --features test-helpers -- --test-threads=1`: **18 passed, 0 failed**; `/tmp/aleph-telecom-task19/sms_delivery_policy_test.log`.
- `cargo clippy -p alephcore --lib --tests --bins --message-format short`: passed; `/tmp/aleph-telecom-task19/clippy.log`, with existing warnings.
- `rustfmt --edition 2021 --config skip_children=true` on changed Rust files and `git diff --check`: passed.

Independent read-only review of the bounded delta reported **PASS, Critical 0, Important 0**. This is a static review, not external-line evidence.

## Residual blockers and non-claims

- `SmsChannelFactory` still constructs a channel without a production `SmsRelayTransport`; an attached transport/hosted Relay authentication, lease issuance, rebinding authority, and remote stale-generation refusal are not implemented or proven in this repository.
- No real SMS/call, carrier provisioning, supplier contact, purchasing, or release-gate G1–G5 evidence was performed.
- Local peer fakes prove Core effects and fences only; they do not prove that an external Relay refuses a stale generation at its irreversible write boundary.
- Full historical Core regression was not used as a waiver: an earlier mixed-source run had 21,050 passed / 25 failed / 19 ignored, including one newly identified forwarding-census leaf gap now fixed and separately targeted above. Unrelated baseline/fixture failures require an isolated baseline before release claims.
