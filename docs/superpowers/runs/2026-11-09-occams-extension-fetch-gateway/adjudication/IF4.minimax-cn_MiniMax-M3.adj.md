# Adjudication — IF4 — minimax-cn/MiniMax-M3

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF4-01 | UPHELD | Line 181 opens a raw TCP stream; no TLS wrapper or STARTTLS appears in the excerpt before authentication. | P0 |
| IF4-02 | UPHELD | Lines 43–46 use `xml.find`, searching the entire XML, so a body attribute can precede the root attribute. | P0 |
| IF4-03 | UPHELD | Line 38 derives `Debug` for `LineConfig`, whose token and secret are plain `String` fields at lines 41 and 44. | P0 |
| IF4-04 | UPHELD | Lines 189–194 treat any participant match as a target match, without requiring a one-to-one chat. | P0 |
| IF4-05 | UPHELD | Line 286 advances the watermark before later row decoding can fail via `row_result?` at line 282. | P0 |
| IF4-06 | UPHELD | Lines 461–462 impose a 100-row limit while querying only by `after`, with no tie-breaker or pagination shown. | P0 |
| IF4-07 | DOWNGRADE | Lines 60–72 define policy and allowlists, but the excerpt does not prove they are ignored by the runtime permission path. | P1 |
| IF4-08 | UPHELD | Line 279 selects only `chunks[0]`, and line 281 replaces the original with that first chunk. | P1 |
| IF4-09 | UPHELD | Lines 349–352 use bare `chat_identifier`; line 328 routes unmatched targets to participant `send_text`. | P1 |
| IF4-10 | UPHELD | Lines 336–345 process only attachments with `path`; pathless attachments are skipped before success at line 347. | P1 |
| IF4-11 | UPHELD | Line 129 synchronously opens SQLite inside an async function; subsequent synchronous polling use is indicated by the finding. | P2 |
| IF4-12 | DOWNGRADE | Lines 143–154 spawn the listener and only warn on registration failure; however, the excerpt also states catch-up polling may remain active. | P2 |
| IF4-13 | UPHELD | Lines 391–396 map every wildcard or loopback listener host to callback `localhost`, unsuitable for remote BlueBubbles. | P1 |
| IF4-14 | UPHELD | Lines 177–186 query exactly one page with fixed `offset: 0`, then return `None` at line 201. | P2 |
| IF4-15 | DOWNGRADE | Lines 39–44 perform synchronous directory enumeration, but the excerpt does not show whether this runs on a runtime worker. | P2 |
| IF4-16 | DOWNGRADE | Line 489 buffers the full response before any cap check, confirming unbounded memory, but disk usage is not shown exceeding the cap. | P2 |
| IF4-17 | UPHELD | Line 503 falls back to the bare temp directory, while line 35 sweeps only the dedicated staging directory. | P2 |
| IF4-18 | UPHELD | Line 84 defaults to `line/webhook` without a leading slash, so the claimed exact request-target comparison cannot match `/line/webhook`. | P1 |
| IF4-19 | UPHELD | Lines 145–146 require snake_case `file_name` and `file_size`, whereas LINE wire events use camelCase fields. | P1 |
| IF4-20 | REFUTED | The excerpt shows `push_flex` constructs `FlexPayload::new(to, ...)`, apparently preserving the recipient; it does not prove the claimed malformed envelope. | - |
| IF4-21 | UPHELD | Line 265 converts missing text to an empty string; no attachment construction for non-text messages appears in the excerpt. | P1 |
| IF4-22 | UPHELD | Lines 178–183 only inspect LINE-specific metadata; the excerpt does not show any standard attachment handling before sending. | P1 |
| IF4-23 | UPHELD | Lines 58–64 spawn one unbounded task per accepted connection; the excerpt shows no concurrency cap or read deadline. | P2 |
| IF4-24 | UPHELD | Line 130 unconditionally sets `is_group: true` for every converted Matrix room message. | P1 |
| IF4-25 | DOWNGRADE | Lines 98–100 perform an adapter mention check, but the excerpt does not prove Core independently rejects an accepted exact Matrix mention. | P2 |
| IF4-26 | DOWNGRADE | Line 78 calls synchronous `deduper.try_insert` from an async loop, but the excerpt does not establish the underlying DB operation blocks this runtime. | P2 |
| IF4-27 | DOWNGRADE | Lines 6 and 9 call it LRU but use an unbounded `HashSet`; runtime pruning or DB growth is not shown in this excerpt. | P2 |
| IF4-28 | DOWNGRADE | Line 72 downloads before checking `bytes.len()` at line 73, confirming post-download enforcement; outbound unbounded allocation is not shown. | P2 |
| IF4-29 | UPHELD | Line 64 compares the MUC resource nick with `own.local`, while the actual joined nickname is separately configurable. | P1 |
| IF4-30 | UPHELD | Lines 209 and 358–360 track authentication state, but the outbound branch writes stanzas without checking it. | P1 |
| IF4-31 | UPHELD | Lines 137–140 acknowledge only enqueueing, while line 361 logs write failure without returning it to the sender. | P1 |
| IF4-32 | DOWNGRADE | Line 210 holds the read guard across the awaited send path, creating a plausible shutdown deadlock, but saturated writer behavior is not shown. | P2 |
| IF4-33 | UPHELD | Lines 195–197 create an unbounded `String`; no maximum-size check appears before parsing or appending stanza data. | P2 |
| IF4-34 | UPHELD | Lines 64–68 replace `&amp;` first and later `&lt;`, allowing newly exposed entities to be decoded a second time. | P1 |
| IF4-35 | DOWNGRADE | Lines 321–322 suppress chat lookup errors and lines 349–356 fall back to a private identity, but `is_group` fallback does not prove a group reply targets one contact. | P2 |
