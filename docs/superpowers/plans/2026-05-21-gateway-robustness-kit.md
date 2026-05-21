# Gateway Robustness Kit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close 4 OpenClaw-parity gaps on Aleph's gateway (lane mapping, HTTP probes, state-version client exposure, gateway.identity.get) — all wiring on top of existing infrastructure.

**Architecture:** Existing two-phase boot (phase-1 placeholder in `handlers/mod.rs`, phase-2 override in `agent_init.rs`) is reused for G4. G1 rewrites a single function. G2 adds two axum routes + a shared atomic flag. G3 threads an existing snapshot type into two existing surfaces. No new modules required for the inner work.

**Tech Stack:** Rust, Tokio, axum, `IdempotencyGuard` + `StateVersionTracker` (already in `src/gateway/`), serde_json, uuid (already in deps).

**Reference spec:** `docs/superpowers/specs/2026-05-21-gateway-robustness-kit-design.md`

---

## File Map

```
src/gateway/lane.rs                          Modify  Lane::for_method rewrite
src/gateway/server/probe.rs                  Create  /health + /ready handlers
src/gateway/server/mod.rs                    Modify  shared-state fields + routes + GatewayServer fields
src/gateway/server/handler.rs                Modify  event envelope state_version on bumps
src/gateway/handlers/auth/connect.rs         Modify  include state_version in response
src/gateway/handlers/identity.rs             Create  gateway.identity.get
src/gateway/handlers/mod.rs                  Modify  phase-1 identity registration + module declaration
src/bin/aleph-server/commands/start/builder/
  agent_init.rs                              Modify  phase-2 identity wire + ready flag flip
tests/gateway_lane_routing.rs                Create  G1 coverage
tests/gateway_http_probes.rs                 Create  G2 coverage
tests/gateway_identity_rpc.rs                Create  G4 coverage
```

Total: ~250 LoC change. Zero new crates.

---

## Task 1: G1 — Lane::for_method rewrite

**Files:**
- Modify: `src/gateway/lane.rs:33-71`

- [ ] **Step 1: Add failing tests for the new heuristic at the bottom of `mod tests`**

Find the existing `mod tests` block in `src/gateway/lane.rs` (search for `#[cfg(test)]`). Add after the last test:

