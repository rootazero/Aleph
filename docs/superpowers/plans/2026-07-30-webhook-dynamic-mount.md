# Webhook Dynamic Mount Table — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the boot-time webhook route snapshot with a shared mount table owned by `ChannelRegistry`, so `channel.start` / `stop` / `delete` / `create` change what the HTTP surface actually serves — and retire the three leftovers that snapshot forced (`RESERVED_ROUTE_PREFIXES`, SPA path shadowing, 503-before-signature).

**Architecture:** `build_router()` registers exactly one constant route, `POST /webhook/{*rest}`, whose state is an `Arc<WebhookMountTable>` (a `path → WebhookMount` map). `ChannelRegistry` owns that table and mutates it at six lifecycle points, so the route table is constant while the *contents* follow the registry. Operator-writable paths never enter axum's route table again, which deletes the boot-panic failure mode the reserved-prefix list existed to prevent.

**Tech Stack:** Rust 1.96 (MSRV 1.95), axum 0.8, tokio, `tokio::sync::RwLock`, `hmac`/`sha2`.

## Global Constraints

- **Spec:** [docs/superpowers/specs/2026-07-30-webhook-dynamic-mount-design.md](../specs/2026-07-30-webhook-dynamic-mount-design.md). Every decision reference below (`D1`–`D7`, `§2.A`–`§2.G`) points there.
- **Never panic on operator input.** Every rejected mount is `warn!` + skip. `path` comes from `[channels.*] path`.
- **Never hold the table lock across `.await`.** `lookup()` clones what the request needs out of the read guard and drops it before the handler's async work. A slow handler must not starve the table.
- **Comments in English, replies to the user in Chinese** (project convention).
- **Commit format:** `<scope>: <description>`, e.g. `gateway: …`. Attribution is disabled globally — no `Co-Authored-By` trailer.
- **R10 applies to `src/harness/` only.** Nothing in this plan touches it; do not add files there.
- **Zero-consumer abstractions get deleted, not kept for later** (R10 YAGNI). That is why Task 3 deletes `RESERVED_ROUTE_PREFIXES` instead of guarding it.
- **Verify with:** `cargo test -p alephcore --lib <filter>` for units, `cargo clippy --all-targets -- -D warnings` before the final commit. `cargo fmt` — **never** `cargo fmt -- <file>` (it formats the whole repo); use `rustfmt --check` + `Edit` if you need file scope.

## File Structure

| File | Responsibility after this plan |
|---|---|
| `src/gateway/webhook_receiver.rs` (modify) | `WEBHOOK_ROUTE_PREFIX`, `WebhookMountTable` (mount/unmount/lookup), the single-route builder, the dispatch endpoint, HMAC helpers (unchanged). This file grows ~120 lines of production code; it is currently ~857 lines including tests, so no split is warranted. |
| `src/gateway/server/mod.rs` (modify) | Carries `Arc<WebhookMountTable>`, merges the one constant route unconditionally. Loses `RESERVED_ROUTE_PREFIXES` + `is_reserved_route`. Gains the source guard test. |
| `src/gateway/channel_registry.rs` (modify) | Owns the table; the six lifecycle hooks live here and nowhere else (D4). |
| `src/gateway/interfaces/webhook/config.rs` (modify) | `validate()` enforces the `/webhook/` prefix (§4.4). |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs` (modify) | Hands the registry's table to the server (two lines replacing a 44-line collection block). |
| `src/gateway/handlers/channel.rs` (modify) | Loses `needs_webhook_restart` and both `"restart_required"` branches. |
| `src/gateway/mod.rs` (modify) | Re-export `WebhookMountTable` + `WEBHOOK_ROUTE_PREFIX`. |
| `docs/reference/GATEWAY.md` (modify) | The "Channel webhook ingestion" section describes the dynamic table, not the snapshot. |

---

### Task 1: `WebhookMountTable` — the table and its admission rules

**Files:**
- Modify: `src/gateway/webhook_receiver.rs` (add after the `WebhookMount` struct, ~line 102)
- Modify: `src/gateway/mod.rs:187` (re-export)
- Test: `src/gateway/webhook_receiver.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: existing `WebhookMount { handler, inbound, status, channel_id }`, `ChannelId`, `InboundMessageSender`, `ChannelStatus`.
- Produces:
  - `pub const WEBHOOK_ROUTE_PREFIX: &str = "/webhook";`
  - `pub struct WebhookMountTable` with `pub fn new() -> Self`, `pub async fn mount(&self, mount: WebhookMount) -> bool`, `pub async fn unmount_channel(&self, channel_id: &ChannelId) -> usize`, `pub async fn mounted_count(&self) -> usize`, and `pub(crate) async fn lookup(&self, path: &str) -> Option<MountedHandler>`.
  - `pub(crate) struct MountedHandler { handler: Arc<dyn WebhookHandler>, inbound: InboundMessageSender, status: Arc<tokio::sync::RwLock<ChannelStatus>> }` — the clone taken out of the read guard.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `src/gateway/webhook_receiver.rs`. `MockWebhookHandler` already exists in that module (it takes `secret` + `handler_path`); reuse it.

```rust
    // --- WebhookMountTable admission rules ---

    fn mount_for(channel: &str, path: &str, secret: &str, state: &ChannelState) -> WebhookMount {
        WebhookMount {
            handler: Arc::new(MockWebhookHandler {
                secret: secret.to_string(),
                handler_path: path.to_string(),
            }),
            inbound: state.sender(),
            status: state.status_handle(),
            channel_id: ChannelId::new(channel),
        }
    }

    #[tokio::test]
    async fn mount_accepts_a_path_under_the_shared_prefix() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);
        let table = WebhookMountTable::new();

        assert!(table.mount(mount_for("a", "/webhook/one", "s", &state)).await);
        assert_eq!(table.mounted_count().await, 1);
        assert!(table.lookup("/webhook/one").await.is_some());
        // Exact match only — a sibling path is not a hit.
        assert!(table.lookup("/webhook/one/").await.is_none());
    }

    #[tokio::test]
    async fn mount_refuses_paths_outside_the_shared_prefix() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);
        let table = WebhookMountTable::new();

        // Every one of these is unreachable behind `/webhook/{*rest}`, so
        // accepting it would re-create the advertised-but-disabled shape this
        // work exists to remove: channel Connected, endpoint deaf.
        for path in [
            "/settings",           // would have shadowed a Panel SPA path
            "/",                   // ditto, the SPA root
            "webhook/no-slash",    // missing leading '/'
            "/webhook",            // no sub-path — `{*rest}` matches nothing
            "/webhook/",           // ditto
            "/webhookx/sneaky",    // prefix look-alike, not a segment match
        ] {
            assert!(
                !table.mount(mount_for("a", path, "s", &state)).await,
                "path {path} must be refused"
            );
        }
        assert_eq!(table.mounted_count().await, 0);
    }

    /// Which handler is live at `path`, observed exactly the way production
    /// observes it: only the live secret verifies. Comparing `Arc` identity
    /// would not catch a table that kept the right *slot* with the wrong
    /// secret, which is the failure that matters.
    async fn live_secret_is(table: &WebhookMountTable, path: &str, secret: &str) -> bool {
        let live = table.lookup(path).await.expect("must still be mounted");
        let body = b"probe".as_slice();
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Webhook-Signature",
            WebhookReceiver::compute_signature(secret, body)
                .parse()
                .unwrap(),
        );
        live.handler.verify(&headers, body)
    }

    #[tokio::test]
    async fn duplicate_path_keeps_the_smaller_channel_id() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);

        // Incumbent is the smaller id → newcomer is refused.
        let table = WebhookMountTable::new();
        assert!(table.mount(mount_for("aaa", "/webhook/dup", "first", &state)).await);
        assert!(!table.mount(mount_for("zzz", "/webhook/dup", "second", &state)).await);
        assert!(
            live_secret_is(&table, "/webhook/dup", "first").await,
            "incumbent must survive"
        );

        // Incumbent is the larger id → newcomer evicts it. Same outcome
        // whichever order the registry happens to start channels in, which is
        // the whole point (D5): route ownership must not be a per-boot coin flip.
        let table = WebhookMountTable::new();
        assert!(table.mount(mount_for("zzz", "/webhook/dup", "first", &state)).await);
        assert!(table.mount(mount_for("aaa", "/webhook/dup", "second", &state)).await);
        assert!(
            live_secret_is(&table, "/webhook/dup", "second").await,
            "the lower channel id must win regardless of arrival order"
        );
    }

    #[tokio::test]
    async fn remounting_the_same_channel_refreshes_the_handler() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);
        let table = WebhookMountTable::new();

        // `restart_channel` builds a fresh handler; keeping the old clone is
        // exactly the staleness this table exists to prevent (spec §2.E).
        assert!(table.mount(mount_for("a", "/webhook/one", "old", &state)).await);
        assert!(table.mount(mount_for("a", "/webhook/one", "new", &state)).await);
        assert_eq!(table.mounted_count().await, 1);
        assert!(live_secret_is(&table, "/webhook/one", "new").await);
    }

    #[tokio::test]
    async fn unmount_channel_removes_every_path_that_channel_owns() {
        use crate::gateway::channel::ChannelState;
        let state = ChannelState::new(4);
        let table = WebhookMountTable::new();

        table.mount(mount_for("a", "/webhook/one", "s", &state)).await;
        table.mount(mount_for("a", "/webhook/two", "s", &state)).await;
        table.mount(mount_for("b", "/webhook/three", "s", &state)).await;

        assert_eq!(table.unmount_channel(&ChannelId::new("a")).await, 2);
        assert!(table.lookup("/webhook/one").await.is_none());
        assert!(table.lookup("/webhook/two").await.is_none());
        // A sibling channel's route must not be collateral damage.
        assert!(table.lookup("/webhook/three").await.is_some());

        // Unmounting an unknown channel is a no-op, not an error.
        assert_eq!(table.unmount_channel(&ChannelId::new("nope")).await, 0);
    }
```

