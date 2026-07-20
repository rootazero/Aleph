# Gateway Auth UX — Phase 2: Bootstrap Nonce & "Open in Browser"

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop putting the long-lived shared token in URLs. Replace Phase 1's `?token=<plaintext>` navigation with a one-time, loopback-gated bootstrap nonce flow. Add user-facing affordances (`aleph open` CLI, Tauri "Open in Browser" menu) that let a same-machine browser become authenticated with zero typing — just one click.

**Architecture:**
- New pure logic module `src/gateway/bootstrap.rs` provides `BootstrapNonceManager` (mirrors the structure of `src/gateway/challenge.rs:74-100` — `DashMap`-backed, TTL-bounded, single-use, replay-guarded).
- New JSON-RPC method `gateway.bootstrap.issue` (auth-gated; returns `{nonce, expires_in_secs}`) so any authenticated caller (CLI, shell) can ask for a nonce without re-implementing HTTP plumbing.
- New HTTP route `GET /auth/bootstrap?nonce=…` in `src/gateway/auth_middleware.rs` validates the nonce, **enforces peer loopback** via `axum::extract::ConnectInfo<SocketAddr>` (which `src/gateway/server/mod.rs:530,555` already wires in), creates a session via the existing `HttpSessionManager`, sets the `aleph_session` HttpOnly cookie, and 302-redirects to `/`.
- New CLI subcommand `aleph open` issues a nonce via RPC and shells out to `open` / `xdg-open` / `start` with the bootstrap URL.
- New macOS menu item "Open in Browser" in `desktop/shell/src/menu.rs` does the same via the Tauri shell plugin.
- The Tauri shell itself migrates off Phase 1's `?token=` URL: it now issues a nonce on first reveal and navigates to `/auth/bootstrap?nonce=…`. The `?token=` codepath in `interfaces/webchat/src/context.rs:284-313` is left intact during Phase 2 (defense-in-depth fallback); Phase 4 removes it.

**Tech Stack:** Rust 1.x with `axum 0.7`, `dashmap`, `hmac` + `sha2` + `uuid` (already pulled by `challenge.rs`), `tauri-plugin-shell` (already in shell), `clap` (CLI), `reqwest` or `ureq` for shell HTTP (verify which is already a dependency before adding new).

**Out of scope (Phase 3+):**
- Cold-visit / remote browser pairing UX
- Replacing the `/login` HTML form (Phase 4)
- Server-Sent Events for "browser approved" push (Phase 3 handles via existing event bus)

---

## Threat Model & Design Constraints

- **Same-UID = trusted:** Phase 1 establishes that anyone who can read `~/.aleph/data/security.db` is the user. The shell, CLI, and same-UID processes can call `gateway.bootstrap.issue` because they already have the shared token. We extend the same trust to "I can prove I issued this nonce within the last 60 seconds via a same-machine TCP connection."
- **Loopback hard-gate:** The HTTP consumer endpoint refuses any request whose peer address is not `127.0.0.0/8` or `::1`. This is the **only** check that prevents a remote attacker from racing for a leaked nonce — we do NOT trust the `Origin` header alone. `SocketAddr` from `ConnectInfo` is authoritative.
- **One-shot nonce:** Successful consumption removes the nonce from the `pending` map and adds it to a `used` map for replay-window protection (mirrors `challenge.rs:80-82` `used` semantics).
- **Short TTL:** 60 seconds default, configurable via TOML `[gateway.bootstrap] nonce_ttl_secs`. Long enough to launch a browser; short enough that a stolen URL is useless by the time it travels.
- **Bearer required to issue:** `gateway.bootstrap.issue` requires a valid bearer / shared_token / session-cookie context. This prevents a hostile browser tab from issuing nonces for itself without already being authenticated.

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/gateway/bootstrap.rs` | **Create** | `BootstrapNonceManager` pure logic + `BootstrapError` enum |
| `src/gateway/mod.rs` | Modify | Add `pub mod bootstrap;` |
| `src/gateway/handlers/auth/bootstrap.rs` | **Create** | `handle_gateway_bootstrap_issue` RPC handler |
| `src/gateway/handlers/auth/mod.rs` | Modify | `pub use bootstrap::handle_gateway_bootstrap_issue;` |
| `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` | Modify | Register `gateway.bootstrap.issue` RPC |
| `src/gateway/auth_middleware.rs` | Modify | Add `GET /auth/bootstrap` route + `handle_bootstrap_consume` |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs` | Modify | Construct `BootstrapNonceManager`, plumb into `AuthContext` + `AuthState` |
| `src/gateway/handlers/auth/mod.rs:AuthContext` | Modify | Add `bootstrap_mgr: Arc<BootstrapNonceManager>` field |
| `src/gateway/config.rs` | Modify | Add `BootstrapConfig` block under `[gateway]` |
| `interfaces/cli/src/cli_args.rs` (or wherever Subcommand enum lives) | Modify | Add `Commands::Open` variant |
| `interfaces/cli/src/commands/open_cmd.rs` | **Create** | Implementation of `aleph open` |
| `interfaces/cli/src/main.rs` | Modify | Dispatch `Commands::Open` |
| `desktop/shell/src/menu.rs:12-16` | Modify | Add `ID_OPEN_BROWSER` menu item + handler |
| `desktop/shell/src/daemon.rs` (the new `load_bootstrap_token` from Phase 1) | Modify | Add `issue_bootstrap_nonce(token: &str) -> Option<String>` HTTP client helper |
| `desktop/shell/src/daemon.rs:build_panel_url` (Phase 1 helper) | Modify | Replace `?token=<plaintext>` with `/auth/bootstrap?nonce=<one-shot>` when nonce is available; fall back to Phase 1 `?token=` path if nonce-issue failed (graceful degradation during rollout) |