```rust
    #[test]
    fn new_rpc_defaults_to_mutate() {
        // Unknown method that wasn't in the original 17-name hardcode.
        // Used to fall through to Query → bypassing idempotency.
        // Spec 2 G1 fix: must now default to Mutate.
        assert_eq!(Lane::for_method("tools.invoke"), Lane::Execute);  // suffix `invoke`
        assert_eq!(Lane::for_method("agents.create"), Lane::Mutate);  // suffix `create`
        assert_eq!(Lane::for_method("agents.delete"), Lane::System);  // suffix `delete`
        assert_eq!(Lane::for_method("cron.toggle"), Lane::Mutate);    // suffix `toggle`
        assert_eq!(Lane::for_method("heartbeat.wake"), Lane::Mutate); // suffix `wake`
        assert_eq!(Lane::for_method("memory.search"), Lane::Query);   // suffix `search`
        assert_eq!(Lane::for_method("graph.neighbors"), Lane::Query); // suffix `neighbors`
        assert_eq!(Lane::for_method("trace.list"), Lane::Query);      // suffix `list`
        assert_eq!(Lane::for_method("daemon.shutdown"), Lane::Mutate); // suffix `shutdown` — no rule → Mutate (default-safe)
        assert_eq!(Lane::for_method("plugins.install"), Lane::System); // suffix `install`
    }

    #[test]
    fn explicit_overrides_win_over_heuristic() {
        // health / echo / version / system.info / request.state are read-only
        // even though they have no dot-suffix or have ambiguous names.
        assert_eq!(Lane::for_method("health"), Lane::Query);
        assert_eq!(Lane::for_method("echo"), Lane::Query);
        assert_eq!(Lane::for_method("version"), Lane::Query);
        assert_eq!(Lane::for_method("system.info"), Lane::Query);
        assert_eq!(Lane::for_method("request.state"), Lane::Query);
    }

    #[test]
    fn unknown_method_with_no_suffix_defaults_to_mutate() {
        // Methods without a dot fall to default → Mutate, ensuring
        // forgotten-to-list methods stay protected.
        assert_eq!(Lane::for_method("totally_unknown_rpc"), Lane::Mutate);
    }

    #[test]
    fn legacy_method_mappings_preserved() {
        // Methods that were in the original hardcode still land in the
        // expected lanes after the heuristic flip.
        assert_eq!(Lane::for_method("agent.run"), Lane::Execute);
        assert_eq!(Lane::for_method("chat.send"), Lane::Execute);
        assert_eq!(Lane::for_method("config.patch"), Lane::Mutate);
        assert_eq!(Lane::for_method("memory.store"), Lane::Mutate);
        assert_eq!(Lane::for_method("memory.delete"), Lane::Mutate);
        assert_eq!(Lane::for_method("plugins.uninstall"), Lane::System);
        assert_eq!(Lane::for_method("skills.install"), Lane::System);
        assert_eq!(Lane::for_method("logs.setLevel"), Lane::Mutate); // suffix `setLevel` not in heuristic → default Mutate
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::lane -- --nocapture`
Expected: 4 new tests fail (some assertions don't match current Query default).

- [ ] **Step 3: Replace `Lane::for_method` with the heuristic + override implementation**

Locate the current `impl Lane { ... }` block (around lane.rs:33-60). Replace the `for_method` and add the helper:

```rust
impl Lane {
    /// Map an RPC method name to its corresponding lane.
    ///
    /// Resolution order:
    /// 1. Explicit override map — looked up by full method name.
    /// 2. Suffix heuristic — last `.`-separated segment of the method name.
    /// 3. Default — `Lane::Mutate` (fail-safe: new side-effecting RPCs are
    ///    idempotency-protected by default).
    ///
    /// The previous implementation defaulted to `Lane::Query`, which let
    /// every uncovered side-effecting method silently bypass idempotency.
    pub fn for_method(method: &str) -> Self {
        if let Some(lane) = Self::override_for(method) {
            return lane;
        }
        if let Some(dot) = method.rfind('.') {
            let suffix = &method[dot + 1..];
            match suffix {
                "get" | "list" | "search" | "status" | "describe" | "history"
                | "effective" | "catalog" | "neighbors" | "subscribe"
                | "unsubscribe" | "stats" => return Lane::Query,
                "install" | "uninstall" | "delete" => return Lane::System,
                "run" | "send" | "invoke" | "execute" => return Lane::Execute,
                _ => {}
            }
        }
        Lane::Mutate
    }

    /// Explicit overrides for methods whose name doesn't match the heuristic.
    fn override_for(method: &str) -> Option<Lane> {
        match method {
            // Read-only operations that have no dot-suffix or look mutating.
            "health" | "echo" | "version" | "system.info" | "request.state" => {
                Some(Lane::Query)
            }
            // gateway.identity.get matches the .get suffix already; listed
            // here defensively so renames don't accidentally drop it from Query.
            "gateway.identity.get" => Some(Lane::Query),
            _ => None,
        }
    }

    /// Whether this lane's methods should be idempotency-guarded.
    /// Query lane is read-only and doesn't need protection.
    pub fn needs_idempotency(&self) -> bool {
        !matches!(self, Lane::Query)
    }
}
```

- [ ] **Step 4: Run tests — verify all pass (old + new)**

Run: `cargo test -p alephcore --lib gateway::lane -- --nocapture`
Expected: all tests in the module green.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/lane.rs
git commit -m "gateway: Lane::for_method heuristic + override (G1)

Replace hardcoded 17-name match with three-step resolver:
explicit override → suffix heuristic → default Mutate.

The previous default was Query, which let any uncovered side-effecting
RPC silently bypass the idempotency guard at handler.rs:367-438. Flipping
the default to Mutate closes that gap; new RPCs are protected by default.

Wire-compat: idempotency only kicks in when the client sends an
idempotency_key. Clients that don't are unaffected; clients that do now
get protection for previously-uncovered methods."
```

---

## Task 2: G2/G4 — Shared state fields for ready + instance_id + start time

**Files:**
- Modify: `src/gateway/server/mod.rs:79-96` (`GatewaySharedState`)
- Modify: `src/gateway/server/mod.rs:137-192` (`GatewayServer`)
- Modify: `src/gateway/server/mod.rs:215-265` (constructors)

- [ ] **Step 1: Add the three new fields to `GatewaySharedState`**

```rust
#[derive(Clone)]
pub struct GatewaySharedState {
    pub handlers: Arc<HandlerRegistry>,
    pub event_bus: Arc<GatewayEventBus>,
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    pub subscription_manager: Arc<SubscriptionManager>,
    pub guest_session_manager: Option<Arc<crate::gateway::security::GuestSessionManager>>,
    pub auth_mode: AuthMode,
    pub max_connections: usize,
    pub presence: Arc<PresenceTracker>,
    pub state_versions: Arc<StateVersionTracker>,
    pub rate_limiter: Arc<RateLimiter>,
    pub lane_manager: Arc<LaneManager>,
    pub idempotency_guard: Arc<crate::gateway::idempotency::IdempotencyGuard>,
    pub event_scope_guard: Arc<EventScopeGuard>,
    pub audit_log: Option<crate::security::audit::SecurityAuditLog>,
    /// Readiness flag — flipped to true after agent_init.rs completes
    /// phase-2 wiring. Read by /ready endpoint.
    pub ready: Arc<std::sync::atomic::AtomicBool>,
    /// Per-process instance identifier (UUID-v4). Clients use this to
    /// detect server restart vs same-server-came-back.
    pub instance_id: String,
    /// Unix epoch seconds at server construction. Surfaced by /health
    /// and gateway.identity.get for uptime calculation.
    pub started_at_unix: i64,
}
```

- [ ] **Step 2: Add the same three fields to `GatewayServer`**

Insert after `pub start_time: Instant,` (line ~164):

```rust
    /// Per-process instance identifier (UUID v4). Stable for the lifetime
    /// of this `GatewayServer`; regenerated on every restart.
    pub instance_id: String,
    /// Unix epoch seconds captured at construction. Sibling of `start_time`
    /// in JSON-serializable form.
    pub started_at_unix: i64,
    /// Readiness flag — flipped to true after boot phase-2 completes.
    pub ready: Arc<std::sync::atomic::AtomicBool>,
```

- [ ] **Step 3: Populate the new fields in both constructors**

Find `GatewayServer::new` (line ~217) and `GatewayServer::with_config` (line ~238). At the end of each (before the final `Self { ... }` block), the construction looks like the existing pattern. Locate the `Self { ... }` initializer in each constructor and add:

```rust
            instance_id: uuid::Uuid::new_v4().to_string(),
            started_at_unix: chrono::Utc::now().timestamp(),
            ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
```

Also, locate `build_router` (line ~327). Where it builds `GatewaySharedState`, add the three new fields to the struct literal — propagate from `self`:

```rust
            ready: self.ready.clone(),
            instance_id: self.instance_id.clone(),
            started_at_unix: self.started_at_unix,
```

- [ ] **Step 4: Verify compile**

Run: `cargo check -p alephcore`
Expected: clean. If any constructor was missed, compile error points to it.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/server/mod.rs
git commit -m "gateway: add ready/instance_id/started_at_unix to shared state

Three new fields on GatewaySharedState + GatewayServer to support:
- /ready HTTP probe (Task 4)
- gateway.identity.get RPC (Task 8)
- /health uptime calculation (Task 3)

ready is an Arc<AtomicBool> defaulted to false; agent_init.rs flips it
true at end of phase-2 (Task 5). instance_id is a fresh UUID v4 per
process. started_at_unix is captured at construction."
```

---

## Task 3: G2 — /health + /ready axum handlers

**Files:**
- Create: `src/gateway/server/probe.rs`
- Modify: `src/gateway/server/mod.rs` (add `mod probe;`)

- [ ] **Step 1: Create the probe module**

Create `src/gateway/server/probe.rs`:

```rust
//! HTTP probe endpoints for liveness (`/health`) and readiness (`/ready`).
//!
//! - `/health` always returns 200 OK with version + instance id + uptime.
//!   Used as a k8s liveness probe / reverse-proxy upstream check.
//! - `/ready` returns 200 once boot phase-2 completes, 503 before that
//!   (and during graceful shutdown). Used as a k8s readiness probe.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde_json::json;

use super::GatewaySharedState;

/// `GET /health` — always 200 OK while the process is alive. The body
/// gives the version + per-process instance_id + uptime so operators can
/// distinguish processes across restarts.
pub async fn handle_health(State(state): State<Arc<GatewaySharedState>>) -> impl IntoResponse {
    let uptime_secs = (chrono::Utc::now().timestamp() - state.started_at_unix).max(0);
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "version": env!("ALEPH_VERSION"),
            "instance_id": state.instance_id,
            "started_at_unix": state.started_at_unix,
            "uptime_secs": uptime_secs,
        })),
    )
}

/// `GET /ready` — 200 OK once boot phase-2 has flipped the ready flag,
/// 503 SERVICE_UNAVAILABLE before that or during shutdown. The body
/// always includes the current phase for diagnostic visibility.
pub async fn handle_ready(State(state): State<Arc<GatewaySharedState>>) -> impl IntoResponse {
    let ready = state.ready.load(Ordering::Acquire);
    let (status, phase) = if ready {
        (StatusCode::OK, "complete")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "booting")
    };
    (
        status,
        Json(json!({
            "ready": ready,
            "phase": phase,
            "version": env!("ALEPH_VERSION"),
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::lane::{LaneConfig, LaneManager};
    use crate::gateway::presence::PresenceTracker;
    use crate::gateway::rate_limiter::RateLimiter;
    use crate::gateway::state_version::StateVersionTracker;
    use crate::gateway::subscription::SubscriptionManager;
    use crate::gateway::{idempotency::IdempotencyGuard, security::EventScopeGuard};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use tokio::sync::RwLock;

    fn make_shared() -> Arc<GatewaySharedState> {
        Arc::new(GatewaySharedState {
            handlers: Arc::new(crate::gateway::handlers::HandlerRegistry::new()),
            event_bus: Arc::new(GatewayEventBus::new(64)),
            connections: Arc::new(RwLock::new(HashMap::new())),
            subscription_manager: Arc::new(SubscriptionManager::new()),
            guest_session_manager: None,
            auth_mode: Default::default(),
            max_connections: 1000,
            presence: Arc::new(PresenceTracker::new()),
            state_versions: Arc::new(StateVersionTracker::new()),
            rate_limiter: Arc::new(RateLimiter::default()),
            lane_manager: Arc::new(LaneManager::new(LaneConfig::default())),
            idempotency_guard: Arc::new(IdempotencyGuard::new(Duration::from_secs(300))),
            event_scope_guard: Arc::new(EventScopeGuard::new()),
            audit_log: None,
            ready: Arc::new(AtomicBool::new(false)),
            instance_id: "test-instance".to_string(),
            started_at_unix: chrono::Utc::now().timestamp(),
        })
    }

    #[tokio::test]
    async fn health_returns_ok_when_not_ready() {
        let state = make_shared();
        let resp = handle_health(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_returns_503_before_flag_flip() {
        let state = make_shared();
        let resp = handle_ready(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn ready_returns_ok_after_flag_flip() {
        let state = make_shared();
        state.ready.store(true, Ordering::Release);
        let resp = handle_ready(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```

- [ ] **Step 2: Register the module in `src/gateway/server/mod.rs`**

Locate the existing `mod` declarations (search for `mod handler;` or similar near the top of the file). Add alongside:

```rust
mod probe;
```

- [ ] **Step 3: Run the new tests**

Run: `cargo test -p alephcore --lib gateway::server::probe -- --nocapture`
Expected: 3 tests green.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/server/probe.rs src/gateway/server/mod.rs
git commit -m "gateway: HTTP /health + /ready handlers (G2)

handle_health always returns 200 OK with version + instance_id +
uptime_secs. Used as k8s livenessProbe / reverse-proxy upstream check.

handle_ready returns 503 SERVICE_UNAVAILABLE before agent_init.rs flips
the ready flag, 200 OK after. Used as k8s readinessProbe so traffic
isn't routed to a still-booting gateway."
```

---

## Task 4: G2 — Mount /health and /ready in build_router

**Files:**
- Modify: `src/gateway/server/mod.rs:367-385` (`build_router`)

- [ ] **Step 1: Add the two routes before `.fallback_service(...)`**

Find the existing block:

```rust
let mut router = Router::new()
    .route("/ws", get(handler::ws_upgrade_handler))
    .fallback_service(control_plane)
    .with_state(shared)
    .merge(openai);
```

Replace with:

```rust
let mut router = Router::new()
    .route("/ws", get(handler::ws_upgrade_handler))
    .route("/health", get(probe::handle_health))
    .route("/ready", get(probe::handle_ready))
    .fallback_service(control_plane)
    .with_state(shared)
    .merge(openai);
```

- [ ] **Step 2: Verify compile**

Run: `cargo check -p alephcore`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/gateway/server/mod.rs
git commit -m "gateway: mount /health and /ready in build_router (G2)

Routes registered before fallback so they take precedence over the
control-plane UI's catch-all. /v1/admin/* and OpenAI routes are
unaffected; existing /ws path unchanged."
```

---

## Task 5: G2 — Flip ready flag at end of agent_init.rs phase-2

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs` (end of phase-2 block, before function return)

- [ ] **Step 1: Locate the end of phase-2 wiring**

Search for the last `server.handlers_mut().register(...)` call in `agent_init.rs`. It's typically followed by `agent_reg = Some(...)` or similar bookkeeping near the end of the function. Insert the flip just before the function returns its `AgentInitOutput` struct (or equivalent).

- [ ] **Step 2: Add the flip**

```rust
    // G2: signal readiness. /ready returns 200 from this point onward.
    server
        .ready
        .store(true, std::sync::atomic::Ordering::Release);
    if !daemon {
        println!("  Gateway readiness: signaled (ready=true)");
    }
```

Place this immediately before the function's final return / `Ok(...)` statement.

- [ ] **Step 3: Verify compile**

Run: `cargo check --bin aleph-server`
Expected: clean. The `ready` field on `server` was added in Task 2.

- [ ] **Step 4: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/agent_init.rs
git commit -m "gateway: flip ready flag at end of agent_init phase-2 (G2)

/ready returns 503 until this flip executes, then 200. Reverse proxies
and orchestrators see the gateway as not-yet-routable during boot,
preventing traffic from hitting a half-wired handler tree."
```

---

## Task 6: G3 — Include state_version in auth.connect response

**Files:**
- Modify: `src/gateway/handlers/auth/connect.rs`

- [ ] **Step 1: Read the existing connect handler to confirm response shape**

Run: `cat src/gateway/handlers/auth/connect.rs | head -80`

Identify the success-response construction. It's the `JsonRpcResponse::success(...)` call near the end of `handle_connect`. Note the existing JSON object shape.

- [ ] **Step 2: Add the AuthContext field for state versions**

The handler signature is `handle_connect(request, ctx: Arc<AuthContext>)`. Locate the `AuthContext` struct definition (likely in the same file or `src/gateway/handlers/auth/mod.rs`). Add:

```rust
pub state_versions: Arc<crate::gateway::state_version::StateVersionTracker>,
```

Update every constructor / `AuthContext::new(...)` call site to pass the tracker. Find them via:

```bash
grep -rn "AuthContext::new\|AuthContext {" src/ | head -20
```

The plumbing flows from `GatewaySharedState.state_versions` → `ConnectionContext` → `AuthContext`. Both already exist; we add one extra `.clone()` per call site.

- [ ] **Step 3: Include the snapshot in the success response**

In `handle_connect`, locate the success-response JSON. Add the new field:

```rust
let response_json = json!({
    // ... existing fields ...
    "state_version": ctx.state_versions.snapshot(),
});
JsonRpcResponse::success(request.id, response_json)
```

- [ ] **Step 4: Verify compile**

Run: `cargo check -p alephcore`
Expected: clean. Any missed call site surfaces immediately.

- [ ] **Step 5: Write a unit test for the new field**

If `connect.rs` has a `#[cfg(test)] mod tests`, add:

```rust
    #[tokio::test]
    async fn connect_response_includes_state_version() {
        let ctx = make_test_auth_context();  // existing helper, if any; otherwise inline
        ctx.state_versions.bump_config();   // bump so the snapshot isn't all zeros
        let req = JsonRpcRequest::with_id("auth.connect", Some(json!({})), json!(1));
        let resp = handle_connect(req, ctx).await;
        assert!(resp.is_success());
        let result = resp.result.unwrap();
        assert!(result.get("state_version").is_some());
        assert_eq!(result["state_version"]["config"], 1);
    }
```

If no test helper exists, this can be deferred to the integration test in Task 8.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/auth/ src/gateway/server/handler.rs
git commit -m "gateway: surface state_version in auth.connect response (G3)

AuthContext gains state_versions: Arc<StateVersionTracker>. The connect
success response now includes a state_version object {presence, health,
config} so clients can capture the snapshot at handshake time and detect
server-side bumps later."
```

---

## Task 7: G3 — Include state_version in bumped-event envelopes

**Files:**
- Modify: `src/gateway/event_bus.rs` (extend `GatewayEvent` envelope)
- Modify: `src/gateway/server/handler.rs:250, 484, 684` (publish sites that follow bumps)

- [ ] **Step 1: Locate the `GatewayEvent` struct**

```bash
grep -nE "pub struct GatewayEvent|publish(" src/gateway/event_bus.rs | head -10
```

- [ ] **Step 2: Add the optional `state_version` field**

In `GatewayEvent`'s struct definition:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayEvent {
    // ... existing fields ...
    /// Snapshot of StateVersionTracker at the time the event was emitted.
    /// Populated only when the event's origin event_bus.publish_with_version
    /// was called (typically after a presence/health/config bump). `None`
    /// for other events so the envelope size stays the same.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state_version: Option<crate::gateway::state_version::StateVersion>,
}
```

- [ ] **Step 3: Add `publish_with_version` to `GatewayEventBus`**

```rust
impl GatewayEventBus {
    /// Publish an event with an accompanying state_version snapshot.
    /// Used at sites that just called `state_versions.bump_*()`.
    pub fn publish_with_version(
        &self,
        mut event: GatewayEvent,
        version: crate::gateway::state_version::StateVersion,
    ) {
        event.state_version = Some(version);
        self.publish(event);
    }
}
```

The original `publish` stays unchanged.

- [ ] **Step 4: Update the three bump call sites in handler.rs**

For each `ctx.state_versions.bump_presence();` at lines ~250, ~484, ~684, audit the surrounding code. If a `publish(...)` follows shortly after, change to `publish_with_version(event, ctx.state_versions.snapshot())`. If no event follows, leave as-is — the bump still happens, just no event-side surfacing for that case.

This is a targeted edit; the agent doing this task should read the surrounding 20 lines of each site to confirm whether an event publish follows.

- [ ] **Step 5: Verify compile**

Run: `cargo check -p alephcore`
Expected: clean.

- [ ] **Step 6: Write a unit test in event_bus.rs**

```rust
    #[tokio::test]
    async fn publish_with_version_decorates_event() {
        let bus = GatewayEventBus::new(64);
        let mut rx = bus.subscribe();

        let event = GatewayEvent::new("test.event", json!({}));
        let snap = crate::gateway::state_version::StateVersion {
            presence: 7,
            health: 2,
            config: 0,
        };
        bus.publish_with_version(event, snap);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.state_version, Some(snap));
    }

    #[tokio::test]
    async fn publish_leaves_state_version_none() {
        let bus = GatewayEventBus::new(64);
        let mut rx = bus.subscribe();
        bus.publish(GatewayEvent::new("test.event", json!({})));
        let received = rx.recv().await.unwrap();
        assert!(received.state_version.is_none());
    }
```

Use the existing helpers in `event_bus.rs`'s test module if present (`GatewayEvent::new` may have a different name; adapt to what's there).

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib gateway::event_bus -- --nocapture`
Expected: existing tests + 2 new ones green.

- [ ] **Step 8: Commit**

```bash
git add src/gateway/event_bus.rs src/gateway/server/handler.rs
git commit -m "gateway: state_version in bumped-event envelope (G3)

