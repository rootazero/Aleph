# Adjudication — IF6 — minimax-cn/MiniMax-M3

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF6-01 | UPHELD | Line 146 uses unrestricted `sender.contains(allowed)`, so `boss@company.com` matches `boss@company.com.attacker.example`. | P0 |
| IF6-02 | UPHELD | Lines 348 and 355 fetch with `RFC822`, discard fetch errors, then line 355 collects before later acknowledgement; failed or unpublished mail can be marked Seen. | P0 |
| IF6-03 | UPHELD | Line 29 derives `Debug` for `EmailConfig`, whose excerpt contains credential fields; equivalent credential-bearing IRC and Mattermost configs are also cited. | P0 |
| IF6-04 | UPHELD | Line 329 uses source timestamps as message identity, so equal timestamps across senders can collide and trigger the cited channel/message-ID deduplication. | P0 |
| IF6-05 | UPHELD | Line 261 constructs `IrcChannel` with fixed `"irc"`; the same factory pattern is reported for email, signal, and Mattermost, preventing distinct instance IDs. | P0 |
| IF6-06 | UPHELD | Line 124 copies Markdown directly into HTML and line 157-style conversion inserts unescaped text and URLs, allowing user text to become markup. | P1 |
| IF6-07 | UPHELD | Line 306 snapshots `config.nick`; later 433 handling reportedly changes registration identity but not this local `nick`, so filtering can use the stale nickname. | P1 |
| IF6-08 | UPHELD | Line 230 only queues the command, then returns success at line 236; later worker write failures are merely logged and cannot affect the returned result. | P1 |
| IF6-09 | UPHELD | Lines 125–131 break normally after retry exhaustion; the normal JoinHandle completion therefore cannot be distinguished from shutdown by the outer status handling. | P1 |
| IF6-10 | UPHELD | Line 90 declares `attachments: true`, while the cited send paths pass only `message.text`; successful sends consequently omit attachments. | P1 |
| IF6-11 | UPHELD | Lines 307–308 perform synchronous stdout locking and writing inside an async future, allowing a blocked pipe to stall the Tokio worker. | P1 |
| IF6-12 | DOWNGRADE | Lines 216–218 spawn a detached stdin reader that is not stopped or joined; restart can leave the old reader consuming input, though impact is robustness/resource behavior. | P2 |
| IF6-13 | DOWNGRADE | Lines 599–605 insert an operation with `outcome: None` before authorization; cancellation can strand it, but the cited public seam makes this a robustness/resource defect rather than P1. | P2 |
| IF6-14 | DOWNGRADE | Line 663 uses captured `now` when unpinned, allowing expiry during authorization to pass the final admission check; the trigger is real but narrowly timed. | P2 |
| IF6-15 | UPHELD | Lines 329–355 search all UNSEEN messages and collect the entire fetch stream into `Vec` without bounds, allowing unbounded batch memory use. | P2 |
| IF6-16 | UPHELD | Line 288 uses `BufReader::lines()` without a maximum line length, so an endless newline-free remote input can grow memory unboundedly. | P2 |
