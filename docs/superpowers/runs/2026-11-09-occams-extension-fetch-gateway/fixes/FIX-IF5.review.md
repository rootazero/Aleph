# Fix review — IF5

| ID | Verdict | Comment |
|---|---|---|
| IF5-01 | NOT_FIXED | The diff removes signature verification entirely (deleted lines 169–178 in ops.rs). The added panic in `build_dm` does not verify existing events; relay-forged events still reach `convert_event_to_inbound` unchecked. |
| IF5-02 | FIXED | Lines 273–285 in mod.rs now refuse DM sends with explicit error; `build_dm` is restored but documented as never published. |
| IF5-03 | NOT_FIXED | No changes to any of the four config files; derived `Debug` still exposes credentials in all four adapters. |
| IF5-04 | PARTIAL | Line 83 in mapper.rs removes the `to_user_id == account_id` logic, but the `room_id.or(chat_room_id)` null-propagation issue is not addressed; different groups can still collapse if serialized Message has null room fields. |
| IF5-05 | NOT_FIXED | No changes to ops.rs or mod.rs send/relay loop; publish failures remain detached from callers. |
| IF5-06 | NOT_FIXED | No changes to config.rs, mod.rs, or gateway.rs; token is still a copied String in GatewayContext. |
| IF5-07 | NOT_FIXED | No changes to qq/config.rs, qq/delivery.rs, or qq/mod.rs; attachments capability vs text-only delivery mismatch persists. |
| IF5-08 | NOT_FIXED | No changes to wechat/mod.rs capabilities or send; advertised attachments are still discarded. |
| IF5-09 | FIXED | Lines 114–116 in mod.rs pass `self.info.id.clone()` to runtime; line 153 in runtime.rs uses `self.channel_id` instead of hardcoded `"wechat"`. |
| IF5-10 | NOT_FIXED | No changes to api.rs send_message/send_typing; HTTP success still masks API-level `ret` errors. |
| IF5-11 | NOT_FIXED | No changes to markdown.rs or mod.rs send; truncation before chunking remains. |
| IF5-12 | NOT_FIXED | No changes to slack/mod.rs react/edit/delete or config.rs; enable flags are never checked. |
| IF5-13 | NOT_FIXED | No changes to converter.rs; `Some(_) => return None` still discards file_share. |
| IF5-14 | NOT_FIXED | No changes to converter.rs app_mention path; `reply_to: None` hardcoding persists. |
| IF5-15 | NOT_FIXED | No changes to mod.rs send_with_attachments or api.rs upload; duplicate caption sending remains. |
| IF5-16 | NOT_FIXED | No changes to mod.rs send_with_attachments error handling; partial success still returns retriable RateLimited. |
| IF5-17 | PARTIAL | Lines 132–141 in debouncer.rs add a final flush on send failure, but the socket.rs shutdown path (line 120) still exits without draining the debouncer; ACKed messages in the window are still lost. |
| IF5-18 | NOT_FIXED | No changes to debouncer.rs enqueue/flush; unbounded trailing-edge buffer persists. |
| IF5-19 | NOT_FIXED | No changes to files.rs or mod.rs download_url; full buffering before size check remains. |
| IF5-20 | NOT_FIXED | No changes to directory.rs or socket.rs; users.info lookup still has no timeout and blocks the socket loop. |
| IF5-21 | NOT_FIXED | No changes to qq/api.rs TokenManager; one-shot receiver coordination and third-caller race persist. |
| IF5-22 | NOT_FIXED | No changes to sync_buf.rs, auth.rs, or runtime.rs start; persistence failures remain silent. |

## New problems

- `src/gateway/interfaces/nostr/message_ops/protocol.rs:59–85` — `build_dm` panic was replaced with a working implementation that returns plaintext kind-4 events, contradicting the added documentation claim that "this builder only exists for local, non-publishing paths" and the send-site refusal. If the send path in mod.rs is bypassed or the check is removed, plaintext DMs will be published.
- `src/gateway/interfaces/wechat/api.rs:222–229` — The diff removes variable `resp` and chains `.send().await?` directly, but the original `.error_for_status()?.json()` chain is deleted without replacement; the method now has no return statement and will not compile.

## Batch verdict

Needs rework. Only 2 of 22 findings are fixed (IF5-02, IF5-09). IF5-17 is partial. The Nostr `build_dm` change creates a new plaintext-DM exposure risk, and the WeChat `send_typing` removal breaks compilation. The core credential-exposure (IF5-03), signature-bypass (IF5-01), conversation-collapse (IF5-04), and all Slack/QQ delivery/persistence issues remain unaddressed.