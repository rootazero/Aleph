# Gateway Enhancement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance Aleph Gateway with connection lifecycle management, flow control & security, and API expansion — inspired by OpenClaw but tailored to Aleph's Rust/axum architecture.

**Architecture:** Incremental enhancement of existing axum-based Gateway. Three phases: P1 (connection lifecycle), P2 (flow control & safety), P3 (API surface). Each phase modifies `server.rs` + adds new focused modules. No breaking changes to existing RPC methods.

**Tech Stack:** Rust, tokio, axum 0.8, dashmap 5.5 (already in Cargo.toml), tokio-tungstenite, HMAC-SHA256 (existing TokenManager)

**Design Doc:** `docs/plans/2026-03-04-gateway-enhancement-design.md`

---

## Phase 1: Connection Lifecycle Enhancement

### Task 1: Presence Tracking Module

**Files:**
- Create: `core/src/gateway/presence.rs`
- Modify: `core/src/gateway/mod.rs` (add module declaration)
- Test: inline `#[cfg(test)] mod tests` in `presence.rs`

**Step 1: Write the failing test**

In `core/src/gateway/presence.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_presence() {
        let tracker = PresenceTracker::new();
        let entry = PresenceEntry {
            conn_id: "conn-1".to_string(),
            device_id: Some("device-1".to_string()),
            device_name: "Aleph macOS".to_string(),
            platform: "macos".to_string(),
            connected_at: chrono::Utc::now(),
            last_heartbeat: chrono::Utc::now(),
        };
        tracker.upsert("conn-1", entry.clone());
        let result = tracker.get("conn-1");
        assert!(result.is_some());
        assert_eq!(result.unwrap().device_id, Some("device-1".to_string()));
    }

    #[test]
    fn test_remove_presence() {
        let tracker = PresenceTracker::new();
        let entry = PresenceEntry {
            conn_id: "conn-1".to_string(),
            device_id: Some("device-1".to_string()),
            device_name: "Test".to_string(),
            platform: "linux".to_string(),
            connected_at: chrono::Utc::now(),
            last_heartbeat: chrono::Utc::now(),
        };
        tracker.upsert("conn-1", entry);
        let removed = tracker.remove("conn-1");
        assert!(removed.is_some());
        assert!(tracker.get("conn-1").is_none());
    }

    #[test]
    fn test_list_all_presence() {
        let tracker = PresenceTracker::new();
        for i in 0..3 {
            let entry = PresenceEntry {
                conn_id: format!("conn-{i}"),
                device_id: Some(format!("dev-{i}")),
                device_name: format!("Device {i}"),
                platform: "test".to_string(),
                connected_at: chrono::Utc::now(),
                last_heartbeat: chrono::Utc::now(),
            };
            tracker.upsert(&format!("conn-{i}"), entry);
        }
        assert_eq!(tracker.list().len(), 3);
    }

    #[test]
    fn test_update_heartbeat() {
        let tracker = PresenceTracker::new();
        let entry = PresenceEntry {
            conn_id: "conn-1".to_string(),
            device_id: None,
            device_name: "Test".to_string(),
            platform: "test".to_string(),
            connected_at: chrono::Utc::now(),
            last_heartbeat: chrono::Utc::now(),
        };
        tracker.upsert("conn-1", entry);
        let old_hb = tracker.get("conn-1").unwrap().last_heartbeat;
        std::thread::sleep(std::time::Duration::from_millis(10));
        tracker.update_heartbeat("conn-1");
        let new_hb = tracker.get("conn-1").unwrap().last_heartbeat;
        assert!(new_hb > old_hb);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::presence::tests -- --nocapture`
Expected: FAIL — module `presence` not found

**Step 3: Write minimal implementation**

In `core/src/gateway/presence.rs`:

```rust
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Serialize;

/// Tracks connected client presence for multi-device awareness.
#[derive(Clone)]
pub struct PresenceTracker {
    entries: DashMap<String, PresenceEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PresenceEntry {
    pub conn_id: String,
    pub device_id: Option<String>,
    pub device_name: String,
    pub platform: String,
    pub connected_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}

impl PresenceTracker {
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    pub fn upsert(&self, conn_id: &str, entry: PresenceEntry) {
        self.entries.insert(conn_id.to_string(), entry);
    }

    pub fn remove(&self, conn_id: &str) -> Option<PresenceEntry> {
        self.entries.remove(conn_id).map(|(_, v)| v)
    }

    pub fn get(&self, conn_id: &str) -> Option<PresenceEntry> {
        self.entries.get(conn_id).map(|e| e.value().clone())
    }

    pub fn list(&self) -> Vec<PresenceEntry> {
        self.entries.iter().map(|e| e.value().clone()).collect()
    }

    pub fn update_heartbeat(&self, conn_id: &str) {
        if let Some(mut entry) = self.entries.get_mut(conn_id) {
            entry.last_heartbeat = Utc::now();
        }
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }
}
```

