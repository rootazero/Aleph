# Adjudication — IF4 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF4-01 | UPHELD | Line 181 `TcpStream::connect` proves bare TCP; no TLS/STARTTLS before SASL at line ~238+ | P0 |
| IF4-02 | UPHELD | Line 43 `xml.find(&dq_pattern)` scans entire string including body text, not root tag only | P0 |
| IF4-03 | UPHELD | Line 38 `#[derive(Debug,` on line 41 `channel_access_token: String,` exposes secrets | P0 |
| IF4-04 | UPHELD | Line 193 `any(|p| p.address == Some(target))` matches group participant, line 196 caches it | P0 |
| IF4-05 | UPHELD | Line 286 advances watermark inside loop, line 282 `row_result?` exits batch on error | P0 |
| IF4-06 | UPHELD | Line 461 `limit: 100` with no pagination; line ~35 (poll.rs) advances cursor losing remainder | P0 |
| IF4-07 | UPHELD | Line 60 `dm_policy`, 64 `group_policy` config fields never passed to Core permission system | P0 |
| IF4-08 | UPHELD | Line 271 splits message, line 279 `chunks[0]` discards remainder, returns success | P0 |
| IF4-09 | UPHELD | Line 350 `chat.chat_identifier` has no chat_id:/chat_guid: prefix; line 328 routes to participant | P1 |
| IF4-10 | UPHELD | Line 337 `if let Some(path)` skips attachments without path; line 348 returns success anyway | P1 |
| IF4-11 | UPHELD | Line 129 `MessagesDb::open` sync call in async start; line 173 spawned task calls sync DB | P1 |
| IF4-12 | UPHELD | Line 143 spawns webhook, line 154 only warns on failure, line 157+ returns Ok/Connected | P1 |
| IF4-13 | UPHELD | Line 392 maps unspecified addresses to "localhost", line 396 sends to remote BlueBubbles | P1 |
| IF4-14 | UPHELD | Line 177 `offset: 0` hardcoded, single query loop lines 186-200, no pagination | P1 |
| IF4-15 | UPHELD | Line 34 `sweep_stale` calls line 39 sync `read_dir` in async start/poll context | P1 |
| IF4-16 | UPHELD | Line 489 `res.bytes()` unbounded buffer, no cap enforcement before memory allocation | P2 |
| IF4-17 | UPHELD | Line 503 fallback to bare temp_dir, line 35 sweep only checks dedicated subdir | P2 |
| IF4-18 | UPHELD | Line 84 default "line/webhook" missing leading slash, line 197 compares with req.path | P1 |
| IF4-19 | UPHELD | Line 146 `file_name: String` snake_case, wire expects camelCase per LINE API docs | P1 |
| IF4-20 | UPHELD | Line 108 `FlexPayload` not wrapped in messages array; line 672 QuickReplyItem missing type field | P1 |
| IF4-21 | UPHELD | Line 265 `.text().unwrap_or("")` loses image/video content, line 266 empty attachments | P1 |
| IF4-22 | UPHELD | Line 169 `fn send` only checks metadata, standard attachments never read/sent | P1 |
| IF4-23 | UPHELD | Line 60 spawns per connection, line 156+ `read` has no timeout, no connection limit | P2 |
| IF4-24 | UPHELD | Line 130 `is_group: true` hardcoded for all Matrix rooms including direct messages | P1 |
| IF4-25 | UPHELD | Line 99 adapter check_mention, line 265 empty metadata; Core re-checks with different rules | P1 |
| IF4-26 | UPHELD | Line 78 `deduper.try_insert` calls sync rusqlite in async loop; line 79+ `query_row` sync | P1 |
| IF4-27 | UPHELD | Line 9 HashSet grows unbounded at runtime, line 23 prune only on startup, no eviction | P2 |
| IF4-28 | UPHELD | Line 72 `get_media_content` allocates full Vec, line 73 checks size after allocation | P2 |
| IF4-29 | UPHELD | Line 64 compares `nick == own.local`, but line 321 joins with `config.nick` (aleph default) | P1 |
| IF4-30 | UPHELD | Line 359 `write_cmd_rx.recv()` selected before line 209 authenticated=false set/checked | P1 |
| IF4-31 | UPHELD | Line 138 `.send(stanza)` returns immediately; line 360 write failure only warns, no retry | P1 |
| IF4-32 | UPHELD | Line 210 `write_tx.read().await` holds guard during send; line 197 stop waits for write lock | P1 |
| IF4-33 | UPHELD | Line 195 `buffer = String::new()`, line 196+ continuously appends with no size limit | P2 |
| IF4-34 | UPHELD | Line 64 replaces `&amp;` first, then line 65 `&lt;` causes double-decode of `&amp;lt;` | P1 |
| IF4-35 | UPHELD | Line 322 `.ok()` converts query failure to None, line 350+ uses handle.id as conversation | P1 |
