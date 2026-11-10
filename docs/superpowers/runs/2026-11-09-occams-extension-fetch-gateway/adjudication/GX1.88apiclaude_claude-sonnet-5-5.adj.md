# Adjudication — GX1 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| GX1-01 | UPHELD | Line 44 accepts no auth input; visible_owner_filter requires CALLER_USER task-local missing in HTTP | P0 |
| GX1-02 | UPHELD | Lines 93-96 serve attachment.mime_type with no CSP; contrast artifact_route.rs sandboxing | P0 |
| GX1-03 | UPHELD | Line 288 logs raw text preview before parsing; connect frames contain credentials in first 200 bytes | P0 |
| GX1-04 | UPHELD | Lines 181-183 snapshot reuse bypasses admin gate after demotion; cached idempotency skips authorization | P0 |
| GX1-05 | UPHELD | Line 96 unwrap() on untrusted attachment.mime_type.parse() panics on invalid header bytes | P1 |
| GX1-06 | UPHELD | Lines 40, 83 call synchronous vault store_secret/delete_secret directly in async handler | P1 |
| GX1-07 | UPHELD | Lines 879, 1139, 1141 call synchronous store.touch_device on Tokio workers without spawn_blocking | P1 |
| GX1-08 | UPHELD | Lines 146-147 read connections.len() then later insert; no atomic reservation before upgrade approval | P2 |
| GX1-09 | UPHELD | Lines 1181, 1191 spawn infinite loops; no cancellation handle retained for shutdown or failed startup | P2 |
| GX1-10 | UPHELD | Line 329 spawns unbounded detached approval tasks before normal RPC admission at line 449 | P2 |
| GX1-11 | UPHELD | Line 1948 reads forward.rs; needles only match comment lines not executable calls; wrong file scanned | P2 |
