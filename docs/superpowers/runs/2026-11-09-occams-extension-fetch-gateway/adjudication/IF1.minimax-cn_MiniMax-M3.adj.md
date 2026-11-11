# Adjudication — IF1 — minimax-cn/MiniMax-M3

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF1-01 | REFUTED | `AccessController::new` behavior is not shown; lines 113-119 only insert the synthetic controller and do not prove the claimed resolver panic. | - |
| IF1-02 | UPHELD | Lines 35-48 use unconditional `HashMap::insert`, so duplicate account/group/topic keys silently replace earlier policy. | P1 |
| IF1-03 | UPHELD | Lines 205 and 45-48 expose `block_streaming` but omit it from `ResolvedConfig`; the excerpt does not show it reaching delivery. | P1 |
| IF1-04 | UPHELD | Lines 20-24 explicitly log a warning and construct the no-proxy fallback after proxy-client build failure. | P1 |
| IF1-05 | UPHELD | Lines 139-143 discard the `send` result with `let _ = ...` and unconditionally return `Ok(())`. | P1 |
| IF1-06 | UPHELD | Lines 363-373 log attachment errors, assign placeholder ID `0`, and return success without requiring a successful attachment. | P1 |
| IF1-07 | DOWNGRADE | Lines 259-262 say the caller checks admin status; the excerpt does not prove an unauthorized caller reaches this helper. | P2 |
| IF1-08 | UPHELD | Lines 107-120 perform a diagnostic `getUpdates` request that can consume a pending update; the excerpt does not show safe restoration or dispatch. | P1 |
| IF1-09 | UPHELD | Lines 661-675 show poll answers entering a handler without an access check; no equivalent authorization gate appears in the cited path. | P1 |
| IF1-10 | UPHELD | Lines 745-754 spawn an unbounded health-check loop, while lines 790-794 stop only polling shutdown senders. | P1 |
| IF1-11 | UPHELD | Lines 153-162 hold the tracker mutex across `send_text_message().await`, making the lock span network I/O. | P1 |
| IF1-12 | UPHELD | Lines 179-183 unconditionally write and finalize an error message without checking the effective error policy. | P1 |
| IF1-13 | UPHELD | Lines 43-62 construct delivery with synthetic account/token/default policy and a fresh cooldown, rather than effective resolved configuration. | P1 |
| IF1-14 | UPHELD | Lines 75-90 construct the synthetic inbound message with ID `1`, which the orchestrator receives as its inbound context. | P1 |
| IF1-15 | DOWNGRADE | Lines 103-115 send `RunComplete` then immediately deliver media; the comment asserts ordering “in practice,” but no completion acknowledgement is shown. | P2 |
| IF1-16 | UPHELD | Lines 809-813 select the first account whose resolver entry exists; the excerpt does not prioritize explicit mappings across accounts. | P1 |
| IF1-17 | UPHELD | Lines 1020-1034 explicitly select `bot_instances.first()` and construct the capability from that single account. | P1 |
| IF1-18 | UPHELD | Lines 250-256 perform a non-atomic `get` followed by `insert`; concurrent callers can both observe no cooldown and return `true`. | P2 |
| IF1-19 | DOWNGRADE | Lines 90-94 set `last_update` before the first send and propagate failure, so the defect is a delayed retry rather than data loss. | P3 |
| IF1-20 | UPHELD | Lines 75-89 advance the in-memory offset before persistence, and the excerpt shows persistence failure is only logged, allowing restart reprocessing. | P2 |
