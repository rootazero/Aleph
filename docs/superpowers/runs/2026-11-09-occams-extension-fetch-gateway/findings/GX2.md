# Findings — GX2 (telecom + inbound_router: routing, R5 delivery, R4 pure I/O)

Files reviewed: 27 (20563 LOC)

Counts: P0=1 P1=3 P2=3 P3=1

---

### GX2-01 [P1] `/new` confirmation sent before session close; session key epoch visible before new epoch row exists

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

### GX2-02 [P1] Group-chat session visibility uses `channel:conversation` key, but `GroupChatSession.source_session_key` is set from `conversation_key` at create time — any conversation key format mismatch silently denies all commands

- **Location**: `src/gateway/inbound_router/group_chat_handler.rs:53-57` (`try_handle_group_chat`) and line 431-436 (`session_visible_to_conversation`)
- **Failure mode**: `conversation_key` is built as `"{channel_id}:{conversation_id}"` in `try_handle_group_chat`. The same format is used in `session_visible_to_conversation`. However, in `handle_group_chat_continue` at line 362, a new `conversation_key` is built from `msg.channel_id` and `msg.conversation_id` using the same local format. If the `GroupChatSession.source_session_key` was stored with a different `channel_id` alias (e.g., due to channel registration renaming after start) or if `conversation_id` contains the separator `:`, the string match fails silently and all subsequent messages fall through without ever entering the group chat, effectively abandoning the session without any notice.
- **Evidence**:
  ```rust
  let conversation_key = format!(
      "{}:{}",
      msg.channel_id.as_str(),
      msg.conversation_id.as_str()
  );
  // ...
  session.source_session_key == expected_key  // line 436: pure string equality
  ```
- **Suggested fix**: Use a typed `(ChannelId, ConversationId)` pair as the session key instead of a concatenated string, eliminating the ambiguity with `:` in conversation IDs. This preserves the single-owner invariant for group sessions.
- **Confidence**: 0.75

---

### GX2-03 [P0] Secret leak: Telegram bot token flows into a `tracing::warn!` via transport error in `ChannelPublicReplySink::send_reply`

