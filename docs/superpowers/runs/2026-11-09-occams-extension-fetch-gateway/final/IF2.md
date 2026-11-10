# Final verdicts — IF2

| ID | Verdict | Severity | Reason |
|---|---|---|---|
| IF2-01 | UPHELD | P0 | Line 12 derives Debug; bot_token at line 15 is plain String with no redaction / `DiscordConfig` derives `Debug` while exposing `pub bot_token: String`, so formatting the config can disclose the credential. |
| IF2-02 | UPHELD | P0 | Line 1297 hardcodes "discord"; no create_with_id override, config ID discarded / `create` hardcodes `DiscordChannel::new("discord", config)`, discarding any configured instance identity. |
| IF2-03 | DOWNGRADE | P0 | Line 243 subtracts marker from budget after body already split; consumption precedes truncation / Line 250 truncates the already-consumed body, confirming silent loss; however no production send-path use of this helper is shown. |
| IF2-04 | DOWNGRADE | P1 | Line 757 only sends to queue; no interaction.create_response or defer call in Command branch / The shown slash path only queues the interaction without an ACK, but upstream acknowledgement may occur outside this excerpt; a protocol failure is not proven. |
| IF2-05 | UPHELD | P1 | Lines 1066, 1059, 1079 parse u64 then pass to ::new without zero check before Serenity panic / Parsed numeric IDs are passed to Serenity constructors at lines 1066 and 1079 without rejecting zero. |
| IF2-06 | UPHELD | P1 | Line 949 timeout returns no error; line 1000 returns Ok unconditionally after timeout warn / Lines 949-957 only warn after readiness timeout, while the function later reaches unconditional `Ok(())`. |
| IF2-07 | UPHELD | P1 | Line 853 starts from empty; lines 854-865 add four intents but never add GUILDS for threads / Line 853 starts with empty intents and lines 854-865 never add `GUILDS` despite a guild-thread handler. |
| IF2-08 | DOWNGRADE | P1 | Line 60 uses .find, returns first match; duplicate slug yields single candidate, not Ambiguous / Line 62 uses `find`, selecting the first matching guild rather than detecting duplicate matches; wrong-target resolution is real but typically P2. |
| IF2-09 | UPHELD | P1 | Line 128 Err(_) skips guild; line 138 still commits cache with incomplete snapshot / Lines 130-132 discard every channel-fetch error, then lines 137-140 cache the resulting incomplete snapshot permanently. |
| IF2-10 | UPHELD | P1 | Line 683 checks channel allowlist for slash; message 483 / component 366 check guild first / Line 683 applies the guild-channel allowlist unconditionally, including to slash interactions whose `guild_id` is absent. |
| IF2-11 | DOWNGRADE | P1 | Line 464 synchronous store.load before async select; line 344 append also blocks in finalize / Line 464 performs synchronous `store.load` inside the async driver; actual Tokio-worker starvation and severity depend on runtime scheduling not shown here. |
| IF2-12 | UPHELD | P2 | Line 1006 stop sends signal and returns immediately; background task at 909 still runs, races new start / `stop` sends shutdown at line 1007 but never awaits the background task before returning and clearing state. |
| IF2-13 | DOWNGRADE | P2 | Line 120 fetches single page of 200; line 123 returns that page as complete list / Line 120 requests only one 200-item page despite the documented pagination intent; incompleteness is real, but severity depends on >200-guild deployments. |
