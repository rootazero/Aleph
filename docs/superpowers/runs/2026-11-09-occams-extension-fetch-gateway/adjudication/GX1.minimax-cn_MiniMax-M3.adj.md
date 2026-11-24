# Adjudication — GX1 — minimax-cn/MiniMax-M3

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| GX1-01 | UPHELD | `serve_attachment` accepts only `Path<String>` at line 44 and fetches storage directly at line 79, with no credential or identity input shown. | P0 |
| GX1-02 | UPHELD | Lines 93-103 return attachment bytes inline using the stored MIME and no attachment-specific sandbox or download policy. | P0 |
| GX1-03 | UPHELD | Line 288 logs the first 200 bytes of raw WebSocket text before authentication or RPC parameter parsing, exposing credential-bearing frames. | P0 |
| GX1-04 | UPHELD | Line 181 explicitly leaves non-viewer snapshots untouched; ordinary queued and cached paths can therefore bypass live authority changes. | P0 |
| GX1-05 | UPHELD | Line 96 unwraps parsing of the stored MIME string, so invalid header bytes can panic the request task. | P1 |
| GX1-06 | UPHELD | Lines 38-41 synchronously call `store_secret` from an async handler without an offload boundary, allowing blocking vault I/O. | P2 |
| GX1-07 | DOWNGRADE | Line 879 synchronously calls `touch_device`; this can block a worker, but the excerpt alone does not establish the broader severe connection-lifecycle impact. | P2 |
| GX1-08 | UPHELD | Lines 146-147 check capacity under a read lock without reserving a slot, allowing concurrent upgrades to pass before later insertion. | P2 |
| GX1-09 | UPHELD | Lines 1181-1183 spawn an infinite task without retaining a cancellation handle, so returning from the server does not stop it. | P2 |
| GX1-10 | UPHELD | Lines 315-329 special-case approval requests and prepare detached asynchronous work before the ordinary limiter reached later. | P2 |
| GX1-11 | UPHELD | Lines 1948-1959 count needles in `connection/forward.rs`; the cited needles occur only in documentation, not executable delivery wiring. | P2 |
