# Adjudication — IF3 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF3-01 | UPHELD | Line 141 `_ => Ok(body_str.to_string())` accepts plaintext when key configured, bypassing signature/freshness | P0 |
| IF3-02 | UPHELD | Line 77 `is_group: false` for CardAction ignores actual chat type, bypasses group policy | P0 |
| IF3-03 | UPHELD | Line 75 `String::from_utf8_lossy` replaces bytes before encrypt; load at 123 `into_bytes()` corrupts bincode | P0 |
| IF3-04 | UPHELD | Lines 409-418 construct policy from `config.access` only; `allowed_chats` field never consulted | P0 |
| IF3-05 | UPHELD | Line 269 passes `app_id` where webhook uses `self.info.id`; misroutes inbound and replies | P1 |
| IF3-06 | UPHELD | Lines 95-100 rewrite conversation_id with scope suffix; breaks allowlist matching and reply delivery | P1 |
| IF3-07 | UPHELD | Lines 62-68 use card `open_message_id` as dedup key; distinct actions share one ID | P1 |
| IF3-08 | UPHELD | Lines 508-511 return Ok on HTTP 200 without checking Lark `code`; business errors treated as success | P1 |
| IF3-09 | UPHELD | Line 87 swaps closed=true before final update; line 96 logs failure but returns; RunComplete skips retry | P1 |
| IF3-10 | UPHELD | Line 262 creates card after fallback at 246; accumulated inner buffer never transferred | P1 |
| IF3-11 | UPHELD | Line 86 `let _ =` discards caption send error; returns Ok despite partial delivery | P1 |
| IF3-12 | UPHELD | Lines 240-248 only serialize `text`; attachment fields declared but never encoded | P1 |
| IF3-13 | UPHELD | Line 22 `extract_text` returns empty for media; lines 44,45 provide no attachments/raw | P1 |
| IF3-14 | UPHELD | Lines 28,32 compare raw JID strings against E.164 config; pre-filter before core's normalizing comparison | P1 |
| IF3-15 | UPHELD | Line 522 maps NotConnected to Internal; loses retryable distinction for queue | P1 |
| IF3-16 | UPHELD | Line 432 stores Connected in atomic; driver line 430 ignores it; status reads only PairingState | P1 |
| IF3-17 | UPHELD | Line 35 `SecretVault::open_or_backup` and 141 `create_dir_all` are synchronous on async path | P1 |
| IF3-18 | UPHELD | Line 64 `image::open` and 69-73 Lanczos resize run synchronously in async function | P1 |
| IF3-19 | UPHELD | Line 78 `entry().or_insert` unbounded; line 83 limits deque but not HashMap keys | P2 |
| IF3-20 | UPHELD | Line 134 spawns detached; line 190 writes None after shutdown returns; no join before restart | P2 |
| IF3-21 | UPHELD | Line 501 clears entire map mid-batch; line 505 inserts same batch; first message lost before consumption | P2 |
