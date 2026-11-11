# Fix brief — GX1

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/GX1.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/GX1.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/GX1.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-GX1.md

## FIX THESE (8)

### GX1-01 [P0] Attachment HTTP ingress treats an unauthenticated caller as unrestricted
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/server/attachment_route.rs:42-79` (`serve_attachment`); mount at `/home/zou/data/workspace/Aleph/src/gateway/server/mod.rs:1058-1067,1103-1105` (`GatewayServer::build_router`).
- **Failure mode**: An HTTP caller who knows a session/attachment identifier requests `/api/memory/sessions/attachment/{session_id}:{attachment_id}` without credentials → receives private attachment bytes, including another user's attachments. The handler accepts no authentication input, and the merged router supplies no caller-identity middleware. The owner check only runs when an RPC task-local identity already exists; ordinary HTTP dispatch has none. The identifier is a storage locator, not an explicitly issued/revocable bearer capability.
- **Evidence**:
  > `Path(id): Path<String>,`
  > `if visibility::visible_owner_filter().is_some() {`
  > `let attachment = match state.store.get_attachment(session_id, attachment_id).await {`
  Cross-module confirmation: `/home/zou/data/workspace/Aleph/src/gateway/visibility.rs:54-66` explicitly defines missing `CALLER_USER` as unrestricted for internal callers; the HTTP route incorrectly consumes that internal default. `/home/zou/data/workspace/Aleph/src/gateway/server/mod.rs:1171` adds security headers, not authentication.
- **Suggested fix**: Authenticate this ingress using the established HTTP credential validation and scope the resolved user before applying the existing session-visibility predicate; deny missing identity instead of interpreting it as internal. Preserve Core's ownership decision and the existing not-found response for inaccessible attachments. If URL-only clients are required, use an explicit session-scoped capability rather than treating storage IDs as authority.
- **Confidence**: 1.0

### GX1-02 [P0] Attachment HTML and SVG execute with the gateway's origin
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/server/attachment_route.rs:92-107` (`serve_attachment`).
- **Failure mode**: A stored attachment contains HTML or script-bearing SVG and its corresponding MIME type; a user opens its download URL → the response renders as an active document on the gateway origin. Unlike the artifact/canvas routes, this route sets no sandbox CSP or forced-download policy. The global Panel CSP explicitly permits inline scripts. An attachment can therefore run same-origin script and access the gateway origin's browser state/interfaces. Authenticating the download alone does not isolate this content.
- **Evidence**:
  > `let mut response = Response::new(attachment.data.into());`
  > `.insert(header::CONTENT_TYPE, attachment.mime_type.parse().unwrap());`
  > `response`
  Cross-module confirmation: `/home/zou/data/workspace/Aleph/src/session/markdown_projector.rs:149-177,184-197` passes attachment-marker/Image MIME strings into storage; `/home/zou/data/workspace/Aleph/src/memory/store/markdown/store_impl.rs:424-440,681-685` persists and returns them without a passive-content restriction. `/home/zou/data/workspace/Aleph/src/security/headers.rs:21,131-146` supplies a default CSP containing `script-src 'self' 'unsafe-inline'` unless a handler sets its own policy. This contrasts with the already-established isolation contract in `/home/zou/data/workspace/Aleph/src/gateway/server/artifact_route.rs:70-76`.
- **Suggested fix**: Apply the existing sandboxed document CSP to attachment HTML/SVG, or serve active document types as downloads. Preserve normal image/media rendering and the shared security-header layer's stricter-handler-policy behavior; do not invent a second artifact policy.
- **Confidence**: 0.99

### GX1-03 [P0] Raw WebSocket previews persist authentication credentials in logs
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/server/connection/mod.rs:286-288` (`handle_connection`, inbound text arm).
- **Failure mode**: Debug logging is enabled and a valid `connect` frame puts `token`, `device_token` or `bootstrap_ticket` within its first 200 bytes → the credential is written verbatim to application logs before authentication or method parsing. For example, a valid frame beginning `{"method":"connect","params":{"token":"..."},"id":1}` exposes the shared Gateway credential. Later redaction/audit handling cannot retract it.
- **Evidence**:
  > `debug!("WS recv from {}: {}", conn_id, &text[..preview_end]);`
- **Suggested fix**: Log only non-sensitive frame metadata such as length and parsed method/request ID; never preview raw RPC text. Preserve the existing UTF-8-safe handling if a preview remains for explicitly non-sensitive data. Do not log credential-bearing params even on rejected requests.
- **Confidence**: 1.0

### GX1-04 [P0] Ordinary RPCs can retain pre-demotion authority through the lane and cache paths
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/server/connection/dispatch.rs:167-186,210-244` (`revalidate_for_dispatch`, `dispatch_for_connection`, `idempotency_principal`); `/home/zou/data/workspace/Aleph/src/gateway/server/connection/mod.rs:672-678,717-743,758-766` (`handle_connection`).
- **Failure mode**: An admin submits an admin-only ordinary RPC while its lane is occupied, then `users.update` demotes that user before the permit becomes available → dispatch still scopes the captured operator snapshot and the Core admin gate permits an operation the live member may no longer perform. A second concrete path needs no queue race: an admin first calls `gateway.token.current` with an idempotency key; after demotion, repeating the same key still returns the cached plaintext token because the namespace contains the user but not current authority, and the Cached/Waiting arms bypass the admin gate. The login wall admits members, so it does not close this hole.
- **Evidence**:
  > `if !is_browser_viewer_method(method) { return Some(start.clone()); }`
  > `snapshot.user.as_deref().unwrap_or("<anon>").to_string()`
  > `let resp = JsonRpcResponse::success(req.id.clone(), cached);`
  The first quote condenses the adjacent lines at `/home/zou/data/workspace/Aleph/src/gateway/server/connection/dispatch.rs:181-183`. Cross-module confirmation: `/home/zou/data/workspace/Aleph/src/gateway/handlers/users.rs:761-801` re-stamps existing connections on demotion; `/home/zou/data/workspace/Aleph/src/gateway/method_admin.rs:108-110` admin-gates `gateway.*`; `/home/zou/data/workspace/Aleph/src/gateway/handlers/gateway_token.rs:25-29` returns the plaintext current token. `/home/zou/data/workspace/Aleph/src/gateway/lane.rs:61-77,195-199` classifies the un-overridden `.current` method as idempotency-guarded Mutate. Only the off-loop allowlist currently refreshes ordinary authority after waiting.
- **Suggested fix**: Refresh the live caller and reapply the canonical login/admin authorization at the actual dispatch/result-return boundary for all methods, including cache hits and waiters. Deny or invalidate cached privileged results after an authority change. Keep authorization derived from the existing shared predicates rather than adding a parallel role policy; preserve per-user/method idempotency and viewer-specific identity checks.
- **Confidence**: 0.99

### GX1-05 [P1] Unvalidated stored MIME types panic the attachment request task
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/server/attachment_route.rs:96` (`serve_attachment`).
- **Failure mode**: An attachment is stored with a MIME string containing an invalid HTTP-header byte, for example `image/png\nX: y`; requesting it → `HeaderValue` parsing returns an error and `unwrap()` panics the request task instead of producing a controlled response. Stored MIME is a plain string, not a validated header type.
- **Evidence**:
  > `.insert(header::CONTENT_TYPE, attachment.mime_type.parse().unwrap());`
  Cross-module confirmation: `/home/zou/data/workspace/Aleph/src/session/markdown_projector.rs:159-170,184-195` accepts string MIME metadata; `/home/zou/data/workspace/Aleph/src/memory/store/markdown/store_impl.rs:424-440,628-640,681-685` stores and returns that string unchanged. No internal type invariant makes the unwrap safe.
- **Suggested fix**: Handle invalid MIME metadata at this HTTP boundary with a controlled error or `application/octet-stream`, matching the established artifact-route header handling. Preserve opaque attachment bytes and avoid attempting to sanitize arbitrary text into a header.
- **Confidence**: 0.99

### GX1-06 [P1] Async admin secret writes perform blocking vault locking and disk commits
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/admin_api/secrets.rs:33-41,75-83` (`create_or_update_secret`, `delete_secret`).
- **Failure mode**: An authenticated admin writes/deletes a secret while storage is slow or another process holds the vault lock → the async handler synchronously waits for the filesystem lock, serialization and atomic disk commit on a Tokio worker. Concurrent calls also wait on the synchronous vault lock, so enough valid requests can stall unrelated gateway work. This is actual blocking I/O, not merely inexpensive synchronous crypto.
- **Evidence**:
  > `.store_secret(&key, &body.value)`
  > `.delete_secret(&key)`
  Cross-module confirmation: `/home/zou/data/workspace/Aleph/src/gateway/security/shared_token.rs:279-282,302-306` takes the synchronous vault write lock and calls `set`/`delete`; `/home/zou/data/workspace/Aleph/src/secrets/vault.rs:200-213,255-256,273-276` performs the synchronous atomic temp/fsync/rename commit through `VaultIo`.
- **Suggested fix**: Run the complete synchronous vault operation in `spawn_blocking`, keeping lock acquisition inside that closure and mapping join/storage failures to the current HTTP error contract. Account for read/list handlers taking the same synchronous lock while a writer commits. Preserve authentication, name validation and the existing atomic vault transaction; do not duplicate vault logic in the Interface layer.
- **Confidence**: 1.0

### GX1-07 [P1] Connection lifecycle writes SQLite directly on Tokio workers
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/server/connection/mod.rs:876-880,1139-1141` (`handle_connection`); `/home/zou/data/workspace/Aleph/src/gateway/server/connection/cleanup.rs:74-83` (`run_cleanup`).
- **Failure mode**: A device/node connects or disconnects while the SecurityStore connection is busy or the SQLite write is slow → the async connection task blocks its runtime worker in the synchronous mutex/UPDATE. Connect/disconnect churn can occupy the worker pool and delay other connections and cleanup. Node admission is already offloaded, but these adjacent writes bypass that boundary.
- **Evidence**:
  > `if let Err(e) = store.touch_device(did) {`
  > `if let Err(e) = store.touch_device(node_id) {`
  > `if let Err(e) = store.touch_device(&node_id) {`
  Cross-module confirmation: `/home/zou/data/workspace/Aleph/src/gateway/security/store/devices.rs:109-115` locks `self.conn` with a synchronous mutex and executes a SQLite UPDATE.
- **Suggested fix**: Move these writes into the existing blocking-storage boundary, or await a small `spawn_blocking` operation with the current best-effort error semantics. Preserve the node/device last-seen update and cleanup ordering; do not hold the async connections-map lock while waiting for storage work.
- **Confidence**: 1.0

### GX1-11 [P2] Event-delivery regression guard succeeds on comments instead of executable wiring
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/server/handler.rs:1947-1972` (`the_delivery_loop_parses_each_event_once_and_projects_it`); `/home/zou/data/workspace/Aleph/src/gateway/server/connection/forward.rs:10-24` (source-level pin documentation).
- **Failure mode**: The real event-loop parse/projection wiring in `connection/mod.rs` is removed or duplicated → this regression test still passes, because it scans `connection/forward.rs` and the one occurrence of each counted needle is in that file's documentation, not a call. Its asserted single parse and connected projection therefore report green without examining either executable operation. This is an incorrectly implemented existing guard, not a request for additional test coverage.
- **Evidence**:
  > `let src = include_str!("connection/forward.rs");`
  > `//! - \`serde_json::from_str::<serde_json::Value>(&event_json)\` (the`
  > `//! - \`project_for(\` (the payload-projection call site, so removing the`
  A read-only string-count check confirmed exactly one occurrence of each needle in `forward.rs`, both on those comment lines. The actual delivery loop is in `/home/zou/data/workspace/Aleph/src/gateway/server/connection/mod.rs:1461-1579`.
- **Suggested fix**: Point the guard at the actual delivery loop and count executable syntax rather than documentation text, preferably using the existing AST-test approach. Preserve the intended invariant—one main envelope parse and a real visibility-projection call—without adding dummy marker comments to satisfy the test.
- **Confidence**: 1.0

## Clean files

No independent finding survived verification in these 20 fully reviewed batch files:

- `/home/zou/data/workspace/Aleph/src/gateway/server/artifact_route.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/browser_viewer_route.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/connection/auth.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/canvas_asset_route.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/control_plane/server.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/connection/off_loop.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/panel_asset_http_tests.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/admin_api/reconciler.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/metrics_endpoint.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/byte_range.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/panel_asset_route.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/panel_asset_route_tests.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/admin_api/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/browser_viewer_issuer_tests.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/probe.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/per_client_buffer.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/server/flood_guard.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/admin_api/resume.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/control_plane/assets.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/control_plane/mod.rs`


## DO NOT FIX (3) — refuted / deferred by adjudication or routing

- GX1-08: ### GX1-08 [P2] Connection limits are checked without reserving the upgrade slot
- GX1-09: ### GX1-09 [P2] Server maintenance tasks outlive successful shutdown and failed startup
- GX1-10: ### GX1-10 [P2] Node approvals bypass request budgets and spawn unbounded detached waiters
