# Task 4 — Queue provenance slice report

Branch: feat/pstn-sms-thin-relay. Base: 7e2184649. Scope: busy_queue durable persistence keeps the
`TelecomPublicContext` provenance marker so reinjection does not fold `Some` into `None`.
**This does NOT complete isolation.** No consumer, private read, or approval is enabled.

## What changed
- NEW `src/gateway/telecom/persisted_marker.rs`: server-side serde DTO `PersistedTelecomMarker`
  `{Legacy, Ordinary, Public(PersistedPublicV1), Unknown(Value), Corrupt(Value)}`.
  - Wire v1: `{"version":1,"state":"ordinary"}` or
    `{"version":1,"state":"public","channel":"sms"|"voice","region","binding_ref","connection_generation"}`.
  - Deserialize never errors: unknown version/state/channel => `Unknown` (raw JSON retained for roundtrip);
    bad shape / missing fields / null => `Corrupt`. `Legacy` is only the serde-default for an absent field
    and is never serialized.
  - `capture(Option<&TelecomPublicContext>)` is the sole write entry (census test: exactly one production
    caller, `QueuedRunPayload::from_request`). `restore()` -> `MarkerRestore {Ordinary, LegacyUnproven, Public(ctx), Refuse(reason)}`.
  - `TelecomPublicContext` itself stays no-serde; reconstruction goes through
    `pub(super) restore_v1` in `public_context.rs`, never hand-built JSON elsewhere. Debug redacts the binding.
- `src/gateway/busy_queue/durable.rs`: `QueuedRunPayload.telecom_marker` (`serde(default, skip_serializing_if = is_legacy)`);
  `from_request` captures it (covers both write seams: `busy_queue/spawn.rs` and `inbound_router/executor.rs`);
  `admit_reinjection` restores it onto `RunRequest.telecom_public`. Unknown/Corrupt => `FireStop::Refused`,
  which `reinject_survivors` tombstones as `SettleReason::AuthorityRefused` with detail (no new SettleReason,
  avoiding a downgrade-parse hazard). Legacy records are logged with their limitation.
- `src/gateway/telecom/mod.rs`: module + re-exports.

## Tests (all pass)
`cargo test -p alephcore --lib -- gateway::busy_queue gateway::telecom` => 87 passed.
`run_loop::tests::the_` + `flow_scope_census` => 17 passed.
Covers: pure capture/restore, serde roundtrip, explicit Ordinary write, old journal (field absent) parses as
Legacy and reports limitation, Unknown/Corrupt refused not run as ordinary, real journal file roundtrip,
Some -> from_request -> serde -> reinjection preserves Some, no hand-built payload at seams.
`cargo clippy -p alephcore --lib --tests`: no new findings in changed files
(pre-existing needless_borrow at `public_context.rs:202` probe test untouched).
Only modified Rust files formatted (rustfmt --edition 2021). Env: CARGO_BUILD_JOBS=2 OPENSSL_NO_VENDOR=1.

## Pre-consumer BLOCKERS / limitations (not done)
1. **Legacy journals** (written before the marker) restore as `None` and *cannot prove non-telecom origin*.
   No speculative `known_telecom` heuristic was added (per instruction; metadata/custom/source_channel are
   not authorization). Any future consumer must NOT treat a reinjected `None` from a Legacy record as proof
   of non-public until this is resolved (e.g. drain/refuse legacy records at upgrade).
2. Unknown/Corrupt markers drop the queued message (tombstoned as AuthorityRefused with detail); the
   SettleReason name is imprecise.
3. No integrity MAC: marker is in the same trust domain as the journal; not unforgeable against journal tampering.
4. Other derived producers still `telecom_public: None`: resume_coordinator, goal_wait spawn_wake_run,
   sessions_send, announce, teams, a2a, cron/heartbeat, openai_api (see task-4-contract reports).
5. No consumers, private reads, approval, or isolation are enabled or claimed.
6. Not run: full alephcore suite, integration tests, aleph-server bin check (no other `QueuedRunPayload`
   literals exist per grep).