`lookup` returns `MountedHandler`, whose `handler` is `Arc<dyn WebhookHandler>` — a trait object, so no test-only accessor on `MockWebhookHandler` is reachable through it. That is why `live_secret_is` above probes through `verify()` instead of reaching for a field.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib webhook_receiver::tests -- --nocapture`
Expected: FAIL — `cannot find type WebhookMountTable in this scope`.

- [ ] **Step 3: Implement the table**

In `src/gateway/webhook_receiver.rs`, add `use std::collections::HashMap;` and `use tokio::sync::RwLock;` to the imports, then insert after the `WebhookMount` struct:

```rust
/// The one path prefix every channel webhook route lives under.
///
/// A single constant route (`/webhook/{*rest}`) carries all channel webhook
/// traffic, so an operator-writable `path` never enters axum's route table.
/// That is what makes the mount table hot-swappable *and* what removed the
/// boot-panic failure mode `RESERVED_ROUTE_PREFIXES` used to guard against.
pub const WEBHOOK_ROUTE_PREFIX: &str = "/webhook";

/// Whether `path` can be reached behind `{WEBHOOK_ROUTE_PREFIX}/{{*rest}}`.
///
/// Requires a whole extra segment: `/webhook` and `/webhookx/y` are both out
/// (the wildcard needs at least one segment, and the prefix must end on a
/// segment boundary).
fn is_mountable_path(path: &str) -> bool {
    path.strip_prefix(WEBHOOK_ROUTE_PREFIX)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|sub| !sub.is_empty())
}

/// What a request needs, cloned out of the table's read guard.
///
/// Handler work is async; holding the table's `RwLock` across that await
/// would let one slow platform starve every other mount, plus block every
/// `channel.start` / `stop`.
pub(crate) struct MountedHandler {
    pub(crate) handler: Arc<dyn WebhookHandler>,
    pub(crate) inbound: InboundMessageSender,
    pub(crate) status: Arc<RwLock<ChannelStatus>>,
}

/// Live `path -> mount` map behind the single webhook route.
///
/// Owned by `ChannelRegistry`, which is the only thing allowed to mutate it —
/// mounting follows channel lifecycle instead of being a boot snapshot. A
/// route that `channel.stop` / `channel.delete` cannot remove is an
/// authenticated endpoint the operator believes is gone.
#[derive(Default)]
pub struct WebhookMountTable {
    mounts: RwLock<HashMap<String, WebhookMount>>,
}

impl WebhookMountTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mount `mount` at its handler's declared path.
    ///
    /// Returns `false` when the mount was refused; every refusal is warned
    /// with the offending channel id, never panicked — `path` is operator
    /// config.
    pub async fn mount(&self, mount: WebhookMount) -> bool {
        let path = mount.handler.path().to_string();

        if !is_mountable_path(&path) {
            warn!(
                channel_id = %mount.channel_id,
                path = %path,
                prefix = WEBHOOK_ROUTE_PREFIX,
                "webhook path must be \"{WEBHOOK_ROUTE_PREFIX}/<name>\" — handler not mounted"
            );
            return false;
        }

        let mut mounts = self.mounts.write().await;
        if let Some(existing) = mounts.get(&path) {
            // Same channel restarting: always take the fresh handler. Keeping
            // the old clone is precisely the staleness this table prevents.
            if existing.channel_id != mount.channel_id {
                // Two channels want one path — operator misconfiguration. Pick
                // by channel id, not by arrival order: `start_all` iterates a
                // HashMap, so arrival order would make route ownership a
                // per-boot coin flip.
                if existing.channel_id.as_str() <= mount.channel_id.as_str() {
                    warn!(
                        path = %path,
                        holder = %existing.channel_id,
                        refused = %mount.channel_id,
                        "duplicate webhook path — keeping the lower channel id, handler not mounted"
                    );
                    return false;
                }
                warn!(
                    path = %path,
                    evicted = %existing.channel_id,
                    holder = %mount.channel_id,
                    "duplicate webhook path — lower channel id takes over the route"
                );
            }
        }

        let channel_id = mount.channel_id.clone();
        mounts.insert(path.clone(), mount);
        info!(path = %path, channel_id = %channel_id, "webhook handler mounted");
        true
    }

    /// Remove every mount owned by `channel_id`. Returns how many were removed.
    ///
    /// Called on stop / delete / re-register. Idempotent.
    pub async fn unmount_channel(&self, channel_id: &ChannelId) -> usize {
        let mut mounts = self.mounts.write().await;
        let before = mounts.len();
        mounts.retain(|_, mount| &mount.channel_id != channel_id);
        let removed = before - mounts.len();
        if removed > 0 {
            info!(
                channel_id = %channel_id,
                removed,
                "webhook handler(s) unmounted"
            );
        }
        removed
    }

    /// How many paths are live. Boot logging and diagnostics only.
    pub async fn mounted_count(&self) -> usize {
        self.mounts.read().await.len()
    }

    /// Exact-path lookup. The key is the configured path verbatim.
    pub(crate) async fn lookup(&self, path: &str) -> Option<MountedHandler> {
        let mounts = self.mounts.read().await;
        let mount = mounts.get(path)?;
        Some(MountedHandler {
            handler: Arc::clone(&mount.handler),
            inbound: mount.inbound.clone(),
            status: Arc::clone(&mount.status),
        })
    }
}
```

Then re-export in `src/gateway/mod.rs:187`:

```rust
pub use webhook_receiver::{
    WebhookHandler, WebhookMount, WebhookMountTable, WebhookReceiver, WEBHOOK_ROUTE_PREFIX,
};
```

If `InboundMessageSender` does not implement `Clone`, derive it where it is defined (`src/gateway/channel.rs`) — it wraps a `broadcast::Sender`, which is `Clone`. Check first: `grep -n "pub struct InboundMessageSender" -A 6 src/gateway/channel.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib webhook_receiver::tests`
Expected: PASS, including the pre-existing HMAC tests.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/webhook_receiver.rs src/gateway/mod.rs src/gateway/channel.rs
git commit -m "gateway: add the webhook mount table with deterministic admission"
```

