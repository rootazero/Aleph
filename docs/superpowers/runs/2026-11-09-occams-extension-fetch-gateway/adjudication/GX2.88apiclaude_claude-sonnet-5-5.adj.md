# Adjudication — GX2 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| GX2-01 | UPHELD | Lines 379-387 confirm: reply sent at 379, `close_session` called at 387, real ordering inversion visible in excerpt. | P1 |
| GX2-02 | DOWNGRADE | Format mismatch is real but requires either a `:` in `conversation_id` or a channel rename after session start — narrow, non-default triggers. | P2 |
| GX2-03 | REFUTED | The cited line 622 logs `send_err` from an *error-reply* send, not from `send_public_telecom`; the `ChannelPublicReplySink::send_reply` path correctly does `map(|_| ())` and the finding's token-leak chain is not visible in the excerpt. | - |
| GX2-04 | DOWNGRADE | Dedup/coalescer interaction issue is real per lines 421-430, but the practical impact requires a specific replay window after coalescing — not a straightforward duplicate execution. | P2 |
| GX2-05 | UPHELD | Lines 384-387 confirm LLM await before `close_session`; inconsistency window is real and bounded only by LLM latency. | P2 |
| GX2-06 | REFUTED | Excerpt lines 487-495 show eviction IDs are used inside the same DB transaction which is correctly rolled back on `TurnTooLarge`; in-memory `Eviction` struct is a local variable, not persisted or returned to callers outside that path. No real defect. | - |
| GX2-07 | REFUTED | The excerpt explicitly shows `take(66)` plus post-read `match bytes.as_slice()` catches all TOCTOU outcomes; the finding's own analysis concedes no security bypass is possible. Purely informational, not a defect. | - |
| GX2-08 | UPHELD | Line 543 confirms `InvalidTimestamp` used for provider-ID validation — wrong variant, real maintainability/observability defect. | P3 |