- **Location**: `src/gateway/inbound_router/executor.rs:739-750` (`ChannelPublicReplySink::send_reply`)
- **Failure mode**: The comment at line 744 says "Do not stringify transport errors here: they can contain bodies or phones." The code correctly propagates the error via `map(|_| ())` without stringifying the `ChannelError`. However, callers of `run_public_sms` ultimately log the `PublicRunDisposition` result via `info!("[Router] public telecom run: {disposition:?}")` at executor.rs:863. While `PublicRunDisposition::Failed` does not carry the error, the `ChannelError` text (which includes the bot token in Telegram's `teloxide::Bot` implementation when authentication fails) can appear via `error!("Failed to send error reply: {}", send_err)` in the spawned task at executor.rs line 622. More directly: the send error from `send_public_telecom` can propagate as an `Err` that the callers in the delivery task at line 620 log with `%e`, which exposes the full transport error string including any embedded token or phone number from the Telegram or SMS provider.
- **Evidence**:
  ```rust
  if let Err(send_err) = error_channel_registry
      .send(&error_reply_route.channel_id, reply)
      .await
  {
      error!("Failed to send error reply: {}", send_err);  // line 620-622
  }
  ```
  This `send_err` is a `ChannelError::SendFailed(String)` where Telegram's transport may include the HTTP response body containing the bot token or user phone.
- **Suggested fix**: Change line 622 to log only the error variant/code without the message string: `error!("Failed to send error reply (channel={:?})", error_reply_route.channel_id.as_str())`. This preserves all operational information without leaking credentials. Complies with the "Secret hygiene" area contract.
- **Confidence**: 0.72

---

### GX2-04 [P1] `InboundDedupTracker` is only held by one `Mutex` per router; the coalescing path bypasses the dedup check before coalescing, so a message deduplicated at the coalescer level is never recorded in the dedup tracker

- **Location**: `src/gateway/inbound_router/mod.rs:420-438` (`run_loop`, coalescing branch)
- **Failure mode**: In the coalescing path, `dedup_check` is called on the *original* inbound message before `admit_to_coalescer`. If the coalescer merges two messages with the same ID, only the first message's dedup key is recorded. The merged message (flushed on tick) carries the ID of the *first* message (coalescer keeps first), so the second message — with the same channel+ID key — is never deduplicated because its key was already consumed by the first, legitimate call. Conversely, a replay of the first message after it's coalesced would pass the dedup check (the key was recorded for the real first message, now expired from the dedup window), allowing duplicate execution of the merged message.
- **Evidence**:
  ```rust
  if !self.dedup_check(&msg).await {  // line 421: checks original msg
      continue;
  }
  // ...
  let immediate = self.admit_to_coalescer(msg);  // line 430: may merge with prior msg
  for flushed in immediate {
      // flushed message ID may differ from msg.id when coalescer re-IDs
  ```
- **Suggested fix**: After the coalescer flushes a merged message, call `dedup_check` on the flushed message before dispatching it. This ensures the dedup tracker sees the actual message that will be executed, not the pre-merge original. Preserves the dedup invariant and the ordering contract.
- **Confidence**: 0.70

---

### GX2-05 [P2] `generate_session_topic` holds an LLM call on the hot-path synchronously before closing the session

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

### GX2-06 [P2] `plan_eviction` in `public_history.rs` uses `saturating_sub` for `bytes`, potentially evicting more rows than needed when `new_plain` alone exceeds the remaining budget

- **Location**: `src/gateway/telecom/public_history.rs:487-494` (`plan_eviction`)
- **Failure mode**: The loop condition at line 488 checks `bytes.saturating_add(new_plain) <= cfg.max_session_bytes`. When `new_plain` alone is larger than `max_session_bytes` (e.g., user sends a very long message), `bytes.saturating_add(new_plain)` overflows the check, the loop evicts ALL existing rows, and then the capacity check at `append` (line 340) correctly returns `TurnTooLarge`. However, the eviction IDs accumulated by the loop are included in the transaction eviction plan (`for id in &evict.ids`) and then rolled back by the `Capacity` return at line 374. The net effect is the DB transaction is started with planned deletions that are correctly rolled back, but the in-memory `Eviction` struct accumulates IDs unnecessarily, making the code misleading and potentially brittle if future callers use the `Eviction` result without reading the final capacity check.
- **Evidence**:
  ```rust
  for (id, plain, cipher) in &rows {
      if turns < cfg.max_turns && bytes.saturating_add(new_plain) <= cfg.max_session_bytes {
          break;
      }
      ev.ids.push(*id);  // can accumulate all rows when new_plain > max_session_bytes
  ```
- **Suggested fix**: Add an early return in `plan_eviction` when `new_plain > cfg.max_session_bytes`, returning an empty `Eviction` and letting `append` detect `TurnTooLarge` from `plain_bytes > self.config.max_session_bytes` at line 340. This makes the code intent explicit and removes the spurious accumulation. Invariants: capacity and per-session byte limits are preserved.
- **Confidence**: 0.65

---

### GX2-07 [P2] `open_secure_key_file` uses `metadata.len()` from `fstat` before `read_to_end` but does not verify concurrently growing file

- **Location**: `src/gateway/telecom/public_history_startup.rs:101-113` (`open_secure_key_file`)
- **Failure mode**: The check `!(64..=65).contains(&metadata.len())` is called on the metadata of the *already-opened* FD (obtained by `file.metadata()` which is `fstat`). Then `file.take(66).read_to_end(&mut bytes)` reads up to 66 bytes. If the file is truncated or grown between the `fstat` and the `read_to_end` (TOCTOU on the file contents via `write()` from another process holding a write-FD), the length check passes but the contents differ. The `take(66)` limit bounds the over-read but if the file grows from 64 to 66+ bytes between `fstat` and `read_to_end`, the final `match bytes.as_slice()` at line 110 correctly catches the malformed length and returns `MalformedKey`. This is not a security bypass — the TOCTOU is bounded by the `O_NOFOLLOW` + `take(66)` defenses — but the length pre-check at line 101 is redundant and gives a false sense of the combined TOCTOU window being closed.
- **Evidence**:
  ```rust
  if !(64..=65).contains(&metadata.len()) {
      return Err(PublicHistoryStartupError::MalformedKey);
  }
  // ...
  file.take(66).read_to_end(&mut bytes).map_err(...)?;
  let encoded = match bytes.as_slice() {
      key if key.len() == 64 => key,
      key if key.len() == 65 && key[64] == b'\n' => &key[..64],
      _ => return Err(PublicHistoryStartupError::MalformedKey),  // catches concurrent change
  ```
- **Suggested fix**: Remove the pre-check `!(64..=65).contains(&metadata.len())` and rely solely on the post-read `match bytes.as_slice()` check, which already catches all malformed lengths. This eliminates the misleading suggestion of a TOCTOU-safe length pre-check. Preserves all security invariants.
- **Confidence**: 0.60

---

### GX2-08 [P3] `validate_inbound_shape` uses `SmsInboxError::InvalidTimestamp` for non-timestamp validation (empty provider IDs)

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