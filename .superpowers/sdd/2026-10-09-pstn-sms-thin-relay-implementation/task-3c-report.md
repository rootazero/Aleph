# Task 3c report — encrypted SMS inbound inbox

## Status

PARTIAL: the Core-side encrypted durable inbox implementation and targeted tests are present and green. This slice does not implement Relay/WSS/provider/factory wiring or Agent delivery and must not be treated as full Task 3 acceptance. The required scoped commit was not created before the execution turn limit.

## Changes

- `src/gateway/telecom/sms_inbox.rs`: synchronous file/in-memory SQLite inbox with dedicated `SecretsCrypto`, encrypted payload, structured durable dedup identity, shared `accept_event` validation, bounded capacity, TTL/prune, pending retrieval, explicit consume, restart persistence, rollback and fail-closed tests.
- `src/gateway/telecom/mod.rs`: exports `sms_inbox` and public inbox types.
- `src/secrets/crypto.rs`: crate-private `SecretsCrypto::is_usable()` so empty dedicated keys are rejected without exposing key material.

## Public API

`SmsInboxConfig::new`, `with_dedup_horizon`; `SmsInbox::open`, `open_with_key`, `open_in_memory`, `accept`, `pending`, `consume`, `pending_count`; `SmsInboxAdmission`, `SmsInboxItem`, and `SmsInboxError`.

The API accepts only typed `TelecomKind::SmsInbound`; version, binding, generation, stale and expiry behavior delegates to `accept_event`. Durable identity is `(protocol_version, binding_id, provider_account_id, provider_message_id)`. Plaintext is never inserted into SQLite. Content is removed logically on expiry or explicit consume; SQLite/WAL logical deletion is not physical secure erasure and no exactly-once Agent delivery is claimed.

## TDD / verification

- RED: an initial targeted command was attempted before implementation, but due the then-current invocation/worktree selection it did not provide a reliable missing-module failure (it returned no matching test); therefore strict RED evidence is incomplete.
- GREEN: `cd /home/zou/data/workspace/Aleph-sms-pstn-sdd && CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib gateway::telecom::sms_inbox::tests -- --nocapture` — exit 0; 7 passed, 0 failed.
- GREEN: `cd /home/zou/data/workspace/Aleph-sms-pstn-sdd && CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib gateway::telecom::protocol::tests -- --nocapture` — exit 0; 4 passed, 0 failed.
- A prior package-wide targeted invocation without `--lib` was killed by SIGKILL during compilation (exit 101), likely resource pressure; the retry with `--lib` completed successfully.
- Rustfmt was run only on changed Rust files with `rustfmt --edition 2021` (not workspace-wide).

Warnings from the existing crate remain (`AgentDef` unused and `outbound_delivery_policy` dead code); no new test failures were observed.

## Residual / blocked

- No Relay connection, WSS, provider, factory activation, wire ACK, or Agent consumer integration was added.
- Strict RED command/output evidence is incomplete.
- The requested commit `feat: add encrypted SMS inbound inbox` and SHA are not available because the turn limit was reached before commit creation.