**Test files:**
- `src/gateway/bootstrap.rs` — embedded unit tests (issue / consume / replay / expiry)
- `src/gateway/handlers/auth/bootstrap.rs` — embedded handler tests
- `src/gateway/auth_middleware.rs` — extend existing `mod tests` with bootstrap-consume tests using axum's `TestServer` or oneshot
- `tests/bootstrap_loopback_gate.rs` (**create**) — integration test that hits `/auth/bootstrap` from a non-loopback simulated addr and confirms refusal
- `interfaces/cli/src/commands/open_cmd.rs` — embedded test for URL composition

---

## Task 1: `BootstrapNonceManager` pure logic

**Files:**
- Create: `src/gateway/bootstrap.rs`
- Modify: `src/gateway/mod.rs`

- [ ] **Step 1: Write failing tests covering issue / consume / replay / expiry**

Create `src/gateway/bootstrap.rs`:

```rust
//! One-time bootstrap nonces for cookie-handoff to local browsers.
//!
//! Mirrors the shape of [`crate::gateway::challenge::ChallengeManager`]
//! (dashmap-backed pending + used sets, TTL-bounded), but with a far
//! simpler purpose: handing a same-machine browser an authenticated
//! session cookie without ever showing the user a token.
//!
//! Threat model: see `docs/superpowers/plans/2026-05-25-gateway-auth-ux-phase2-bootstrap-nonce.md`.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use uuid::Uuid;

/// Default nonce lifetime: long enough to launch a browser and complete
/// the redirect, short enough that a leaked URL is useless soon after.
pub const DEFAULT_NONCE_TTL: Duration = Duration::from_secs(60);

/// Default replay-guard retention: how long a consumed nonce stays in
/// the `used` set before pruning.
pub const DEFAULT_USED_RETENTION: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BootstrapError {
    #[error("bootstrap nonce not found")]
    NonceNotFound,
    #[error("bootstrap nonce already consumed (replay)")]
    NonceReplay,
    #[error("bootstrap nonce expired")]
    NonceExpired,
}

struct PendingNonce {
    issued_at: Instant,
}

/// Thread-safe issuer + consumer of one-shot bootstrap nonces.
pub struct BootstrapNonceManager {
    pending: DashMap<String, PendingNonce>,
    used: DashMap<String, Instant>,
    ttl: Duration,
    used_retention: Duration,
}

impl Default for BootstrapNonceManager {
    fn default() -> Self {
        Self::new(DEFAULT_NONCE_TTL, DEFAULT_USED_RETENTION)
    }
}

impl BootstrapNonceManager {
    pub fn new(ttl: Duration, used_retention: Duration) -> Self {
        Self {
            pending: DashMap::new(),
            used: DashMap::new(),
            ttl,
            used_retention,
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Issue a new nonce. Returns `(nonce, expires_in_secs)`.
    pub fn issue(&self) -> (String, u64) {
        self.prune();
        let nonce = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        self.pending
            .insert(nonce.clone(), PendingNonce { issued_at: Instant::now() });
        (nonce, self.ttl.as_secs())
    }

    /// Consume a nonce. Returns `Ok(())` on success and atomically moves
    /// the nonce from `pending` to `used`; further attempts fail with
    /// `NonceReplay`.
    pub fn consume(&self, nonce: &str) -> Result<(), BootstrapError> {
        self.prune();
        if self.used.contains_key(nonce) {
            return Err(BootstrapError::NonceReplay);
        }
        let entry = self
            .pending
            .remove(nonce)
            .ok_or(BootstrapError::NonceNotFound)?;
        if entry.1.issued_at.elapsed() > self.ttl {
            return Err(BootstrapError::NonceExpired);
        }
        self.used.insert(nonce.to_string(), Instant::now());
        Ok(())
    }

    fn prune(&self) {
        self.pending
            .retain(|_, p| p.issued_at.elapsed() <= self.ttl);
        self.used
            .retain(|_, used_at| used_at.elapsed() <= self.used_retention);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issued_nonce_consumes_once() {
        let mgr = BootstrapNonceManager::default();
        let (n, ttl) = mgr.issue();
        assert!(n.len() >= 32, "expected long nonce");
        assert_eq!(ttl, 60);
        mgr.consume(&n).unwrap();
    }

    #[test]
    fn second_consume_is_replay() {
        let mgr = BootstrapNonceManager::default();
        let (n, _) = mgr.issue();
        mgr.consume(&n).unwrap();
        assert_eq!(mgr.consume(&n), Err(BootstrapError::NonceReplay));
    }

    #[test]
    fn unknown_nonce_is_not_found() {
        let mgr = BootstrapNonceManager::default();
        assert_eq!(
            mgr.consume("never-issued"),
            Err(BootstrapError::NonceNotFound)
        );
    }

    #[test]
    fn expired_nonce_is_rejected() {
        let mgr = BootstrapNonceManager::new(
            Duration::from_millis(10),
            Duration::from_secs(60),
        );
        let (n, _) = mgr.issue();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(mgr.consume(&n), Err(BootstrapError::NonceExpired));
    }

    #[test]
    fn nonces_are_unique() {
        let mgr = BootstrapNonceManager::default();
        let (a, _) = mgr.issue();
        let (b, _) = mgr.issue();
        assert_ne!(a, b);
    }
}
```

- [ ] **Step 2: Run test to verify the file does not yet compile**

