# Adjudication — IF1 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF1-01 | REFUTED | Lines 113-119 show `AccessController::new` is called but no `expect` panic in the excerpt; claim not proven | - |
| IF1-02 | UPHELD | Lines 38, 42, 47 insert duplicate keys without checking; later overwrites earlier silently | P1 |
| IF1-03 | DOWNGRADE | Line 205 shows `block_streaming` exists but no merge/usage visible; dead field not P1 | P3 |
| IF1-04 | UPHELD | Lines 19-24 show proxy failure returns `Bot::new(token)` directly, bypassing intended route | P1 |
| IF1-05 | UPHELD | Line 140 discards send result with `let _ =`; line 143 returns `Ok(())` regardless | P1 |
| IF1-06 | UPHELD | Lines 364-369 show loop logs warnings, returns `"0"` on all-fail; line 372 returns success | P1 |
| IF1-07 | UPHELD | Lines 259-262 return `true` for AdminOnly without checking admin; comment admits caller should check | P1 |
| IF1-08 | REFUTED | Excerpt shows diagnostic request but no offset advance visible in lines 107-121 | - |
| IF1-09 | UPHELD | Lines 661-675 show poll answer builds InboundMessage and sends without access check | P1 |
| IF1-10 | UPHELD | Lines 745-759 spawn health loop with no cancel; stop() at 787-798 only signals shutdown_tx | P1 |
| IF1-11 | UPHELD | Line 153 locks tracker, line 162 awaits send while locked, line 163 updates after | P1 |
| IF1-12 | UPHELD | Lines 179-183 unconditionally write/finalize error without policy check | P1 |
| IF1-13 | UPHELD | Lines 43-57 construct ResolvedConfig with hardcoded defaults, not chat-specific policy | P1 |
| IF1-14 | UPHELD | Line 76 hardcodes `id: MessageId::new("1")` for synthetic inbound used by reactions | P1 |
| IF1-15 | UPHELD | Lines 102-115 send event then immediately call deliver_run_media without finalize ack | P1 |
| IF1-16 | UPHELD | Lines 809-812 find first account where resolve succeeds; account fallback at (0, None) matches first | P1 |
| IF1-17 | UPHELD | Lines 1023-1034 clone only first bot instance for approval capability | P1 |
| IF1-18 | UPHELD | Lines 250-256 show get-check-insert race; concurrent calls both return true for Once | P2 |
| IF1-19 | UPHELD | Line 92 sets last_update before send at 94; failure leaves throttle consumed with no message | P2 |
| IF1-20 | UPHELD | Lines 75-89 CAS succeeds but persistence error at 86-88 only logs, returns regardless | P2 |
