# Fix review — GX1

| ID | Verdict | Comment |
|---|---|---|
| GX1-01 | PARTIAL | The global-validate issue is fixed (`manager.validate(token)` at the new `authenticate` helper), and the loopback flag now uses `client.local` from `resolve_client`, but the visibility predicate is now applied unconditionally (the `if visibility::visible_owner_filter().is_some()` guard was removed), which changes behavior for single-owner deployments that previously skipped the check. The core unauthenticated-read bypass is closed by the `StatusCode::UNAUTHORIZED` denial. |
| GX1-02 | FIXED | The `attachment_response` helper adds `ARTIFACT_DOCUMENT_CSP` and forces `Content-Disposition: attachment` for `text/html`/`image/svg` prefixes, and critically the `if active_document` branch inserts `Content-Disposition: attachment` even when `original_name` is `None` (line `response.headers_mut().insert(header::CONTENT_DISPOSITION, "attachment".parse().unwrap());`), covering the gap the reviewer identified. |
| GX1-04 | REGRESSION | The rewritten `revalidate_for_dispatch` still contains the malformed logic the reviewer flagged: `if !super::auth::wall_admits(live.role.as_deref(), method) { return None; } Some(live)` — wait, this actually looks correct now. But the diff also shows leftover dead code: `Some(live) .then_some(live); }` which is a no-op expression statement, and more importantly the `idempotency_principal` path and the `Cached`/`Waiting` arms in `dispatch_for_connection` are not shown in the diff, so the second concrete path (cached `gateway.token.current` after demotion) cannot be proven closed. The new test `cached_and_waiting_admin_results_require_live_authority` exercises `revalidate_for_dispatch` directly but does not exercise the actual `dispatch_for_connection` `Cached`/`Waiting` arms, so the integration gap remains. |
| GX1-07 | PARTIAL | The `touch_device` call at line 876 is now wrapped via `cleanup::touch_device_last_seen`, and the node-connect path at line 1139 is also wrapped, but `cleanup.rs:74-83` (`run_cleanup`) still contains the synchronous `store.touch_device(&node_id)` call — the diff only adds the helper and uses it in `mod.rs`, not in `run_cleanup` itself. |

## New problems

- `src/gateway/server/connection/dispatch.rs:181-186` — Dead code: `Some(live) .then_some(live); }` is a no-op expression statement left in the source; it compiles but is confusing and suggests an incomplete edit.
- `src/gateway/server/connection/dispatch.rs` — The `authorization_error` helper uses `crate::gateway::method_admin::method_requires_admin(method)` and `caller_is_member()`, but the diff does not show imports for these; if they were not already imported, this will fail to compile. The diff context does not include the import block, so this is unverifiable from the diff alone.
- `src/gateway/server/attachment_route.rs` — The `authenticate` function returns `Option<(String, &'static str)>` but the test asserts `Some((OWNER_USER_ID.to_string(), "operator"))` with a `String` first element; the type mismatch between `String` and `&str` in the tuple suggests the test may not compile as written, or the function signature was changed after the test was written.

## Batch verdict

Needs rework: GX1-04 and GX1-07 remain unproven or partially addressed, and the dead code in `dispatch.rs` should be cleaned up before commit.