Run: `cargo test -p alephcore --lib gateway::bootstrap`
Expected: FAIL — module not declared in `gateway/mod.rs`.

- [ ] **Step 3: Wire module into `gateway/mod.rs`**

Edit `src/gateway/mod.rs` — find the existing list of `pub mod xxx;` (one is `pub mod challenge;`) and add alphabetically:

```rust
pub mod bootstrap;
```

- [ ] **Step 4: Run tests again**

Run: `cargo test -p alephcore --lib gateway::bootstrap`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/bootstrap.rs src/gateway/mod.rs
git commit -m "gateway: BootstrapNonceManager — one-shot loopback handoff nonces"
```

---

## Task 2: Plumb `BootstrapNonceManager` through config + `AuthContext` + `AuthState`

**Files:**
- Modify: `src/gateway/config.rs`
- Modify: `src/gateway/handlers/auth/mod.rs:AuthContext` struct
- Modify: `src/gateway/auth_middleware.rs:AuthState` struct
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs:166-186` (AuthContext construction site)
- Modify: wherever `AuthState` is constructed (grep for it)

- [ ] **Step 1: Add `BootstrapConfig` to gateway config**

In `src/gateway/config.rs`, add (place next to the other small `*Config` structs — `ChallengeConfig` or `TransportPolicy` are natural neighbors):

```rust
/// Bootstrap nonce knobs for the loopback cookie-handoff endpoint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct BootstrapConfig {
    /// One-shot nonce TTL in seconds. Default 60.
    pub nonce_ttl_secs: u64,
    /// How long consumed nonces are remembered for replay protection.
    /// Default 300 (5 minutes).
    pub used_retention_secs: u64,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            nonce_ttl_secs: 60,
            used_retention_secs: 300,
        }
    }
}
```

Add field to `GatewayConfig` (or whatever top-level `[gateway]` struct is — grep for `ChallengeConfig` to find the right struct):

```rust
    #[serde(default)]
    pub bootstrap: BootstrapConfig,
```

- [ ] **Step 2: Test that config parses with and without the new block**

Locate the existing config-roundtrip tests in `src/gateway/config.rs` (search `#[test].*config`); add:

```rust
#[test]
fn bootstrap_block_defaults() {
    let toml = "[gateway]\nbind = \"127.0.0.1\"\nport = 0\n";
    let cfg: AppConfig = toml::from_str(toml).expect("parse");
    assert_eq!(cfg.gateway.bootstrap.nonce_ttl_secs, 60);
    assert_eq!(cfg.gateway.bootstrap.used_retention_secs, 300);
}

#[test]
fn bootstrap_block_overrides() {
    let toml = "[gateway]\nbind = \"127.0.0.1\"\nport = 0\n\n[gateway.bootstrap]\nnonce_ttl_secs = 30\nused_retention_secs = 600\n";
    let cfg: AppConfig = toml::from_str(toml).expect("parse");
    assert_eq!(cfg.gateway.bootstrap.nonce_ttl_secs, 30);
    assert_eq!(cfg.gateway.bootstrap.used_retention_secs, 600);
}
```

Run: `cargo test -p alephcore config -- bootstrap`
Expected: PASS.

- [ ] **Step 3: Add `bootstrap_mgr` field to `AuthContext`**

In `src/gateway/handlers/auth/mod.rs`, locate `pub struct AuthContext { … }` (search for `pub struct AuthContext`). Add:

```rust
    pub bootstrap_mgr: Arc<crate::gateway::bootstrap::BootstrapNonceManager>,
```

This breaks every `AuthContext { … }` literal in the codebase — they all need the new field. Use `cargo check -p alephcore` to enumerate sites; the canonical construction site is `src/bin/aleph-server/commands/start/builder/subsystems.rs:166-186`. Test fixtures in `tests/` and within `#[cfg(test)] mod`s also need updating — use `..Default::default()`-style or explicit `bootstrap_mgr: Arc::new(BootstrapNonceManager::default())`.

- [ ] **Step 4: Add `bootstrap_mgr` to `AuthState`**

In `src/gateway/auth_middleware.rs:21-26`, change:

```rust
pub struct AuthState {
    pub shared_token_mgr: Arc<SharedTokenManager>,
    pub session_mgr: Arc<HttpSessionManager>,
    pub auth_mode: AuthMode,
    pub bootstrap_mgr: Arc<crate::gateway::bootstrap::BootstrapNonceManager>,
}
```

Find every `AuthState { … }` literal (`grep -rn "AuthState\s*{" src/`) and supply the new field.

- [ ] **Step 5: Construct the manager in `subsystems.rs`**

In `src/bin/aleph-server/commands/start/builder/subsystems.rs`, just before the `AuthContext { … }` literal (around line 166), add:

```rust
    let bootstrap_mgr = Arc::new(
        alephcore::gateway::bootstrap::BootstrapNonceManager::new(
            std::time::Duration::from_secs(app_config.gateway.bootstrap.nonce_ttl_secs),
            std::time::Duration::from_secs(app_config.gateway.bootstrap.used_retention_secs),
        ),
    );
```

(`app_config` is the variable name in this scope — verify by reading nearby lines; rename to whatever the local binding is called.)

Then add to the literal:

```rust
        bootstrap_mgr: bootstrap_mgr.clone(),
```

Pass the same `Arc` into wherever `AuthState` is constructed.

- [ ] **Step 6: Compile + run all auth-touching tests**

