# Fix review — IF5

| ID | Verdict | Comment |
|---|---|---|
| IF5-01 | FIXED | `key.verify_raw(&id, &signature).map_err(|e| e.to_string())` now performs the required BIP-340 verification, and `if let Err(e) = verify_event(&event)` rejects invalid events before conversion. |
| IF5-02 | FIXED | `build_dm` now `panic!("DM encryption not yet implemented; plaintext DMs are not supported for security reasons")`, removing plaintext kind-4 publication. |
| IF5-03 | FIXED | The four credential-bearing configs no longer derive `Debug`; for example, `#[derive(Clone, Serialize, Deserialize)] pub struct NostrConfig` removes credential-bearing debug formatting while preserving serde. |
| IF5-04 | FIXED | The mapper now uses `if has_room { ... } else { ("dm".to_string(), from_user_id.to_string()) }`, so roomless private chats no longer collapse to the account identity. |
| IF5-05 | PARTIAL | Relay acceptance is now associated with `pending.remove(&event_id)` and returned through `PublishCommand.result`, but `pending` waiters are never timed out or failed when a connection drops; `drop(pending);` merely closes the oneshot senders, and the caller can still wait until its 30-second timeout for a lost delivery. |
| IF5-06 | FIXED | `GatewayContext` now owns `api: Arc<QQApiClient>`, and each Identify calls `identify_payload(&ctx.api)`, which obtains the current token through `api.get_access_token().await?`. |
| IF5-07 | FIXED | QQ capabilities now report `attachments: false` and `images: false`, matching the text-only delivery path instead of advertising discarded media support. |
| IF5-08 | FIXED | WeChat capabilities now report `attachments: false`, `images: false`, `audio: false`, and `video: false`, matching its text-only send implementation. |
| IF5-09 | FIXED | The runtime now receives the actual `ChannelId` in the constructor and uses that stored `channel_id` for mapped inbound messages instead of hardcoding `"wechat"`. |
| IF5-10 | FIXED | `send_message` now parses `GetUpdatesResponse`, returns `WeChatApiError::SessionExpired` for the expired code, and returns `WeChatApiError::Api` for nonzero `ret`. |
| IF5-11 | FIXED | `markdown_to_wechat` no longer calls `truncate`, leaving the complete formatted body for the existing chunking path. |
| IF5-12 | PARTIAL | Reactions and editing now check `enable_reactions` and `enable_editing`, but the diff shown does not add an `enable_deletion` guard to `SlackChannel::delete`, so deletion remains executable while disabled. |
| IF5-13 | FIXED | `Some("file_share") => (event, false)` now routes file-share events through normal message conversion instead of discarding them. |
| IF5-14 | FIXED | App mentions now derive `reply_to` from `event["thread_ts"]` and assign it to `InboundMessage.reply_to`, preserving the existing thread root. |
| IF5-15 | FIXED | The attachment path tracks `caption_sent` and only sends standalone text when `!caption_sent`, preventing the first-file caption from being sent again. |
| IF5-16 | FIXED | After a successful attachment, subsequent upload failure returns `ChannelError::SendFailed("Partial success: ...")`, preventing the retry-unsafe `RateLimited` verdict from escaping after partial publication. |
| IF5-17 | FIXED | Every socket-loop exit now reaches `if debouncer.drain().await`, so acknowledged debounce entries are forwarded before the task terminates. |
| IF5-18 | FIXED | The debouncer now enforces `MAX_MESSAGES`, `MAX_BYTES`, and `MAX_RESIDENCE_MS`, including attachment bytes, and flushes an existing entry when a budget is exceeded. |
| IF5-19 | FIXED | Both inbound and outbound downloads now use `crate::utils::reqwest_limit::bytes_with_limit(...)` and stop at the configured byte budget rather than buffering the full body with `resp.bytes()`. |
| IF5-20 | FIXED | The Slack directory client is now built with `.timeout(Duration::from_secs(5))`, bounding the optional username lookup. |
| IF5-21 | FIXED | Token refresh now serializes on `fetch_lock` and rechecks the cache after acquiring it, replacing the consumable one-shot `in_flight` receiver. |
| IF5-22 | PARTIAL | `save_sync_buf` now logs write and rename failures instead of discarding them, but it still returns `()` and the diff does not propagate failure to the runtime or update the in-memory cursor only after successful persistence. |

## New problems

- `src/gateway/interfaces/nostr/message_ops/ops.rs:127` — Pending publish waiters are not failed when the relay connection closes or reconnect begins; `drop(pending)` only causes delayed channel closure, so a failed connection can leave callers waiting until the outer timeout and loses the actual write/connection error.
- `src/gateway/interfaces/slack/message_ops/socket.rs:218` — The shutdown branch now breaks without closing the WebSocket sink; this may leave the peer connection open while the task performs its drain and exits, unlike the prior `ws_tx.close().await` behavior.
- `src/gateway/interfaces/wechat/api.rs:196` — The sending response is decoded as `GetUpdatesResponse`, coupling the send API to an unrelated response schema and potentially misclassifying a valid send response whose fields differ from the updates envelope.
- `src/gateway/interfaces/wechat/sync_buf.rs:35` — Persistence still returns no `Result`; although errors are logged, `save_sync_buf` cannot tell the runtime that persistence failed, so the finding’s requested propagation and state-consistency guarantee remains absent.

## Batch verdict

Needs rework: Nostr publish failures still detach from callers, Slack deletion ignores its disabled setting, and WeChat persistence errors remain visible only in logs rather than propagated to runtime state.