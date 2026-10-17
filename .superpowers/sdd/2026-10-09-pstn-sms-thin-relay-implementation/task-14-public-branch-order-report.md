# Task #14 fix: public inbound branch ordering

## Change
- `src/gateway/inbound_router/mod.rs::handle_message`: typed
  `MessageMeta::Sms -> Some(TelecomPublicContext)` (via
  `executor::telecom_public_from_message_meta(&msg.metadata)`) now branches as
  the FIRST statement, before the approval-callback sink,
  `resolve_agent_id_async`, `get_allowed_links`/`check_link_access`,
  `build_context_with_agent` (session key/epoch), `check_permission`,
  `record_activity`, hooks, STT, HITL, slash parsing, busy lane, journal.
  The old post-permission branch is removed.
- `executor.rs::handle_public_telecom` now takes `&InboundMessage` (no
  `InboundContext` is built); `ChannelPublicReplySink` replies on
  `msg.channel_id`/`msg.conversation_id`. `/stop` still cancels by binding;
  other slash text is plain caller text. None/ordinary path unchanged.

## Tests (new, in `inbound_router/mod.rs` tests)
- `handle_message_branches_public_before_owner_state`: source-order guard.
- `typed_public_sms_never_reaches_owner_callback_sink`: `cb_`-id SMS never hits
  the owner callback sink; same id without typed meta still does.

## Verification
- `cargo test -p alephcore --lib inbound_router -- --test-threads=1`: 80 passed.
- `cargo check -p alephcore --lib --tests`: OK.
- `rustfmt --edition 2021` on the two touched files only.

## Not done / residuals
- Public spend ledger (still process-global `record_spend`), encrypted public
  history, approvals for public runs, tool census, PSTN: untouched.
- Dedup (`should_process`) runs before `handle_message` as before; not audited
  here for owner-state touch.
- Behavior test does not assert zero route/permission calls directly (covered by
  source-order test only); no full-lib run.