Run: `cargo test -p alephcore --lib gateway:: -- --skip integration`
Expected: PASS — config, auth, bootstrap tests green.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/config.rs src/gateway/handlers/auth/mod.rs \
        src/gateway/auth_middleware.rs \
        src/bin/aleph-server/commands/start/builder/subsystems.rs
git commit -m "gateway: plumb BootstrapNonceManager through AuthContext + AuthState"
```

---

## Task 3: `gateway.bootstrap.issue` RPC handler

**Files:**
- Create: `src/gateway/handlers/auth/bootstrap.rs`
- Modify: `src/gateway/handlers/auth/mod.rs` (add submodule + re-export)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/auth.rs:60` (register handler)

- [ ] **Step 1: Write failing handler test**

Create `src/gateway/handlers/auth/bootstrap.rs`:

```rust
//! `gateway.bootstrap.issue` — issue a one-time nonce that a local browser
//! can exchange for an `aleph_session` cookie via `GET /auth/bootstrap?nonce=…`.
//!
//! Auth: requires a previously-authenticated context (bearer / session /
//! existing device token). Anonymous callers receive `-32001 unauthorized`.

use std::sync::Arc;

use serde::Serialize;

use super::AuthContext;
use crate::gateway::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};

#[derive(Serialize)]
pub struct BootstrapIssueResult {
    pub nonce: String,
    pub expires_in_secs: u64,
    /// The full URL a client (CLI / Tauri menu) should open in the
    /// browser. Composed server-side so callers don't have to know the
    /// bind address.
    pub url: String,
}

pub async fn handle_gateway_bootstrap_issue(
    req: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    // Phase 2 placeholder: re-use the request's authentication context.
    // The gateway dispatcher only invokes us for authenticated callers
    // (bearer / session / valid device-token connection), so reaching
    // this handler is itself proof of auth. If you add an anonymous
    // dispatch path later, add an explicit `ctx.require_authenticated(&req)?`
    // check here.

    let (nonce, expires_in_secs) = ctx.bootstrap_mgr.issue();
    let url = format!(
        "http://{bind}/auth/bootstrap?nonce={nonce}",
        bind = ctx.public_bind_for_loopback(),
    );
    JsonRpcResponse::success(
        req.id,
        serde_json::to_value(BootstrapIssueResult {
            nonce,
            expires_in_secs,
            url,
        })
        .expect("serialize"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::bootstrap::BootstrapNonceManager;

    fn test_ctx() -> Arc<AuthContext> {
        // Minimal context for handler test. Copy the pattern from
        // existing handler tests in src/gateway/handlers/auth/connect.rs
        // — they all use a `make_test_ctx()` helper. Reuse it, then
        // override bootstrap_mgr.
        let mut ctx = crate::gateway::handlers::auth::connect::tests::make_test_ctx();
        let inner = Arc::get_mut(&mut ctx).expect("unique ctx");
        inner.bootstrap_mgr = Arc::new(BootstrapNonceManager::default());
        ctx
    }

    #[tokio::test]
    async fn issues_nonce_and_url() {
        let ctx = test_ctx();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            method: "gateway.bootstrap.issue".into(),
            params: None,
        };
        let resp = handle_gateway_bootstrap_issue(req, ctx).await;
        let result = resp.result.expect("result");
        let issued: BootstrapIssueResult = serde_json::from_value(result).unwrap();
        assert!(issued.nonce.len() >= 32);
        assert_eq!(issued.expires_in_secs, 60);
        assert!(issued.url.starts_with("http://"));
        assert!(issued.url.contains("/auth/bootstrap?nonce="));
    }
}
```

- [ ] **Step 2: Add helper `AuthContext::public_bind_for_loopback`**

In `src/gateway/handlers/auth/mod.rs`, add to the `impl AuthContext` block:

```rust
    /// Bind address for loopback URLs (used by bootstrap issue handler).
    /// Always returns `127.0.0.1:<port>` regardless of the configured
    /// bind address — the issued URL is only valid loopback-locally.
    pub fn public_bind_for_loopback(&self) -> String {
        format!("127.0.0.1:{}", self.bind_port)
    }
```

…and add a `bind_port: u16` field if not already present (verify by reading the struct; if absent, accept it in the constructor and plumb from `subsystems.rs`).

- [ ] **Step 3: Run test, verify fails**

Run: `cargo test -p alephcore gateway::handlers::auth::bootstrap`
Expected: FAIL (helper `make_test_ctx` may need a small tweak; if `connect.rs::tests::make_test_ctx` is not pub, expose it as `pub(crate)`).

- [ ] **Step 4: Iterate until test passes**

Resolve any test-helper visibility issues. Run again:

Run: `cargo test -p alephcore gateway::handlers::auth::bootstrap`
Expected: PASS.

- [ ] **Step 5: Wire submodule + register RPC**

In `src/gateway/handlers/auth/mod.rs` add:

```rust
pub mod bootstrap;
pub use bootstrap::handle_gateway_bootstrap_issue;
```

In `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` after the `auth.list_sessions` registration (around line 65), append:

```rust
    register_handler!(
        server,
        "gateway.bootstrap.issue",
        auth_handlers::handle_gateway_bootstrap_issue,
        auth_ctx
    );
```

- [ ] **Step 6: Compile + targeted test**

Run: `cargo check -p alephcore && cargo test -p alephcore gateway::handlers::auth`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/auth/bootstrap.rs src/gateway/handlers/auth/mod.rs \
        src/bin/aleph-server/commands/start/builder/handlers/auth.rs