---

### Task 2: Dispatch through the table, and check the signature first

**Files:**
- Modify: `src/gateway/webhook_receiver.rs:104-168` (`WebhookReceiver::router`), `:194-271` (`HandlerState` + `webhook_endpoint`)
- Test: same file's `mod tests`

**Interfaces:**
- Consumes: Task 1's `WebhookMountTable`, `MountedHandler`, `WEBHOOK_ROUTE_PREFIX`.
- Produces: `WebhookReceiver::router(table: Arc<WebhookMountTable>) -> Router` (was `router(mounts: Vec<WebhookMount>)`). `HandlerState` is deleted — the route's axum state is the table itself.

- [ ] **Step 1: Write the failing tests**

Replace the four `WebhookReceiver::router()` integration tests (`signed_post_reaches_the_channel_broadcast`, `unsigned_post_is_rejected_and_publishes_nothing`, `disconnected_channel_returns_503_and_publishes_nothing`, `duplicate_webhook_paths_are_deduped`) and delete the two now-obsolete reserved-path tests (`reserved_path_is_skipped_not_panicked`, `path_missing_leading_slash_is_skipped_not_panicked` — Task 1's `mount_refuses_paths_outside_the_shared_prefix` covers both) and `reserved_route_matches_prefix_segments_only` (its subject is deleted in Task 3). New set:

```rust
    // --- Dispatch through the shared table ---

    /// POST `body` signed with `secret` to `path`, through a router built over
    /// `table`. The router is built ONCE per test so that a mount added
    /// afterwards proves the table is live, not snapshotted.
    async fn signed_post(
        router: &Router,
        path: &str,
        secret: &str,
        body: &'static [u8],
    ) -> StatusCode {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let sig = WebhookReceiver::compute_signature(secret, body);
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("x-webhook-signature", sig)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn signed_post_reaches_the_channel_broadcast() {
        use crate::gateway::channel::ChannelState;

        let state = ChannelState::new(16);
        state.set_status(ChannelStatus::Connected).await;
        // Subscribe FIRST: `InboundMessageSender::send` errors with no
        // subscribers (broadcast semantics); in production the subscriber is
        // `ChannelRegistry::start_message_forwarder`.
        let mut rx = state.inbound_subscribe();

        let table = Arc::new(WebhookMountTable::new());
        table.mount(mount_for("a", "/webhook/mock", "s", &state)).await;
        let router = WebhookReceiver::router(Arc::clone(&table));

        let status = signed_post(&router, "/webhook/mock", "s", br#"{"text":"hi"}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            rx.try_recv().expect("must reach the channel broadcast").text,
            "hi"
        );
    }

    #[tokio::test]
    async fn a_mount_added_after_the_router_was_built_is_reachable() {
        use crate::gateway::channel::ChannelState;

        // This is the whole point of the table: `channel.create` / `channel.start`
        // at runtime happen long after `build_router()`, and the router is
        // immutable once `serve()` owns it. Before this task the answer was
        // "restart the daemon".
        let state = ChannelState::new(16);
        state.set_status(ChannelStatus::Connected).await;
        let mut rx = state.inbound_subscribe();

        let table = Arc::new(WebhookMountTable::new());
        let router = WebhookReceiver::router(Arc::clone(&table));

        assert_eq!(
            signed_post(&router, "/webhook/late", "s", br#"{"text":"hi"}"#).await,
            StatusCode::NOT_FOUND,
            "nothing is mounted yet"
        );

        table.mount(mount_for("a", "/webhook/late", "s", &state)).await;

        assert_eq!(
            signed_post(&router, "/webhook/late", "s", br#"{"text":"hi"}"#).await,
            StatusCode::OK,
            "the same router must now serve it — no rebuild, no restart"
        );
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn unmounting_makes_the_endpoint_disappear() {
        use crate::gateway::channel::ChannelState;

        // `channel.stop` / `channel.delete` used to return "stopped" while the
        // endpoint kept answering. 404 (not 503) is the fix: the route is gone.
        let state = ChannelState::new(16);
        state.set_status(ChannelStatus::Connected).await;
        let mut rx = state.inbound_subscribe();

        let table = Arc::new(WebhookMountTable::new());
        table.mount(mount_for("a", "/webhook/mock", "s", &state)).await;
        let router = WebhookReceiver::router(Arc::clone(&table));

        assert_eq!(
            signed_post(&router, "/webhook/mock", "s", br#"{"text":"hi"}"#).await,
            StatusCode::OK
        );
        assert!(rx.try_recv().is_ok());

        table.unmount_channel(&ChannelId::new("a")).await;

        assert_eq!(
            signed_post(&router, "/webhook/mock", "s", br#"{"text":"hi"}"#).await,
            StatusCode::NOT_FOUND
        );
        assert!(
            rx.try_recv().is_err(),
            "an unmounted channel must publish nothing, even with a valid signature"
        );
    }

    #[tokio::test]
    async fn unsigned_post_is_rejected_and_publishes_nothing() {
        use crate::gateway::channel::ChannelState;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = ChannelState::new(16);
        state.set_status(ChannelStatus::Connected).await;
        let mut rx = state.inbound_subscribe();

        let table = Arc::new(WebhookMountTable::new());
        table.mount(mount_for("a", "/webhook/mock", "s", &state)).await;
        let router = WebhookReceiver::router(table);

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/mock")
                    .header("x-webhook-signature", "sha256=deadbeef")
                    .body(Body::from(br#"{"text":"forged"}"#.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(rx.try_recv().is_err(), "rejected request must publish nothing");
    }

    #[tokio::test]
    async fn signature_is_checked_before_channel_status() {
        use crate::gateway::channel::ChannelState;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        // A fresh ChannelState is Disconnected. An unauthenticated caller must
        // not be able to tell "channel is down" (503) from "wrong secret" (403):
        // that difference is a state oracle on an unauthenticated surface.
        let state = ChannelState::new(16);
        let table = Arc::new(WebhookMountTable::new());
        table.mount(mount_for("a", "/webhook/mock", "s", &state)).await;
        let router = WebhookReceiver::router(table);

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/mock")
                    .header("x-webhook-signature", "sha256=deadbeef")
                    .body(Body::from(br#"{"text":"probe"}"#.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn disconnected_channel_returns_503_for_a_valid_signature() {
        use crate::gateway::channel::ChannelState;

        // Still needed as depth: a channel can move itself to Error /
        // Connecting without any RPC, so the mount outlives Connected.
        let state = ChannelState::new(16);
        let mut rx = state.inbound_subscribe();

        let table = Arc::new(WebhookMountTable::new());
        table.mount(mount_for("a", "/webhook/mock", "s", &state)).await;
        let router = WebhookReceiver::router(table);

        assert_eq!(
            signed_post(&router, "/webhook/mock", "s", br#"{"text":"hi"}"#).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn unmounted_path_under_the_prefix_is_404() {
        let table = Arc::new(WebhookMountTable::new());
        let router = WebhookReceiver::router(table);

        assert_eq!(
            signed_post(&router, "/webhook/nothing-here", "s", b"{}").await,
            StatusCode::NOT_FOUND
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib webhook_receiver::tests`
Expected: FAIL to compile — `WebhookReceiver::router` still takes `Vec<WebhookMount>`.

- [ ] **Step 3: Rewrite the builder and the endpoint**

Replace the whole `impl WebhookReceiver { pub fn router(...) }` body (`webhook_receiver.rs:104-168`) with:

```rust
/// Builds the single axum route that carries channel webhook ingestion.
///
/// This does **not** own a listener. The gateway's own server merges this
/// route into `build_router()`, so webhook traffic inherits the configured
/// bind address, TLS, and security headers. An earlier version bound
/// `0.0.0.0` itself, which silently opened a LAN port regardless of
/// `[gateway] host`.
///
/// The route pattern is a constant, and the *contents* live in the shared
/// [`WebhookMountTable`]. That split is what lets `channel.start` /
/// `channel.stop` / `channel.delete` change the served surface after
/// `serve()` has taken the router, and it keeps operator-writable paths out
/// of axum's route table entirely.
pub struct WebhookReceiver;

impl WebhookReceiver {
    /// Route all `{WEBHOOK_ROUTE_PREFIX}/…` POSTs at `table`.
    #[must_use]
    pub fn router(table: Arc<WebhookMountTable>) -> Router {
        Router::new().route(
            &format!("{WEBHOOK_ROUTE_PREFIX}/{{*rest}}"),
            post(webhook_endpoint).with_state(table),
        )
    }
```

Keep `compute_signature` / `verify_signature` exactly as they are (real consumers: `GenericWebhookHandler`, `interfaces/webhook/message_ops.rs`).

Delete `struct HandlerState` (`:194-199`) and replace `webhook_endpoint` (`:201-271`) with:

```rust
/// Axum endpoint for every mounted channel webhook.
///
/// Order matters and is deliberate:
///   1. table lookup  → 404. An unmounted path is simply not served.
///   2. signature     → 403. FIRST, so an unauthenticated caller cannot tell
///                      "channel down" from "wrong secret" (a state oracle).
///   3. channel status → 503. Depth only: `stop`/`delete` already removed the
///                      mount, so this catches a channel that moved itself to
///                      Error/Connecting without any RPC.
///   4. parse + forward.
async fn webhook_endpoint(
    State(table): State<Arc<WebhookMountTable>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // `uri.path()` rather than the extracted `{*rest}`: the table is keyed by
    // the configured path verbatim, and going through the wildcard would add a
    // percent-decoding step on one side only.
    let Some(mounted) = table.lookup(uri.path()).await else {
        return (
            StatusCode::NOT_FOUND,
            String::from("Not Found: no webhook mounted at this path"),
        );
    };

    if !mounted.handler.verify(&headers, &body) {
        warn!(path = %uri.path(), "Webhook signature verification failed");
        return (
            StatusCode::FORBIDDEN,
            String::from("Forbidden: invalid signature"),
        );
    }

    // `try_read` intentionally fails OPEN on contention: a momentary write-lock
    // holder (another request's status flip in flight) is not evidence the
    // channel is down, and dropping live traffic on a lock race would be worse
    // than the case this guards.
    if let Ok(status) = mounted.status.try_read() {
        if *status != ChannelStatus::Connected {
            warn!(
                path = %uri.path(),
                status = ?*status,
                "webhook received for a channel that is not connected — rejecting"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                String::from("Service Unavailable: channel is not connected"),
            );
        }
    }

    match mounted.handler.handle(&headers, body).await {
        Ok(messages) => {
            let mut dropped = 0usize;
            for msg in messages {
                if mounted.inbound.send(msg).is_err() {
                    dropped += 1;
                    warn!(
                        path = %uri.path(),
                        "Failed to forward inbound message (no subscriber on the channel)"
                    );
                }
            }
            if dropped > 0 {
                // 503 so the sender retries: silently returning 200 would let
                // messages vanish while the channel looks healthy.
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("Dropped {dropped} messages: channel has no subscriber"),
                );
            }
            (StatusCode::OK, String::from("ok"))
        }
        Err(e) => {
            warn!(path = %uri.path(), error = %e, "Webhook handler error");
            (StatusCode::BAD_REQUEST, String::from("Bad request"))
        }
    }
}
```

Also update `WebhookMount`'s doc comment for the `status` field (`:90-98`) — it currently claims the status check is "what actually stops a stopped/deleted channel from still answering HTTP, without building a dynamic mount table". Replace with:

```rust
    /// The owning channel's shared status cell (`ChannelState::status_handle()`).
    ///
    /// Defence in depth, not the primary guard: `stop` / `delete` remove the
    /// mount outright (see [`WebhookMountTable::unmount_channel`]). This cell
    /// covers a channel that moved itself to `Error` / `Connecting` without any
    /// RPC, where the mount legitimately outlives `Connected`.
    pub status: Arc<tokio::sync::RwLock<ChannelStatus>>,
```

Delete the three now-dangling test helpers if the compiler flags them (`HandlerState` construction in `test_webhook_endpoint_*`). Those three tests (`test_webhook_endpoint_valid_signature`, `_invalid_signature`, `_missing_signature`) built `HandlerState` directly; rewrite them as table-based or delete them — they are strictly weaker duplicates of `signed_post_reaches_the_channel_broadcast` / `unsigned_post_is_rejected_and_publishes_nothing` / the missing-header case. **Delete the first two; keep the missing-header case** rewritten over the table:

```rust
    #[tokio::test]
    async fn post_without_a_signature_header_is_rejected() {
        use crate::gateway::channel::ChannelState;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = ChannelState::new(16);
        state.set_status(ChannelStatus::Connected).await;
        let table = Arc::new(WebhookMountTable::new());
        table.mount(mount_for("a", "/webhook/mock", "s", &state)).await;
        let router = WebhookReceiver::router(table);

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/mock")
                    .body(Body::from(br#"{"text":"no sig"}"#.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib webhook_receiver`
Expected: PASS. `cargo check -p alephcore` will still fail on `server/mod.rs` and `subsystems.rs` — that is Tasks 3 and 6.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/webhook_receiver.rs
git commit -m "gateway: dispatch webhooks through the shared table, signature before status"
```

---

### Task 3: `GatewayServer` carries the table; retire the reserved-prefix list

**Files:**
- Modify: `src/gateway/server/mod.rs:366-369` (field), `:393-417` (delete const + fn), `:461` and `:513` (both constructors), `:561-565` (setter), `:732-737` (merge), `:1064-1115` (tests)
- Test: same file's `mod tests`

**Interfaces:**
- Consumes: `WebhookMountTable`, `WebhookReceiver::router`, `WEBHOOK_ROUTE_PREFIX`.
- Produces: `GatewayServer::set_webhook_mounts(&mut self, table: Arc<WebhookMountTable>)`. `RESERVED_ROUTE_PREFIXES` and `is_reserved_route` no longer exist.

- [ ] **Step 1: Write the failing tests**

Replace `webhook_routes_are_absent_until_set` and `set_webhook_routes_are_merged_into_build_router` in `server/mod.rs`'s `mod tests` with:

```rust
    #[tokio::test]
    async fn webhook_prefix_is_always_routed_and_404s_when_nothing_is_mounted() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        // The route is a constant, present with or without configured
        // channels. That is what lets a channel created at runtime become
        // reachable without a restart.
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = GatewayServer::new(addr);
        let router = server.build_router();

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/none")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 404 from the dispatcher — NOT 405 from the SPA fallback. A 405 here
        // would mean the wildcard route is missing and the request fell through.
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn set_webhook_mounts_makes_a_mounted_path_reachable() {
        use crate::gateway::channel::{ChannelId, ChannelState, ChannelStatus};
        use crate::gateway::webhook_receiver::{WebhookMountTable, WebhookReceiver};
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = ChannelState::new(8);
        state.set_status(ChannelStatus::Connected).await;
        let _rx = state.inbound_subscribe();

        let table = Arc::new(WebhookMountTable::new());
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut server = GatewayServer::new(addr);
        server.set_webhook_mounts(Arc::clone(&table));
        let router = server.build_router();

        // Mounted AFTER build_router: the router holds the table, not a snapshot.
        table
            .mount(crate::gateway::webhook_receiver::WebhookMount {
                handler: Arc::new(AlwaysOkHandler),
                inbound: state.sender(),
                status: state.status_handle(),
                channel_id: ChannelId::new("probe"),
            })
            .await;

        let body = br#"{"text":"hi"}"#.to_vec();
        let sig = WebhookReceiver::compute_signature("probe-secret", &body);
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/probe")
                    .header("x-webhook-signature", sig)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn panel_spa_paths_are_untouched_by_the_webhook_route() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        // Confining webhooks to one prefix is what makes SPA shadowing
        // unexpressible: `path = "/settings"` can no longer become a real
        // POST-only route that turns `GET /settings` into 405.
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = GatewayServer::new(addr);
        let router = server.build_router();

        for path in ["/", "/settings"] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{path} must still reach the Panel fallback"
            );
        }
    }

    #[test]
    fn build_router_registers_no_second_route_under_the_webhook_prefix() {
        // matchit lets `/webhook/foo` coexist with `/webhook/{*rest}` — the more
        // specific static route simply wins, with NO panic. So a future gateway
        // route under this prefix would silently steal a channel's webhook path.
        // axum cannot be asked what is in its route table, so scan the source of
        // the only function that builds it.
        let src = include_str!("mod.rs");
        for (idx, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            let offends = code.contains(".route(\"/webhook")
                || code.contains(".nest(\"/webhook")
                || code.contains(".nest_service(\"/webhook");
            assert!(
                !offends,
                "server/mod.rs:{} registers a route under {}; channel webhooks \
                 must enter only through WebhookReceiver::router()",
                idx + 1,
                crate::gateway::webhook_receiver::WEBHOOK_ROUTE_PREFIX
            );
        }
    }
