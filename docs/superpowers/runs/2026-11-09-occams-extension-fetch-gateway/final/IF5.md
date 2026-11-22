# Final verdicts — IF5

| ID | Verdict | Severity | Reason |
|---|---|---|---|
| IF5-01 | UPHELD | P0 | Line 169 checks `event.sig.is_empty()` but never verifies signature cryptographically; impersonation is real / Lines 162-169 validate only canonical ID and nonempty signature; a relay can recompute the ID and forge `sig`. |
| IF5-02 | UPHELD | P0 | Line 73 `content: content.to_string()` sends plaintext DM; no NIP-04 encryption exists / Lines 59-64 explicitly document plaintext kind-4 DMs, and `build_dm` copies content unchanged; relay-visible private text is reachable. |
| IF5-03 | UPHELD | P0 | Four configs derive Debug with credential fields (lines 16-22, 40-50, 36-42, 14-20) / Line 16 derives `Debug` for `SlackConfig`, whose token fields are plain Strings; derived Debug exposes them. |
| IF5-04 | UPHELD | P0 | Line 84 `room_id.or(chat_room_id)` fails when serialized Message has `null`; private chats collapse to account ID / Lines 84 and 94-97 infer group identity from `to_user_id == account_id` and use `room_id` without checking null semantics. |
| IF5-05 | UPHELD | P1 | Line 270 `.send(event_msg)` returns Ok before relay confirms; relay rejection is logged, not surfaced / Lines 270-273 return success after queuing locally; later relay/write failures are detached from the caller. |
| IF5-06 | DOWNGRADE | P1 | Line 156 `token: String` is copied at startup; GatewayContext never refreshes expired token / Lines 143-156 snapshot the initial token into `GatewayContext`; reconnect behavior is not shown, so expiry impact is unproven. |
| IF5-07 | UPHELD | P1 | Line 64 advertises `attachments: true`; line 147-160 only sends `msg_type: 0` text, ignores attachments / Lines 64-66 advertise attachments/images, while the shown delivery path constructs text and no media handling. |
| IF5-08 | UPHELD | P1 | Line 61 advertises `attachments: true`; send only formats text (line 149-199), no attachment upload / Lines 61-65 advertise multimedia, but the excerpt’s send path only formats `message.text` and returns a text result. |
| IF5-09 | UPHELD | P1 | Line 144 hardcodes `"wechat"` instead of using registered instance `id` from line 35-45 |
| IF5-10 | UPHELD | P1 | Lines 181-200 map `.error_for_status()` to Ok without reading `ret` field from response body |
| IF5-11 | UPHELD | P1 | Line 13 truncates before chunker runs; remainder lost despite chunking path existing |
| IF5-12 | UPHELD | P1 | Lines 279-323 call delete/edit/react unconditionally; config fields 44-46 never checked |
| IF5-13 | UPHELD | P1 | Line 85 `Some(_) => return None` discards all subtypes including user file_share events |
| IF5-14 | UPHELD | P1 | Line 56 hardcodes `reply_to: None` for app_mention; ordinary messages preserve thread_ts (145-148) |
| IF5-15 | UPHELD | P1 | Line 378 sends caption with first file; line 456 sends text again for multi-file case |
| IF5-16 | UPHELD | P1 | Line 451 returns RateLimited after partial success (line 444); violates retry safety contract |
| IF5-17 | UPHELD | P1 | Line 111 ACKs envelope, line 113 enqueues; shutdown line 120 exits loop without draining debouncer |
| IF5-18 | UPHELD | P2 | Line 47 `messages.push(msg)`, line 48 resets deadline; continuous input delays flush indefinitely |
| IF5-19 | UPHELD | P2 | Line 98 buffers full `resp.bytes()` before checking size (line 127-163); no streaming limit |
| IF5-20 | DOWNGRADE | P2 | Line 29 uses `Client::new()` with no timeout; line 165-168 awaits synchronously in socket loop / Lines 27-32 create a client without an overall timeout, but the excerpt does not establish that `users.info` can indefinitely withhold a response. |
| IF5-21 | UPHELD | P2 | Line 49 `in_flight.take()` consumed by one waiter; third concurrent caller line 59 starts duplicate fetch |
| IF5-22 | UPHELD | P2 | Lines 44-45 `let _ = write; let _ = rename` silently discard filesystem errors; no Result return |