Add to `core/src/gateway/mod.rs` (after other module declarations):
```rust
pub mod presence;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::presence::tests -- --nocapture`
Expected: 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/gateway/presence.rs core/src/gateway/mod.rs
git commit -m "gateway: add PresenceTracker module"
```

---

### Task 2: State Version Tracker

**Files:**
- Create: `core/src/gateway/state_version.rs`
- Modify: `core/src/gateway/mod.rs` (add module declaration)
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing test**

In `core/src/gateway/state_version.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_versions_are_zero() {
        let tracker = StateVersionTracker::new();
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.presence, 0);
        assert_eq!(snapshot.health, 0);
        assert_eq!(snapshot.config, 0);
    }

    #[test]
    fn test_bump_increments_version() {
        let tracker = StateVersionTracker::new();
        let v1 = tracker.bump_presence();
        assert_eq!(v1, 1);
        let v2 = tracker.bump_presence();
        assert_eq!(v2, 2);
        assert_eq!(tracker.snapshot().presence, 2);
    }

    #[test]
    fn test_independent_version_domains() {
        let tracker = StateVersionTracker::new();
        tracker.bump_presence();
        tracker.bump_presence();
        tracker.bump_health();
        tracker.bump_config();
        tracker.bump_config();
        tracker.bump_config();
        let snap = tracker.snapshot();
        assert_eq!(snap.presence, 2);
        assert_eq!(snap.health, 1);
        assert_eq!(snap.config, 3);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::state_version::tests -- --nocapture`
Expected: FAIL — module not found

**Step 3: Write minimal implementation**

```rust
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Tracks monotonic version numbers for state domains.
/// Clients use versions to skip redundant event processing.
pub struct StateVersionTracker {
    presence: AtomicU64,
    health: AtomicU64,
    config: AtomicU64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct StateVersion {
    pub presence: u64,
    pub health: u64,
    pub config: u64,
}

impl StateVersionTracker {
    pub fn new() -> Self {
        Self {
            presence: AtomicU64::new(0),
            health: AtomicU64::new(0),
            config: AtomicU64::new(0),
        }
    }

    pub fn bump_presence(&self) -> u64 {
        self.presence.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn bump_health(&self) -> u64 {
        self.health.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn bump_config(&self) -> u64 {
        self.config.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn snapshot(&self) -> StateVersion {
        StateVersion {
            presence: self.presence.load(Ordering::SeqCst),
            health: self.health.load(Ordering::SeqCst),
            config: self.config.load(Ordering::SeqCst),
        }
    }
}
```

Add to `core/src/gateway/mod.rs`:
```rust
pub mod state_version;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::state_version::tests -- --nocapture`
Expected: 3 tests PASS

**Step 5: Commit**

```bash
git add core/src/gateway/state_version.rs core/src/gateway/mod.rs
git commit -m "gateway: add StateVersionTracker module"
```

---

### Task 3: Hello Snapshot Data Structure

**Files:**
- Create: `core/src/gateway/hello_snapshot.rs`
- Modify: `core/src/gateway/mod.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_snapshot_serializes() {
        let snapshot = HelloSnapshot {
            server_id: "aleph-001".to_string(),
            uptime_ms: 12345,
            state_version: crate::gateway::state_version::StateVersion {
                presence: 0,
                health: 0,
                config: 0,
            },
            presence: vec![],
            limits: ConnectionLimits {
                max_connections: 1000,
                current_connections: 1,
            },
            capabilities: vec!["health".to_string(), "echo".to_string()],
            active_workspace: None,
        };
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["server_id"], "aleph-001");
        assert_eq!(json["uptime_ms"], 12345);
        assert!(json["capabilities"].is_array());
    }

    #[test]
    fn test_connection_limits_serializes() {
        let limits = ConnectionLimits {
            max_connections: 500,
            current_connections: 42,
        };
        let json = serde_json::to_value(&limits).unwrap();
        assert_eq!(json["max_connections"], 500);
        assert_eq!(json["current_connections"], 42);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::hello_snapshot::tests -- --nocapture`
Expected: FAIL

**Step 3: Write minimal implementation**

```rust
use serde::Serialize;

use super::presence::PresenceEntry;
use super::state_version::StateVersion;

/// State snapshot sent to clients upon successful authentication.
/// Eliminates the need for multiple follow-up RPC calls.
#[derive(Debug, Clone, Serialize)]
pub struct HelloSnapshot {
    pub server_id: String,
    pub uptime_ms: u64,
    pub state_version: StateVersion,
    pub presence: Vec<PresenceEntry>,
    pub limits: ConnectionLimits,
    pub capabilities: Vec<String>,
    pub active_workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionLimits {
    pub max_connections: u32,
    pub current_connections: u32,
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib gateway::hello_snapshot::tests -- --nocapture`
Expected: 2 tests PASS

**Step 5: Commit**

```bash
git add core/src/gateway/hello_snapshot.rs core/src/gateway/mod.rs
git commit -m "gateway: add HelloSnapshot data structure"
```

---

### Task 4: Integrate Presence + Hello Snapshot into Server

**Files:**
- Modify: `core/src/gateway/server.rs` (lines 34-74, 127-141, 227-244, 327-706)
- Test: manual WebSocket client test

**Step 1: Add PresenceTracker and StateVersionTracker to GatewayServer**

In `core/src/gateway/server.rs`, add fields to `GatewayServer` struct (after line 140):

```rust
pub presence: Arc<PresenceTracker>,
pub state_versions: Arc<StateVersionTracker>,
pub start_time: std::time::Instant,
```

Update `GatewayServer::new()` and `with_config()` to initialize these fields.

**Step 2: Track presence on connection/disconnection**

In `handle_connection()`:
- After successful authentication (where `state.authenticate()` is called), create a `PresenceEntry` and call `presence.upsert()`.
- Emit `presence.joined` event via `event_bus`.
- In the cleanup section (lines 657-705), call `presence.remove()` and emit `presence.left`.
- Bump `state_versions.bump_presence()` on join/leave.

**Step 3: Build HelloSnapshot on auth success**

After authentication succeeds, construct `HelloSnapshot`:

```rust
let hello = HelloSnapshot {
    server_id: server_id.clone(),
    uptime_ms: start_time.elapsed().as_millis() as u64,
    state_version: state_versions.snapshot(),
    presence: presence.list(),
    limits: ConnectionLimits {
        max_connections: config.max_connections,
        current_connections: connections.read().await.len() as u32,
    },
    capabilities: handlers.methods(),
    active_workspace: None, // filled from workspace manager if available
};
```

Include the snapshot in the `connect` response result field.

**Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles without errors

**Step 5: Commit**

```bash
git add core/src/gateway/server.rs
git commit -m "gateway: integrate presence tracking and hello snapshot"
```

---

### Task 5: Graceful Shutdown

**Files:**
- Modify: `core/src/gateway/server.rs`
- Test: unit test for shutdown event emission

**Step 1: Write shutdown broadcast logic**

Add a `shutdown` method to `GatewayServer`:

```rust
pub async fn graceful_shutdown(&self, reason: &str) {
    tracing::info!("Gateway graceful shutdown: {reason}");

    // 1. Broadcast shutdown event
    let event = TopicEvent::new(
        "system.shutdown",
        serde_json::json!({
            "reason": reason,
            "grace_period_ms": 5000
        }),
    );
    let _ = self.event_bus.publish(event);

    // 2. Wait grace period
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // 3. Clear all connections (they'll receive Close frame via drop)
    let mut conns = self.connections.write().await;
    let count = conns.len();
    conns.clear();
    tracing::info!("Closed {count} connections");
}
```

**Step 2: Hook into SIGTERM/ctrl-c**

In the server startup code, wrap the main serve future with a shutdown signal:

```rust
let shutdown_signal = async {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::terminate()
    ).expect("failed to install SIGTERM handler");

    tokio::select! {
        _ = ctrl_c => {},
        #[cfg(unix)]
        _ = sigterm.recv() => {},
    }
};
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles

**Step 4: Commit**

```bash
git add core/src/gateway/server.rs
git commit -m "gateway: add graceful shutdown with event broadcast"
```

---

## Phase 2: Flow Control & Security

### Task 6: Rate Limiter Module

**Files:**
- Create: `core/src/gateway/rate_limiter.rs`
- Modify: `core/src/gateway/mod.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn test_config() -> RateLimitConfig {
        RateLimitConfig {
            auth: WindowConfig { max_requests: 3, window_secs: 60, lockout_secs: Some(300) },
            rpc_default: WindowConfig { max_requests: 10, window_secs: 60, lockout_secs: None },
            rpc_write: WindowConfig { max_requests: 2, window_secs: 60, lockout_secs: None },
            rpc_heavy: WindowConfig { max_requests: 2, window_secs: 60, lockout_secs: None },
            exempt_loopback: true,
        }
    }

    #[test]
    fn test_allows_under_limit() {
        let limiter = RateLimiter::new(test_config());
        let key = RateLimitKey::new("192.168.1.1", RateLimitScope::RpcDefault);
        for _ in 0..10 {
            assert!(limiter.check_and_record(&key).is_ok());
        }
    }

    #[test]
    fn test_rejects_over_limit() {
        let limiter = RateLimiter::new(test_config());
        let key = RateLimitKey::new("192.168.1.1", RateLimitScope::RpcDefault);
        for _ in 0..10 {
            limiter.check_and_record(&key).unwrap();
        }
        let result = limiter.check_and_record(&key);
        assert!(result.is_err());
        match result.unwrap_err() {
            RateLimitError::Exceeded { retry_after_ms, .. } => {
                assert!(retry_after_ms > 0);
            }
            _ => panic!("expected Exceeded error"),
        }
    }

    #[test]
    fn test_auth_lockout() {
        let limiter = RateLimiter::new(test_config());
        let key = RateLimitKey::new("10.0.0.1", RateLimitScope::Auth);
        for _ in 0..3 {
            limiter.check_and_record(&key).unwrap();
        }
        match limiter.check_and_record(&key) {
            Err(RateLimitError::LockedOut { lockout_remaining_ms, .. }) => {
                assert!(lockout_remaining_ms > 0);
            }
            other => panic!("expected LockedOut, got {other:?}"),
        }
    }

    #[test]
    fn test_loopback_exempt() {
        let limiter = RateLimiter::new(test_config());
        let key = RateLimitKey::new("127.0.0.1", RateLimitScope::Auth);
        for _ in 0..100 {
            assert!(limiter.check_and_record(&key).is_ok());
        }
    }

    #[test]
    fn test_scopes_are_independent() {
        let limiter = RateLimiter::new(test_config());
        let auth_key = RateLimitKey::new("10.0.0.1", RateLimitScope::Auth);
        let rpc_key = RateLimitKey::new("10.0.0.1", RateLimitScope::RpcDefault);
        for _ in 0..3 {
            limiter.check_and_record(&auth_key).unwrap();
        }
        // Auth exhausted, but RPC should still work
        assert!(limiter.check_and_record(&auth_key).is_err());
        assert!(limiter.check_and_record(&rpc_key).is_ok());
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::rate_limiter::tests -- --nocapture`
Expected: FAIL

**Step 3: Write implementation**

```rust
use dashmap::DashMap;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum RateLimitError {
    /// Window limit exceeded, retryable after given ms
    Exceeded {
        scope: RateLimitScope,
        retry_after_ms: u64,
    },
    /// Hard lockout (e.g. too many auth failures)
    LockedOut {
        scope: RateLimitScope,
        lockout_remaining_ms: u64,
    },
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum RateLimitScope {
    Auth,
    RpcDefault,
    RpcWrite,
    RpcHeavy,
    WebhookAuth,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct RateLimitKey {
    identity: String,
    scope: RateLimitScope,
}

impl RateLimitKey {
    pub fn new(identity: &str, scope: RateLimitScope) -> Self {
        Self {
            identity: identity.to_string(),
            scope,
        }
    }
}

#[derive(Clone)]
pub struct WindowConfig {
    pub max_requests: u32,
    pub window_secs: u64,
    pub lockout_secs: Option<u64>,
}

#[derive(Clone)]
pub struct RateLimitConfig {
    pub auth: WindowConfig,
    pub rpc_default: WindowConfig,
    pub rpc_write: WindowConfig,
    pub rpc_heavy: WindowConfig,
    pub exempt_loopback: bool,
}

struct SlidingWindow {
    timestamps: VecDeque<Instant>,
    lockout_until: Option<Instant>,
}

pub struct RateLimiter {
    buckets: DashMap<RateLimitKey, SlidingWindow>,
    config: RateLimitConfig,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            buckets: DashMap::new(),
            config,
        }
    }

    fn window_config(&self, scope: &RateLimitScope) -> &WindowConfig {
        match scope {
            RateLimitScope::Auth => &self.config.auth,
            RateLimitScope::RpcDefault => &self.config.rpc_default,
            RateLimitScope::RpcWrite => &self.config.rpc_write,
            RateLimitScope::RpcHeavy | RateLimitScope::WebhookAuth => &self.config.rpc_heavy,
        }
    }

    fn is_loopback(identity: &str) -> bool {
        identity == "127.0.0.1" || identity == "::1" || identity == "localhost"
    }

    pub fn check_and_record(&self, key: &RateLimitKey) -> Result<(), RateLimitError> {
        if self.config.exempt_loopback && Self::is_loopback(&key.identity) {
            return Ok(());
        }

        let wc = self.window_config(&key.scope).clone();
        let now = Instant::now();
        let window = Duration::from_secs(wc.window_secs);

        let mut entry = self.buckets.entry(key.clone()).or_insert_with(|| SlidingWindow {
            timestamps: VecDeque::new(),
            lockout_until: None,
        });

        // Check lockout
        if let Some(until) = entry.lockout_until {
            if now < until {
                return Err(RateLimitError::LockedOut {
                    scope: key.scope.clone(),
                    lockout_remaining_ms: (until - now).as_millis() as u64,
                });
            }
            entry.lockout_until = None;
        }

        // Evict expired timestamps
        while entry.timestamps.front().is_some_and(|t| now.duration_since(*t) > window) {
            entry.timestamps.pop_front();
        }

        // Check limit
        if entry.timestamps.len() >= wc.max_requests as usize {
            // Trigger lockout if configured
            if let Some(lockout_secs) = wc.lockout_secs {
                entry.lockout_until = Some(now + Duration::from_secs(lockout_secs));
                return Err(RateLimitError::LockedOut {
                    scope: key.scope.clone(),
                    lockout_remaining_ms: lockout_secs * 1000,
                });
            }
            // Calculate retry_after from oldest timestamp
            let oldest = entry.timestamps.front().unwrap();
            let retry_after = window.saturating_sub(now.duration_since(*oldest));
            return Err(RateLimitError::Exceeded {
                scope: key.scope.clone(),
                retry_after_ms: retry_after.as_millis() as u64,
            });
        }

        entry.timestamps.push_back(now);
        Ok(())
    }

    /// Periodic cleanup of stale entries
    pub fn prune_stale(&self, max_age: Duration) {
        let now = Instant::now();
        self.buckets.retain(|_, window| {
            // Keep if has active lockout or recent timestamps
            if window.lockout_until.is_some_and(|u| u > now) {
                return true;
            }
            window.timestamps.back().is_some_and(|t| now.duration_since(*t) < max_age)
        });
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib gateway::rate_limiter::tests -- --nocapture`
Expected: 5 tests PASS

**Step 5: Commit**

```bash
git add core/src/gateway/rate_limiter.rs core/src/gateway/mod.rs
git commit -m "gateway: add sliding window rate limiter"
```

---

### Task 7: RPC Method → Rate Limit Scope Mapping

**Files:**
- Modify: `core/src/gateway/rate_limiter.rs` (add `scope_for_method()`)
- Test: inline test

**Step 1: Write the failing test**

```rust
#[test]
fn test_scope_for_method() {
    assert_eq!(scope_for_method("health"), RateLimitScope::RpcDefault);
    assert_eq!(scope_for_method("config.patch"), RateLimitScope::RpcWrite);
    assert_eq!(scope_for_method("config.apply"), RateLimitScope::RpcWrite);
    assert_eq!(scope_for_method("agent.run"), RateLimitScope::RpcHeavy);
    assert_eq!(scope_for_method("chat.send"), RateLimitScope::RpcHeavy);
    assert_eq!(scope_for_method("poe.run"), RateLimitScope::RpcHeavy);
    assert_eq!(scope_for_method("memory.store"), RateLimitScope::RpcWrite);
    assert_eq!(scope_for_method("unknown.method"), RateLimitScope::RpcDefault);
}
```

**Step 2: Run test, verify fail**

**Step 3: Write implementation**

```rust
pub fn scope_for_method(method: &str) -> RateLimitScope {
    match method {
        // Write operations
        "config.patch" | "config.apply" | "config.set"
        | "memory.store" | "memory.delete"
        | "session.compact" | "session.delete"
        | "plugins.install" | "plugins.uninstall"
        | "skills.install" | "skills.delete" => RateLimitScope::RpcWrite,

        // Heavy operations (long-running, resource-intensive)
        "agent.run" | "chat.send" | "poe.run" | "poe.prepare" => RateLimitScope::RpcHeavy,

        // Everything else is default
        _ => RateLimitScope::RpcDefault,
    }
}
```

**Step 4: Run tests, verify pass**

**Step 5: Commit**

```bash
git add core/src/gateway/rate_limiter.rs
git commit -m "gateway: add method-to-scope mapping for rate limiting"
```

---

### Task 8: Lane Concurrency Manager

**Files:**
- Create: `core/src/gateway/lane.rs`
- Modify: `core/src/gateway/mod.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> LaneConfig {
        LaneConfig {
            query_concurrency: 3,
            execute_concurrency: 1,
            mutate_concurrency: 2,
            system_concurrency: 1,
            acquire_timeout_secs: 1,
        }
    }

    #[tokio::test]
    async fn test_acquire_query_lane() {
        let mgr = LaneManager::new(test_config());
        let permit = mgr.acquire("health").await;
        assert!(permit.is_ok());
    }

    #[tokio::test]
    async fn test_lane_for_method() {
        assert_eq!(Lane::for_method("health"), Lane::Query);
        assert_eq!(Lane::for_method("agent.run"), Lane::Execute);
        assert_eq!(Lane::for_method("config.patch"), Lane::Mutate);
        assert_eq!(Lane::for_method("plugins.install"), Lane::System);
        assert_eq!(Lane::for_method("unknown"), Lane::Query);
    }

    #[tokio::test]
    async fn test_execute_lane_saturation() {
        let mgr = LaneManager::new(test_config());
        // Execute lane has concurrency 1, hold the permit
        let _permit1 = mgr.acquire("agent.run").await.unwrap();
        // Second acquire should timeout (1s configured)
        let result = mgr.acquire("agent.run").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_different_lanes_independent() {
        let mgr = LaneManager::new(test_config());
        // Saturate execute lane
        let _permit1 = mgr.acquire("agent.run").await.unwrap();
        // Query lane should still work
        let permit2 = mgr.acquire("health").await;
        assert!(permit2.is_ok());
    }
}
```

**Step 2: Run tests to verify failure**

Run: `cargo test -p alephcore --lib gateway::lane::tests -- --nocapture`

**Step 3: Write implementation**

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Lane {
    Query,   // Read-only queries
    Execute, // Agent execution (long-running)
    Mutate,  // State mutations
    System,  // System management
}

impl Lane {
    pub fn for_method(method: &str) -> Self {
        match method {
            // Execute lane
            "agent.run" | "chat.send" | "poe.run" | "poe.prepare" => Lane::Execute,

            // Mutate lane
            "config.patch" | "config.apply" | "config.set"
            | "memory.store" | "memory.delete"
            | "session.compact" | "session.delete" => Lane::Mutate,

            // System lane
            "plugins.install" | "plugins.uninstall"
            | "skills.install" | "skills.delete"
            | "logs.setLevel" => Lane::System,

            // Everything else: Query
            _ => Lane::Query,
        }
    }
}

#[derive(Clone)]
pub struct LaneConfig {
    pub query_concurrency: usize,
    pub execute_concurrency: usize,
    pub mutate_concurrency: usize,
    pub system_concurrency: usize,
    pub acquire_timeout_secs: u64,
}

impl Default for LaneConfig {
    fn default() -> Self {
        Self {
            query_concurrency: 50,
            execute_concurrency: 5,
            mutate_concurrency: 10,
            system_concurrency: 3,
            acquire_timeout_secs: 30,
        }
    }
}

#[derive(Debug)]
pub enum LaneError {
    Congested(Lane),
}

impl std::fmt::Display for LaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaneError::Congested(lane) => write!(f, "lane {lane:?} congested"),
        }
    }
}

pub struct LaneManager {
    lanes: HashMap<Lane, Arc<Semaphore>>,
    timeout: Duration,
}

impl LaneManager {
    pub fn new(config: LaneConfig) -> Self {
        let mut lanes = HashMap::new();
        lanes.insert(Lane::Query, Arc::new(Semaphore::new(config.query_concurrency)));
        lanes.insert(Lane::Execute, Arc::new(Semaphore::new(config.execute_concurrency)));
        lanes.insert(Lane::Mutate, Arc::new(Semaphore::new(config.mutate_concurrency)));
        lanes.insert(Lane::System, Arc::new(Semaphore::new(config.system_concurrency)));
        Self {
            lanes,
            timeout: Duration::from_secs(config.acquire_timeout_secs),
        }
    }

    pub async fn acquire(&self, method: &str) -> Result<OwnedSemaphorePermit, LaneError> {
        let lane = Lane::for_method(method);
        let semaphore = self.lanes.get(&lane).expect("all lanes initialized");
        match tokio::time::timeout(self.timeout, semaphore.clone().acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            _ => Err(LaneError::Congested(lane)),
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib gateway::lane::tests -- --nocapture`
Expected: 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/gateway/lane.rs core/src/gateway/mod.rs
git commit -m "gateway: add lane-based concurrency manager"
```

---

### Task 9: Event Scope Guard

**Files:**
- Create: `core/src/gateway/event_scope.rs`
- Modify: `core/src/gateway/mod.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unguarded_event_allowed_for_all() {
        let guard = EventScopeGuard::default_rules();
        // Random event topic with no rules
        assert!(guard.can_receive("agent.progress", &[]));
        assert!(guard.can_receive("chat.message", &["viewer".to_string()]));
    }

    #[test]
    fn test_pairing_event_requires_permission() {
        let guard = EventScopeGuard::default_rules();
        assert!(!guard.can_receive("pairing.requested", &[]));
        assert!(!guard.can_receive("pairing.approved", &["viewer".to_string()]));
        assert!(guard.can_receive("pairing.requested", &["admin".to_string()]));
        assert!(guard.can_receive("pairing.approved", &["pairing".to_string()]));
    }

    #[test]
    fn test_exec_approval_requires_permission() {
        let guard = EventScopeGuard::default_rules();
        assert!(!guard.can_receive("exec.approval.requested", &[]));
        assert!(guard.can_receive("exec.approval.requested", &["exec.approver".to_string()]));
        assert!(guard.can_receive("exec.approval.requested", &["admin".to_string()]));
    }

    #[test]
    fn test_admin_has_access_to_all_guarded_events() {
        let guard = EventScopeGuard::default_rules();
        let admin = vec!["admin".to_string()];
        assert!(guard.can_receive("pairing.requested", &admin));
        assert!(guard.can_receive("exec.approval.requested", &admin));
        assert!(guard.can_receive("guest.invited", &admin));
        assert!(guard.can_receive("config.changed", &admin));
    }
}
```

**Step 2: Run tests, verify fail**

**Step 3: Write implementation**

```rust
/// Guards sensitive events behind permission checks.
/// Events without rules are allowed for all clients.
pub struct EventScopeGuard {
    /// Each rule: (topic_prefix, required_permissions)
    /// Client needs ANY of the listed permissions.
    rules: Vec<(String, Vec<String>)>,
}

impl EventScopeGuard {
    pub fn default_rules() -> Self {
        Self {
            rules: vec![
                ("pairing.".into(), vec!["admin".into(), "pairing".into()]),
                ("poe.sign.".into(), vec!["admin".into(), "poe.approver".into()]),
                ("guest.".into(), vec!["admin".into(), "guest.manager".into()]),
                ("exec.approval.".into(), vec!["admin".into(), "exec.approver".into()]),
                ("config.changed".into(), vec!["admin".into(), "config.viewer".into()]),
            ],
        }
    }

    pub fn can_receive(&self, topic: &str, permissions: &[String]) -> bool {
        for (prefix, required) in &self.rules {
            if topic.starts_with(prefix) || topic == prefix {
                return required.iter().any(|r| permissions.contains(r));
            }
        }
        true
    }
}
```

**Step 4: Run tests, verify pass**

**Step 5: Commit**

```bash
git add core/src/gateway/event_scope.rs core/src/gateway/mod.rs
git commit -m "gateway: add event scope guard for sensitive events"
```

---

### Task 10: Integrate Rate Limiter + Lane + Scope Guard into Server

**Files:**
- Modify: `core/src/gateway/server.rs`

**Step 1: Add dependencies to GatewayServer**

Add to `GatewayServer` struct:

```rust
pub rate_limiter: Arc<RateLimiter>,
pub lane_manager: Arc<LaneManager>,
pub event_scope_guard: Arc<EventScopeGuard>,
```

Initialize in `new()` / `with_config()` with default configs.

**Step 2: Add rate limiting in message processing**

In `handle_connection()`, before dispatching to `handlers.handle()`, insert:

```rust
// Rate limit check
let scope = scope_for_method(&request.method);
let rl_key = RateLimitKey::new(
    conn_state.device_id.as_deref().unwrap_or(&peer_addr),
    scope,
);
if let Err(e) = rate_limiter.check_and_record(&rl_key) {
    let (code, msg, data) = match e {
        RateLimitError::Exceeded { retry_after_ms, .. } => (
            RATE_LIMITED,
            "Rate limit exceeded",
            serde_json::json!({"retry_after_ms": retry_after_ms}),
        ),
        RateLimitError::LockedOut { lockout_remaining_ms, .. } => (
            RATE_LIMITED,
            "Rate limit lockout",
            serde_json::json!({"lockout_remaining_ms": lockout_remaining_ms}),
        ),
    };
    let response = JsonRpcResponse::error_with_data(request.id.clone(), code, msg, data);
    // send response and continue loop
    continue;
}
```

**Step 3: Add lane control before dispatch**

```rust
// Acquire lane permit
let permit = match lane_manager.acquire(&request.method).await {
    Ok(permit) => permit,
    Err(LaneError::Congested(lane)) => {
        let response = JsonRpcResponse::error(
            request.id.clone(),
            SERVICE_UNAVAILABLE,
            format!("Service congested ({lane:?}), try again later"),
        );
        // send response and continue loop
        continue;
    }
};
// permit auto-drops after handler completes
let response = handlers.handle(&request).await;
drop(permit);
```

**Step 4: Add scope guard in event forwarding**

In the event forwarding branch of `tokio::select!`, add:

```rust
// Check event scope guard
if !event_scope_guard.can_receive(&topic, &conn_state.permissions) {
    continue; // skip this event for this client
}
```

**Step 5: Add slow consumer detection**

In the event send:

```rust
const SLOW_CONSUMER_TIMEOUT: Duration = Duration::from_secs(5);

match tokio::time::timeout(SLOW_CONSUMER_TIMEOUT, ws_sender.send(msg)).await {
    Ok(Ok(())) => {},
    Ok(Err(_)) => break, // connection closed
    Err(_) => {
        tracing::warn!(conn_id = %peer_addr, "slow consumer detected, closing");
        break;
    }
}
```

**Step 6: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles

**Step 7: Commit**

```bash
git add core/src/gateway/server.rs
git commit -m "gateway: integrate rate limiting, lane control, scope guard, slow consumer"
```

---

### Task 11: Rate Limiter Background Pruning

**Files:**
- Modify: `core/src/gateway/server.rs` (start pruning task)

**Step 1: Start background prune task in server startup**

```rust
let rate_limiter_clone = rate_limiter.clone();
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        rate_limiter_clone.prune_stale(Duration::from_secs(300));
    }
});
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`

**Step 3: Commit**

```bash
git add core/src/gateway/server.rs
git commit -m "gateway: add rate limiter background pruning"
```

---

## Phase 3: API Surface Expansion

### Task 12: OpenAI-Compatible API — Data Types

**Files:**
- Create: `core/src/gateway/openai_api/mod.rs`
- Create: `core/src/gateway/openai_api/types.rs`
- Modify: `core/src/gateway/mod.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing tests**

In `types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_completion_request_deserializes() {
        let json = serde_json::json!({
            "model": "claude-3-opus",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello!"}
            ],
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 1024
        });
        let req: ChatCompletionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.model, "claude-3-opus");
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.stream, Some(true));
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.max_tokens, Some(1024));
    }

    #[test]
    fn test_chat_completion_response_serializes() {
        let resp = ChatCompletionResponse {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion".to_string(),
            created: 1234567890,
            model: "claude-3-opus".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some("Hello!".to_string()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
                delta: None,
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["object"], "chat.completion");
        assert_eq!(json["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn test_streaming_chunk_serializes() {
        let chunk = ChatCompletionResponse {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1234567890,
            model: "claude-3-opus".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: None,
                },
                finish_reason: None,
                delta: Some(Delta { content: Some("Hi".to_string()), role: None }),
            }],
            usage: None,
        };
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json["object"], "chat.completion.chunk");
        assert_eq!(json["choices"][0]["delta"]["content"], "Hi");
    }

    #[test]
    fn test_model_object_serializes() {
        let model = ModelObject {
            id: "claude-3-opus".to_string(),
            object: "model".to_string(),
            created: 1234567890,
            owned_by: "anthropic".to_string(),
        };
        let json = serde_json::to_value(&model).unwrap();
        assert_eq!(json["id"], "claude-3-opus");
        assert_eq!(json["owned_by"], "anthropic");
    }

    #[test]
    fn test_minimal_request_deserializes() {
        let json = serde_json::json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "Hi"}]
        });
        let req: ChatCompletionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.stream, None);
        assert_eq!(req.temperature, None);
    }
}
```

**Step 2: Run tests, verify fail**

**Step 3: Write implementation**

`types.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: Option<bool>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f64>,
    pub stop: Option<Vec<String>>,
    pub tools: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Serialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<Delta>,
}