GatewayEvent gains an Option<StateVersion> field that
publish_with_version() populates from the snapshot at bump time. The
three bump call sites in handler.rs (presence/health/config) now
surface the new version to clients.

Wire-compat: the new field uses #[serde(skip_serializing_if =
\"Option::is_none\")] so events that don't bump anything emit the same
JSON they did before."
```

---

## Task 8: G4 — gateway.identity.get handler

**Files:**
- Create: `src/gateway/handlers/identity.rs`
- Modify: `src/gateway/handlers/mod.rs` (declare module + phase-1 placeholder)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs` (phase-2 wire)

- [ ] **Step 1: Create the handler**

```rust
//! `gateway.identity.get` — return per-process identity for client
//! sanity checks (version match, restart detection, supported protocols).

use std::sync::Arc;

use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::server::GatewaySharedState;

/// Handle `gateway.identity.get`. Wired in phase-2 with a captured
/// GatewaySharedState + the registry method count snapshot.
pub async fn handle_identity_get(
    request: JsonRpcRequest,
    state: Arc<GatewaySharedState>,
    method_count: usize,
) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id,
        json!({
            "version": env!("ALEPH_VERSION"),
            "instance_id": state.instance_id,
            "started_at_unix": state.started_at_unix,
            "supported_protocols": ["jsonrpc/2.0", "openai-compat/v1", "a2a/v1"],
            "registered_method_count": method_count,
            "state_version": state.state_versions.snapshot(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::server::test_helpers::make_shared_state;
    use serde_json::json;

    #[tokio::test]
    async fn returns_expected_shape() {
        let state = make_shared_state();
        let req = JsonRpcRequest::with_id("gateway.identity.get", None, json!(1));
        let resp = handle_identity_get(req, state.clone(), 42).await;
        assert!(resp.is_success());
        let result = resp.result.unwrap();
        assert_eq!(result["instance_id"], state.instance_id);
        assert_eq!(result["registered_method_count"], 42);
        assert!(result["supported_protocols"].is_array());
        assert!(result["state_version"].is_object());
    }
}
```

