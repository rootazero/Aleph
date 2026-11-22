# Fix review — IF3

| ID | Verdict | Comment |
|---|---|---|
| IF3-03 | NOT_FIXED | The diff only changes a test import path (`FakeWaRuntime` location); no production code in `vault_store.rs` was modified. Lines 75 and 123 still use `String::from_utf8_lossy` and `.into_bytes()`. |
| IF3-05 | NOT_FIXED | No changes to `src/gateway/interfaces/feishu/mod.rs:266-272`. WebSocket still passes `app_id` instead of `self.info.id`. |
| IF3-06 | PARTIAL | Line 33 now calls `build_conversation_id` with scope config, but only for `Message` events. The original finding at lines 62–80 (`CardAction` arm) is unchanged—card actions still use the raw `chat_id` at line 77 without scoping, so they do not match scoped message allowlists. |
| IF3-01 | NOT_FIXED | No changes to `webhook_server.rs:141-161`. The `_ => Ok(body_str.to_string())` fallback still accepts plaintext when a key is configured. |
| IF3-02 | NOT_FIXED | No changes to `mapper.rs:62-80` (`CardAction` arm). Card actions still map with `is_group: false` regardless of actual chat type. |
| IF3-04 | NOT_FIXED | No changes to `mod.rs:409-418` or `config.rs:46-50`. `allowed_chats` is still not consulted during inbound policy construction or dispatch. |
| IF3-07 | NOT_FIXED | No changes to the `CardAction` mapping at lines 62–68 or `parse_card_action` at lines 128–155. Card actions still use `open_message_id` as the dedup key. |
| IF3-08 | NOT_FIXED | No changes to `api.rs:508-511` or `api.rs:553-556`. Both methods still return `Ok(())` on HTTP 200 without checking the Lark response body `code`. |
| IF3-09 | NOT_FIXED | No changes to `streaming.rs:86-109` or `streaming.rs:274-306`. Line 87 still swaps `closed=true` before the final update, and line 96 only warns on failure. |
| IF3-10 | NOT_FIXED | No changes to `streaming.rs:258-270` or `streaming.rs:288-306`. Card creation after fallback still does not transfer the inner buffer. |
| IF3-11 | NOT_FIXED | No changes to `sender.rs:85-92`. Line 86 still discards the caption error with `let _ = ...` and returns `Ok(...)`. |
| IF3-12 | NOT_FIXED | No changes to `client.rs:234-261`. Lines 240–248 still serialize only `msg.text`; no attachment fields are populated. |
| IF3-13 | NOT_FIXED | No changes to `mapper.rs:22,34-45,66-74`. Media messages still map to empty text with no attachments or raw payload. |
| IF3-14 | NOT_FIXED | No changes to `dm_policy.rs:27-37` or `group_policy.rs:32-45`. Both still compare raw JID strings against allowlist entries. |
| IF3-15 | NOT_FIXED | No changes to `mod.rs:519-522`. Runtime errors are still wrapped as `ChannelError::Internal`. |
| IF3-16 | NOT_FIXED | No changes to `mod.rs:430-436,663-693`. `Event::Connected` still only updates the atomic; `PairingStateDriver` still ignores it. |
| IF3-17 | NOT_FIXED | No changes to `vault_store.rs:33-35,136-141`. Synchronous vault and directory operations remain in async startup. |
| IF3-18 | NOT_FIXED | No changes to `media.rs:59-80`. Synchronous image operations remain in the async method body. |
| IF3-19 | NOT_FIXED | No changes to `history_buffer.rs:71-84`. The HashMap is still unbounded. |
| IF3-20 | NOT_FIXED | No changes to `client.rs:134-192,199-223` or `mod.rs:422-481,488-499`. Detached tasks still execute without join/cancellation before stop returns. |
| IF3-21 | NOT_FIXED | No changes to `client.rs:496-511`. Line 501 still clears the entire map before inserting current batch messages. |

## New problems

none

## Batch verdict

**Needs rework.** The diff contains only a test import correction and one partial message-scoping fix. All 21 findings remain unaddressed in production code. The fix agent's claim of "FIXED" for 18 findings is contradicted by the empty diff for those files.