git commit -m "gateway: gateway.bootstrap.issue RPC (auth-gated, returns URL)"
```

---

## Task 4: `GET /auth/bootstrap?nonce=…` HTTP route with loopback hard-gate

**Files:**
- Modify: `src/gateway/auth_middleware.rs` (extend `auth_routes` + new handler)
- Modify: `src/gateway/server/mod.rs` (verify `ConnectInfo<SocketAddr>` propagation — already in place; just confirm)

- [ ] **Step 1: Write the failing test — loopback-gate refuses non-loopback peers**

Create `tests/bootstrap_loopback_gate.rs`:

```rust
//! Loopback hard-gate for `/auth/bootstrap?nonce=…`.
//!
//! `axum::extract::ConnectInfo<SocketAddr>` reports the real peer address;
//! the handler must refuse anything that is not 127.0.0.0/8 or ::1, even
//! if the nonce is otherwise valid.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use alephcore::gateway::auth_middleware::is_loopback_peer;

#[test]
fn loopback_v4_is_accepted() {
    assert!(is_loopback_peer(&SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        12345
    )));
    assert!(is_loopback_peer(&SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 99)),
        12345
    )));
}

#[test]
fn loopback_v6_is_accepted() {
    assert!(is_loopback_peer(&SocketAddr::new(
        IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        12345
    )));
}

#[test]
fn lan_address_is_refused() {
    assert!(!is_loopback_peer(&SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
        12345
    )));
}

#[test]
fn public_address_is_refused() {
    assert!(!is_loopback_peer(&SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
        443
    )));
}
```

- [ ] **Step 2: Run test — fails (helper not yet pub)**

Run: `cargo test --test bootstrap_loopback_gate`
Expected: FAIL — `is_loopback_peer` undefined.

- [ ] **Step 3: Implement the loopback helper + consume handler**

In `src/gateway/auth_middleware.rs`, add:

```rust
use std::net::SocketAddr;
use axum::extract::{ConnectInfo, Query};

/// True when the peer is on a loopback interface (127.0.0.0/8 or ::1).
/// Used by the bootstrap-consume endpoint to refuse non-local peers
/// regardless of the `Origin` header.
pub fn is_loopback_peer(addr: &SocketAddr) -> bool {
    match addr.ip() {
        std::net::IpAddr::V4(v4) => v4.is_loopback(),
        std::net::IpAddr::V6(v6) => v6.is_loopback(),
    }
}

#[derive(Deserialize)]
struct BootstrapQuery {
    nonce: String,
}

async fn handle_bootstrap_consume(
    State(state): State<Arc<AuthState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(q): Query<BootstrapQuery>,
) -> Response {
    if !is_loopback_peer(&peer) {
        tracing::warn!(
            peer = %peer,
            "rejected non-loopback /auth/bootstrap request"
        );
        return (StatusCode::FORBIDDEN, "loopback only").into_response();
    }
    if let Err(e) = state.bootstrap_mgr.consume(&q.nonce) {
        tracing::info!(error = %e, "bootstrap nonce rejected");
        return (StatusCode::UNAUTHORIZED, "invalid or expired nonce").into_response();
    }
    // Fabricate a fresh session keyed by the shared-token HMAC, mirroring
    // handle_login at line 53-69.
    let token = match state.shared_token_mgr.current_token() {
        Some(t) => t,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "no shared token").into_response(),
    };
    let hash = crate::gateway::security::hmac_sign(state.shared_token_mgr.secret(), &token);
    let session_id = match state.session_mgr.create_session(&hash) {
        Ok(id) => id,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "session creation failed").into_response(),
    };
    let max_age = state.session_mgr.expiry_hours() * 3600;
    let cookie = format!(
        "aleph_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        session_id, max_age,
    );
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/".to_string()),
            (header::SET_COOKIE, cookie),
        ],
    )
        .into_response()
}
```

Update `auth_routes` (line 34) to include the new route:

```rust
pub fn auth_routes(state: Arc<AuthState>) -> Router {
    Router::new()
        .route("/login", get(show_login))
        .route("/auth/login", post(handle_login))
        .route("/auth/logout", post(handle_logout))
        .route("/auth/bootstrap", get(handle_bootstrap_consume))
        .with_state(state)
}
```

- [ ] **Step 4: Add `SharedTokenManager::current_token` accessor (if not present)**

In `src/gateway/security/shared_token.rs` add:

```rust
    /// Currently active plaintext token (None if uninitialized).
    pub fn current_token(&self) -> Option<String> {
        self.current_token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
```

- [ ] **Step 5: Run loopback-gate test**

Run: `cargo test --test bootstrap_loopback_gate`
Expected: PASS — 4 tests.

- [ ] **Step 6: Write integration test — full consume flow**

Add to the bottom of `tests/bootstrap_loopback_gate.rs`:

```rust
// Full-flow test: issue via manager, simulate /auth/bootstrap, check cookie set.
// Uses tower::ServiceExt::oneshot — see auth_probe_tests.rs for the pattern.
//
// See `src/gateway/auth_probe_tests.rs` for the existing axum-test harness
// pattern this builds on.
#[tokio::test]
async fn consume_sets_cookie_on_loopback() {
    use alephcore::gateway::auth_middleware::{auth_routes, AuthState};
    use alephcore::gateway::bootstrap::BootstrapNonceManager;
    use alephcore::gateway::config::AuthMode;
    use alephcore::gateway::security::{store::SecurityStore, SharedTokenManager};
    use alephcore::gateway::session::HttpSessionManager;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tower::ServiceExt;

    let dir = tempdir().unwrap();
    let store = Arc::new(SecurityStore::open(&dir.path().join("sec.db")).unwrap());
    let shared = Arc::new(SharedTokenManager::new(store.clone(), dir.path().join("vault")));
    let _ = shared.generate_token().unwrap();
    let session_mgr = Arc::new(HttpSessionManager::new(store, 24));
    let bootstrap = Arc::new(BootstrapNonceManager::default());
    let state = Arc::new(AuthState {
        shared_token_mgr: shared,
        session_mgr,
        auth_mode: AuthMode::Token,
        bootstrap_mgr: bootstrap.clone(),
    });

    let (nonce, _) = bootstrap.issue();
    let app = auth_routes(state).into_make_service_with_connect_info::<SocketAddr>();
    let mut svc = app.into_service();
    let req = Request::builder()
        .method("GET")
        .uri(format!("/auth/bootstrap?nonce={nonce}"))
        .extension(axum::extract::ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            54321,
        )))
        .body(Body::empty())
        .unwrap();
    let resp = svc.ready().await.unwrap().call(req).await.unwrap();
    assert_eq!(resp.status(), 303);
    assert!(resp.headers().get("set-cookie").is_some());
}
```

(If the `into_make_service_with_connect_info` + manual `ConnectInfo` extension pattern is awkward in your tower version, fall back to spinning up a real `TcpListener` on `127.0.0.1:0` and using `reqwest` — there's already an `axum-test` style helper in `src/gateway/auth_probe_tests.rs`, mimic it.)

- [ ] **Step 7: Run integration test**

Run: `cargo test --test bootstrap_loopback_gate`
Expected: PASS — 5 tests.

- [ ] **Step 8: Commit**

```bash
git add src/gateway/auth_middleware.rs src/gateway/security/shared_token.rs \
        tests/bootstrap_loopback_gate.rs
