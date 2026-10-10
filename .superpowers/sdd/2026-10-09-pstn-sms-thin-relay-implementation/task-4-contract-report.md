# Task 4 slice 1 — TelecomPublicContext contract + minimal propagation

## Done
- `src/gateway/telecom/public_context.rs`: `TelecomChannel{Sms,Voice}`, `TelecomProvenance{channel,region,binding_ref,connection_generation}`, `TelecomPublicContext`.
  Private fields, getters, NO serde, only `pub(crate) from_admitted_sms(&SmsInboundMeta)`. No trust bool / role / approval_reference / secret / phone number. Debug redacts region + binding_ref. Presence == public ceiling; `None` == legacy (neither deny nor grant).
- `RunRequest.telecom_public` and `FlowRequest.telecom_public` (`Option<TelecomPublicContext>`), both in Debug.
- Propagation: inbound_router executor (stamped from typed `MessageMeta::Sms`, never from the string metadata map) -> `run_loop/inner.rs` FlowRequest -> `Orchestrator::dispatch` (captured before spawn) -> new trailing param of `HarnessRunner::run` -> all 5 impls (real `runner_impl.rs` binds it as `_telecom_public`; `src/harness/**` untouched).
- Derived path copied explicitly: steering rescue (`steering.rs`) copies `request.telecom_public`.
- Tests: 4 contract unit tests (provenance, Debug redaction, None legacy, no serde surface); dispatch propagation test (Some verbatim / None stays None); executor stamping test (Sms stamps, other/empty -> None).
- ScopeId unchanged; ordinary channels pass `None`.

## Verification
- `cargo check --lib --tests`: clean. rustfmt --edition 2021 on modified .rs only.
- `cargo test --lib -- telecom orchestrator::tests::dispatch execution_engine::run_loop::tests steering the_run_markers inbound_router::executor`: 113 passed.
- `cargo test --lib -- gateway::execution_engine orchestrator:: gateway::busy_queue gateway::resume`: 825 passed, 1 failed = `btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question` (known worktree-path environmental red, unrelated).

## NOT done / residual gaps
- No consumer: nothing reads the context yet (history, memory, approval, tool surface, prompt_build) — later slices. The runner only binds it. Absent-provenance-on-telecom-ingress => deny is NOT enforced here.
- `None` is carried by: busy-queue durable reinjection, crash resume (`resume_coordinator`), announce delivery, cron, heartbeat, a2a, teams, openai_api, slash/continuation (`execute.rs continuation_request`), `sessions_send` (send_tool, incl. continue_request), execution_adapter. Context is not persisted, so a queued/resumed SMS-originated run loses it: must be closed before any consumer relies on it (persist as server-authored marker, not via metadata/serde-forgeable payload).
- No real line/voice stamping (`from_admitted_*` only for SMS); no full isolation, approval, public history.
- Not run: full test suite, clippy, integration tests (`tests/`) beyond `cargo check --tests`.