```

Add the test-only handler next to the other test helpers in that module:

```rust
    /// Minimal `WebhookHandler` for router-level assertions: fixed secret,
    /// fixed path, produces no inbound messages so no subscriber is needed.
    struct AlwaysOkHandler;

    #[async_trait::async_trait]
    impl crate::gateway::webhook_receiver::WebhookHandler for AlwaysOkHandler {
        fn verify(&self, headers: &axum::http::HeaderMap, body: &[u8]) -> bool {
            let sig = headers
                .get("x-webhook-signature")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            crate::gateway::webhook_receiver::WebhookReceiver::verify_signature(
                "probe-secret",
                body,
                sig,
            )
        }

        async fn handle(
            &self,
            _headers: &axum::http::HeaderMap,
            _body: axum::body::Bytes,
        ) -> crate::gateway::channel::ChannelResult<Vec<crate::gateway::channel::InboundMessage>> {
            Ok(vec![])
        }

        fn path(&self) -> &str {
            "/webhook/probe"
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::server`
Expected: FAIL to compile — `set_webhook_mounts` does not exist; `webhook_routes` field type mismatch.

- [ ] **Step 3: Swap the field, merge unconditionally, delete the list**

In `src/gateway/server/mod.rs`:

Field (`:366-369`) becomes:

```rust
    /// Live channel webhook mount table, shared with `ChannelRegistry`.
    ///
    /// `build_router()` always registers the one wildcard route over this
    /// table, so the route table does not depend on configuration — that is
    /// what lets a channel started or created after `serve()` become
    /// reachable without a restart. An empty table 404s every webhook path.
    webhook_mounts: Arc<crate::gateway::webhook_receiver::WebhookMountTable>,
```

Both constructors: replace `webhook_routes: None,` (`:461` and `:513`) with

```rust
            webhook_mounts: Arc::new(crate::gateway::webhook_receiver::WebhookMountTable::new()),
```

Setter (`:561-565`) becomes:

```rust
    /// Serve channel webhook ingestion from `table`.
    ///
    /// Idempotent. Call order does not matter: the table is shared state, not
    /// a snapshot, so mounts added before or after this call are both served.
    pub fn set_webhook_mounts(
        &mut self,
        table: Arc<crate::gateway::webhook_receiver::WebhookMountTable>,
    ) {
        self.webhook_mounts = table;
    }
```

Merge site (`:732-737`) becomes:

```rust
        // Channel webhook ingestion. One constant route over the shared mount
        // table; auth is per-handler HMAC, the same posture as /metrics and
        // /a2a — see the design spec.
        router = router.merge(crate::gateway::webhook_receiver::WebhookReceiver::router(
            self.webhook_mounts.clone(),
        ));
```

Delete `RESERVED_ROUTE_PREFIXES` (`:393-407`) and `is_reserved_route` (`:409-417`) outright. Operator paths no longer enter the route table (Task 1's prefix rule), so the list has no consumer, and R10 says a zero-consumer abstraction is withdrawn rather than kept for later.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::server`
Expected: PASS. Then `cargo check -p alephcore` — the only remaining errors must be in `channel_registry.rs`/`subsystems.rs` (Tasks 4 and 6). If anything else references `is_reserved_route`, grep and fix: `grep -rn "is_reserved_route\|RESERVED_ROUTE_PREFIXES" --include="*.rs" .`

- [ ] **Step 5: Commit**

```bash
git add src/gateway/server/mod.rs
git commit -m "gateway: serve webhooks from the shared table and retire RESERVED_ROUTE_PREFIXES"
```

---

### Task 4: `ChannelRegistry` becomes the single throat

**Files:**
- Modify: `src/gateway/channel_registry.rs:121-141` (field), `:143-157` (`new`), `:193-215` (`create_channel`), `:217-224` (`register`), `:226-245` (`unregister`), `:296-311` (`start_channel`), `:313-324` (`stop_channel`), `:749-760` (`restart_channel`)
- Test: same file's `mod tests`

**Interfaces:**
- Consumes: `WebhookMountTable`, `WebhookMount`.
- Produces: `ChannelRegistry::webhook_mounts(&self) -> Arc<WebhookMountTable>` — Task 6 hands this to the server.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `channel_registry.rs`. Model the mock on the existing `FlakyChannel`.

```rust
    // --- Webhook mount follows channel lifecycle ---

    /// Channel that materialises a webhook handler in `start()` and drops it in
    /// `stop()` — the shape of `WebhookChannel`.
    struct WebhookyChannel {
        info: ChannelInfo,
        state: ChannelState,
        path: String,
        handler: Option<Arc<TestHandler>>,
    }

    struct TestHandler {
        path: String,
    }

    #[async_trait::async_trait]
    impl crate::gateway::webhook_receiver::WebhookHandler for TestHandler {
        fn verify(&self, _headers: &axum::http::HeaderMap, _body: &[u8]) -> bool {
            true
        }
        async fn handle(
            &self,
            _headers: &axum::http::HeaderMap,
            _body: axum::body::Bytes,
        ) -> ChannelResult<Vec<crate::gateway::channel::InboundMessage>> {
            Ok(vec![])
        }
        fn path(&self) -> &str {
            &self.path
        }
    }

    impl WebhookyChannel {
        fn new(id: &str, path: &str) -> Self {
            Self {
                info: ChannelInfo {
                    id: ChannelId::new(id),
                    name: id.to_string(),
                    channel_type: "test-webhook".to_string(),
                    status: ChannelStatus::Disconnected,
                    capabilities: ChannelCapabilities::default(),
                },
                state: ChannelState::new(8),
                path: path.to_string(),
                handler: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl Channel for WebhookyChannel {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }
        fn state(&self) -> &ChannelState {
            &self.state
        }
        async fn start(&mut self) -> ChannelResult<()> {
            self.handler = Some(Arc::new(TestHandler {
                path: self.path.clone(),
            }));
            self.state.set_status(ChannelStatus::Connected).await;
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            self.handler = None;
            self.state.set_status(ChannelStatus::Disconnected).await;
            Ok(())
        }
        fn webhook_handler(
            &self,
        ) -> Option<Arc<dyn crate::gateway::webhook_receiver::WebhookHandler>> {
            self.handler
                .clone()
                .map(|h| h as Arc<dyn crate::gateway::webhook_receiver::WebhookHandler>)
        }
        async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
            Ok(SendResult {
                message_id: MessageId::new("ok"),
                timestamp: Utc::now(),
            })
        }
    }

    #[tokio::test]
    async fn start_channel_mounts_the_webhook() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;

        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        registry.start_channel(&id).await.unwrap();
        assert_eq!(registry.webhook_mounts().mounted_count().await, 1);
    }

    #[tokio::test]
    async fn stop_channel_unmounts_the_webhook() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        registry.start_channel(&id).await.unwrap();

        registry.stop_channel(&id).await.unwrap();
        assert_eq!(
            registry.webhook_mounts().mounted_count().await,
            0,
            "a stopped channel must not keep an authenticated HTTP endpoint"
        );
    }

    #[tokio::test]
    async fn unregister_unmounts_the_webhook() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        registry.start_channel(&id).await.unwrap();

        registry.unregister(&id).await;
        assert_eq!(
            registry.webhook_mounts().mounted_count().await,
            0,
            "channel.delete must remove the endpoint even when the Arc is still held"
        );
    }

    #[tokio::test]
    async fn restart_channel_refreshes_the_mount() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        registry.start_channel(&id).await.unwrap();
        let first = registry
            .webhook_mounts()
            .lookup("/webhook/wh")
            .await
            .expect("mounted");

        // restart_channel does NOT go through stop_channel/start_channel, so it
        // needs its own hook — otherwise the table keeps the pre-restart
        // handler clone forever.
        registry.restart_channel(&id).await.unwrap();
        let second = registry
            .webhook_mounts()
            .lookup("/webhook/wh")
            .await
            .expect("still mounted");

        assert!(
            !Arc::ptr_eq(&first.handler, &second.handler),
            "the table must hold the handler built by the restart"
        );
    }

    #[tokio::test]
    async fn re_registering_drops_the_outgoing_instance_mount() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        registry.start_channel(&id).await.unwrap();

        // `channel.start` re-creates the instance from fresh config and
        // re-registers it. The replacement has not started, so it owns no
        // handler — the old mount must not keep serving with the old secret.
        registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
    }
```

`lookup` is `pub(crate)`, so these tests can call it from inside the crate. If the compiler objects to `MountedHandler`'s private fields, mark `handler` `pub(crate)` (Task 1 already does).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib channel_registry`
Expected: FAIL to compile — `ChannelRegistry::webhook_mounts` does not exist.

- [ ] **Step 3: Add the field and the six hooks**

Add to the `ChannelRegistry` struct (after `delivery_store`, `:140`):

```rust
    /// Live webhook mount table. The registry is the single writer: mounting
    /// follows channel lifecycle instead of being a boot-time snapshot, so
    /// `stop` / `delete` / runtime `create` change what HTTP actually serves.
    /// Handed to `GatewayServer::set_webhook_mounts` at boot.
    webhook_mounts: Arc<super::webhook_receiver::WebhookMountTable>,
```

In `new()` (`:157`), add:

```rust
            webhook_mounts: Arc::new(super::webhook_receiver::WebhookMountTable::new()),
```

Add the accessor right after `new()` / the builder methods:

```rust
    /// The live webhook mount table (shared handle).
    #[must_use]
    pub fn webhook_mounts(&self) -> Arc<super::webhook_receiver::WebhookMountTable> {
        Arc::clone(&self.webhook_mounts)
    }
```

`start_channel` (`:297-311`) — build the mount while the write guard is held, then release it before touching the table:

```rust
    pub async fn start_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        let mut channel = channel_arc.write().await;
        channel.start().await?;

        // A webhook channel only materialises its handler in `start()`, so this
        // is the earliest point the mount exists. The sink is the channel's OWN
        // broadcast so `start_message_forwarder` still stamps channel health —
        // going to the registry's sender directly would make a receiving channel
        // look dead to the health monitor.
        let mount = channel.webhook_handler().map(|handler| {
            super::webhook_receiver::WebhookMount {
                handler,
                inbound: channel.state().sender(),
                status: channel.state().status_handle(),
                channel_id: channel_id.clone(),
            }
        });

        // Start forwarding inbound messages
        self.start_message_forwarder(channel_id.clone(), channel_arc.clone())
            .await;
        drop(channel);

        if let Some(mount) = mount {
            self.webhook_mounts.mount(mount).await;
        }

        info!("Started channel: {}", channel_id);
        Ok(())
    }
```

`stop_channel` (`:314-324`):

```rust
    pub async fn stop_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        let mut channel = channel_arc.write().await;
        channel.stop().await?;
        drop(channel);

        // The route holds its own handler clone, so dropping the channel's copy
        // is not enough — without this the endpoint keeps answering 200 and
        // driving agent runs after `channel.stop` reported "stopped".
        self.webhook_mounts.unmount_channel(channel_id).await;

        info!("Stopped channel: {}", channel_id);
        Ok(())
    }
```

`restart_channel` (`:749-760`):

```rust
    pub async fn restart_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        let mut channel = channel_arc.write().await;
        channel.stop().await?;
        channel.start().await?;

        // This path does NOT go through stop_channel/start_channel, so it needs
        // its own refresh: `start()` builds a NEW handler and the table would
        // otherwise keep serving the pre-restart clone.
        let mount = channel.webhook_handler().map(|handler| {
            super::webhook_receiver::WebhookMount {
                handler,
                inbound: channel.state().sender(),
                status: channel.state().status_handle(),
                channel_id: channel_id.clone(),
            }
        });
        drop(channel);

        match mount {
            Some(mount) => {
                self.webhook_mounts.mount(mount).await;
            }
            None => {
                self.webhook_mounts.unmount_channel(channel_id).await;
            }
        }

        info!("Restarted channel in place: {}", channel_id);
        Ok(())
    }
```

`register` (`:218-224`) — unmount before the replacement lands:

```rust
    pub async fn register(&self, channel: Box<dyn Channel>) -> ChannelId {
        let channel_id = channel.id().clone();

        // A replacement instance has not started, so it owns no handler. Drop
        // whatever the outgoing instance left mounted, or the route keeps
        // serving with the old secret until someone happens to start the new one.
        self.webhook_mounts.unmount_channel(&channel_id).await;

        let mut channels = self.channels.write().await;
        channels.insert(channel_id.clone(), Arc::new(RwLock::new(channel)));
        info!("Registered channel: {}", channel_id);
        channel_id
    }
```

`create_channel` (`:206-208`) — same, right before the insert:

```rust
        self.webhook_mounts.unmount_channel(&channel_id).await;

        let mut channels = self.channels.write().await;
        channels.insert(channel_id.clone(), Arc::new(RwLock::new(channel)));
```

`unregister` (`:227`) — first line of the function, before the channels lock:

```rust
    pub async fn unregister(&self, channel_id: &ChannelId) -> Option<Box<dyn Channel>> {
        // Drop the HTTP surface before the instance leaves the registry —
        // including when `Arc::try_unwrap` below fails and this returns None.
        // `channel.delete` otherwise leaves an authenticated endpoint the
        // operator believes is gone.
        self.webhook_mounts.unmount_channel(channel_id).await;

        let mut channels = self.channels.write().await;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib channel_registry`
Expected: PASS, including the pre-existing `FlakyChannel` retry tests.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/channel_registry.rs
git commit -m "gateway: make the channel registry the single writer of webhook mounts"
```

---

### Task 5: `validate()` enforces the shared prefix

**Files:**
- Modify: `src/gateway/interfaces/webhook/config.rs:57-68` (`validate`), and the doc comment on `path` (`:23-25`) plus the module usage example (`:24`)
- Test: same file's `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `WEBHOOK_ROUTE_PREFIX` from `crate::gateway::webhook_receiver`.
- Produces: nothing new. `validate()` gains one rejection case.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `src/gateway/interfaces/webhook/config.rs`:

```rust
    #[test]
    fn validate_requires_the_shared_webhook_prefix() {
        // A path outside the prefix cannot be reached behind the single
        // `/webhook/{*rest}` route. Failing `start()` here is the honest
        // outcome: accepting it would give back a channel that reports
        // Connected and is deaf — the exact shape this work removes.
        let base = WebhookChannelConfig {
            secret: "s".to_string(),
            callback_url: "http://127.0.0.1:1/cb".to_string(),
            path: String::new(),
            allowed_senders: vec![],
        };

        for bad in ["/settings", "/", "webhook/generic", "/webhook", "/webhook/"] {
            let cfg = WebhookChannelConfig {
                path: bad.to_string(),
                ..base.clone()
            };
            let err = cfg.validate().expect_err(&format!("{bad} must be rejected"));
            assert!(
                err.contains("/webhook/"),
                "the error must name the required prefix, got: {err}"
            );
        }

        // The default and any sub-path under the prefix are fine.
        assert!(WebhookChannelConfig {
            path: "/webhook/generic".to_string(),
            ..base.clone()
        }
        .validate()
        .is_ok());
        assert!(WebhookChannelConfig {
            path: "/webhook/team/alerts".to_string(),
            ..base
        }
        .validate()
        .is_ok());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alephcore --lib interfaces::webhook::config`
Expected: FAIL — `/settings` currently validates fine (only `starts_with('/')` is checked).

- [ ] **Step 3: Tighten `validate()`**

Replace the path check in `validate()`:

```rust
        // All channel webhook traffic enters through one constant route,
        // `{WEBHOOK_ROUTE_PREFIX}/{{*rest}}`, so a path outside that prefix is
        // unreachable. Reject it here rather than warn at mount time: a channel
        // that starts, reports Connected, and cannot receive is the failure
        // shape this whole subsystem was rewired to remove.
        if !crate::gateway::webhook_receiver::is_mountable_path(&self.path) {
            return Err(format!(
                "path must be \"{}/<name>\" (got {:?})",
                crate::gateway::webhook_receiver::WEBHOOK_ROUTE_PREFIX,
                self.path
            ));
        }
```

Task 1 declared `is_mountable_path` private. Promote it to `pub(crate)` in `webhook_receiver.rs` so `validate()` and `mount()` share one predicate — two copies of this rule is exactly the drift this codebase keeps getting bitten by:

```rust
pub(crate) fn is_mountable_path(path: &str) -> bool {
```

Update the `path` field doc and the module-level TOML example so the constraint is documented where an operator reads it:

```rust
    /// URL path to receive inbound webhooks on. Must be under `/webhook/`
    /// (default: `/webhook/generic`) — that prefix is the single route all
    /// channel webhook traffic enters through.
    #[serde(default = "default_path")]
    pub path: String,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p alephcore --lib interfaces::webhook`
Expected: PASS. Existing config tests that assert `validate()` accepts some other path must be updated to a `/webhook/…` path — grep the module for `path:` in tests.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/webhook/config.rs src/gateway/webhook_receiver.rs
git commit -m "webhook: require the shared /webhook/ prefix in channel config validation"
```

---

### Task 6: Boot wiring, and honest `channel.start` / `channel.create` receipts

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs:516-559`
- Modify: `src/gateway/handlers/channel.rs:353-375` (delete `needs_webhook_restart`), `:442-468` (`handle_start`), `:729-755` (`handle_create`)

**Interfaces:**
- Consumes: `ChannelRegistry::webhook_mounts()` (Task 4), `GatewayServer::set_webhook_mounts` (Task 3).
- Produces: nothing new.

- [ ] **Step 1: Replace the boot collection block**

In `subsystems.rs`, replace the whole `{ use alephcore::gateway::{WebhookMount, WebhookReceiver}; … }` block (`:516-559`) with:

```rust
    // Hand the registry's live webhook mount table to the HTTP surface.
    //
    // The registry is the single writer (`start_channel` / `stop_channel` /
    // `restart_channel` / `register` / `create_channel` / `unregister`), so
    // this is a one-time handoff of a shared handle — not a snapshot. Order
    // does not matter: mounts made by the `start_all` above are already in the
    // table, and anything started later through `channel.start` lands in the
    // same table the router is reading.
    {
        let mounts = channel_registry.webhook_mounts();
        let count = mounts.mounted_count().await;
        server.set_webhook_mounts(mounts);
        if !daemon && count > 0 {
            println!("  Gateway: {count} webhook ingestion route(s) mounted");
        }
    }
```

- [ ] **Step 2: Delete the restart-required branches**

In `src/gateway/handlers/channel.rs`, delete `needs_webhook_restart` entirely (`:353-375`, including the long doc comment above it — the condition it documented no longer exists).

In `handle_start`, replace the `Ok(())` arm (`:443-462`) with:

```rust
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "channel_id": channel_id.as_str(),
                "status": "started",
            }),
        ),
```

In `handle_create`, replace the `Ok(())` arm (`:734-761`) with:

```rust
            Ok(()) => JsonRpcResponse::success(
                request.id,
                json!({
                    "id": id,
                    "type": channel_type,
                    "status": "started",
                }),
            ),
```

- [ ] **Step 3: Verify the whole crate compiles and the suite is green**

Run: `cargo check -p alephcore && cargo check -p alephcore --bin aleph-server`
Expected: clean.

Run: `cargo test -p alephcore --lib gateway`
Expected: PASS.

If any test asserted `"restart_required"`, delete that assertion — grep: `grep -rn "restart_required" --include="*.rs" .`

- [ ] **Step 4: Full suite + lints**

Run: `cargo test -p alephcore --lib`
Expected: PASS (baseline is 0 failures; if something unrelated is already red, say so explicitly rather than attributing it to this change).

Run: `cargo fmt` then `cargo clippy --all-targets -- -D warnings`
Expected: clean. Note `-D warnings` propagates to path dependencies and surfaces one crate at a time — do not truncate the output.

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/subsystems.rs src/gateway/handlers/channel.rs
git commit -m "gateway: wire the live webhook table at boot and drop the restart-required receipts"
```

---

### Task 7: Documentation

**Files:**
- Modify: `docs/reference/GATEWAY.md:609-637`

- [ ] **Step 1: Rewrite the "Channel webhook ingestion" section**

Replace lines 609-637 with:

```markdown
### Channel webhook ingestion

Channels that receive over HTTP POST (`generic webhook`, and future ones)
return a handler from `Channel::webhook_handler()`. `build_router()` registers
**one constant route** — `POST /webhook/{*rest}` — whose state is the shared
`WebhookMountTable`. `ChannelRegistry` owns that table and is its only writer.

- **Mounting follows the registry, not boot.** `start_channel` /
  `restart_channel` mount; `stop_channel` / `unregister` / `register` /
  `create_channel` unmount. So `channel.stop` and `channel.delete` really do
  remove the endpoint (404, not 503), and a channel created at runtime is
  reachable without restarting the daemon. The earlier version built the route
  table once at boot: `stop` returned `"stopped"` while the endpoint kept
  answering 200 and driving agent runs, because the route held its own
  `Arc<Handler>` clone and its own `broadcast::Sender` clone — the forwarder's
  only exit condition (`RecvError::Closed`) could never fire.
- **⚠️ `restart_channel` does not go through `stop_channel`/`start_channel`.**
  It calls `channel.stop()` + `channel.start()` directly, so it carries its own
  mount refresh. A hook set that only covers start/stop leaves the pre-restart
  handler clone in the table forever.
- **`path` must be under `/webhook/`**, enforced by
  `WebhookChannelConfig::validate()` and again by `WebhookMountTable::mount()`
  (one predicate, `is_mountable_path`). Because operator-writable paths never
  enter axum's route table, a bad path can no longer panic `Router::merge` at
  boot, and can no longer shadow a Panel SPA path (`path = "/settings"` used to
  turn `GET /settings` into 405). `RESERVED_ROUTE_PREFIXES` existed only to
  guard that boot panic and was withdrawn with it.
- **⚠️ matchit does not panic on `/webhook/foo` next to `/webhook/{*rest}`** —
  the more specific static route just wins. A future gateway route under this
  prefix would therefore *silently* steal a channel's webhook path. The guard
  is a source scan in `server/mod.rs`'s own tests
  (`build_router_registers_no_second_route_under_the_webhook_prefix`); axum
  cannot be asked what is in its route table.
- **Two channels, one path** → the lower `channel_id` keeps the route, warned
  with both ids. Deterministic on purpose: `start_all` iterates a HashMap, so
  arrival order would make route ownership a per-boot coin flip. The loser is
  only warned — it still reports `Connected` in `channels.list` while being
  deaf. Recorded limit, not a fix.
- **One port.** Webhook traffic rides the gateway's own listener, so it
  inherits `[gateway] host`, TLS, and `SecurityHeadersLayer`. `WebhookReceiver`
  deliberately owns no listener — the version that bound `0.0.0.0` itself would
  have opened a LAN surface regardless of the configured host.
- **Auth is per-handler HMAC**, not the login wall — an external platform
  cannot present a device token. Same posture as `/health`, `/metrics`, `/a2a`:
  no transport-level auth, no rate limiter (that lives in `MiddlewareChain`,
  on the JSON-RPC/WS path only). The signature also binds no timestamp or
  nonce (unlike Stripe/GitHub's `t=…,v1=…`), so replay protection is
  incidental — it comes only from inbound dedup at
  `src/gateway/inbound_router/dedup.rs`, whose window is **5 minutes**; a
  captured signed request replayed after that re-triggers an agent run. This
  is posture, not a known gap requiring action.
- **Check order is deliberate**: lookup → 404, signature → 403, channel status
  → 503, then parse and forward. Signature comes *before* status so an
  unauthenticated caller cannot tell "channel is down" from "wrong secret".
  The status check is depth only, for a channel that moved itself to
  `Error`/`Connecting` without any RPC; `try_read` fails **open** on
  contention, because a momentary write-lock holder is not evidence the
  channel is down.
- ⚠️ The sink is the channel's **own** `ChannelState::sender()`, not the
  registry's. Going direct to the registry bypasses
  `start_message_forwarder`, the only place inbound traffic stamps
  `health.record_event()` — the channel would receive while health monitoring
  reported it dead.
```

- [ ] **Step 2: Commit**

```bash
git add docs/reference/GATEWAY.md
git commit -m "docs: describe the dynamic webhook mount table in GATEWAY.md"
```

---

### Task 8: Real-machine verification

The unit tests prove the table's semantics; only a running daemon proves that
`channel.create` at runtime is reachable **without a restart** — the claim the
previous round could not make. Reuse the QA recipe from
[the previous round's spec §9.4](../specs/2026-07-29-webhook-inbound-wiring-design.md).

- [ ] **Step 1: Build and prepare the QA config**

```bash
CARGO_TARGET_DIR=/tmp/aleph-webhook-qa cargo build --bin aleph-server
cp ~/.aleph/config.toml /tmp/aleph-webhook-qa/aleph_qa.toml
```

Edit `/tmp/aleph-webhook-qa/aleph_qa.toml`: delete `[channels.AlephzBot]` (a QA
daemon must not pull the user's real Telegram bot online), keep the provider
section, and add:

```toml
[channels.webhook]
type = "webhook"
secret = "qa-secret"
callback_url = "http://127.0.0.1:8788"
path = "/webhook/qa"
```

The section name must be `webhook` — `WebhookChannelFactory::create` hardcodes
that id, and per-channel policy is registered under the *config section* name.
The daemon rewrites the file it is given (moving `secret` into the vault), so
re-add `secret` if you rename the section.

- [ ] **Step 2: Start the daemon and confirm the boot mount**

```bash
/tmp/aleph-webhook-qa/debug/aleph-server --config /tmp/aleph-webhook-qa/aleph_qa.toml start
```

Expected in the log: `Gateway: 1 webhook ingestion route(s) mounted`.
Note `--config` is a global flag and must come **before** the subcommand.

- [ ] **Step 3: Prove `stop` removes the endpoint**

```bash
# Before: signed POST is accepted (200 "ok"). Sign with:
#   printf '%s' "$BODY" | openssl dgst -sha256 -hmac qa-secret -hex
curl -si -X POST http://127.0.0.1:8787/webhook/qa \
  -H "X-Webhook-Signature: sha256=$SIG" -d "$BODY" | head -1

aleph-server gateway call --url ws://127.0.0.1:8787/ws channel.stop -p '{"channel_id":"webhook"}'

# After: the same signed POST must be 404, not 503 and not 200.
curl -si -X POST http://127.0.0.1:8787/webhook/qa \
  -H "X-Webhook-Signature: sha256=$SIG" -d "$BODY" | head -1
```

Expected: `HTTP/1.1 200 OK` → `HTTP/1.1 404 Not Found`.

- [ ] **Step 4: Prove runtime start needs no restart**

```bash
aleph-server gateway call --url ws://127.0.0.1:8787/ws channel.start -p '{"channel_id":"webhook"}'
```

Expected: the response says `"status":"started"` (**not** `"restart_required"`),
and the same signed POST is `200 ok` again — with no daemon restart in between.

- [ ] **Step 5: Prove the oracle is closed**

```bash
aleph-server gateway call --url ws://127.0.0.1:8787/ws channel.stop -p '{"channel_id":"webhook"}'
# Wrong signature against a stopped channel: must be 404 (path gone), never 503.
curl -si -X POST http://127.0.0.1:8787/webhook/qa \
  -H "X-Webhook-Signature: sha256=deadbeef" -d "$BODY" | head -1
```

Expected: `404`. Then start the channel again and repeat with the bad
signature: expected `403` — identical to what a caller sees for a
`Connecting`/`Error` channel, so no state leaks.

- [ ] **Step 6: Record the results**

Append a `## QA 结果` section to
`docs/superpowers/specs/2026-07-30-webhook-dynamic-mount-design.md` with the
observed status lines, then commit:

```bash
git add docs/superpowers/specs/2026-07-30-webhook-dynamic-mount-design.md
git commit -m "docs: record the real-machine QA for the webhook dynamic mount table"
```

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| §4.1 `WebhookMountTable` + `WEBHOOK_ROUTE_PREFIX` + D5 tie-break | Task 1 |
| §4.2 dispatch, `uri.path()` key, lock released before await, D7 order | Task 2 |
| §4.3 registry as single throat, six hooks, §2.E `restart_channel` | Task 4 |
| §4.4 `validate()` prefix enforcement | Task 5 |
| §4.5 `GatewayServer` field/setter/merge, D6 deletion | Task 3 |
| §4.6 boot handoff + honest receipts | Task 6 |
| §4.7 both guards (validate + source scan) | Tasks 5 and 3 |
| §5 test table | Tasks 1–5 (every row mapped) |
| §6 operator-visible behavior changes | Task 7 (docs), Task 8 (verified) |
| §7 out-of-scope | not implemented, recorded in Task 7's docs (duplicate-path loser) |
| §8 risks | lock discipline in Task 1/2 comments; `restart_channel` in Task 4; grep for deleted symbols in Task 3 step 4 |

**Placeholder scan:** No TBD/TODO. Every code step carries real code. The one
place with a decision left to the implementer — Task 1 step 3's "if
`InboundMessageSender` is not `Clone`, derive it" — names the exact file and the
exact grep to settle it.

**Type consistency:** `WebhookMountTable::{new, mount, unmount_channel, mounted_count, lookup}`,
`MountedHandler::{handler, inbound, status}`, `is_mountable_path`,
`WEBHOOK_ROUTE_PREFIX`, `WebhookReceiver::router(Arc<WebhookMountTable>)`,
`GatewayServer::set_webhook_mounts`, `ChannelRegistry::webhook_mounts` — each is
declared once (Task 1/2/3/4) and used with the same name and signature
everywhere after. `mounted_count` is deliberately not `len` (an inherent `len`
without `is_empty` trips `clippy::len_without_is_empty`, and the name reads
better in the boot log).
