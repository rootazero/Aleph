# Webhook Inbound Wiring & /stop Receipt QA — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mount the generic webhook channel's inbound HTTP handler on the gateway's existing axum router so `WebhookChannel` can actually receive, then use that path to red/green the `/stop` queued-message receipt count on a real machine.

**Architecture:** `Channel` gains a `webhook_handler()` method defaulting to `None`. `WebhookReceiver` stops being an HTTP server (it hardcoded `bind(0.0.0.0)`, violating the trust model) and becomes a pure `Router` builder that pairs each handler with its owning channel's `InboundMessageSender`. `initialize_channels` collects the handlers after every channel has started and hands the resulting `Router` to `GatewayServer`, which merges it in `build_router()` exactly like `a2a_routes` / `artifact_routes` already do.

**Tech Stack:** Rust 1.96 (MSRV 1.95), tokio, axum 0.8-style routing (`{param}` captures), hmac/sha2, tracing.

**Spec:** `docs/superpowers/specs/2026-07-29-webhook-inbound-wiring-design.md`

## Global Constraints

- **R1 / R4:** Gateway and interface layers are pure I/O. No business logic added here.
- **P6 (YAGNI):** Anything that ends up with zero consumers after this change gets deleted in the same commit, not left "for later".
- Commit messages: `<scope>: <description>`, English, e.g. `gateway: mount webhook handlers on the shared router`.
- Formatting gate: `cargo fmt --all` must be clean before every commit. **Do NOT run bare `cargo fmt -- <file>`** in this repo — it reformats the whole tree. Use `cargo fmt --all` (whole-tree is the intended behavior) or `rustfmt --check` plus targeted `Edit`s.
- Lint gate: `cargo clippy -p alephcore --all-targets -- -D warnings` must pass. CI runs fmt **before** clippy, so a fmt failure hides clippy results.
- Every task ends with a commit. Never batch two tasks into one commit.
- `Arc` in this crate is `crate::sync_primitives::Arc`, a re-export of `std::sync::Arc` — unsize coercion to `Arc<dyn Trait>` works normally.

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `src/gateway/channel.rs` | `Channel` trait + `ChannelState` | Modify: add `webhook_handler()` default method (~line 738 trait body) |
| `src/gateway/webhook_receiver.rs` | Webhook HTTP ingestion: handler trait, HMAC helpers, router construction | Modify heavily: `WebhookReceiver` loses its listener, gains `router()`; new `WebhookMount`; `HandlerState` switches sink type |
| `src/gateway/server/mod.rs` | Gateway axum server: route table, `build_router()` | Modify: add `RESERVED_ROUTE_PREFIXES` + `is_reserved_route()` next to the route table; add `webhook_routes` field, `set_webhook_routes()`, merge in `build_router()` |
| `src/gateway/interfaces/webhook/mod.rs` | Generic webhook channel adapter | Modify: inherent `webhook_handler()` / `inbound_sender()` become the trait impl / get deleted |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs` | Boot wiring for channels | Modify: collect handlers at the end of `initialize_channels`, call `server.set_webhook_routes()` |
| `docs/reference/GATEWAY.md` | Gateway reference doc | Modify: document the webhook inbound surface (Task 5) |

No new files. The reserved-prefix list deliberately lives in `server/mod.rs` beside the routes it must not collide with, so the two cannot drift.

---

### Task 1: `Channel::webhook_handler()` — the capability question

**Files:**
- Modify: `src/gateway/channel.rs` (inside `pub trait Channel`, after `approval_capability()` at ~line 773)
- Modify: `src/gateway/interfaces/webhook/mod.rs:133-148` (replace the two inherent getters)
- Test: `src/gateway/interfaces/webhook/mod.rs` (`#[cfg(test)] mod tests` at the bottom of that file)

**Interfaces:**
- Consumes: nothing (first task).
- Produces: `Channel::webhook_handler(&self) -> Option<Arc<dyn WebhookHandler>>` — Task 5 calls this on every registered channel. `WebhookChannel` returns `Some` only after `start()`.

**Why a trait method rather than special-casing `WebhookChannel` in the boot wiring:** the registry stores `Box<dyn Channel>` (`ChannelHandle = Arc<RwLock<Box<dyn Channel>>>`), and without `Any` there is no downcast. The trait method is forced by the type system — and it happens to be the "declare it and it's wired" shape, so the next webhook-based channel (msteams) only has to override one method.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of `src/gateway/interfaces/webhook/mod.rs`:

```rust
#[tokio::test]
async fn webhook_handler_is_none_before_start_and_some_after() {
    let config = WebhookChannelConfig {
        secret: "test-secret".to_string(),
        callback_url: "http://127.0.0.1:9/sink".to_string(),
        path: "/webhook/generic".to_string(),
        allowed_senders: Vec::new(),
    };
    let mut channel = WebhookChannel::new("wh1", config);

    // Before start there is no handler to mount.
    assert!(
        crate::gateway::channel::Channel::webhook_handler(&channel).is_none(),
        "handler must not exist before start()"
    );

    channel.start().await.expect("start");

    let handler = crate::gateway::channel::Channel::webhook_handler(&channel)
        .expect("handler must exist after start()");
    assert_eq!(handler.path(), "/webhook/generic");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib webhook_handler_is_none_before_start -- --nocapture`