#[derive(Debug, Serialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct ModelObject {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
}

#[derive(Debug, Serialize)]
pub struct ModelList {
    pub object: String,
    pub data: Vec<ModelObject>,
}
```

`mod.rs`:

```rust
pub mod types;
```

**Step 4: Run tests, verify pass**

**Step 5: Commit**

```bash
git add core/src/gateway/openai_api/ core/src/gateway/mod.rs
git commit -m "gateway: add OpenAI-compatible API data types"
```

---

### Task 13: OpenAI-Compatible API — Auth Middleware

**Files:**
- Create: `core/src/gateway/openai_api/auth.rs`
- Modify: `core/src/gateway/openai_api/mod.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_bearer_token() {
        assert_eq!(
            extract_bearer_token("Bearer sk-test-123"),
            Some("sk-test-123")
        );
        assert_eq!(extract_bearer_token("bearer sk-test"), Some("sk-test"));
        assert_eq!(extract_bearer_token("Basic abc"), None);
        assert_eq!(extract_bearer_token(""), None);
        assert_eq!(extract_bearer_token("Bearer "), None);
    }
}
```

**Step 2: Run tests, verify fail**

**Step 3: Write implementation**

```rust
/// Extract bearer token from Authorization header value.
pub fn extract_bearer_token(header_value: &str) -> Option<&str> {
    let stripped = header_value.strip_prefix("Bearer ")
        .or_else(|| header_value.strip_prefix("bearer "))?;
    if stripped.is_empty() {
        return None;
    }
    Some(stripped)
}