git commit -m "gateway: /auth/bootstrap?nonce= sets cookie on loopback peers"
```

---

## Task 5: `aleph open` CLI subcommand

**Files:**
- Modify: `interfaces/cli/src/commands/cli_args.rs` (Commands enum — verify path with `grep -n "enum Commands" interfaces/cli/src/`)
- Create: `interfaces/cli/src/commands/open_cmd.rs`
- Modify: `interfaces/cli/src/main.rs` (dispatch + import)

- [ ] **Step 1: Write failing CLI parse test**

In `interfaces/cli/src/main.rs` (the existing `#[cfg(test)] mod tests` block — search line 1039 vicinity), add:

```rust
#[test]
fn parses_open_subcommand() {
    assert!(Cli::try_parse_from(["aleph", "open"]).is_ok());
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p aleph-cli --bins parses_open_subcommand`
Expected: FAIL — `Open` variant unknown.

- [ ] **Step 3: Add `Open` variant**

In the file holding `pub enum Commands { … }` (Phase 1's exploration showed it's `interfaces/cli/src/commands/cli_args.rs`), append:

```rust
    /// Open the Aleph Panel in the system browser, auto-authenticated via a
    /// one-time bootstrap nonce. Same UX as the desktop app's "Open in Browser"
    /// menu item — no token typing.
    Open,
```

- [ ] **Step 4: Implement the handler**

Create `interfaces/cli/src/commands/open_cmd.rs`:

```rust
//! `aleph open` — issue a bootstrap nonce and open the system browser.
//!
//! Mirrors the desktop app's "Open in Browser" menu. Requires the daemon
//! to be running.

use serde::Deserialize;
use serde_json::Value;

use crate::error::CliResult;
use crate::client::RpcClient;

#[derive(Deserialize)]
struct BootstrapIssueResult {
    url: String,
    expires_in_secs: u64,
}

pub async fn open_panel(server_url: &str, json: bool) -> CliResult<()> {
    let client = RpcClient::connect(server_url).await?;
    let raw: Value = client.call("gateway.bootstrap.issue", None::<()>).await?;
    let issued: BootstrapIssueResult = serde_json::from_value(raw)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "url": issued.url,
            "expires_in_secs": issued.expires_in_secs,
        }))?);
        return Ok(());
    }

    println!(
        "Opening {} (valid for {}s)",
        issued.url, issued.expires_in_secs
    );

    // Cross-platform `open` — use the `opener` crate if a dependency, else
    // fall back to platform-specific commands.
    open_url(&issued.url)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> CliResult<()> {
    std::process::Command::new("open").arg(url).status()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_url(url: &str) -> CliResult<()> {
    std::process::Command::new("xdg-open").arg(url).status()?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> CliResult<()> {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_result_roundtrip() {
        let json = serde_json::json!({
            "nonce": "abc",
            "url": "http://127.0.0.1:18790/auth/bootstrap?nonce=abc",
            "expires_in_secs": 60u64,
        });
        let parsed: BootstrapIssueResult = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.expires_in_secs, 60);
    }
}
```

(If `opener` crate is preferred over manual `Command::new`, check `interfaces/cli/Cargo.toml`; the manual fallback above is fine and zero-dep.)

- [ ] **Step 5: Wire dispatch in `main.rs`**

In `interfaces/cli/src/main.rs`, find the `match` dispatching `Commands::*` and add:

```rust
        Commands::Open => open_cmd::open_panel(server_url, json).await,
```

Add the import at the top:

```rust
use crate::commands::open_cmd;
```

And ensure `interfaces/cli/src/commands/mod.rs` declares the new submodule:

```rust
pub mod open_cmd;
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p aleph-cli`
Expected: PASS (parse test + roundtrip test).

- [ ] **Step 7: Commit**

