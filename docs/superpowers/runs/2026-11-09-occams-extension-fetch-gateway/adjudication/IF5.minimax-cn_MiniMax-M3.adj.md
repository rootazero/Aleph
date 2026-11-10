# Adjudication — IF5 — minimax-cn/MiniMax-M3

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF5-01 | UPHELD | Lines 162-169 validate only canonical ID and nonempty signature; a relay can recompute the ID and forge `sig`. | P0 |
| IF5-02 | UPHELD | Lines 59-64 explicitly document plaintext kind-4 DMs, and `build_dm` copies content unchanged; relay-visible private text is reachable. | P0 |
| IF5-03 | UPHELD | Line 16 derives `Debug` for `SlackConfig`, whose token fields are plain Strings; derived Debug exposes them. | P0 |
| IF5-04 | UPHELD | Lines 84 and 94-97 infer group identity from `to_user_id == account_id` and use `room_id` without checking null semantics. | P0 |
| IF5-05 | UPHELD | Lines 270-273 return success after queuing locally; later relay/write failures are detached from the caller. | P1 |
| IF5-06 | DOWNGRADE | Lines 143-156 snapshot the initial token into `GatewayContext`; reconnect behavior is not shown, so expiry impact is unproven. | P2 |
| IF5-07 | UPHELD | Lines 64-66 advertise attachments/images, while the shown delivery path constructs text and no media handling. | P1 |
| IF5-08 | UPHELD | Lines 61-65 advertise multimedia, but the excerpt’s send path only formats `message.text` and returns a text result. | P1 |
| IF5-09 | UPHOLD | Line 144 hardcodes `ChannelId::new("wechat")` despite runtime construction using an arbitrary registered ID. | P1 |
| IF5-10 | UPHOLD | Lines 190-195 send the request, while the shown code only checks HTTP status; no response envelope error parsing occurs. | P1 |
| IF5-11 | UPHOLD | Line 13 truncates the formatted result to 4000 bytes before chunking, permanently discarding long-message content. | P1 |
| IF5-12 | UPHOLD | `react` directly invokes the operation, and the excerpt shows no `enable_reactions` check; the analogous disable controls are not consulted. | P1 |
| IF5-13 | UPHOLD | Line 85 returns `None` for every subtype except `message_changed`, so `file_share` events cannot reach later file handling. | P1 |
| IF5-14 | UPHOLD | Line 56 sets `reply_to: None` unconditionally, despite app-mention events potentially carrying `thread_ts`. | P1 |
| IF5-15 | UPHOLD | Lines 377-380 assign the caption only to the first attachment, while lines 456 onward send the text again when multiple attachments exist. | P1 |
| IF5-16 | UPHOLD | Lines 442-451 propagate attachment errors after earlier successful uploads, allowing a retry to duplicate already-published files. | P1 |
| IF5-17 | UPHOLD | Line 111 ACKs the envelope, and the shutdown path returns without draining the debouncer, so acknowledged buffered input can be lost. | P1 |
| IF5-18 | UPHOLD | Lines 46-48 append messages and reset deadlines without bounds; continuous traffic can postpone `flush_expired` indefinitely. | P2 |
| IF5-19 | UPHOLD | Lines 97-98 allow unknown-size downloads, and the shown `bytes()` buffering occurs before the post-fetch size check. | P2 |
| IF5-20 | DOWNGRADE | Lines 27-32 create a client without an overall timeout, but the excerpt does not establish that `users.info` can indefinitely withhold a response. | P3 |
| IF5-21 | UPHOLD | Lines 47-59 take the single receiver from `in_flight`, allowing a third concurrent caller to start another refresh. | P2 |
| IF5-22 | UPHOLD | Lines 39-45 discard filesystem errors via `let _ =`, making persistence failures silent; runtime persistence failure handling is also not shown. | P2 |
