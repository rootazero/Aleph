# Final verdicts — GX2

| ID | Verdict | Severity | Reason |
|---|---|---|---|
| GX2-03 | REFUTED | P0 | The cited line 622 logs `send_err` from an *error-reply* send, not from `send_public_telecom`; the `ChannelPublicReplySink::send_reply` path correctly does `map( / Lines 744-750 explicitly avoid stringifying transport errors; the cited excerpt does not show a token-bearing error being logged. |
| GX2-01 | UPHELD | P1 | Lines 379-387 confirm: reply sent at 379, `close_session` called at 387, real ordering inversion visible in excerpt. |
| GX2-02 | REFUTED | P1 | tie-break(88apiclaude/claude-opus-5-5): Excerpt shows identical `channel_id:conversation_id` format at both sites; `:` in IDs or channel rename are speculative, unsupported by the code shown. |
| GX2-04 | REFUTED | P1 | tie-break(88apiclaude/claude-opus-5-5): Excerpt shows no re-IDing of merged messages; dedup runs before admission; claimed replay window failure is speculative, not supported by shown code. |
| GX2-05 | UPHELD | P2 | Lines 384-387 confirm LLM await before `close_session`; inconsistency window is real and bounded only by LLM latency. |
| GX2-06 | REFUTED | P2 | Excerpt lines 487-495 show eviction IDs are used inside the same DB transaction which is correctly rolled back on `TurnTooLarge`; in-memory `Eviction` struct is a local variable, not persisted or returned to callers outside that path. No real defect. / Lines 487-495 may accumulate eviction IDs, but the finding itself states the transaction is rolled back; no externally observable defect is shown. |
| GX2-07 | REFUTED | P2 | The excerpt explicitly shows `take(66)` plus post-read `match bytes.as_slice()` catches all TOCTOU outcomes; the finding's own analysis concedes no security bypass is possible. Purely informational, not a defect. / Lines 101-113 revalidate the actual bytes after reading; concurrent growth or truncation is rejected by the post-read match, not a security bypass. |
| GX2-08 | UPHELD | P3 | Line 543 confirms `InvalidTimestamp` used for provider-ID validation — wrong variant, real maintainability/observability defect. |