Expected: FAIL to compile — `no method named 'webhook_handler' found for reference '&WebhookChannel' in the current scope` (the trait method does not exist yet; the inherent method has a different return type and is not reachable through the trait path used in the test).

- [ ] **Step 3: Add the trait method**

In `src/gateway/channel.rs`, add the import near the other `super::` imports at the top of the file:

```rust
use super::webhook_receiver::WebhookHandler;
```

(A module cycle between `channel.rs` and `webhook_receiver.rs` is fine — cycles are only forbidden between crates, not modules.)

Then inside `pub trait Channel`, immediately after the `approval_capability()` method:

```rust
    /// Webhook ingestion handler for channels that receive over HTTP POST.
    ///
    /// A channel returning `Some` gets its `path()` mounted on the gateway's
    /// shared axum router by `initialize_channels`; `None` (the default) means
    /// the channel receives some other way — a poll loop, a socket, a bridge.
    ///
    /// Returning `Some` only after `start()` is expected: collection runs once,
    /// after every channel has started.
    fn webhook_handler(&self) -> Option<Arc<dyn WebhookHandler>> {
        None
    }
```

- [ ] **Step 4: Implement it on `WebhookChannel`**

In `src/gateway/interfaces/webhook/mod.rs`, **delete** both inherent methods at lines 133-148 (`inbound_sender()` and `webhook_handler()`) — the trait supplies `state().sender()` for the first and the method below for the second, so both would be zero-consumer duplicates.

Add to the `impl Channel for WebhookChannel` block (after `stop()`):

```rust
    fn webhook_handler(&self) -> Option<Arc<dyn WebhookHandler>> {
        self.handler
            .clone()
            .map(|h| h as Arc<dyn WebhookHandler>)
    }
```

Ensure `WebhookHandler` is in scope — the file already has
`use crate::gateway::webhook_receiver::{WebhookHandler, WebhookReceiver};` at line 39.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p alephcore --lib webhook_handler_is_none_before_start -- --nocapture`
Expected: PASS

- [ ] **Step 6: Verify nothing else referenced the deleted getters**

Run: `grep -rn "inbound_sender()" --include="*.rs" src/ | grep -v channel_registry`
Expected: no hits pointing at `WebhookChannel`. (`ChannelRegistry::inbound_sender()` is a different, live method — leave it.)

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add src/gateway/channel.rs src/gateway/interfaces/webhook/mod.rs
git commit -m "gateway: ask every channel whether it ingests over webhook

Channels are stored as Box<dyn Channel>, so there is no downcast to
WebhookChannel from the boot wiring. A default-None trait method is the
only way to collect handlers uniformly — and it means the next
webhook-based channel only has to override one method to be wired.

Drops the two inherent getters it replaces; both had zero consumers."
```

---

### Task 2: `WebhookReceiver` — from HTTP server to `Router` builder