If `gateway::server::test_helpers::make_shared_state` doesn't exist, inline the fixture from the probe.rs tests at Task 3.

- [ ] **Step 2: Add module declaration + phase-1 placeholder in `src/gateway/handlers/mod.rs`**

Add near the other `pub mod` declarations:

```rust
pub mod identity;
```

In `HandlerRegistry::new()` (around line 217+), add a phase-1 registration:

```rust
        // gateway.identity.get — phase-1 placeholder; agent_init.rs overrides at boot.
        registry.register("gateway.identity.get", |req| async move {
            service_unavailable(
                req,
                "gateway.identity.get requires GatewaySharedState (boot phase 2)",
            )
        });
```

- [ ] **Step 3: Phase-2 wire in `agent_init.rs`**

After Task 5's ready flip block but *before* the function returns, add:

```rust
    // G4: register gateway.identity.get with captured shared state.
    {
        let shared_for_identity = server.shared_state();  // assumes a getter; see Step 4
        let method_count = server.handlers_mut().len();
        server
            .handlers_mut()
            .register("gateway.identity.get", move |req| {
                let state = shared_for_identity.clone();
                let count = method_count;
                async move {
                    alephcore::gateway::handlers::identity::handle_identity_get(req, state, count)
                        .await
                }
            });
        if !daemon {
            println!("  gateway.identity.get: wired");
        }
    }
```

