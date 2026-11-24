# Final verdicts — GX1

| ID | Verdict | Severity | Reason |
|---|---|---|---|
| GX1-01 | UPHELD | P0 | Line 44 accepts no auth input; visible_owner_filter requires CALLER_USER task-local missing in HTTP / `serve_attachment` accepts only `Path<String>` at line 44 and fetches storage directly at line 79, with no credential or identity input shown. |
| GX1-02 | UPHELD | P0 | Lines 93-96 serve attachment.mime_type with no CSP; contrast artifact_route.rs sandboxing / Lines 93-103 return attachment bytes inline using the stored MIME and no attachment-specific sandbox or download policy. |
| GX1-03 | UPHELD | P0 | Line 288 logs raw text preview before parsing; connect frames contain credentials in first 200 bytes / Line 288 logs the first 200 bytes of raw WebSocket text before authentication or RPC parameter parsing, exposing credential-bearing frames. |
| GX1-04 | UPHELD | P0 | Lines 181-183 snapshot reuse bypasses admin gate after demotion; cached idempotency skips authorization / Line 181 explicitly leaves non-viewer snapshots untouched; ordinary queued and cached paths can therefore bypass live authority changes. |
| GX1-05 | UPHELD | P1 | Line 96 unwrap() on untrusted attachment.mime_type.parse() panics on invalid header bytes / Line 96 unwraps parsing of the stored MIME string, so invalid header bytes can panic the request task. |
| GX1-06 | UPHELD | P1 | Lines 40, 83 call synchronous vault store_secret/delete_secret directly in async handler / Lines 38-41 synchronously call `store_secret` from an async handler without an offload boundary, allowing blocking vault I/O. |
| GX1-07 | DOWNGRADE | P1 | Lines 879, 1139, 1141 call synchronous store.touch_device on Tokio workers without spawn_blocking / Line 879 synchronously calls `touch_device`; this can block a worker, but the excerpt alone does not establish the broader severe connection-lifecycle impact. |
| GX1-08 | UPHELD | P2 | Lines 146-147 read connections.len() then later insert; no atomic reservation before upgrade approval / Lines 146-147 check capacity under a read lock without reserving a slot, allowing concurrent upgrades to pass before later insertion. |
| GX1-09 | UPHELD | P2 | Lines 1181, 1191 spawn infinite loops; no cancellation handle retained for shutdown or failed startup / Lines 1181-1183 spawn an infinite task without retaining a cancellation handle, so returning from the server does not stop it. |
| GX1-10 | UPHELD | P2 | Line 329 spawns unbounded detached approval tasks before normal RPC admission at line 449 / Lines 315-329 special-case approval requests and prepare detached asynchronous work before the ordinary limiter reached later. |
| GX1-11 | UPHELD | P2 | Line 1948 reads forward.rs; needles only match comment lines not executable calls; wrong file scanned / Lines 1948-1959 count needles in `connection/forward.rs`; the cited needles occur only in documentation, not executable delivery wiring. |
