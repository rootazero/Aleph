# Task 3c report — encrypted SMS inbound inbox

## Status

PARTIAL: the Core-side encrypted durable inbox implementation and targeted tests are present and green. This slice does not implement Relay/WSS/provider/factory wiring or Agent delivery and must not be treated as full Task 3 acceptance. Commits: `aaf3d4dee`, `08a327132`, and `3dae1f40a`.

## Changes

- `src/gateway/telecom/sms_inbox.rs`: synchronous file/in-memory SQLite inbox with dedicated `SecretsCrypto`, encrypted payload, structured durable dedup identity, shared `accept_event` validation, bounded capacity, TTL/prune, pending retrieval, explicit consume, restart persistence, rollback and fail-closed tests.
- `src/gateway/telecom/mod.rs`: exports `sms_inbox` and public inbox types.
- `src/secrets/crypto.rs`: crate-private `SecretsCrypto::is_usable()` so empty dedicated keys are rejected without exposing key material.

## Public API

`SmsInboxConfig::new`, `with_dedup_horizon`; `SmsInbox::open`, `open_with_key`, `open_in_memory`, `accept`, `pending`, `consume`, `pending_count`; `SmsInboxAdmission`, `SmsInboxItem`, and `SmsInboxError`.

The API accepts only typed `TelecomKind::SmsInbound`; version, binding, generation, stale and expiry behavior delegates to `accept_event`. Durable identity is `(protocol_version, binding_id, provider_account_id, provider_message_id)`. Plaintext is never inserted into SQLite. Content is removed logically on expiry or explicit consume; SQLite/WAL logical deletion is not physical secure erasure and no exactly-once Agent delivery is claimed.

## TDD / verification

- RED: an initial targeted command was attempted before implementation, but due the then-current invocation/worktree selection it did not provide a reliable missing-module failure (it returned no matching test); strict RED evidence is incomplete (process gap, not a claim of test success).
- GREEN: `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib gateway::telecom::sms_inbox::tests -- --nocapture` — exit 0; 11 passed, 0 failed, 0 ignored, 20760 filtered out (fresh main verification after expiry/tombstone fixes and rollback-test correction).
- GREEN: `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib gateway::telecom::protocol::tests -- --nocapture` — exit 0; 4 passed, 0 failed.
- Static review: original review found 2 Important and 1 Minor; fix review confirmed effective-deadline admission, durable expiry tombstones, and exact-horizon pruning. A follow-up review found the new rollback test was initially false-green; commit `3dae1f40a` changes it to `created_at=now-2s`, `expires_at=now-1s`, and asserts `Err(SmsInboxError::Storage(_))`. Main reran the 11-test suite successfully after this fix.
- A prior package-wide targeted invocation without `--lib` was killed by SIGKILL during compilation (exit 101), likely resource pressure; the retry with `--lib` completed successfully.
- Rustfmt was run only on changed Rust files with `rustfmt --edition 2021` (not workspace-wide).

Warnings from the existing crate remain (`AgentDef` unused and `outbound_delivery_policy` dead code); no new test failures were observed.

## Residual / blocked

- No Relay connection, WSS, provider, factory activation, wire ACK, or Agent consumer integration was added.
- Strict RED command/output evidence is incomplete.
- The implementation is split across scoped commits `aaf3d4dee feat: add encrypted SMS inbound inbox`, `08a327132 fix: preserve SMS expiry tombstones`, and `3dae1f40a test: prove SMS expiry tombstone rollback`; no push or merge was performed.