```bash
git add interfaces/cli/src/main.rs \
        interfaces/cli/src/commands/cli_args.rs \
        interfaces/cli/src/commands/mod.rs \
        interfaces/cli/src/commands/open_cmd.rs
git commit -m "cli: aleph open — issue bootstrap nonce, launch system browser"
```

---

## Task 6: Tauri "Open in Browser" menu item

**Files:**
- Modify: `desktop/shell/src/menu.rs:12-15` (add const) and `:85-95` (add handler)
- Modify: `desktop/shell/src/daemon.rs` (new helper `open_in_system_browser`)

- [ ] **Step 1: Add menu constant + item**

In `desktop/shell/src/menu.rs:12-15`, add:

```rust
const ID_OPEN_BROWSER: &str = "menu_open_browser";
```

In the `app_menu` `Submenu::with_items` (around line 19-50), insert after `ID_SHOW`:

```rust
            &MenuItem::with_id(app, ID_OPEN_BROWSER, "Open in Browser", true, None::<&str>)?,
```

- [ ] **Step 2: Implement `open_in_system_browser` in `daemon.rs`**

Add to `desktop/shell/src/daemon.rs`:

```rust
/// Issue a bootstrap nonce via the daemon's HTTP route and open the
/// resulting URL in the system browser. Best-effort — failures are
/// logged; user-visible feedback is a no-op.
pub(crate) async fn open_in_system_browser() {
    let Some(token) = load_bootstrap_token() else {
        tracing::warn!("cannot open browser: no bootstrap token available");
        return;
    };
    let url = match issue_nonce_url(&token).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("failed to issue bootstrap nonce: {e}");
            return;
        }
    };
    if let Err(e) = opener::open(&url) {
        tracing::warn!("failed to open browser: {e}");
    }
}

async fn issue_nonce_url(token: &str) -> Result<String, String> {
    // Tiny HTTP client. Reuse whichever client is already in shell deps.
    // If `reqwest` not present, use `ureq` (blocking) wrapped in
    // `tokio::task::spawn_blocking`. For this plan we assume `reqwest`
    // is added as a workspace dep — verify with `grep '^reqwest' desktop/shell/Cargo.toml`.
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "gateway.bootstrap.issue",
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{DAEMON_HOST}:{DAEMON_PORT}/rpc"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    v["result"]["url"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("malformed response: {v}"))
}
```

If `opener` and `reqwest` aren't deps yet, add to `desktop/shell/Cargo.toml`:

```toml
opener = "0.7"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

- [ ] **Step 3: Wire menu handler**

In `desktop/shell/src/menu.rs:85-95`, extend `on_event`:

```rust
        ID_OPEN_BROWSER => {
            let h = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = &h;
                crate::daemon::open_in_system_browser().await;
            });
        }
```

- [ ] **Step 4: Compile**

Run: `cargo check -p aleph-desktop-shell`
Expected: PASS.

- [ ] **Step 5: Manual verification**

```bash
just shell-dev
# In the macOS menu bar: Aleph → Open in Browser.
# Expected: Default browser opens, lands on the Panel signed in,
# address bar shows `http://127.0.0.1:18790/` (no token, no nonce).
```

- [ ] **Step 6: Commit**

```bash
git add desktop/shell/src/menu.rs desktop/shell/src/daemon.rs \
        desktop/shell/Cargo.toml