/// Error type for OpenAI API errors.
#[derive(Debug)]
pub enum ApiError {
    Unauthorized(String),
    BadRequest(String),
    InternalError(String),
    ServiceUnavailable(String),
}

impl ApiError {
    pub fn status_code(&self) -> u16 {
        match self {
            ApiError::Unauthorized(_) => 401,
            ApiError::BadRequest(_) => 400,
            ApiError::InternalError(_) => 500,
            ApiError::ServiceUnavailable(_) => 503,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let (error_type, message) = match self {
            ApiError::Unauthorized(m) => ("unauthorized", m.as_str()),
            ApiError::BadRequest(m) => ("invalid_request_error", m.as_str()),
            ApiError::InternalError(m) => ("server_error", m.as_str()),
            ApiError::ServiceUnavailable(m) => ("service_unavailable", m.as_str()),
        };
        serde_json::json!({
            "error": {
                "message": message,
                "type": error_type,
            }
        })
    }
}
```

**Step 4: Run tests, verify pass**

**Step 5: Commit**

```bash
git add core/src/gateway/openai_api/auth.rs core/src/gateway/openai_api/mod.rs
git commit -m "gateway: add OpenAI API auth helpers and error types"
```

---

### Task 14: OpenAI-Compatible API — Router & Endpoints

**Files:**
- Create: `core/src/gateway/openai_api/routes.rs`
- Modify: `core/src/gateway/openai_api/mod.rs`
- Modify: `core/src/gateway/server.rs` (mount router)

**Step 1: Write the routes module**

```rust
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use super::auth::{extract_bearer_token, ApiError};
use super::types::*;

/// Shared state for OpenAI API routes.
/// This will be expanded as we integrate with Aleph's core systems.
pub struct OpenAiApiState {
    pub server_id: String,
    // Will add: token_manager, provider_factory, agent_registry, etc.
}

/// Create the OpenAI-compatible API router.
pub fn openai_routes(state: Arc<OpenAiApiState>) -> Router {
    Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/health", get(health))
        .with_state(state)
}

async fn list_models(
    State(state): State<Arc<OpenAiApiState>>,
) -> Json<ModelList> {
    // Placeholder — will be populated from ProviderFactory
    Json(ModelList {
        object: "list".to_string(),
        data: vec![],
    })
}

async fn health(
    State(state): State<Arc<OpenAiApiState>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "server_id": state.server_id,
    }))
}
```

**Step 2: Mount in server.rs build_router()**

In `build_router()`, add:

```rust
let openai_state = Arc::new(OpenAiApiState {
    server_id: "aleph".to_string(),
});
let openai_router = openai_routes(openai_state);

