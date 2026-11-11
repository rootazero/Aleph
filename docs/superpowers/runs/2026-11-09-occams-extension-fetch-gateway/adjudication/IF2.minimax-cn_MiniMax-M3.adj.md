# Adjudication — IF2 — minimax-cn/MiniMax-M3

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF2-01 | UPHELD | `DiscordConfig` derives `Debug` while exposing `pub bot_token: String`, so formatting the config can disclose the credential. | P0 |
| IF2-02 | UPHELD | `create` hardcodes `DiscordChannel::new("discord", config)`, discarding any configured instance identity. | P1 |
| IF2-03 | DOWNGRADE | Line 250 truncates the already-consumed body, confirming silent loss; however no production send-path use of this helper is shown. | P2 |
| IF2-04 | DOWNGRADE | The shown slash path only queues the interaction without an ACK, but upstream acknowledgement may occur outside this excerpt; a protocol failure is not proven. | P2 |
| IF2-05 | UPHELD | Parsed numeric IDs are passed to Serenity constructors at lines 1066 and 1079 without rejecting zero. | P1 |
| IF2-06 | UPHELD | Lines 949-957 only warn after readiness timeout, while the function later reaches unconditional `Ok(())`. | P1 |
| IF2-07 | UPHELD | Line 853 starts with empty intents and lines 854-865 never add `GUILDS` despite a guild-thread handler. | P1 |
| IF2-08 | DOWNGRADE | Line 62 uses `find`, selecting the first matching guild rather than detecting duplicate matches; wrong-target resolution is real but typically P2. | P2 |
| IF2-09 | UPHELD | Lines 130-132 discard every channel-fetch error, then lines 137-140 cache the resulting incomplete snapshot permanently. | P1 |
| IF2-10 | UPHELD | Line 683 applies the guild-channel allowlist unconditionally, including to slash interactions whose `guild_id` is absent. | P1 |
| IF2-11 | DOWNGRADE | Line 464 performs synchronous `store.load` inside the async driver; actual Tokio-worker starvation and severity depend on runtime scheduling not shown here. | P2 |
| IF2-12 | UPHELD | `stop` sends shutdown at line 1007 but never awaits the background task before returning and clearing state. | P2 |
| IF2-13 | DOWNGRADE | Line 120 requests only one 200-item page despite the documented pagination intent; incompleteness is real, but severity depends on >200-guild deployments. | P2 |