- [ ] **Step 4: If no `shared_state()` getter on `GatewayServer`, add one**

```rust
impl GatewayServer {
    /// Build a fresh `Arc<GatewaySharedState>` snapshot from the server's
    /// fields. Used by phase-2 wiring sites that need to capture state
    /// for handler closures.
    pub fn shared_state(&self) -> Arc<GatewaySharedState> {
        Arc::new(GatewaySharedState {
            handlers: self.handlers.clone(),
            event_bus: self.event_bus.clone(),
            connections: self.connections.clone(),
            subscription_manager: self.subscription_manager.clone(),
            guest_session_manager: self.guest_session_manager.clone(),
            auth_mode: self.config.auth_mode.clone(),
            max_connections: self.config.max_connections,
            presence: self.presence.clone(),
            state_versions: self.state_versions.clone(),
            rate_limiter: self.rate_limiter.clone(),
            lane_manager: self.lane_manager.clone(),
            idempotency_guard: self.idempotency_guard.clone(),
            event_scope_guard: self.event_scope_guard.clone(),
            audit_log: None,
            ready: self.ready.clone(),
            instance_id: self.instance_id.clone(),
            started_at_unix: self.started_at_unix,
        })
    }
}
```

If `build_router` already constructs a `GatewaySharedState`, refactor it to call `self.shared_state()` so there's one construction site.