**Files:**
- Modify: `src/gateway/webhook_receiver.rs:85-256` (struct, `start`/`stop`, `HandlerState`, `webhook_endpoint`)
- Test: `src/gateway/webhook_receiver.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Channel::webhook_handler()` from Task 1 (indirectly — callers pass the `Arc<dyn WebhookHandler>` it returns).
- Produces:
  - `pub struct WebhookMount { pub handler: Arc<dyn WebhookHandler>, pub inbound: InboundMessageSender }`
  - `pub fn WebhookReceiver::router(mounts: Vec<WebhookMount>) -> axum::Router`
  - `WebhookReceiver::compute_signature` / `verify_signature` unchanged (Task 6's QA script and `interfaces/webhook/message_ops.rs` both use them).

**Why the sink type changes:** the old `start()` took `mpsc::Sender<InboundMessage>`, but channels publish onto `ChannelState.inbound_broadcast` (`InboundMessageSender`, `channel.rs:642`). Feeding an mpsc would bypass `ChannelRegistry::start_message_forwarder` (`channel_registry.rs:587`) — the one place that stamps `health.record_event()` on each inbound message, which `ChannelHealthMonitor::is_stale` reads. The channel would receive messages while health monitoring reported it dead.

- [ ] **Step 1: Write the failing test**

Replace nothing; **add** to `#[cfg(test)] mod tests` in `src/gateway/webhook_receiver.rs`:

```rust
#[tokio::test]
async fn signed_post_reaches_the_channel_broadcast() {
    use crate::gateway::channel::ChannelState;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let secret = "router-secret";
    let state = ChannelState::new(16);
    // Subscribe FIRST: InboundMessageSender::send returns Err when there are
    // no subscribers (broadcast semantics), and in production the subscriber
    // is ChannelRegistry::start_message_forwarder.
    let mut rx = state.inbound_subscribe();

    let handler = Arc::new(MockWebhookHandler {
        secret: secret.to_string(),
        path: "/webhook/mock".to_string(),
    });

    let app = WebhookReceiver::router(vec![WebhookMount {
        handler,
        inbound: state.sender(),
    }]);

    let body = br#"{"text":"hello from webhook"}"#.to_vec();
    let sig = WebhookReceiver::compute_signature(secret, &body);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/mock")
                .header("x-webhook-signature", sig)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let msg = rx.try_recv().expect("message must reach the channel broadcast");
    assert_eq!(msg.text, "hello from webhook");
}

#[tokio::test]
async fn unsigned_post_is_rejected_and_publishes_nothing() {
    use crate::gateway::channel::ChannelState;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let state = ChannelState::new(16);
    let mut rx = state.inbound_subscribe();

    let handler = Arc::new(MockWebhookHandler {
        secret: "router-secret".to_string(),
        path: "/webhook/mock".to_string(),
    });

    let app = WebhookReceiver::router(vec![WebhookMount {
        handler,
        inbound: state.sender(),
    }]);

    let response = app
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
```

The existing `MockWebhookHandler` in that test module (declared around line 373) already implements `verify` via `WebhookReceiver::verify_signature`. Inspect it first and adjust the struct literal above to its real field names — if it has no `secret`/`path` fields, add them plus a `handle()` that parses `{"text": ...}` into a single `InboundMessage`. Its `handle()` must produce `msg.text` equal to the JSON `text` field for these assertions to mean anything.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib webhook_receiver -- --nocapture`
Expected: FAIL to compile — `cannot find struct 'WebhookMount'` and `no function or associated item named 'router' found for struct 'WebhookReceiver'`.

- [ ] **Step 3: Rewrite the receiver**

In `src/gateway/webhook_receiver.rs`:

Update the imports — drop `tokio::sync::{mpsc, watch}`, add the broadcast-backed sender:

```rust
use super::channel::{ChannelResult, InboundMessage, InboundMessageSender};
```

(`InboundMessage` stays in use for `WebhookHandler::handle`'s return type.)

Replace the `WebhookReceiver` struct and its `new` / `start` / `stop` (lines 85-175) with:

```rust
/// One webhook handler mounted at its own path, paired with the inbound sink
/// of the channel that owns it.
///
/// The sink is the channel's own broadcast (`ChannelState::sender()`), not the
/// registry's — going direct would bypass `start_message_forwarder`, the only
/// place inbound traffic stamps channel health.
pub struct WebhookMount {
    pub handler: Arc<dyn WebhookHandler>,
    pub inbound: InboundMessageSender,
}

/// Builds the axum routes for channel webhook ingestion.
///
/// This does **not** own a listener. The gateway's own server merges these
/// routes into `build_router()`, so webhook traffic inherits the configured
/// bind address, TLS, and security headers. The previous version bound
/// `0.0.0.0` itself, which silently opened a LAN port regardless of
/// `[gateway] host`.
pub struct WebhookReceiver;

impl WebhookReceiver {
    /// Build the router for the given mounts.
    ///
    /// A mount whose path collides with a gateway route, or with an earlier
    /// mount, is skipped with a warning — `Router::merge` panics on duplicate
    /// routes and `path` is an operator-writable config field, so a typo must
    /// not take the daemon down at boot.
    #[must_use]
    pub fn router(mounts: Vec<WebhookMount>) -> Router {
        let mut router = Router::new();
        let mut mounted: Vec<String> = Vec::new();

        for mount in mounts {
            let path = mount.handler.path().to_string();

            if crate::gateway::server::is_reserved_route(&path) {
                warn!(
                    path = %path,
                    "webhook path collides with a gateway route — handler not mounted"
                );
                continue;
            }
            if mounted.iter().any(|p| p == &path) {
                warn!(path = %path, "duplicate webhook path — handler not mounted");
                continue;
            }

            let handler_state = Arc::new(HandlerState {
                handler: mount.handler,
                inbound: mount.inbound,
            });
            router = router.route(&path, post(webhook_endpoint).with_state(handler_state));
            info!(path = %path, "Registered webhook handler");
            mounted.push(path);
        }

        router
    }

    // compute_signature / verify_signature stay exactly as they are.
}
```

Update `HandlerState` (line 202) and the send site in `webhook_endpoint` (line 227):

```rust
struct HandlerState {
    handler: Arc<dyn WebhookHandler>,
    inbound: InboundMessageSender,
}
```

```rust
            let mut dropped = 0usize;
            for msg in messages {
                if state.inbound.send(msg).is_err() {
                    dropped += 1;
                    warn!(
                        path = %state.handler.path(),
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
```

Note `InboundMessageSender::send` is synchronous and returns `Result<(), InboundMessage>` — drop the `.await`.

- [ ] **Step 4: Update the re-export and any stale callers**

`src/gateway/mod.rs:187` currently reads
`pub use webhook_receiver::{WebhookHandler, WebhookReceiver};` — extend it:

```rust
pub use webhook_receiver::{WebhookHandler, WebhookMount, WebhookReceiver};
```

Run: `cargo check -p alephcore 2>&1 | grep -E "^error" | head -20`
Fix any remaining callers of the deleted `new`/`start`/`stop`. (Expect only the old in-file tests; the production tree had zero construction sites — that is the bug this round exists to fix. `is_reserved_route` does not exist yet, so one error about it is expected and Task 3 fixes it.)

- [ ] **Step 5: Run the tests**

Run: `cargo test -p alephcore --lib webhook_receiver -- --nocapture`
Expected: PASS, including the pre-existing HMAC signature tests.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add src/gateway/webhook_receiver.rs src/gateway/mod.rs
git commit -m "gateway: turn WebhookReceiver into a router builder

It used to bind 0.0.0.0 on its own port, which would have opened a LAN
surface regardless of [gateway] host. It now only builds routes; the
gateway's existing server owns the listener, TLS and security headers.

The sink type changes from mpsc::Sender to the channel's own
InboundMessageSender: the mpsc path would have bypassed
start_message_forwarder, the only place inbound traffic stamps channel
health, so a receiving channel would still have looked dead."
```

---

### Task 3: Reserved-path guard

**Files:**
- Modify: `src/gateway/server/mod.rs` (add const + fn next to `build_router()` at ~line 670)
- Test: `src/gateway/webhook_receiver.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub const RESERVED_ROUTE_PREFIXES: &[&str]` and `pub fn is_reserved_route(path: &str) -> bool` in `crate::gateway::server` — Task 2's `WebhookReceiver::router` already calls the latter.

The list lives beside `build_router()` on purpose: a future route added there and not added here would silently become panic-able again, and a reviewer editing one sees the other.

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `src/gateway/webhook_receiver.rs`:

```rust
#[tokio::test]
async fn reserved_path_is_skipped_not_panicked() {
    use crate::gateway::channel::ChannelState;

    let state = ChannelState::new(16);
    let handler = Arc::new(MockWebhookHandler {
        secret: "s".to_string(),
        path: "/ws".to_string(),
    });

    // Must not panic, and must produce a router with no /ws route of its own
    // (merging one into the gateway router is what would panic at boot).
    let router = WebhookReceiver::router(vec![WebhookMount {
        handler,
        inbound: state.sender(),
    }]);

    // A router with zero routes 404s everything.
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ws")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn duplicate_webhook_paths_are_deduped() {
    use crate::gateway::channel::ChannelState;

    let state_a = ChannelState::new(16);
    let state_b = ChannelState::new(16);
    let mut rx_a = state_a.inbound_subscribe();
    let mut rx_b = state_b.inbound_subscribe();

    let mounts = vec![
        WebhookMount {
            handler: Arc::new(MockWebhookHandler {
                secret: "s".to_string(),
                path: "/webhook/dup".to_string(),
            }),
            inbound: state_a.sender(),
        },
        WebhookMount {
            handler: Arc::new(MockWebhookHandler {
                secret: "s".to_string(),
                path: "/webhook/dup".to_string(),
            }),
            inbound: state_b.sender(),
        },
    ];

    // Must not panic on the duplicate route.
    let router = WebhookReceiver::router(mounts);

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let body = br#"{"text":"dup"}"#.to_vec();
    let sig = WebhookReceiver::compute_signature("s", &body);
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/dup")
                .header("x-webhook-signature", sig)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // First mount wins; the second was skipped.
    assert!(rx_a.try_recv().is_ok(), "first mount must be the live one");
    assert!(rx_b.try_recv().is_err(), "second mount must have been skipped");
}

#[test]
fn reserved_route_matches_prefix_segments_only() {
    use crate::gateway::server::is_reserved_route;

    assert!(is_reserved_route("/ws"));
    assert!(is_reserved_route("/health"));
    assert!(is_reserved_route("/v1/chat/completions"));
    assert!(is_reserved_route("/a2a/stream"));
    assert!(is_reserved_route("/artifact/x/y/z"));
    assert!(is_reserved_route("/.well-known/agent-card.json"));

    // A path that merely starts with the same letters is NOT reserved.
    assert!(!is_reserved_route("/wsx"));
    assert!(!is_reserved_route("/healthcheck"));
    assert!(!is_reserved_route("/webhook/generic"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib webhook -- --nocapture`
Expected: FAIL to compile — `cannot find function 'is_reserved_route' in module 'crate::gateway::server'`.

- [ ] **Step 3: Implement the guard**

In `src/gateway/server/mod.rs`, directly **above** the `build_router()` method's `impl` block (module level, so it is importable):

```rust
/// Path prefixes the gateway router owns.
///
/// Keep this beside `build_router()` — a route added there and not added here
/// becomes a boot panic waiting to happen, because `Router::merge` panics on
/// duplicate routes and webhook paths come from operator-writable config.
pub const RESERVED_ROUTE_PREFIXES: &[&str] = &[
    "/ws",
    "/health",
    "/ready",
    "/metrics",
    "/artifact",
    "/v1",
    "/a2a",
    "/.well-known",
];

/// Whether `path` collides with a route the gateway itself serves.
///
/// Matches on whole path segments: `/wsx` is not reserved even though `/ws` is.
#[must_use]
pub fn is_reserved_route(path: &str) -> bool {
    RESERVED_ROUTE_PREFIXES
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p alephcore --lib webhook -- --nocapture`
Expected: PASS (all five webhook tests from Tasks 2 and 3).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src/gateway/server/mod.rs src/gateway/webhook_receiver.rs
git commit -m "gateway: skip webhook paths that collide with gateway routes

Router::merge panics on duplicate routes and WebhookChannelConfig.path
is operator-writable, so path = \"/ws\" would have been a boot panic.
Colliding and duplicate mounts are skipped with a warning instead.

The prefix list lives next to build_router() so a new gateway route and
its reservation are visible in the same edit."
```

---

### Task 4: `GatewayServer` carries and merges the webhook routes

**Files:**
- Modify: `src/gateway/server/mod.rs:340-365` (struct fields), `:412-481` (both constructors), `:519-526` (setters), `:670-695` (`build_router`)
- Test: `src/gateway/server/mod.rs` (`#[cfg(test)] mod tests`, near the existing `MiddlewareChain` test at ~line 943)

**Interfaces:**
- Consumes: `WebhookReceiver::router()` from Task 2 (produces the `Router` this stores).
- Produces: `GatewayServer::set_webhook_routes(&mut self, router: axum::Router)` — Task 5 calls it.

This mirrors `set_a2a_state` (`:519`) and `set_admin_router` (`:525`) exactly: an `Option` field set during bootstrap, consumed at serve time in `build_router()`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/gateway/server/mod.rs`:

```rust
#[tokio::test]
async fn webhook_routes_are_absent_until_set() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let server = GatewayServer::new(addr);
    let router = server.build_router();

    // No webhook routes set → the path is not served by the gateway.
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
    assert_ne!(
        response.status(),
        StatusCode::OK,
        "an unset webhook surface must not answer 200"
    );
}

#[tokio::test]
async fn set_webhook_routes_are_merged_into_build_router() {
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;
    use tower::ServiceExt;

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut server = GatewayServer::new(addr);
    server.set_webhook_routes(
        Router::new().route("/webhook/probe", post(|| async { "mounted" })),
    );

    let router = server.build_router();
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
```

If `GatewayServer::new(addr)` is not the constructor's real name/arity, read `src/gateway/server/mod.rs:400-490` and use whatever the two existing constructors are called; `build_router` may be private — if so, mark it `pub(crate)` rather than duplicating it in the test.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib set_webhook_routes_are_merged -- --nocapture`
Expected: FAIL to compile — `no method named 'set_webhook_routes' found for struct 'GatewayServer'`.

- [ ] **Step 3: Add the field, constructors, setter and merge**

Field, next to `admin_router: Option<Router>` (~line 365):

```rust
    /// Channel webhook ingestion routes, built by `WebhookReceiver::router()`
    /// once every channel has started. `None` when no configured channel
    /// ingests over HTTP — the route table is then byte-identical to before.
    webhook_routes: Option<Router>,
```

Both constructors (~lines 430 and 481) get `webhook_routes: None,` alongside `admin_router: None,`.

Setter, next to `set_admin_router` (~line 525):

```rust
    /// Mount channel webhook ingestion routes on the shared HTTP surface.
    pub fn set_webhook_routes(&mut self, router: Router) {
        self.webhook_routes = Some(router);
    }
```

Merge in `build_router()`, after the admin `nest` and before the final `layer`:

```rust
        // Channel webhook ingestion (generic webhook channel, and any future
        // channel that receives over HTTP POST). Auth is per-handler HMAC, the
        // same posture as /metrics and /a2a — see the design spec.
        if let Some(webhooks) = self.webhook_routes.clone() {
            router = router.merge(webhooks);
        }

        router.layer(SecurityHeadersLayer::new())
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p alephcore --lib webhook_routes -- --nocapture`
Expected: PASS (both new tests).

- [ ] **Step 5: Run the whole gateway test module for regressions**

Run: `cargo test -p alephcore --lib gateway:: 2>&1 | tail -20`
Expected: no new failures. Record the pass/fail counts — if anything was already failing on `main`, say so rather than attributing it to this change.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add src/gateway/server/mod.rs
git commit -m "gateway: merge channel webhook routes into build_router

Same shape as set_a2a_state / set_admin_router: an Option field set
during bootstrap, merged at serve time. With no webhook-ingesting
channel configured the route table is unchanged."
```

---

### Task 5: Collect the handlers at boot

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs` (end of `initialize_channels`, before it returns `channel_registry`)
- Modify: `docs/reference/GATEWAY.md` (document the inbound surface)

**Interfaces:**
- Consumes: `Channel::webhook_handler()` (Task 1), `WebhookMount` + `WebhookReceiver::router()` (Task 2), `GatewayServer::set_webhook_routes()` (Task 4).
- Produces: the live wiring. Nothing downstream consumes a new symbol.

- [ ] **Step 1: Find the return site**

Run: `grep -n "^}" src/bin/aleph-server/commands/start/builder/subsystems.rs | head -20` and read the tail of `initialize_channels` to locate where it returns `channel_registry`.

Confirm ordering with: `grep -n "initialize_channels\|run_until_shutdown" src/bin/aleph-server/commands/start/mod.rs`
Expected: `initialize_channels` at ~2480, `run_until_shutdown` at ~2828 — collection happens well before the server serves.

- [ ] **Step 2: Add the collection block**

Immediately before `initialize_channels` returns `channel_registry`:

```rust
    // Mount webhook ingestion for every channel that receives over HTTP POST.
    //
    // Runs here, after every channel has started, because a channel only
    // materialises its handler in `start()`. Without this block the generic
    // webhook channel starts, reports Connected, and is deaf — the handler it
    // built has no HTTP surface. (That was the state until 2026-07-29.)
    {
        use alephcore::gateway::{WebhookMount, WebhookReceiver};

        let mut mounts: Vec<WebhookMount> = Vec::new();
        for info in channel_registry.list().await {
            let Some(handle) = channel_registry.get(&info.id).await else {
                continue;
            };
            let channel = handle.read().await;
            if let Some(handler) = channel.webhook_handler() {
                // The channel's OWN broadcast, so start_message_forwarder
                // still sees the traffic and stamps channel health.
                mounts.push(WebhookMount {
                    handler,
                    inbound: channel.state().sender(),
                });
            }
        }

        if !mounts.is_empty() {
            let count = mounts.len();
            server.set_webhook_routes(WebhookReceiver::router(mounts));
            if !daemon {
                println!("  Gateway: {count} webhook ingestion route(s) mounted");
            }
        }
    }

    channel_registry
```

- [ ] **Step 3: Compile**

Run: `cargo check -p alephcore && cargo check --bin aleph-server 2>&1 | grep -E "^error" | head -20`
Expected: no errors. If `channel_registry.get()` returns a private type alias, bind it with `let` (as above) rather than naming the type.

- [ ] **Step 4: Verify the wire exists — grep the seam**

Run: `grep -rn "set_webhook_routes\|WebhookMount" --include="*.rs" src/ | grep -v "^src/gateway/webhook_receiver.rs" | grep -v "^src/gateway/server/mod.rs"`
Expected: at least one hit in `subsystems.rs`. This is the grep-diff guard: if a future refactor drops this call, the webhook channel goes deaf again with no test failing.

- [ ] **Step 5: Full test run**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: no new failures versus the pre-task baseline. State the counts explicitly.

- [ ] **Step 6: Document the surface**

Append to `docs/reference/GATEWAY.md` (find a "routes" or "HTTP surface" section; create one if absent):

```markdown
### Channel webhook ingestion

Channels that receive over HTTP POST (`generic webhook`, and future ones)
return a handler from `Channel::webhook_handler()`. `initialize_channels`
collects those after every channel has started and hands the resulting router
to `GatewayServer::set_webhook_routes()`, which merges it in `build_router()`.

- **One port.** Webhook traffic rides the gateway's own listener, so it
  inherits `[gateway] host`, TLS, and `SecurityHeadersLayer`. `WebhookReceiver`
  deliberately owns no listener — the version that bound `0.0.0.0` itself would
  have opened a LAN surface regardless of the configured host.
- **Auth is per-handler HMAC**, not the login wall — an external platform
  cannot present a device token. Same posture as `/health`, `/metrics`, `/a2a`:
  no transport-level auth, no rate limiter (that lives in `MiddlewareChain`,
  on the JSON-RPC/WS path only).
- **`path` is operator-writable**, so a collision with a gateway route would
  panic `Router::merge` at boot. `is_reserved_route()` in `server/mod.rs` skips
  those with a warning. Add every new gateway route to
  `RESERVED_ROUTE_PREFIXES` in the same edit.
- ⚠️ The sink is the channel's **own** `ChannelState::sender()`, not the
  registry's. Going direct to the registry bypasses
  `start_message_forwarder`, the only place inbound traffic stamps
  `health.record_event()` — the channel would receive while health monitoring
  reported it dead.
```

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add src/bin/aleph-server/commands/start/builder/subsystems.rs docs/reference/GATEWAY.md
git commit -m "gateway: mount webhook handlers at boot

WebhookChannel::start() built a handler and stored it; nothing ever
collected it, so the channel reported Connected while being deaf. This
collects every started channel's handler and hands the router to the
gateway server.

The advertised-but-disabled shape: the adapter, its config and its tests
were all complete and all passing."
```

---

### Task 6: `/stop` receipt real-machine QA

**Files:**
- Create: `/private/tmp/claude-502/-Volumes-TBU4-Workspace-Aleph/<session>/scratchpad/webhook_qa/sink.py` (throwaway, not committed)
- Create: `/private/tmp/.../scratchpad/webhook_qa/post.py` (throwaway, not committed)
- Create: `/private/tmp/.../scratchpad/webhook_qa/aleph_qa.toml` (throwaway config, not committed)
- Modify: `docs/superpowers/specs/2026-07-29-webhook-inbound-wiring-design.md` (append a QA results section)

**Interfaces:**
- Consumes: the live wiring from Task 5.
- Produces: recorded red/green evidence. No code symbols.

This is the whole reason the round exists: `handle_stop` (`src/gateway/inbound_router/command_handler.rs:373`) computes `busy_queue::purge()` and splices the count into the reply, and that path is reachable **only** from a channel — `chat.send` never touches it. Until now no channel inbound path existed on this machine.

- [ ] **Step 1: Write the outbound sink**

`sink.py` — the observation point for `/stop`'s reply, because `WebhookChannel::send` POSTs to `callback_url`:

```python
#!/usr/bin/env python3
"""Records every POST body to stdout and to sink.log. Ctrl-C to stop."""
import http.server, sys

LOG = open("sink.log", "a", buffering=1)

class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("content-length", 0))
        body = self.rfile.read(n).decode("utf-8", "replace")
        line = f"[sink] {body}"
        print(line, flush=True)
        LOG.write(line + "\n")
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *args):
        pass

if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8788
    print(f"[sink] listening on 127.0.0.1:{port}", flush=True)
    http.server.HTTPServer(("127.0.0.1", port), Handler).serve_forever()
```

- [ ] **Step 2: Write the inbound poster**

`post.py` — signs the body the way `GenericWebhookHandler::verify` expects. Read `src/gateway/interfaces/webhook/mod.rs:227-260` first and match the **exact** header name and payload field names it parses; the script below assumes `X-Webhook-Signature` and a `{sender_id, text, conversation_id}` payload. Fix it to match the code before running.

```python
#!/usr/bin/env python3
"""POST a signed message into the Aleph generic webhook channel."""
import hashlib, hmac, json, sys, urllib.request

SECRET = "qa-webhook-secret"
URL = "http://127.0.0.1:8787/webhook/qa"

def main():
    text = sys.argv[1]
    payload = {
        "sender_id": "qa-user",
        "conversation_id": "qa-conv",
        "text": text,
    }
    body = json.dumps(payload).encode()
    sig = "sha256=" + hmac.new(SECRET.encode(), body, hashlib.sha256).hexdigest()
    req = urllib.request.Request(
        URL,
        data=body,
        headers={"Content-Type": "application/json", "X-Webhook-Signature": sig},
    )
    with urllib.request.urlopen(req) as r:
        print(f"[post] {r.status} <- {text!r}")

if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Write the QA config**

`aleph_qa.toml` — a full config is required; copy the machine's real one and add this block, or start from `~/.aleph/config.toml`. `busy_input_mode = "queue"` is what makes messages pile into the lane instead of steering.

```toml
[gateway]
host = "127.0.0.1"
port = 8787

[channels.webhook_qa]
type = "webhook"
secret = "qa-webhook-secret"
callback_url = "http://127.0.0.1:8788/"
path = "/webhook/qa"
busy_input_mode = "queue"
```

Verify the config actually reaches the router (`subsystems.rs:710-752` registers `ChannelPolicyConfig` for every channel type except imessage/telegram, so `busy_input_mode` is honored). Pass it with `--config` — that flag is pinned process-wide via a `OnceLock` in `main()`, so all consumers read the same file.

- [ ] **Step 4: RED — prove the wire is severed on `main`**

```bash
git stash            # or: git checkout main -- src/ && rebuild
cargo build --bin aleph-server
./target/debug/aleph-server start --config /path/to/aleph_qa.toml   # terminal 1
python3 sink.py 8788                                                 # terminal 2
python3 post.py "hello"                                              # terminal 3
```

Expected RED: `post.py` fails to connect or gets 404 — the path is not served. `sink.log` stays empty. **Record the exact output**; this is the runtime evidence that the wire was severed, and it is the round's red.

Then `git stash pop` (or return to the working branch) and rebuild.

- [ ] **Step 5: GREEN part 1 — inbound arrives**

```bash
cargo build --bin aleph-server
./target/debug/aleph-server start --config /path/to/aleph_qa.toml
python3 post.py "hello"
```

Expected: `post.py` prints `200`. The server log shows the inbound message being routed. `sink.log` eventually shows the agent's reply.

If nothing arrives, check in this order: (1) server log for `Registered webhook handler` at boot — absent means collection did not run; (2) `webhook ingestion route(s) mounted` line; (3) signature header name against `GenericWebhookHandler::verify`; (4) `allowed_senders` — an empty list allows all, a non-empty one must contain `qa-user`.

- [ ] **Step 6: GREEN part 2 — the receipt count**

```bash
python3 post.py "Count slowly from 1 to 200, one number per line."   # occupies the session
sleep 2
python3 post.py "queued one"
python3 post.py "queued two"
sleep 1
python3 post.py "/stop"
```

Expected in `sink.log`: a reply containing the "run stopped" text **plus** a queued-messages-dropped clause whose count is **2**. Look up the exact strings in `src/gateway/i18n.rs` (`Msg::RunStopped`, `Msg::NoActiveRun`, `Msg::QueuedMessagesDropped`) and assert against those, not against a remembered phrasing.

Poll `sink.log` rather than sleeping a fixed amount — the run has to actually be in flight when messages 2 and 3 arrive, or they will not queue.

- [ ] **Step 7: GREEN part 3 — the negative case**

```bash
python3 post.py "/stop"     # nothing running, nothing queued
```

Expected: the reply carries the no-active-run text and **no** count clause. This proves the count is real rather than a constant.

- [ ] **Step 8: Record the evidence**

Append to `docs/superpowers/specs/2026-07-29-webhook-inbound-wiring-design.md`:

```markdown
## 9. QA 结果 (Real-Machine QA Results, 2026-07-29)

| 场景 | 期望 | 实测 |
|------|------|------|
| RED · 接线前签名 POST | 连不上 / 404，sink 空 | <粘贴实测> |
| GREEN · 入站到达 | POST 200，sink 收到 agent 回复 | <粘贴实测> |
| GREEN · /stop 回执计数 | 回执含计数 = 2 | <粘贴实测> |
| GREEN · 无排队时 /stop | 回执不带计数子句 | <粘贴实测> |

命令与脚本：见本节下方（sink.py / post.py / aleph_qa.toml 均为一次性脚本，未入库）。
```

Paste the **actual** captured output. If any scenario failed, write what failed and why — do not paste an expected value as if it were observed.

- [ ] **Step 9: Commit**

```bash
git add docs/superpowers/specs/2026-07-29-webhook-inbound-wiring-design.md
git commit -m "docs: record the real-machine QA for webhook inbound and /stop receipt

The /stop queued-message count had unit coverage only, because no
channel inbound path existed on this machine. Wiring webhook ingestion
created one; this records the red (severed wire, no inbound at all) and
the green (receipt count = 2, and no clause when nothing was queued)."
```

---

## Self-Review

**Spec coverage**

| Spec section | Task |
|---|---|
| §4.1 `Channel::webhook_handler()` default `None` | Task 1 |
| §4.2 `WebhookReceiver` → router builder; `InboundMessageSender` not mpsc; HMAC helpers kept | Task 2 |
| §4.3 `set_webhook_routes` + `build_router` merge; collection in `initialize_channels` | Tasks 4, 5 |
| §4.4 Reserved-path guard, skip+warn not panic | Task 3 |
| §5 all five tests | Tasks 1 (1), 2 (2), 3 (3), 4 (2) — the spec's `no_webhook_channel_means_no_route` is Task 4 Step 1's first test |
| §6 `/stop` QA red/green/negative | Task 6 |
| §7 B and C out of scope | Not implemented, recorded in the spec |
| §8 risks | Path collision → Task 3; posture → Task 5 Step 6 doc; trait blast radius → Task 1 default `None`; QA timing → Task 6 Step 6 "poll rather than sleep" |

**Type consistency check**

- `webhook_handler()` returns `Option<Arc<dyn WebhookHandler>>` in Tasks 1, 5 — consistent.
- `WebhookMount { handler, inbound }` field names identical in Tasks 2, 3, 5 — consistent.
- `WebhookReceiver::router(Vec<WebhookMount>) -> Router` in Tasks 2, 3, 5 — consistent.
- `is_reserved_route(&str) -> bool` defined in Task 3, called in Task 2 Step 3 — Task 2 will not compile standalone; this is intentional and flagged in Task 2 Step 4's expected output. **Tasks 2 and 3 must be executed in order and Task 2's commit is verified by Task 3's test run.**
- `set_webhook_routes(&mut self, Router)` in Tasks 4, 5 — consistent.
- `ChannelState::sender() -> InboundMessageSender` and `inbound_subscribe() -> broadcast::Receiver` — real methods at `channel.rs:642` and `:622`.

**Known soft spots the implementer must resolve by reading, not guessing**

1. `MockWebhookHandler`'s real fields (Task 2 Step 1) — adjust the struct literals to match.
2. `GenericWebhookHandler::verify`'s exact signature header and payload field names (Task 6 Step 2).
3. `GatewayServer`'s constructor name/arity and whether `build_router` is private (Task 4 Step 1).
4. The exact `i18n::Msg` strings for the receipt assertions (Task 6 Step 6).
