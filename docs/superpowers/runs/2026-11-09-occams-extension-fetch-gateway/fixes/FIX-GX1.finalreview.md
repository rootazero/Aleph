# Fix review — GX1

| ID | Verdict | Comment |
|---|---|---|
| GX1-01 | PARTIAL | The route now rejects missing credentials with `StatusCode::UNAUTHORIZED` and scopes the resolved user via `with_caller_identity(...)`, but the diff also contains an extra `}` after the shared-token branch in `authenticate`, so this code does not compile and the authentication path is not usable as shown. |
| GX1-02 | FIXED | `attachment_response` now identifies HTML/SVG as active documents, inserts `ARTIFACT_DOCUMENT_CSP`, and forces `Content-Disposition: attachment` even when `original_name` is absent. |
| GX1-03 | FIXED | The raw frame preview was replaced with `debug!("WS recv from {}: {} bytes", conn_id, text.len());`, so credential-bearing frame contents are no longer logged. |
| GX1-04 | REGRESSION | The diff leaves dangling fragments such as `}.then_some(live);` and `if !is_browser_viewer_method(method) {` after the rewritten `revalidate_for_dispatch`, producing invalid Rust and an unclosed function/block; the authority fix cannot work as shown. |
| GX1-05 | FIXED | Invalid stored MIME values now fall back with `.parse().unwrap_or_else(|_| "application/octet-stream".parse().unwrap())`, removing the panic path. |
| GX1-06 | FIXED | Both vault mutations now run inside `tokio::task::spawn_blocking`, including `shared_token.delete_secret(&delete_key)`, so the synchronous vault lock and disk commit no longer execute directly on the async worker. |
| GX1-07 | PARTIAL | Connect and cleanup writes are offloaded through `spawn_blocking`, but the diff’s `cleanup::touch_device_last_seen(...)` call is structurally misplaced inside the node-admission branch and the surrounding braces are malformed, so the cited node-connect and cleanup paths cannot be proven fixed from this diff. |
| GX1-08 | NOT_FIXED | No change reserves a connection slot before approving the WebSocket upgrade; the diff does not address the concurrent-upgrade capacity race. |
| GX1-09 | NOT_FIXED | No background-task handles are retained or cancelled on shutdown, failed startup, or browser-runtime replacement, so detached maintenance loops can still outlive their owner. |
| GX1-10 | NOT_FIXED | The diff does not bound or cancel detached `node.approval.request` tasks, so approval requests can still bypass the normal request budgets and teardown cancellation. |
| GX1-11 | FIXED | The guard now reads `include_str!("connection/mod.rs")`, where the executable delivery loop resides, rather than scanning the documentation-only needles in `connection/forward.rs`. |

## New problems

- `src/gateway/server/attachment_route.rs:73` — An extra `}` follows the shared-token validation branch, leaving the `if let Some(manager)` block and function malformed; the file cannot compile.
- `src/gateway/server/connection/dispatch.rs:190` — Dangling `}.then_some(live);` and subsequent stale branches remain after the rewritten function, causing invalid Rust syntax and duplicated control flow.
- `src/gateway/server/connection/mod.rs:1149` — The `cleanup::touch_device_last_seen(...)` call is placed inside the node-admission match with mismatched braces rather than in a valid lifecycle path, so the node-connect stamp is not correctly integrated.

## Batch verdict

Needs rework: the attachment and dispatch diffs contain compile-breaking syntax, while GX1-08 through GX1-10 remain unaddressed.