- [ ] **Step 5: Verify compile + run tests**

Run: `cargo check -p alephcore && cargo check --bin aleph-server && cargo test -p alephcore --lib gateway::handlers::identity -- --nocapture`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/identity.rs \
        src/gateway/handlers/mod.rs \
        src/gateway/server/mod.rs \
        src/bin/aleph-server/commands/start/builder/agent_init.rs
git commit -m "gateway: gateway.identity.get RPC (G4)

Returns version + per-process instance_id + started_at + supported
protocols + method count + state_version snapshot. Clients use this
for:
- Version-mismatch detection (client built against an older protocol)
- Restart detection (instance_id changes across reconnect)
- Supported-protocol negotiation

Wired via the two-phase pattern: phase-1 placeholder in
HandlerRegistry::new uses service_unavailable; phase-2 in agent_init.rs
binds the live GatewaySharedState."
```

---

## Task 9: Integration tests

**Files:**
- Create: `tests/gateway_http_probes.rs`
- Create: `tests/gateway_identity_rpc.rs`

- [ ] **Step 1: Create probe integration test**

```rust
//! Integration coverage for the gateway HTTP probes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alephcore::gateway::{GatewayConfig, GatewayServer};
use reqwest::StatusCode;

async fn spawn_server_on_port(port: u16) -> Arc<AtomicBool> {
    let addr = format!("127.0.0.1:{port}").parse().unwrap();
    let server = GatewayServer::with_config(addr, GatewayConfig::default());
    let ready = server.ready.clone();
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    // Give axum a moment to bind.
    tokio::time::sleep(Duration::from_millis(200)).await;
    ready
}