// Merge into existing router
Router::new()
    .route("/ws", get(ws_upgrade_handler))
    .merge(openai_router)
    // ... existing fallback
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`

**Step 4: Commit**

```bash
git add core/src/gateway/openai_api/routes.rs core/src/gateway/openai_api/mod.rs core/src/gateway/server.rs
git commit -m "gateway: add OpenAI API router with /v1/models and /v1/health"
```

---

### Task 15: OpenAI-Compatible API — Chat Completions Endpoint (Stub)

**Files:**
- Modify: `core/src/gateway/openai_api/routes.rs`

**Step 1: Add chat completions route**

Add `POST /v1/chat/completions` endpoint that validates the request and returns a stub response. This establishes the contract; full agent integration comes later.

```rust
async fn chat_completions(
    State(state): State<Arc<OpenAiApiState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChatCompletionRequest>,
) -> Response {
    // Auth check
    let auth_header = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let _token = match extract_bearer_token(auth_header) {
        Some(t) => t,
        None => {
            let err = ApiError::Unauthorized("Missing or invalid Authorization header".into());
            return (StatusCode::UNAUTHORIZED, Json(err.to_json())).into_response();
        }
    };

    // TODO: Validate token via TokenManager
    // TODO: Route to Aleph agent loop via ProviderFactory

    // Stub response
    let response = ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: req.model.clone(),
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some("OpenAI API endpoint is ready. Full integration pending.".to_string()),
                tool_calls: None,
            },
            finish_reason: Some("stop".to_string()),
            delta: None,
        }],
        usage: Some(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }),
    };

    Json(response).into_response()
}
```

Add to `openai_routes()`:
```rust
.route("/v1/chat/completions", post(chat_completions))
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`

**Step 3: Commit**

```bash
git add core/src/gateway/openai_api/routes.rs
git commit -m "gateway: add /v1/chat/completions stub endpoint"
```

---

### Task 16: Tick Heartbeat Background Task

**Files:**
- Modify: `core/src/gateway/server.rs`

**Step 1: Add tick broadcast task**

In the server startup, after event bus is created, spawn a tick task:

```rust
let event_bus_tick = event_bus.clone();
let state_versions_tick = state_versions.clone();
let connections_tick = connections.clone();
let start_time = std::time::Instant::now();

tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    loop {
        interval.tick().await;
        let snapshot = state_versions_tick.snapshot();
        let conn_count = connections_tick.read().await.len();
        let event = TopicEvent::new(
            "system.tick",
            serde_json::json!({
                "ts": chrono::Utc::now().timestamp_millis(),
                "state_version": snapshot,
                "connections": conn_count,
                "uptime_ms": start_time.elapsed().as_millis() as u64,
            }),
        );
        let _ = event_bus_tick.publish(event);
    }
});
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`

**Step 3: Commit**

```bash
git add core/src/gateway/server.rs
git commit -m "gateway: add system.tick heartbeat broadcast (10s interval)"
```

---

### Task 17: Tailscale Integration Module

**Files:**
- Create: `core/src/gateway/tailscale.rs`
- Modify: `core/src/gateway/mod.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tailscale_identity_from_headers() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("Tailscale-User-Login", "alice@example.com".parse().unwrap());
        headers.insert("Tailscale-User-Name", "Alice".parse().unwrap());
        headers.insert("X-Forwarded-For", "100.64.0.1".parse().unwrap());

        let identity = TailscaleIdentity::from_headers(&headers);
        assert!(identity.is_some());
        let id = identity.unwrap();
        assert_eq!(id.login, "alice@example.com");
        assert_eq!(id.display_name, "Alice");
        assert_eq!(id.peer_ip, "100.64.0.1");
    }

    #[test]
    fn test_missing_headers_returns_none() {
        let headers = axum::http::HeaderMap::new();
        assert!(TailscaleIdentity::from_headers(&headers).is_none());
    }

    #[test]
    fn test_partial_headers_returns_none() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("Tailscale-User-Login", "alice@example.com".parse().unwrap());
        // Missing User-Name
        assert!(TailscaleIdentity::from_headers(&headers).is_none());
    }

    #[test]
    fn test_is_tailscale_ip() {
        assert!(is_tailscale_ip("100.64.0.1"));
        assert!(is_tailscale_ip("100.127.255.254"));
        assert!(!is_tailscale_ip("192.168.1.1"));
        assert!(!is_tailscale_ip("10.0.0.1"));
        assert!(!is_tailscale_ip("100.63.255.255"));
        assert!(!is_tailscale_ip("100.128.0.0"));
    }
}
```

**Step 2: Run tests, verify fail**

**Step 3: Write implementation**

```rust
use axum::http::HeaderMap;
use serde::Serialize;

