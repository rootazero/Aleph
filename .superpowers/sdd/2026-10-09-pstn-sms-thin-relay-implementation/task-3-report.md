
### Foundation recovery checkpoint 1 / 恢复检查点 1
Resumed uncommitted I1 partial on HEAD 07173000f, verified the 18 policy assertions and existing 13 pass / 5 fail RED record; no guards removed. Read recovery/fix/review/main verification and project constraints. Missing types/call signatures and read/reacquire race confirmed. Resources: MemAvailable 17,252 MiB, disk available 286 GiB. No new compile/GREEN evidence yet.

### Foundation recovery checkpoint 2 / 恢复检查点 2
Types/custody report added; actual channel guard revalidates policy on each retry. Durable channel marker and sticky dead-letter retirement column added. Transactional all-row scrub preserves historical reasons and removes payload/error content. Fresh enqueue admission checks marker atomically under persistence lock. Replay context now includes store + claimed row id: transport choke point checks row existence and marker, then stamps attempt, preventing a stale in-memory claim from surviving scrub + Automatic re-registration. Queue drain/preflush now consume Attempt instead of old send signature. Startup reconciliation, registration, drain and public outbox-entry preparation hooks included. No compile/GREEN evidence yet; focused policy build is next. No I2/I3 consumers implemented. Plan: same-read-guard replay choke point + truthful SendReport; store channel marker and independent sticky row retirement with transactional all-row scrub; lifecycle/startup/drain/outbox entry seam. I2/I3 consumers remain explicitly deferred. No full gateway/cargo fmt/live/submodule changes. Scope still foundation only.

### Foundation recovery checkpoint 3 / 恢复检查点 3
First focused build completed in 9m37s, proving restored type/signature compatibility. `/tmp/sms-foundation-policy.log`: 17 policy cases passed / 1 failed. Failure was the historical `Exhausted` reason-only replay predicate: recovery contract explicitly preserves historical reasons and requires independent sticky row retirement. Replaced that obsolete predicate with BOTH `retired_by_policy` and `!DeadLetter::redrivable()` assertions; no case removed or ignored. Added four storage guard tests (disk restart + Automatic replacement, transactional scrub rollback + durable fence, lookup failure fail-closed, all-row scrub + historical ambiguity + stale claim), same-Arc Box replacement during replay retry, and actual custody report tests. Not yet GREEN after these changes. No new fullgateway run. I2/I3 still deferred; PARTIAL parent-fix.


### Slice3a checkpoint 1 — RED (semantic)
`CARGO_BUILD_JOBS=2 cargo test -p alephcore --test sms_delivery_policy_test` (log /tmp/sl/red.log) exit=101: 6 passed / 5 failed with only the trait seam + SmsChannel ExplicitOnly declaration in place and no enforcement. Semantic failures: disconnected fresh SMS persisted a live queue row (len 1 != 0); RateLimited SMS submitted 3x (!= 1); preflush replayed legacy SMS rows (3 submits != 1); failed new send + legacy row = 6 submits; background drain submitted legacy rows (2 != 0). Ordinary-channel tests (RateLimited retry, NotConnected queue+drain replay, unregistered "ghost" row) already green as baseline. Unfinished: enforcement (registry/delivery_queue), GREEN run. Blockers: none.

### Slice3a checkpoint 2 — GREEN + done
Enforcement added: `Channel::outbound_delivery_policy()` (default Automatic, SMS ExplicitOnly); `ChannelRegistry::outbound_delivery_policy(&id) -> Option<_>` (None for unregistered = old behaviour); `send_attempt` gives ExplicitOnly 0 in-memory RateLimited retries; `maybe_enqueue` never stores ExplicitOnly failures; `deliver_one` (drain, inline/pre-flush, legacy rows) retires ExplicitOnly rows before mark_inflight/send via new `DeliveryStore::record_scrubbed_dead_letter` (payload replaced by content-free message, fallback drop_record) with new `DeadLetterReason::ExplicitOnlyRetired` ("explicit_only_retired"; replay_safe, not redrivable). GREEN: `cargo test -p alephcore --test sms_delivery_policy_test` exit 0, 11/11 (was 6/5 RED); `--lib gateway::delivery_queue` 47 ok (+2 new scrub tests, reason tests updated); gateway::channel_registry 14 ok; channel_outbox 4 ok; sms_contract_test 30 ok, sms_fixture_test 14 ok, telegram_delivery_e2e 9 ok. `--lib gateway::` full: 4553 pass, 3 fail (visibility::every_ambient_owner_call_site_is_in_the_census — confirmed failing on baseline via stash; btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question and run_loop project_skill_block_none_when_no_project_skills — untouched code, unrelated, not investigated). Not done / limitations: pre-existing dead_letters rows and reconcile_inflight UnknownOutcome rows for SMS keep payloads (not scrubbed); redrive reports ExplicitOnlyRetired under skipped_unsafe wording ("may already have been sent"); Task 3b+ (WSS/inbox/budget) and Task4 untouched; no cargo clippy run.

