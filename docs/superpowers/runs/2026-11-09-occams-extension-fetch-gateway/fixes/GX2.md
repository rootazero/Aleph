# Fix brief — GX2

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/GX2.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/GX2.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/GX2.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-GX2.md

## FIX THESE (3)

### GX2-01 [P1] `/new` confirmation sent before session close; session key epoch visible before new epoch row exists
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/inbound_router/command_handler.rs:379-393` (`handle_new_session`)
- **Failure mode**: The "New session started" reply is sent at line 379 while the old session row is not yet closed (that happens at line 387) and the new session row is created at line 369. Any inbound message arriving in the window between the reply and `sm.close_session` will be routed to the new epoch key, but the reply emitter in `execute_for_context_inner` calls `sm.get_current_epoch` at context-build time (agent_resolver.rs:211) and may still read the old epoch, landing the follow-up message in the old session. This is an ordering issue: the user is told "new session" but a concurrent message may execute under the old session key.
- **Evidence**:
  ```rust
  // line 379 – reply sent here
  let reply = OutboundMessage::text(msg.conversation_id.as_str(), &reply_text);
  if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await { ... }
  // line 384 – topic generation happens AFTER the reply
  let topic = self.generate_session_topic(&old_key_resolved).await;
  // line 387 – close_session called LAST
  if let Err(e) = sm.close_session(&old_key_resolved, topic.as_deref()).await { ... }
  ```
- **Suggested fix**: Reorder: create new session row → close old session row → generate topic → send reply. This preserves the session invariant that the new epoch row exists before the user is told to use it and prevents the close from racing concurrent dispatches into the old session. Preserves the "single epoch owner" invariant.
- **Confidence**: 0.82

---

### GX2-05 [P2] `generate_session_topic` holds an LLM call on the hot-path synchronously before closing the session
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/inbound_router/command_handler.rs:384` (`handle_new_session`)
- **Failure mode**: `generate_session_topic` is an async LLM call that may take several seconds. It is awaited synchronously at line 384, which means the old session is not yet closed (line 387) and the new session row is "open" during the entire LLM call. A concurrent message during this window is routed to the new epoch (since `get_current_epoch` sees `current_epoch + 1`) but the old session row still shows as active. For long `/new` topic generations, this window is measured in LLM latency — potentially 5–30 seconds — during which the session state is inconsistent.
- **Evidence**:
  ```rust
  let topic = self.generate_session_topic(&old_key_resolved).await;  // line 384: LLM call
  if let Some(ref sm) = self.session_store {
      if let Err(e) = sm.close_session(&old_key_resolved, topic.as_deref()).await {  // line 387
  ```
- **Suggested fix**: Spawn the topic generation as a background task (`tokio::spawn`) and close the session immediately with `None` topic; update the topic asynchronously. This is consistent with how the Panel `sessions.new` RPC handles topic generation. Preserves session lifecycle ordering.
- **Confidence**: 0.85

---

### GX2-08 [P3] `validate_inbound_shape` uses `SmsInboxError::InvalidTimestamp` for non-timestamp validation (empty provider IDs)
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P3
- **Location**: `src/gateway/telecom/sms_inbox.rs:542-546` (`validate_inbound_shape`)
- **Failure mode**: Empty `provider_account_id` or `provider_message_id` returns `InvalidTimestamp("provider account and message ids must be non-empty")`. This is a misleading error variant that causes any downstream error handler (log aggregation, monitoring) inspecting `SmsInboxError::InvalidTimestamp` to receive non-timestamp errors. An operator monitoring timestamp errors would see provider ID validation failures mixed in, obscuring the distinction.
- **Evidence**:
  ```rust
  if inbound.provider_account_id.is_empty() || inbound.provider_message_id.is_empty() {
      return Err(SmsInboxError::InvalidTimestamp(
          "provider account and message ids must be non-empty".into(),
      ));
  }
  ```
- **Suggested fix**: Add a `SmsInboxError::InvalidIdentity(String)` variant, or repurpose the existing `WrongKind` variant with a specific message, to correctly classify provider-identity validation failures. This has no behavioral consequence but preserves error semantics and avoids future maintenance confusion.
- **Confidence**: 0.90

---

## Clean files

- `src/gateway/inbound_router/approval_callback.rs` — no finding
- `src/gateway/inbound_router/agent_resolver.rs` — no finding
- `src/gateway/inbound_router/permission.rs` — no finding
- `src/gateway/inbound_router/types.rs` — no finding
- `src/gateway/telecom/operator_control.rs` — no finding
- `src/gateway/telecom/public_admission.rs` — no finding
- `src/gateway/telecom/public_approval.rs` — no finding
- `src/gateway/telecom/public_context.rs` — no finding
- `src/gateway/telecom/public_history.rs` — no finding (GX2-06 is P2 robustness only)
- `src/gateway/telecom/public_history_startup.rs` — no finding (GX2-07 is informational)
- `src/gateway/telecom/private_read.rs` — no finding
- `src/gateway/telecom/private_turn.rs` — no finding
- `src/gateway/telecom/protocol.rs` — no finding
- `src/gateway/telecom/public_run.rs` — no finding
- `src/gateway/telecom/run_origin.rs` — no finding
- `src/gateway/telecom/lifecycle_bridge.rs` — no finding
- `src/gateway/telecom/lifecycle_tools.rs` — no finding
- `src/gateway/telecom/persisted_marker.rs` — no finding
- `src/gateway/telecom/owner_memory_read.rs` — no finding
- `src/gateway/telecom/mod.rs` — no finding
- `src/gateway/telecom/sms_meta.rs` — no finding
- `src/gateway/inbound_router/dedup.rs` — no finding


## DO NOT FIX (5) — refuted / deferred by adjudication or routing

- GX2-02: ### GX2-02 [P1] Group-chat session visibility uses `channel:conversation` key, but `GroupChatSession.source_session_key` is set from `conversation_key` at create time — any conversation key format mismatch silently denies all commands
- GX2-03: ### GX2-03 [P0] Secret leak: Telegram bot token flows into a `tracing::warn!` via transport error in `ChannelPublicReplySink::send_reply`
- GX2-04: ### GX2-04 [P1] `InboundDedupTracker` is only held by one `Mutex` per router; the coalescing path bypasses the dedup check before coalescing, so a message deduplicated at the coalescer level is never recorded in the dedup tracker
- GX2-06: ### GX2-06 [P2] `plan_eviction` in `public_history.rs` uses `saturating_sub` for `bytes`, potentially evicting more rows than needed when `new_plain` alone exceeds the remaining budget
- GX2-07: ### GX2-07 [P2] `open_secure_key_file` uses `metadata.len()` from `fstat` before `read_to_end` but does not verify concurrently growing file