/// Identity extracted from Tailscale proxy headers.
#[derive(Debug, Clone, Serialize)]
pub struct TailscaleIdentity {
    pub login: String,
    pub display_name: String,
    pub peer_ip: String,
}

impl TailscaleIdentity {
    /// Extract identity from HTTP headers set by Tailscale proxy.
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let login = headers.get("Tailscale-User-Login")?.to_str().ok()?;
        let name = headers.get("Tailscale-User-Name")?.to_str().ok()?;
        let ip = headers.get("X-Forwarded-For")?.to_str().ok()?;

        if login.is_empty() || name.is_empty() {
            return None;
        }

        Some(Self {
            login: login.to_string(),
            display_name: name.to_string(),
            peer_ip: ip.to_string(),
        })
    }
}

/// Check if an IP is in the Tailscale CGNAT range (100.64.0.0/10).
pub fn is_tailscale_ip(ip: &str) -> bool {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let first: u8 = match parts[0].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let second: u8 = match parts[1].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    // 100.64.0.0/10 = 100.64.0.0 - 100.127.255.255
    first == 100 && (64..=127).contains(&second)
}

/// Tailscale integration configuration.
#[derive(Debug, Clone)]
pub struct TailscaleConfig {
    pub enabled: bool,
    /// Path to tailscaled Unix socket for whois lookups.
    pub socket_path: Option<String>,
}

impl Default for TailscaleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            socket_path: Some("/var/run/tailscale/tailscaled.sock".to_string()),
        }
    }
}
```

**Step 4: Run tests, verify pass**

**Step 5: Commit**

```bash
git add core/src/gateway/tailscale.rs core/src/gateway/mod.rs
git commit -m "gateway: add Tailscale identity extraction and IP detection"
```

---

### Task 18: Multi-Bind Mode & Reload Mode Enums

**Files:**
- Modify: `core/src/gateway/hot_reload.rs` (add `ReloadMode`)
- Create or modify: bind mode in server config
- Test: inline `#[cfg(test)]`

**Step 1: Write tests for ReloadMode**

Add to `hot_reload.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reload_mode_hot_reload_decisions() {
        assert!(!ReloadMode::Off.should_hot_reload("ui"));
        assert!(!ReloadMode::Off.should_hot_reload("auth"));
        assert!(ReloadMode::Hot.should_hot_reload("ui"));
        assert!(ReloadMode::Hot.should_hot_reload("auth"));
        assert!(!ReloadMode::Restart.should_hot_reload("ui"));
        assert!(!ReloadMode::Restart.should_hot_reload("auth"));
        assert!(ReloadMode::Hybrid.should_hot_reload("ui"));
        assert!(ReloadMode::Hybrid.should_hot_reload("channels"));
        assert!(ReloadMode::Hybrid.should_hot_reload("skills"));
        assert!(!ReloadMode::Hybrid.should_hot_reload("auth"));
        assert!(!ReloadMode::Hybrid.should_hot_reload("providers"));
        assert!(!ReloadMode::Hybrid.should_hot_reload("gateway"));
    }
}
```

**Step 2: Run tests, verify fail**

**Step 3: Write implementation**

Add to `hot_reload.rs`:

```rust
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReloadMode {
    Off,
    Hot,
    Restart,
    Hybrid,
}

impl Default for ReloadMode {
    fn default() -> Self {
        Self::Hot
    }
}

impl ReloadMode {
    /// Whether a config section change should be applied via hot reload.
    pub fn should_hot_reload(&self, section: &str) -> bool {
        match self {
            ReloadMode::Off => false,
            ReloadMode::Hot => true,
            ReloadMode::Restart => false,
            ReloadMode::Hybrid => matches!(section,
                "ui" | "channels" | "skills" | "workspace" | "cron"
            ),
        }
    }
}
```

Add `BindMode` enum (can go in `server.rs` or a dedicated config module):

```rust
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BindMode {
    Loopback,
    Lan,
    Tailnet,
    Auto,
}

impl Default for BindMode {
    fn default() -> Self {
        Self::Loopback
    }
}

impl BindMode {
    pub fn resolve_addr(&self, port: u16) -> std::net::SocketAddr {
        let ip = match self {
            BindMode::Loopback => std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            BindMode::Lan => std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            BindMode::Tailnet | BindMode::Auto => {
                // For Tailnet: would need to resolve Tailscale IP
                // Fallback to loopback
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            }
        };
        std::net::SocketAddr::new(ip, port)
    }
}
```

**Step 4: Run tests, verify pass**

**Step 5: Commit**

```bash
git add core/src/gateway/hot_reload.rs core/src/gateway/server.rs
git commit -m "gateway: add ReloadMode and BindMode enums"
```

---

### Task 19: Challenge-Response Handshake