git commit -m "shell: macOS menu — Open in Browser via bootstrap nonce"
```

---

## Task 7: Migrate shell Phase-1 `?token=` to `/auth/bootstrap?nonce=`

**Files:**
- Modify: `desktop/shell/src/daemon.rs:build_panel_url` (the Phase 1 helper)
- Modify: `desktop/shell/src/main.rs:reveal_panel` (Phase 1 caller)

- [ ] **Step 1: Update `build_panel_url` to prefer nonce**

Change Phase 1's helper signature:

```rust
/// Build the navigation URL for the embedded Panel.
///
/// `bootstrap_url` (if `Some`) is the full server-issued `/auth/bootstrap?nonce=…`
/// URL — when present, we navigate there directly so the daemon sets the
/// session cookie, and the Panel never sees a token in any URL parameter.
///
/// `legacy_token` is the Phase 1 fallback path used when the daemon does not
/// yet expose `gateway.bootstrap.issue` (e.g., old binary, dev rolling
/// upgrade). Removed in Phase 4 once Phase 2 has shipped a release cycle.
pub(crate) fn build_panel_url(
    bootstrap_url: Option<&str>,
    legacy_token: Option<&str>,
) -> Result<Url, url::ParseError> {
    if let Some(u) = bootstrap_url {
        return Url::parse(u);
    }
    let mut url: Url = super::PANEL_URL.parse()?;
    if let Some(t) = legacy_token {
        url.query_pairs_mut().append_pair("token", t);
    }
    Ok(url)
}
```

Update the Phase 1 unit tests accordingly to call `build_panel_url(None, Some("aleph-..."))` and add a new test:

```rust
#[test]
fn build_panel_url_prefers_bootstrap_when_present() {
    let url = build_panel_url(
        Some("http://127.0.0.1:18790/auth/bootstrap?nonce=abc"),
        Some("aleph-deadbeef"),
    )
    .unwrap();
    assert_eq!(url.path(), "/auth/bootstrap");
    assert!(url.query().unwrap().contains("nonce=abc"));
    assert!(!url.query().unwrap().contains("token="));
}
```

- [ ] **Step 2: Update `reveal_panel` in `main.rs`**

Phase 1's `reveal_panel`:

```rust
fn reveal_panel(handle: &tauri::AppHandle) {
    let token = daemon::load_bootstrap_token();
    daemon::navigate_to_panel(handle, token.as_deref());
    focus_window(handle);
}
```

becomes:

```rust
fn reveal_panel(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let token = daemon::load_bootstrap_token();
        let bootstrap_url = match token.as_deref() {
            Some(t) => daemon::issue_nonce_url(t).await.ok(),
            None => None,
        };
        daemon::navigate_to_panel(&handle, bootstrap_url.as_deref(), token.as_deref());
        focus_window(&handle);
    });
}
```

Make `issue_nonce_url` `pub(crate)` if it isn't already, and update `navigate_to_panel`'s signature:

```rust
pub(crate) fn navigate_to_panel(
    handle: &tauri::AppHandle,
    bootstrap_url: Option<&str>,
    legacy_token: Option<&str>,
) { /* uses build_panel_url(bootstrap_url, legacy_token) */ }
```

Update every caller (recovery reload + tray) to pass `(None, None)`.

- [ ] **Step 3: Run shell tests**

Run: `cargo test -p aleph-desktop-shell`
Expected: PASS (existing + 1 new).

- [ ] **Step 4: Manual verification — daemon Console output**

```bash
just shell-dev
# Tail daemon log: tail -f ~/.aleph/data/aleph.log (or wherever info! is going)
# Expected: a "bootstrap nonce consumed" info line at boot, NO "?token=" leaking
# in any URL.
```

- [ ] **Step 5: Commit**

```bash
git add desktop/shell/src/daemon.rs desktop/shell/src/main.rs
git commit -m "shell: migrate first-reveal to /auth/bootstrap?nonce= (Phase 2)"
```

---

## Self-Review Checklist

1. **Goal achieved?** Shell-launched Panel auths via a one-shot nonce, never via plaintext token in URL. CLI `aleph open` + Tauri menu give 1-click browser auth on same machine. ✓
2. **Loopback gate enforced?** `is_loopback_peer` checked in `handle_bootstrap_consume`; non-loopback peers get 403. Test `lan_address_is_refused` + `public_address_is_refused` cover it. ✓
3. **Replay defended?** `BootstrapNonceManager.used` map + 5-min retention; test `second_consume_is_replay` covers. ✓
4. **Expiry defended?** TTL 60s, `nonce_expired` test covers. ✓
5. **Auth required to issue?** `gateway.bootstrap.issue` only reachable from authenticated dispatch contexts (gateway dispatcher enforces). The `bearer_auth` middleware blocks anon HTTP calls. ✓
6. **Backwards compatibility?** Phase 1's `?token=` fallback retained in `build_panel_url`. Old shell binaries against new daemon: still work via `?token=`. New shell against old daemon: nonce-issue fails → falls back to `?token=`. ✓
7. **Tests cover the new code?** 5 unit tests (manager) + 1 handler test + 4 loopback tests + 1 full-flow integration test + 1 URL builder test + 1 CLI parse test = 13 new tests. ✓
8. **No new big deps?** `reqwest` already in workspace (verify). `opener` is a 0-bloat single-purpose crate; alternative is shell-out which adds zero deps. ✓

---

## Verification Commands (Definition of Done)

```bash
# 1. New unit + integration tests
cargo test -p alephcore --lib gateway::bootstrap
cargo test -p alephcore gateway::handlers::auth::bootstrap
cargo test --test bootstrap_loopback_gate
cargo test -p aleph-cli parses_open_subcommand
cargo test -p aleph-desktop-shell daemon::tests

# 2. Compile + lint
cargo check -p alephcore
cargo check -p aleph-desktop-shell
cargo check -p aleph-cli
cargo clippy -p alephcore --lib -- -D warnings

# 3. Regression
cargo test -p alephcore --lib gateway::

# 4. Manual smoke
just shell-dev   # → menu → Open in Browser → Panel shows in browser, signed in
./target/debug/aleph open   # → opens system browser, Panel signed in
```

---

## Risk Notes

- **Tauri `tauri-plugin-shell` vs `opener` crate:** Tauri's shell plugin requires explicit allowlist in `tauri.conf.json` `permissions`. Easier to use `opener` crate directly — it bypasses the allowlist check. Verify with `grep shell desktop/shell/tauri.conf.json` whether the plugin is mounted; if so, use it. Either path works.
- **`current_token` accessor:** Adding this method to `SharedTokenManager` exposes the plaintext token to callers who already hold a `SharedTokenManager` reference (just `auth_middleware` in practice). Audit the call site is single-use; do not export as `pub` in `lib.rs` re-exports beyond what's needed for `auth_middleware`. Mark `pub(crate)` if it suffices.
- **`/rpc` endpoint:** Verify the exact HTTP path for JSON-RPC POST. The shell-side `issue_nonce_url` assumes `/rpc`. Grep `Router::new().*post.*rpc` in `src/gateway/server/`. Adjust if the path is `/v1/rpc` or `/json-rpc`.
- **`expires_in_secs` is u64 but cookie `Max-Age` accepts negative for delete:** No conflict — bootstrap cookie always uses positive `expiry_hours()` from session manager.
- **CSRF concerns on `GET /auth/bootstrap`:** Using GET is intentional — browsers can land here from a system-initiated `open` command, and POST-redirect from a launcher is awkward. The loopback gate + one-shot nonce together provide CSRF-equivalent protection (only the legit launcher process knows the nonce, and only same-machine processes can consume it).
- **Browser CONNECT preflight:** Standard `GET` redirects do not trigger CORS preflight. `SameSite=Strict` on the issued cookie ensures only same-site contexts can use it subsequently.
