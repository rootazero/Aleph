# Adjudication — IF3 — minimax-cn/MiniMax-M3

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF3-01 | UPHELD | Line 141 falls through to plaintext when encrypted field is absent despite configured key; token-only bypass is reachable. | P0 |
| IF3-02 | UPHELD | Lines 62–76 map every CardAction without authoritative chat type; the shown arm defaults to a non-group-like classification. | P0 |
| IF3-03 | UPHELD | Lines 60–62 serialize arbitrary bytes, but subsequent lossy UTF-8 conversion destroys non-UTF-8 credentials before encryption. | P0 |
| IF3-04 | UPHELD | Lines 409–419 construct policy from `config.access` only; the separate nonempty `allowed_chats` restriction is not applied. | P0 |
| IF3-05 | UPHELD | Lines 266–270 pass `self.config.app_id` as WebSocket channel ID, unlike webhook's `self.info.id` at line 252. | P1 |
| IF3-06 | UPHELD | Lines 33–34 derive scoped conversation IDs before group-policy checks; the synthetic ID consequently replaces the transport chat ID. | P1 |
| IF3-07 | UPHELD | Lines 62–68 use the card `message_id` as the inbound ID, so distinct card actions can share one deduplication key. | P1 |
| IF3-08 | UPHELD | Lines 508–511 check only HTTP status, so a successful HTTP response with a nonzero Feishu business code returns `Ok(())`. | P1 |
| IF3-09 | UPHELD | Lines 86–97 set `closed` before final update and only warn on failure, so failed final delivery is treated as settled. | P1 |
| IF3-10 | UPHELD | Lines 258–270 fall back to `inner.emit` on creation failure but later create a card containing only subsequent deltas. | P1 |
| IF3-11 | UPHELD | Line 86 explicitly discards the caption-send error with `let _ = ...`, then returns success at lines 89–92. | P1 |
| IF3-12 | UPHELD | Lines 234–248 serialize only `msg.text`; no attachment or media payload is included in the WhatsApp message. | P1 |
| IF3-13 | UPHELD | Lines 22–36 derive text but create no attachment/raw payload for media messages, allowing unsupported bodies to become empty prompts. | P1 |
| IF3-14 | UPHELD | Lines 28–33 compare the raw JID directly against allowlist entries, unlike the documented normalized phone-number policy. | P1 |
| IF3-15 | UPHELD | Lines 519–522 wrap every runtime error, including `NotConnected`, as `ChannelError::Internal`, losing retry classification. | P1 |
| IF3-16 | UPHELD | Lines 430–434 apply `Connected` to the separate atomic only; `PairingStateDriver::apply` is called before the match and is not shown updating canonical state. | P1 |
| IF3-17 | UPHELD | Lines 33–37 perform synchronous vault opening during constructor; callers reach it from async startup. | P2 |
| IF3-18 | UPHELD | Lines 64–72 perform synchronous image file decoding and processing inside an async method. | P2 |
| IF3-19 | UPHELD | Lines 76–85 bound each queue length but never bound the number of conversation entries in the HashMap. | P2 |
| IF3-20 | UPHELD | Line 134 detaches the builder task; shared state is published later, allowing an old task to outlive shutdown and affect a restart. | P2 |
| IF3-21 | UPHELD | Lines 501–502 clear the entire receipt map at capacity before inserting current messages, so earlier batch entries are lost. | P2 |
