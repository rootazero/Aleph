# Task 13 SMS Offline Validation Report

## Scope

This report covers only deterministic Core/protocol/fixture validation for the SMS transport. No provider, carrier, real SMS, provisioning, supplier contact, purchase, or release action was performed.

## Validation

Executed in `/home/zou/data/workspace/Aleph-sms-pstn-sdd` with `OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2`, one cargo command at a time:

- `cargo test -p aleph-protocol telecom_relay -- --test-threads=1`: **7 passed, 0 failed**.
- `cargo test -p alephcore --test sms_fixture_test --features test-helpers -- --test-threads=1`: **14 passed, 0 failed**.
- `cargo test -p alephcore --test sms_contract_test --features test-helpers -- --test-threads=1`: **30 passed, 0 failed**.
- `cargo test -p alephcore --test sms_delivery_policy_test --features test-helpers -- --test-threads=1`: **18 passed, 0 failed**.
- `cargo test -p alephcore --lib gateway::interfaces::sms -- --test-threads=1`: **28 passed, 0 failed**.

The preceding Task 19 verification also passed the related registry/router/harness/resume suites, `cargo check -p alephcore --lib --tests --bins`, and `cargo clippy -p alephcore --lib --tests --bins`; see `task-19-live-lease-report.md` and `/tmp/aleph-telecom-task19/` for the logs.

## Covered offline guarantees

The tests cover strict envelope/version handling; committed transcript gating; SMS factory/configuration and capabilities; strict E.164 and segment metadata; inbound identity/generation checks and dedup; explicit transport outcomes; TTL and unknown retention; single-active generation; stale event rejection; reconnect/heartbeat fences; private witness scope and caller checks; exact operation/body/expiry/cancellation checks; no private ordinary-send fallback; and legacy queue behavior remaining unchanged for ordinary channels.

## Remaining gate

The repository still has no production `SmsRelayTransport` factory/hosted Relay authentication and no authorized real-line evidence that a handset receives the text or that a remote Relay rejects stale generations at its irreversible write boundary. Therefore this report establishes offline validation only, not Task 3 real-line completion, G1–G5 eligibility, or release readiness.
