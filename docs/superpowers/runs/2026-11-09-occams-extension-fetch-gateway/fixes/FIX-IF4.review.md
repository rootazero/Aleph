# Fix review — IF4

| ID | Verdict | Comment |
|---|---|---|
| IF4-01 | NOT_FIXED | No diff for `src/gateway/interfaces/xmpp/message_ops/ops.rs` or `src/gateway/interfaces/xmpp/config.rs`; the TLS authentication failure mode is not addressed. |
| IF4-02 | NOT_FIXED | No diff for `src/gateway/interfaces/xmpp/message_ops/xml_helpers.rs` or `src/gateway/interfaces/xmpp/message_ops/stanza.rs`; attribute extraction still scans entire XML. |
| IF4-03 | NOT_FIXED | No diff for the four config files; derived `Debug` continues to expose secrets. |
| IF4-04 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/bluebubbles/api.rs`; participant match still returns the first matching chat without requiring one-to-one. |
| IF4-05 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/db.rs`; watermark advancement before batch validation remains. |
| IF4-06 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/bluebubbles/api.rs` or `src/gateway/interfaces/imessage/bluebubbles/inbound/poll.rs`; 100-row limit with no pagination persists. |
| IF4-07 | NOT_FIXED | No diff for `src/gateway/interfaces/line/config.rs` or `src/gateway/interfaces/line/webhook.rs`; policy fields are not mapped to Core permission system. |
| IF4-08 | NOT_FIXED | No diff for `src/gateway/interfaces/matrix/outbound.rs`; edit still selects only `chunks[0]`. |
| IF4-09 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/db.rs` or `src/gateway/interfaces/imessage/mod.rs`; bare `chat_identifier` routing to participant send remains. |
| IF4-10 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/mod.rs` or `src/gateway/interfaces/imessage/bluebubbles/mod.rs`; pathless attachments are still skipped with success result. |
| IF4-11 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/mod.rs` or `src/gateway/interfaces/imessage/db.rs`; synchronous SQLite calls in async context persist. |
| IF4-12 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/bluebubbles/mod.rs` or webhook server file; startup still reports `Connected` without awaiting bind result. |
| IF4-13 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/bluebubbles/api.rs` or config; localhost callback for remote server remains. |
| IF4-14 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/bluebubbles/api.rs`; hardcoded `offset: 0` without pagination persists. |
| IF4-15 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/bluebubbles/staging.rs` or calling sites; synchronous directory sweep in async context remains. |
| IF4-16 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/bluebubbles/api.rs`; `res.bytes()` unbounded buffer before cap enforcement persists. |
| IF4-17 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/bluebubbles/api.rs` or staging module; fallback to bare temp without cleanup remains. |
| IF4-18 | NOT_FIXED | No diff for `src/gateway/interfaces/line/config.rs` or webhook handler; default path still `line/webhook` without leading slash. |
| IF4-19 | NOT_FIXED | No diff for `src/gateway/interfaces/line/types.rs`; snake_case field names without camelCase rename persist. |
| IF4-20 | FIXED | Lines 109–110 wrap flex in envelope with `"to": flex.to, "messages": [message]`; line 392 adds skip-serialization directive to `to` field. |
| IF4-21 | NOT_FIXED | No diff for `src/gateway/interfaces/line/webhook.rs`; non-text messages still converted to empty text with no attachments. |
| IF4-22 | NOT_FIXED | No diff for `src/gateway/interfaces/line/mod.rs`; standard attachments remain unprocessed. |
| IF4-23 | NOT_FIXED | No diff for `src/gateway/interfaces/line/webhook.rs`; unbounded connection spawning without timeout persists. |
| IF4-24 | NOT_FIXED | No diff for `src/gateway/interfaces/matrix/events.rs`; hardcoded `is_group: true` for all rooms remains. |
| IF4-25 | NOT_FIXED | No diff for `src/gateway/interfaces/matrix/events.rs` or config; adapter mention check without Core metadata coordination persists. |
| IF4-26 | NOT_FIXED | No diff for `src/gateway/interfaces/matrix/sync.rs` or dedupe module; synchronous SQLite per event in async loop remains. |
| IF4-27 | NOT_FIXED | No diff for `src/gateway/interfaces/matrix/dedupe.rs`; unbounded HashSet with startup-only pruning persists. |
| IF4-28 | NOT_FIXED | No diff for `src/gateway/interfaces/matrix/media.rs` or outbound; full allocation before cap check remains. |
| IF4-29 | FIXED | Line 69–75 add MUC nick parameter and check `nick == joined_nick` for self-echo filtering; line 381 passes `Some(&config.nick)`. |
| IF4-30 | NOT_FIXED | No diff for `src/gateway/interfaces/xmpp/message_ops/ops.rs`; outbound queue consumption before authentication guard persists. |
| IF4-31 | NOT_FIXED | No diff for `src/gateway/interfaces/xmpp/message_ops/ops.rs` or send logic; queue acknowledgment without write confirmation remains. |
| IF4-32 | NOT_FIXED | No diff for `src/gateway/interfaces/xmpp/mod.rs`; read guard held across awaited send and write-lock stop remains. |
| IF4-33 | NOT_FIXED | No diff for `src/gateway/interfaces/xmpp/message_ops/ops.rs`; unbounded buffer accumulation persists. |
| IF4-34 | NOT_FIXED | No diff for `src/gateway/interfaces/xmpp/message_ops/xml_helpers.rs`; recursive entity decode order remains. |
| IF4-35 | NOT_FIXED | No diff for `src/gateway/interfaces/imessage/db.rs`; `.ok()` suppression of chat query failure with private fallback persists. |

## New problems

- `src/gateway/interfaces/line/message_ops.rs:397` — `#[serde(skip)]` on `to` prevents serialization, but `push_flex` at line 109 expects `flex.to` to exist at serialization time. The envelope now depends on the struct field that is skipped during `to_value`.

## Batch verdict

**Needs rework.** Only IF4-20 (Flex envelope) and IF4-29 (MUC nick self-echo) are fixed. The diff does not address 33 of the 35 findings. The new `#[serde(skip)]` introduces a structural inconsistency where the envelope construction references a field that will not serialize.