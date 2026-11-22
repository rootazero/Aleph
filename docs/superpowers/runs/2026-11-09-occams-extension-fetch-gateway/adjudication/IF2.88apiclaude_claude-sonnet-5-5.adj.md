# Adjudication — IF2 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF2-01 | UPHELD | Line 12 derives Debug; bot_token at line 15 is plain String with no redaction | P0 |
| IF2-02 | UPHELD | Line 1297 hardcodes "discord"; no create_with_id override, config ID discarded | P0 |
| IF2-03 | UPHELD | Line 243 subtracts marker from budget after body already split; consumption precedes truncation | P0 |
| IF2-04 | UPHELD | Line 757 only sends to queue; no interaction.create_response or defer call in Command branch | P1 |
| IF2-05 | UPHELD | Lines 1066, 1059, 1079 parse u64 then pass to ::new without zero check before Serenity panic | P1 |
| IF2-06 | UPHELD | Line 949 timeout returns no error; line 1000 returns Ok unconditionally after timeout warn | P1 |
| IF2-07 | UPHELD | Line 853 starts from empty; lines 854-865 add four intents but never add GUILDS for threads | P1 |
| IF2-08 | UPHELD | Line 60 uses .find, returns first match; duplicate slug yields single candidate, not Ambiguous | P1 |
| IF2-09 | UPHELD | Line 128 Err(_) skips guild; line 138 still commits cache with incomplete snapshot | P1 |
| IF2-10 | UPHELD | Line 683 checks channel allowlist for slash; message 483 / component 366 check guild first | P1 |
| IF2-11 | UPHELD | Line 464 synchronous store.load before async select; line 344 append also blocks in finalize | P1 |
| IF2-12 | UPHELD | Line 1006 stop sends signal and returns immediately; background task at 909 still runs, races new start | P2 |
| IF2-13 | UPHELD | Line 120 fetches single page of 200; line 123 returns that page as complete list | P2 |
