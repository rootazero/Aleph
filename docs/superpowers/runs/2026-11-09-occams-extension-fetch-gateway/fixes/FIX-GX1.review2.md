# Fix review — GX1

| ID | Verdict | Comment |
|---|---|---|
| GX1-01 | REGRESSION | The diff cannot compile: `authenticate` contains an extra closing brace after `if let Some(manager) = shared_token { ... }` (`+    }` followed by `+    let manager = device_tokens?;`), so the function body is prematurely closed; it also does not address the first-wave requirement to use the per-request instance credential consistently beyond the manager API shown. |
| GX1-02 | FIXED | `active_document` now covers HTML/SVG and the response always receives `Content-Disposition: attachment` and `ARTIFACT_DOCUMENT_CSP`, including the no-filename branch (`let disposition_type = ...; if active_document { ... "attachment".parse() ... }`). |
| GX1-04 | REGRESSION | The rewritten function contains malformed syntax and control flow: `Some(live)` is followed by `.then_some(live);` and then an extra `if !is_browser_viewer_method(method)` block, so the non-viewer branch is not valid Rust and the intended authorization boundary cannot execute. |
| GX1-07 | FIXED | The three synchronous `touch_device` paths are now routed through `cleanup::touch_device_last_seen`, which uses `spawn_blocking` (`tokio::task::spawn_blocking(move || store.touch_device(&device_id))`) and preserves the best-effort error semantics. |
| GX1-06 | FIXED | `delete_secret` now offloads the vault write with `tokio::task::spawn_blocking(move || shared_token.delete_secret(&delete_key))`, matching the existing blocking-storage boundary. |
| GX1-01 | REGRESSION | The attachment route's `ConnectInfo<SocketAddr>` extractor is not guaranteed to be available for this merged router; no `into_make_service_with_connect_info` change is shown, so the route can panic or fail to serve attachments in deployments without connect-info. |
| GX1-02 | FIXED | The response builder preserves ordinary image/media behavior by selecting `inline` for non-active MIME types and only applies the sandbox CSP to `text/html` and `image/svg` types. |

## New problems

- `src/gateway/server/attachment_route.rs:74` — The authentication helper has an unmatched extra `}` after the shared-token block, making the module syntactically invalid and preventing the entire gateway from compiling.
- `src/gateway/server/connection/dispatch.rs:197` — The diff leaves `.then_some(live);` and a second `if !is_browser_viewer_method(method)` block after an unconditional `Some(live)`, creating malformed syntax and leaving the non-viewer authorization path undefined.
- `src/gateway/server/connection/dispatch.rs:181` — The live-authority check is performed before the `is_context_method` branch, but the function still compares `live.user != start.user` and rejects absent live users; this changes ordinary RPC behavior beyond the requested cache/queue authorization refresh and can deny requests after a connection snapshot is cleared.
- `src/gateway/server/attachment_route.rs:135` — The `with_caller_identity` scope supplies `caller_role` as the role and `caller_user` as the user, but the shared-token path resolves the operator to `OWNER_USER_ID`; if that value is not the user ID expected by the existing visibility predicate, legitimate shared-token downloads are incorrectly denied.
- `src/gateway/server/connection/mod.rs:1139` — The node-connect last-seen write now awaits `spawn_blocking` inline in the connection task, rather than merely moving the write to an existing background boundary; this can still delay the connection event loop on a saturated blocking pool, despite avoiding synchronous mutex blocking.

## Batch verdict

needs rework — the batch contains syntax errors in the attachment authentication helper and dispatch revalidation function, plus unresolved connect-info and visibility-scoping risks; it is not safe to commit.