#[tokio::test]
async fn health_returns_200_always() {
    let port = 18801;
    let _ready = spawn_server_on_port(port).await;
    let resp = reqwest::get(format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["instance_id"].is_string());
    assert!(body["uptime_secs"].is_number());
}

#[tokio::test]
async fn ready_returns_503_before_flag_flip_then_200_after() {
    let port = 18802;
    let ready = spawn_server_on_port(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/ready"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ready"], false);
    assert_eq!(body["phase"], "booting");

    ready.store(true, Ordering::Release);

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/ready"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ready"], true);
    assert_eq!(body["phase"], "complete");
}
```

If `reqwest` isn't already a dev-dependency, check `Cargo.toml`. If absent, use `hyper` or skip HTTP and call the handlers directly via `axum::Router::oneshot()`. Existing tests under `tests/` should give a precedent.

- [ ] **Step 2: Create identity RPC integration test**

```rust
//! Integration coverage for gateway.identity.get.

use std::sync::Arc;

use alephcore::gateway::handlers::identity::handle_identity_get;
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::gateway::server::test_helpers::make_shared_state;
use serde_json::json;

#[tokio::test]
async fn identity_get_returns_required_fields() {
    let state = make_shared_state();
    let req = JsonRpcRequest::with_id("gateway.identity.get", None, json!(1));
    let resp = handle_identity_get(req, state.clone(), 100).await;
    assert!(resp.is_success(), "expected success: {:?}", resp.error);
    let result = resp.result.unwrap();
    assert_eq!(result["instance_id"], state.instance_id);
    assert_eq!(result["registered_method_count"], 100);
    assert!(result["supported_protocols"].is_array());
    assert!(result["state_version"]["presence"].is_number());
    assert!(result["state_version"]["health"].is_number());
    assert!(result["state_version"]["config"].is_number());
}

#[tokio::test]
async fn identity_get_state_version_reflects_bumps() {
    let state = make_shared_state();
    state.state_versions.bump_config();
    state.state_versions.bump_config();
    state.state_versions.bump_health();

    let req = JsonRpcRequest::with_id("gateway.identity.get", None, json!(1));
    let resp = handle_identity_get(req, state, 1).await;
    let result = resp.result.unwrap();
    assert_eq!(result["state_version"]["config"], 2);
    assert_eq!(result["state_version"]["health"], 1);
    assert_eq!(result["state_version"]["presence"], 0);
}
```

`make_shared_state` should be a small helper added in `src/gateway/server/mod.rs` under a `pub mod test_helpers` block (mirror the pattern from `src/gateway/server/probe.rs::tests`). Add this helper now if it doesn't exist:

```rust
#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    pub fn make_shared_state() -> Arc<GatewaySharedState> {
        Arc::new(GatewaySharedState {
            handlers: Arc::new(crate::gateway::handlers::HandlerRegistry::new()),
            event_bus: Arc::new(crate::gateway::event_bus::GatewayEventBus::new(64)),
            connections: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            subscription_manager: Arc::new(crate::gateway::subscription::SubscriptionManager::new()),
            guest_session_manager: None,
            auth_mode: Default::default(),
            max_connections: 1000,
            presence: Arc::new(crate::gateway::presence::PresenceTracker::new()),
            state_versions: Arc::new(crate::gateway::state_version::StateVersionTracker::new()),
            rate_limiter: Arc::new(crate::gateway::rate_limiter::RateLimiter::default()),
            lane_manager: Arc::new(crate::gateway::lane::LaneManager::new(
                crate::gateway::lane::LaneConfig::default(),
            )),
            idempotency_guard: Arc::new(crate::gateway::idempotency::IdempotencyGuard::new(
                Duration::from_secs(300),
            )),
            event_scope_guard: Arc::new(crate::gateway::security::EventScopeGuard::new()),
            audit_log: None,
            ready: Arc::new(AtomicBool::new(false)),
            instance_id: "test-instance".to_string(),
            started_at_unix: chrono::Utc::now().timestamp(),
        })
    }
}
```

- [ ] **Step 3: Run the new tests**

Run: `cargo test --test gateway_http_probes --test gateway_identity_rpc 2>&1 | tail -25`
Expected: all green. If `reqwest` is missing from dev-deps and the probe test won't compile, fall back to `axum::Router::oneshot` per the precedent in Spec 1's existing integration tests.

- [ ] **Step 4: Commit**

```bash
git add tests/gateway_http_probes.rs \
        tests/gateway_identity_rpc.rs \
        src/gateway/server/mod.rs