**Files:**
- Create: `core/src/gateway/challenge.rs`
- Modify: `core/src/gateway/mod.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_challenge() {
        let mgr = ChallengeManager::new();
        let challenge = mgr.generate();
        assert_eq!(challenge.nonce.len(), 64); // 32 bytes hex
        assert!(challenge.timestamp > 0);
        assert!(!challenge.server_id.is_empty());
    }

    #[test]
    fn test_verify_challenge_success() {
        let mgr = ChallengeManager::new();
        let challenge = mgr.generate();
        let device_id = "device-1";
        let token = "my-secret-token";

        // Client side: compute signature
        let signature = compute_signature(token, &challenge.nonce, challenge.timestamp, device_id);

        // Server side: verify
        let result = mgr.verify(&challenge.nonce, device_id, &signature, token);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_wrong_signature_fails() {
        let mgr = ChallengeManager::new();
        let challenge = mgr.generate();
        let result = mgr.verify(&challenge.nonce, "device-1", "bad-signature", "token");
        assert!(result.is_err());
    }

    #[test]
    fn test_nonce_replay_prevention() {
        let mgr = ChallengeManager::new();
        let challenge = mgr.generate();
        let device_id = "device-1";
        let token = "my-token";
        let sig = compute_signature(token, &challenge.nonce, challenge.timestamp, device_id);

        // First verify should succeed
        assert!(mgr.verify(&challenge.nonce, device_id, &sig, token).is_ok());
        // Second verify with same nonce should fail
        assert!(mgr.verify(&challenge.nonce, device_id, &sig, token).is_err());
    }

    #[test]
    fn test_prune_expired_nonces() {
        let mgr = ChallengeManager::new();
        // Generate some challenges
        for _ in 0..5 {
            mgr.generate();
        }
        assert_eq!(mgr.pending_count(), 5);
        // Pruning with 0 TTL should remove all
        mgr.prune(std::time::Duration::ZERO);
        assert_eq!(mgr.pending_count(), 0);
    }
}
```

**Step 2: Run tests, verify fail**

**Step 3: Write implementation**

```rust
use dashmap::DashSet;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Challenge {
    pub nonce: String,
    pub timestamp: u64,
    pub server_id: String,
}

struct PendingNonce {
    nonce: String,
    created_at: Instant,
    timestamp: u64,
}

/// Manages challenge-response handshakes for WebSocket connections.
pub struct ChallengeManager {
    pending: dashmap::DashMap<String, PendingNonce>,
    used: DashSet<String>,
    server_id: String,
}

impl ChallengeManager {
    pub fn new() -> Self {
        Self::with_server_id(format!("aleph-{}", &uuid::Uuid::new_v4().to_string()[..8]))
    }

    pub fn with_server_id(server_id: String) -> Self {
        Self {
            pending: dashmap::DashMap::new(),
            used: DashSet::new(),
            server_id,
        }
    }

    pub fn generate(&self) -> Challenge {
        let nonce = hex::encode(uuid::Uuid::new_v4().as_bytes())
            + &hex::encode(uuid::Uuid::new_v4().as_bytes());
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.pending.insert(nonce.clone(), PendingNonce {
            nonce: nonce.clone(),
            created_at: Instant::now(),
            timestamp,
        });

        Challenge {
            nonce,
            timestamp,
            server_id: self.server_id.clone(),
        }
    }

    pub fn verify(
        &self,
        nonce: &str,
        device_id: &str,
        signature: &str,
        token: &str,
    ) -> Result<(), ChallengeError> {
        // Check replay
        if self.used.contains(nonce) {
            return Err(ChallengeError::NonceReplay);
        }

        // Get pending challenge
        let pending = self.pending.remove(nonce)
            .ok_or(ChallengeError::NonceNotFound)?;
        let (_, pending) = pending;

        // Check timestamp window (±30s)
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let diff = if now_ms > pending.timestamp {
            now_ms - pending.timestamp
        } else {
            pending.timestamp - now_ms
        };
        if diff > 30_000 {
            return Err(ChallengeError::TimestampExpired);
        }

        // Verify HMAC signature
        let expected = compute_signature(token, nonce, pending.timestamp, device_id);
        if signature != expected {
            return Err(ChallengeError::InvalidSignature);
        }

        // Mark nonce as used
        self.used.insert(nonce.to_string());

        Ok(())
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn prune(&self, max_age: Duration) {
        let now = Instant::now();
        self.pending.retain(|_, v| now.duration_since(v.created_at) < max_age);
        // Also limit used set size (keep last 10000)
        if self.used.len() > 10_000 {
            self.used.clear();
        }
    }
}

/// Compute HMAC-SHA256(key=token, msg=nonce+timestamp+device_id).
pub fn compute_signature(token: &str, nonce: &str, timestamp: u64, device_id: &str) -> String {
    let msg = format!("{nonce}{timestamp}{device_id}");
    let mut mac = HmacSha256::new_from_slice(token.as_bytes())
        .expect("HMAC can take any size key");
    mac.update(msg.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[derive(Debug)]
pub enum ChallengeError {
    NonceNotFound,
    NonceReplay,
    TimestampExpired,
    InvalidSignature,
}

impl std::fmt::Display for ChallengeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChallengeError::NonceNotFound => write!(f, "challenge nonce not found"),
            ChallengeError::NonceReplay => write!(f, "nonce already used (replay attack)"),
            ChallengeError::TimestampExpired => write!(f, "challenge timestamp expired"),
            ChallengeError::InvalidSignature => write!(f, "invalid challenge signature"),
        }
    }
}
```

**Note:** This requires `hmac`, `sha2`, and `hex` crates. Check if they're already in `Cargo.toml` — the existing `TokenManager` uses HMAC so they likely are.

**Step 4: Run tests, verify pass**

Run: `cargo test -p alephcore --lib gateway::challenge::tests -- --nocapture`
Expected: 5 tests PASS

**Step 5: Commit**

```bash
git add core/src/gateway/challenge.rs core/src/gateway/mod.rs
git commit -m "gateway: add challenge-response handshake manager"
```

---

### Task 20: Final Integration & Verification

**Files:**
- Modify: `core/src/gateway/server.rs` (integrate challenge into connection flow)
- Modify: `core/src/gateway/mod.rs` (ensure all modules exported)

**Step 1: Integrate challenge into connection handshake**

In `handle_connection()`, after WebSocket upgrade and before the main message loop:

```rust
// If challenge-response is enabled
if config.auth.challenge_response {
    let challenge = challenge_manager.generate();
    let challenge_msg = JsonRpcResponse::result(
        None,
        serde_json::json!({
            "method": "connect.challenge",
            "challenge": challenge,
        }),
    );
    ws_sender.send(Message::Text(serde_json::to_string(&challenge_msg)?)).await?;
    // Client must respond with connect.authorize as first message
}
```

**Step 2: Full build verification**

Run: `cargo check -p alephcore`
Expected: Compiles without errors

**Step 3: Run all new tests**

Run: `cargo test -p alephcore --lib gateway::presence gateway::state_version gateway::hello_snapshot gateway::rate_limiter gateway::lane gateway::event_scope gateway::challenge gateway::tailscale -- --nocapture`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/gateway/
git commit -m "gateway: integrate challenge-response into connection flow"
```

---

## Summary

| Phase | Tasks | New Files | New Lines (approx) |
|-------|-------|-----------|---------------------|
| P1: Connection Lifecycle | Tasks 1-5 | `presence.rs`, `state_version.rs`, `hello_snapshot.rs` | ~400 |
| P2: Flow Control & Security | Tasks 6-11 | `rate_limiter.rs`, `lane.rs`, `event_scope.rs` | ~500 |
| P3: API Expansion | Tasks 12-20 | `openai_api/`, `tailscale.rs`, `challenge.rs` | ~800 |
| **Total** | **20 tasks** | **9 new files** | **~1,700 lines** |

Each task is self-contained with tests, compilable independently, and produces a clean commit.