### Task 3 transport slice — offline Core lease/outcome seam

Status: PARTIAL by design. Added only an offline, auditable Core-side transport state machine; this is not a production Relay, provider connector, or release completion.

Changes:

- `src/gateway/interfaces/sms/transport.rs`: added `SmsRelayPeer` and `SmsTransportDriver` seams, required `SmsSendAuthorizer`, lease connect/heartbeat/reconnect state, single-active connection generations, binding and generation fences, immutable operation retention, segment/encoding admission, deterministic expiry, monotonic outcome records, `Unknown` and `OutcomeUnknownExpired` no-resend behavior, and typed inbound `RelayAckState::NetworkAckPending` results before delegating to existing `SmsChannel::ingest_envelope`.
- `src/gateway/interfaces/sms/mod.rs`: exports `transport` and preserves the existing public `SmsTransport`/`SmsSubmission` API through the new module.
- Test-only `InMemorySmsPeer` does not implement networking, credentials, TLS, URLs, webhooks, or production activation.

Transport tests: 6 passed, 0 failed, 0 ignored, 20,771 filtered:

- `generation_fences_old_peer_events`: heartbeat, reconnect, single-active generation, stale heartbeat, and stale outcome event refusal.
- `disconnected_and_expired_operations_are_not_submitted`: no peer submission while disconnected or after operation expiry.
- `unknown_is_terminal_after_expiry_and_never_resubmits`: `Unknown` becomes `OutcomeUnknownExpired` and the same operation is never submitted again.
- `outcomes_remain_distinct_and_duplicate_text_is_not_deduped`: accepted/submitted/unknown/delivered remain distinct; identical bodies with distinct operation ids are both submitted.
- `admission_preserves_operation_encoding_and_segments`: operation encoding/segment count/body are preserved without truncation or route substitution.
- `inbound_ack_is_explicitly_pending_relay_adapter`: only local committed/duplicate/expired results are ACK-eligible, while the absent network adapter remains explicitly pending.

Verification (all commands run in `/home/zou/data/workspace/Aleph-sms-pstn-sdd`):

- `rustfmt --edition 2021 src/gateway/interfaces/sms/transport.rs src/gateway/interfaces/sms/mod.rs` — exit 0; single-file rustfmt only, not workspace formatting.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib gateway::interfaces::sms::transport` — exit 0; 6 passed, 0 failed, 0 ignored, 20,771 filtered.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore --test sms_contract_test --test sms_delivery_policy_test --test sms_fixture_test` — exit 0; 30 + 18 + 14 = 62 passed, 0 failed, 0 ignored.
- `CARGO_BUILD_JOBS=2 cargo check -p alephcore --no-default-features` — exit 0; existing warnings only (`AgentDef` unused and `outbound_delivery_policy` dead code).
- A combined test invocation after source formatting exceeded the 1,200-second process timeout during rebuild; it was not treated as a pass. The separated targeted commands above completed successfully; no further retry was made for that timeout.

Implementation commit SHA: `74c481ab59a063363454504efa984ba7aeaf65a2` (`feat: add offline SMS relay transport state`).

Not implemented and intentionally not claimed: real Relay/WSS/TLS/provider connection, credentials, public webhook, factory activation, durable network ACK, provider/carrier segment quotation, external SMS delivery evidence, Task 4 approval wiring, generic retry/delivery queue expansion, and production authorization. The authorizer is an explicit required capability, and the in-memory peer is test-only; therefore this slice cannot prove a Relay handshake, a provider effect, handset delivery, durable wire ACK, or production readiness.