git commit -m "tests: integration coverage for G2 (HTTP probes) + G4 (identity RPC)

gateway_http_probes.rs (2 tests): GET /health returns 200 with version
+ instance_id + uptime; GET /ready returns 503 before flag flip and
200 after.

gateway_identity_rpc.rs (2 tests): identity.get returns expected
fields; state_version mirrors live tracker bumps.

Adds gateway::server::test_helpers::make_shared_state for both this
spec's integration tests and any future ones that need a fixture
shared state."
```

---

## Task 10: Final verification + memory note

- [ ] **Step 1: Cargo check (lib + bin)**

Run:
```bash
cargo check -p alephcore && cargo check --bin aleph-server
```
Expected: 0 errors.

- [ ] **Step 2: Touched-module test sweep**

Run:
```bash
cargo test -p alephcore --lib -- gateway::lane gateway::server::probe gateway::handlers::identity gateway::event_bus
cargo test --test gateway_http_probes --test gateway_identity_rpc
```
Expected: all green. Do NOT run the full `cargo test --lib` — main has 19 pre-existing baseline failures per `project_baseline_test_failures`.

- [ ] **Step 3: rustfmt the touched files (targeted, NOT project-wide)**

```bash
rustfmt --edition 2021 \
  src/gateway/lane.rs \
  src/gateway/server/probe.rs \
  src/gateway/server/mod.rs \
  src/gateway/server/handler.rs \
  src/gateway/handlers/identity.rs \
  src/gateway/handlers/mod.rs \
  src/gateway/handlers/auth/connect.rs \
  src/gateway/event_bus.rs \
  src/bin/aleph-server/commands/start/builder/agent_init.rs \
  tests/gateway_http_probes.rs \
  tests/gateway_identity_rpc.rs
```

- [ ] **Step 4: Verify the commit chain**

Run: `git log --oneline main..HEAD`
Expected: 9 commits in order (Tasks 1, 2, 3, 4, 5, 6, 7, 8, 9).

- [ ] **Step 5: Create / update memory note**

Append to `/Users/zouguojun/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_gateway_robustness_kit.md`:

```markdown
---
name: gateway-robustness-kit-cycle
description: Spec 2 of OpenClaw-inspired gateway improvement roadmap. Lane mapping fix + HTTP probes + state_version client exposure + gateway.identity.get RPC.
metadata:
  type: project
---

# Gateway Robustness Kit (Spec 2)

Closes G1-G4 from the spec. Spec at
[[spec]] `docs/superpowers/specs/2026-05-21-gateway-robustness-kit-design.md`,
plan at `docs/superpowers/plans/2026-05-21-gateway-robustness-kit.md`.

**Status**: <fill in: shipped via <merge-commit>; tests green; not yet
pushed to origin>.

**Why**: OpenClaw's gateway exposes a tight robustness baseline that
Aleph almost had — most machinery existed but wasn't surfaced. G1 was
the most consequential: Lane::for_method's hardcoded 17-name match
meant any new side-effecting RPC silently bypassed idempotency. Flipping
the default from Query to Mutate (with suffix heuristic + explicit
overrides) closes the gap permanently.

**Follow-up**: G3's event-envelope state_version may need a wider sweep
later — only 3 bump sites in handler.rs were rewired. Tools.* and other
domains that bump versions don't yet surface them.
```

Add to MEMORY.md index.

- [ ] **Step 6: Final commit**

```bash
git add docs/superpowers/plans/2026-05-21-gateway-robustness-kit.md \
        /Users/zouguojun/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_gateway_robustness_kit.md \
        /Users/zouguojun/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md
git commit -m "docs: gateway robustness kit plan + memory note"
```

---

## Self-Review Summary

- **Spec coverage**: G1 → Task 1; G2 → Tasks 2-5 + Task 9; G3 → Tasks 6+7; G4 → Tasks 8 + 9. All covered.
- **No placeholders**: each step has concrete code + commands.
- **Type consistency**: `state.ready` is `Arc<AtomicBool>` everywhere (Tasks 2, 3, 5); `state.instance_id` is `String` (Tasks 2, 3, 8); `state.started_at_unix` is `i64` (Tasks 2, 3, 8); `state.state_versions.snapshot()` returns `StateVersion` (Tasks 6, 7, 8).
- **Drive-by risk**: Task 6 (`AuthContext`) may touch more call sites than expected. The Step 2 grep should surface them all; if the list is long (>5), reassess whether to split this into its own commit.
