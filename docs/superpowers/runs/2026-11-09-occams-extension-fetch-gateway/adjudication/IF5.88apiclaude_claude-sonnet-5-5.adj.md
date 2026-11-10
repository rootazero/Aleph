# Adjudication — IF5 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF5-01 | UPHELD | Line 169 checks `event.sig.is_empty()` but never verifies signature cryptographically; impersonation is real | P0 |
| IF5-02 | UPHELD | Line 73 `content: content.to_string()` sends plaintext DM; no NIP-04 encryption exists | P0 |
| IF5-03 | UPHELD | Four configs derive Debug with credential fields (lines 16-22, 40-50, 36-42, 14-20) | P0 |
| IF5-04 | UPHELD | Line 84 `room_id.or(chat_room_id)` fails when serialized Message has `null`; private chats collapse to account ID | P0 |
| IF5-05 | UPHELD | Line 270 `.send(event_msg)` returns Ok before relay confirms; relay rejection is logged, not surfaced | P1 |
| IF5-06 | UPHELD | Line 156 `token: String` is copied at startup; GatewayContext never refreshes expired token | P1 |
| IF5-07 | UPHELD | Line 64 advertises `attachments: true`; line 147-160 only sends `msg_type: 0` text, ignores attachments | P1 |
| IF5-08 | UPHELD | Line 61 advertises `attachments: true`; send only formats text (line 149-199), no attachment upload | P1 |
| IF5-09 | UPHELD | Line 144 hardcodes `"wechat"` instead of using registered instance `id` from line 35-45 | P1 |
| IF5-10 | UPHELD | Lines 181-200 map `.error_for_status()` to Ok without reading `ret` field from response body | P1 |
| IF5-11 | UPHELD | Line 13 truncates before chunker runs; remainder lost despite chunking path existing | P1 |
| IF5-12 | UPHELD | Lines 279-323 call delete/edit/react unconditionally; config fields 44-46 never checked | P1 |
| IF5-13 | UPHELD | Line 85 `Some(_) => return None` discards all subtypes including user file_share events | P1 |
| IF5-14 | UPHELD | Line 56 hardcodes `reply_to: None` for app_mention; ordinary messages preserve thread_ts (145-148) | P1 |
| IF5-15 | UPHELD | Line 378 sends caption with first file; line 456 sends text again for multi-file case | P1 |
| IF5-16 | UPHELD | Line 451 returns RateLimited after partial success (line 444); violates retry safety contract | P1 |
| IF5-17 | UPHELD | Line 111 ACKs envelope, line 113 enqueues; shutdown line 120 exits loop without draining debouncer | P1 |
| IF5-18 | UPHELD | Line 47 `messages.push(msg)`, line 48 resets deadline; continuous input delays flush indefinitely | P2 |
| IF5-19 | UPHELD | Line 98 buffers full `resp.bytes()` before checking size (line 127-163); no streaming limit | P2 |
| IF5-20 | UPHELD | Line 29 uses `Client::new()` with no timeout; line 165-168 awaits synchronously in socket loop | P2 |
| IF5-21 | UPHELD | Line 49 `in_flight.take()` consumed by one waiter; third concurrent caller line 59 starts duplicate fetch | P2 |
| IF5-22 | UPHELD | Lines 44-45 `let _ = write; let _ = rename` silently discard filesystem errors; no Result return | P2 |
