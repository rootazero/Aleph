# UI Token Authentication Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Secure Aleph's Leptos Panel UI and HTTP API with token-based authentication using a three-layer model (shared token + session cookie + device token).

**Architecture:** Replace `require_auth: bool` with `AuthMode` enum. Add Axum middleware for HTTP session cookie auth on Panel UI routes. Add Bearer token auth on `/v1/*` API routes. Auto-generate shared token on first start. Reuse existing `SecurityStore` (SQLite) for session storage.

**Tech Stack:** Rust, Axum (tower middleware), rusqlite, HMAC-SHA256, Leptos (WASM panel)

**Design Doc:** `docs/plans/2026-03-12-ui-token-auth-design.md`

---

### Task 1: AuthConfig & AuthMode — Config Layer Refactoring

**Files:**
- Modify: `src/gateway/config.rs:64-89` (GatewayServerConfig)
- Test: existing tests in `src/gateway/config.rs:390-483`

**Step 1: Write the failing test**

Add to `src/gateway/config.rs` tests:

```rust
#[test]
fn test_parse_auth_config() {
    let toml = r#"
[gateway]
port = 18790

[gateway.auth]
mode = "token"
session_expiry_hours = 48
token_expiry_hours = 12

[agents.main]
model = "test"
"#;
    let config = GatewayConfig::from_toml(toml).unwrap();
    assert!(matches!(config.gateway.auth.mode, AuthMode::Token));
    assert_eq!(config.gateway.auth.session_expiry_hours, 48);
    assert_eq!(config.gateway.auth.token_expiry_hours, 12);
}

#[test]
fn test_auth_mode_default_is_token() {
    let config = GatewayConfig::default();
    assert!(matches!(config.gateway.auth.mode, AuthMode::Token));
}

#[test]
fn test_legacy_require_auth_compat() {
    // Old configs with require_auth should still parse
    let toml = r#"
[gateway]
port = 18790
require_auth = true

[agents.main]
model = "test"
"#;
    let config = GatewayConfig::from_toml(toml).unwrap();
    // require_auth=true should be accepted without error (ignored, auth.mode takes precedence)
    assert!(matches!(config.gateway.auth.mode, AuthMode::Token));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::tests::test_parse_auth_config`
Expected: FAIL — `AuthMode` type doesn't exist yet

**Step 3: Write minimal implementation**

In `src/gateway/config.rs`:

1. Add the new types after `GatewayServerConfig`:

```rust
/// Authentication mode
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// Require shared token for access (default)
    #[default]
    Token,
    /// No authentication required
    None,
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Authentication mode
    pub mode: AuthMode,
    /// HTTP session cookie expiry (hours)
    pub session_expiry_hours: u64,
    /// Device token expiry (hours)
    pub token_expiry_hours: u64,
    /// Allowed WebSocket origins (additional to same-origin)
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::Token,
            session_expiry_hours: 72,
            token_expiry_hours: 24,
            allowed_origins: vec![],
        }
    }
}

impl AuthConfig {
    /// Whether authentication is required
    pub fn is_auth_required(&self) -> bool {
        matches!(self.mode, AuthMode::Token)
    }
}
```

2. Add `auth: AuthConfig` field to `GatewayServerConfig`:

```rust
pub struct GatewayServerConfig {
    pub host: String,
    pub port: u16,
    pub max_connections: usize,
    /// Legacy field — ignored when `auth.mode` is set. Kept for TOML compat.
    #[serde(default)]
    pub require_auth: bool,
    pub protocol_version: u32,
    /// Authentication configuration
    #[serde(default)]
    pub auth: AuthConfig,
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib config::tests`
Expected: All PASS

**Step 5: Commit**

```
gateway: add AuthMode and AuthConfig to gateway config
```

---

### Task 2: Shared Token Manager

**Files:**
- Create: `src/gateway/security/shared_token.rs`
- Modify: `src/gateway/security/mod.rs:19-48` (add module + re-export)

**Step 1: Write the failing test**

Create `src/gateway/security/shared_token.rs` with tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;

    #[test]
    fn test_generate_and_validate() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store);
        let token = manager.generate_token().unwrap();
        assert!(!token.is_empty());
        assert!(manager.validate(&token).unwrap());
    }

    #[test]
    fn test_invalid_token_rejected() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store);
        let _token = manager.generate_token().unwrap();
        assert!(!manager.validate("wrong-token").unwrap());
    }

    #[test]
    fn test_regenerate_invalidates_old() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store);
        let old = manager.generate_token().unwrap();
        let new = manager.generate_token().unwrap();
        assert_ne!(old, new);
        assert!(!manager.validate(&old).unwrap());
        assert!(manager.validate(&new).unwrap());
    }

    #[test]
    fn test_load_existing_token() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store.clone());
        let token = manager.generate_token().unwrap();

        // Create new manager with same store — should find existing token
        let manager2 = SharedTokenManager::new(store);
        assert!(manager2.validate(&token).unwrap());
    }

    #[test]
    fn test_get_current_token() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store);
        assert!(manager.get_current_token().is_none());
        let token = manager.generate_token().unwrap();
        assert_eq!(manager.get_current_token(), Some(token));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib security::shared_token`
Expected: FAIL — no implementation

**Step 3: Write minimal implementation**

```rust
//! Shared Token Management
//!
//! A single shared token used as the "entry key" for UI login and API access.
//! The plaintext token is held in memory; only the hash is stored in SQLite.

use crate::sync_primitives::{Arc, Mutex};
use super::crypto::{generate_secret, hmac_sign, hmac_verify};
use super::store::SecurityStore;
use uuid::Uuid;

/// Manages the shared access token for Aleph.
pub struct SharedTokenManager {
    store: Arc<SecurityStore>,
    /// HMAC secret for hashing
    secret: [u8; 32],
    /// Current plaintext token (held in memory only)
    current_token: Mutex<Option<String>>,
}

impl SharedTokenManager {
    pub fn new(store: Arc<SecurityStore>) -> Self {
        Self {
            store,
            secret: generate_secret(),
            current_token: Mutex::new(None),
        }
    }

    /// Generate a new shared token (invalidates any previous one).
    pub fn generate_token(&self) -> Result<String, SharedTokenError> {
        let token = format!("aleph-{}", Uuid::new_v4());
        let hash = hmac_sign(&self.secret, &token);

        // Store hash in DB (replace any existing)
        self.store
            .set_shared_token_hash(&hash)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))?;

        let mut current = self.current_token.lock().unwrap_or_else(|e| e.into_inner());
        *current = Some(token.clone());

        Ok(token)
    }

    /// Validate a token against the stored hash.
    pub fn validate(&self, token: &str) -> Result<bool, SharedTokenError> {
        let hash = hmac_sign(&self.secret, token);
        self.store
            .validate_shared_token_hash(&hash)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))
    }

    /// Get the current plaintext token (only available in the process that generated it).
    pub fn get_current_token(&self) -> Option<String> {
        let current = self.current_token.lock().unwrap_or_else(|e| e.into_inner());
        current.clone()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SharedTokenError {
    #[error("Storage error: {0}")]
    Storage(String),
}
```

**Step 4: Add SecurityStore methods for shared token**

In `src/gateway/security/store.rs`, add:

```rust
// ========== Shared Token Operations ==========

/// Store a shared token hash (replaces any existing)
pub fn set_shared_token_hash(&self, hash: &str) -> SqliteResult<()> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    conn.execute("DELETE FROM shared_token", [])?;
    conn.execute(
        "INSERT INTO shared_token (token_hash, created_at) VALUES (?1, ?2)",
        params![hash, current_timestamp_ms()],
    )?;
    Ok(())
}

/// Validate a shared token hash
pub fn validate_shared_token_hash(&self, hash: &str) -> SqliteResult<bool> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM shared_token WHERE token_hash = ?1",
        params![hash],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}
```

**Step 5: Add `shared_token` table to schema migration**

In `src/gateway/security/store.rs`:
- Bump `SCHEMA_VERSION` from `2` to `3`
- Add to migration (alongside existing tables), append to schema SQL:

```sql
CREATE TABLE IF NOT EXISTS shared_token (
    token_hash  TEXT PRIMARY KEY,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    session_id    TEXT PRIMARY KEY,
    token_hash    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL,
    last_used_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);
```

**Step 6: Register module in `security/mod.rs`**

Add `pub mod shared_token;` and re-export:
```rust
pub use shared_token::{SharedTokenManager, SharedTokenError};
```

**Step 7: Run tests**

Run: `cargo test -p alephcore --lib security::shared_token`
Expected: All PASS

**Step 8: Commit**

```
gateway: add SharedTokenManager and session table to SecurityStore
```

---

### Task 3: HTTP Session Manager

**Files:**
- Create: `src/gateway/session.rs`
- Modify: `src/gateway/mod.rs` (add module)

**Step 1: Write failing test**

Create `src/gateway/session.rs` with tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use crate::gateway::security::SecurityStore;

    #[test]
    fn test_create_and_validate_session() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SessionManager::new(store, 72);
        let session_id = manager.create_session("test-hash").unwrap();
        assert!(!session_id.is_empty());
        assert!(manager.validate_session(&session_id).unwrap());
    }

    #[test]
    fn test_invalid_session_rejected() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SessionManager::new(store, 72);
        assert!(!manager.validate_session("nonexistent").unwrap());
    }

    #[test]
    fn test_revoke_session() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SessionManager::new(store, 72);
        let session_id = manager.create_session("test-hash").unwrap();
        assert!(manager.validate_session(&session_id).unwrap());
        manager.revoke_session(&session_id).unwrap();
        assert!(!manager.validate_session(&session_id).unwrap());
    }

    #[test]
    fn test_list_sessions() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SessionManager::new(store, 72);
        let _s1 = manager.create_session("hash1").unwrap();
        let _s2 = manager.create_session("hash2").unwrap();
        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_cleanup_expired() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        // 0 hours = immediately expired
        let manager = SessionManager::new(store, 0);
        let session_id = manager.create_session("test-hash").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(!manager.validate_session(&session_id).unwrap());
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib gateway::session`
Expected: FAIL — module doesn't exist

**Step 3: Implement SessionManager**

Note: this is NOT `session_manager.rs` (which already exists for agent sessions). This is `session.rs` for HTTP auth sessions.

```rust
//! HTTP Session Management for Panel UI authentication.
//!
//! Sessions are created after successful shared token login.
//! Session IDs are stored in HttpOnly cookies.

use crate::sync_primitives::Arc;
use crate::gateway::security::SecurityStore;
use uuid::Uuid;

pub struct HttpSessionManager {
    store: Arc<SecurityStore>,
    expiry_hours: u64,
}

/// Session info returned by list
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_used_at: i64,
}

impl HttpSessionManager {
    pub fn new(store: Arc<SecurityStore>, expiry_hours: u64) -> Self {
        Self { store, expiry_hours }
    }

    pub fn create_session(&self, token_hash: &str) -> Result<String, SessionError> {
        let session_id = Uuid::new_v4().to_string();
        let now = current_timestamp_ms();
        let expires_at = now + (self.expiry_hours as i64 * 3600 * 1000);

        self.store
            .insert_session(&session_id, token_hash, now, expires_at)
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        Ok(session_id)
    }

    pub fn validate_session(&self, session_id: &str) -> Result<bool, SessionError> {
        let valid = self.store
            .validate_session(session_id)
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        if valid {
            // Sliding renewal: update last_used_at
            let _ = self.store.touch_session(session_id);
        }

        Ok(valid)
    }

    pub fn revoke_session(&self, session_id: &str) -> Result<(), SessionError> {
        self.store
            .delete_session(session_id)
            .map_err(|e| SessionError::Storage(e.to_string()))
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>, SessionError> {
        self.store
            .list_active_sessions()
            .map_err(|e| SessionError::Storage(e.to_string()))
    }

    pub fn cleanup_expired(&self) -> Result<u64, SessionError> {
        self.store
            .delete_expired_sessions()
            .map_err(|e| SessionError::Storage(e.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Storage error: {0}")]
    Storage(String),
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
```

**Step 4: Add SecurityStore session CRUD methods**

In `src/gateway/security/store.rs`, add session operations:

```rust
// ========== Session Operations ==========

pub fn insert_session(&self, session_id: &str, token_hash: &str, created_at: i64, expires_at: i64) -> SqliteResult<()> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    conn.execute(
        "INSERT INTO sessions (session_id, token_hash, created_at, expires_at, last_used_at) VALUES (?1, ?2, ?3, ?4, ?3)",
        params![session_id, token_hash, created_at, expires_at],
    )?;
    Ok(())
}

pub fn validate_session(&self, session_id: &str) -> SqliteResult<bool> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let now = current_timestamp_ms();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sessions WHERE session_id = ?1 AND expires_at > ?2",
        params![session_id, now],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn touch_session(&self, session_id: &str) -> SqliteResult<()> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let now = current_timestamp_ms();
    conn.execute(
        "UPDATE sessions SET last_used_at = ?1 WHERE session_id = ?2",
        params![now, session_id],
    )?;
    Ok(())
}

pub fn delete_session(&self, session_id: &str) -> SqliteResult<()> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    conn.execute("DELETE FROM sessions WHERE session_id = ?1", params![session_id])?;
    Ok(())
}

pub fn list_active_sessions(&self) -> SqliteResult<Vec<crate::gateway::session::SessionInfo>> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let now = current_timestamp_ms();
    let mut stmt = conn.prepare(
        "SELECT session_id, created_at, expires_at, last_used_at FROM sessions WHERE expires_at > ?1 ORDER BY created_at DESC",
    )?;
    let sessions = stmt.query_map(params![now], |row| {
        Ok(crate::gateway::session::SessionInfo {
            session_id: row.get(0)?,
            created_at: row.get(1)?,
            expires_at: row.get(2)?,
            last_used_at: row.get(3)?,
        })
    })?.filter_map(|r| r.ok()).collect();
    Ok(sessions)
}

pub fn delete_expired_sessions(&self) -> SqliteResult<u64> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let now = current_timestamp_ms();
    let count = conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", params![now])?;
    Ok(count as u64)
}
```

**Step 5: Register module**

In `src/gateway/mod.rs`, add `pub mod session;`.

**Step 6: Run tests**

Run: `cargo test -p alephcore --lib gateway::session`
Expected: All PASS

**Step 7: Commit**

```
gateway: add HttpSessionManager for Panel UI auth sessions
```

---

### Task 4: Login Page & Auth Routes

**Files:**
- Create: `src/gateway/auth_middleware.rs`
- Modify: `src/gateway/mod.rs` (add module)

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_html_not_empty() {
        let html = login_page_html("");
        assert!(html.contains("<form"));
        assert!(html.contains("token"));
    }

    #[test]
    fn test_login_html_shows_error() {
        let html = login_page_html("Invalid token");
        assert!(html.contains("Invalid token"));
    }
}
```

**Step 2: Implement auth routes and login page**

```rust
//! HTTP Authentication Middleware and Login Routes
//!
//! Provides session-cookie-based auth for Panel UI and
//! Bearer token auth for /v1/* API routes.

use axum::{
    Router,
    routing::{get, post},
    response::{Html, IntoResponse, Redirect, Response},
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    extract::{State, Form},
    body::Body,
};
use serde::Deserialize;
use crate::sync_primitives::Arc;
use crate::gateway::security::{SharedTokenManager, SecurityStore};
use crate::gateway::session::HttpSessionManager;
use crate::gateway::config::AuthMode;

/// Shared state for auth middleware
pub struct AuthState {
    pub shared_token_mgr: Arc<SharedTokenManager>,
    pub session_mgr: Arc<HttpSessionManager>,
    pub auth_mode: AuthMode,
}

/// Login form data
#[derive(Deserialize)]
pub struct LoginForm {
    token: String,
}

/// Build auth routes (login/logout — no auth required)
pub fn auth_routes(state: Arc<AuthState>) -> Router {
    Router::new()
        .route("/login", get(show_login))
        .route("/auth/login", post(handle_login))
        .route("/auth/logout", post(handle_logout))
        .with_state(state)
}

async fn show_login() -> Html<String> {
    Html(login_page_html(""))
}

async fn handle_login(
    State(state): State<Arc<AuthState>>,
    Form(form): Form<LoginForm>,
) -> Response {
    match state.shared_token_mgr.validate(&form.token) {
        Ok(true) => {
            // Create session
            let hash = crate::gateway::security::hmac_sign(
                state.shared_token_mgr.secret(),
                &form.token,
            );
            match state.session_mgr.create_session(&hash) {
                Ok(session_id) => {
                    let cookie = format!(
                        "aleph_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
                        session_id,
                        state.session_mgr.expiry_hours() * 3600,
                    );
                    (
                        StatusCode::SEE_OTHER,
                        [
                            (header::LOCATION, "/"),
                            (header::SET_COOKIE, &cookie),
                        ],
                        "",
                    ).into_response()
                }
                Err(_) => Html(login_page_html("Internal error")).into_response(),
            }
        }
        Ok(false) => Html(login_page_html("Invalid token")).into_response(),
        Err(_) => Html(login_page_html("Internal error")).into_response(),
    }
}

async fn handle_logout() -> Response {
    let cookie = "aleph_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0";
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/login"),
            (header::SET_COOKIE, cookie),
        ],
        "",
    ).into_response()
}

/// Session cookie middleware for Panel UI routes
pub async fn session_auth_middleware(
    State(state): State<Arc<AuthState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Skip auth if mode is None
    if matches!(state.auth_mode, AuthMode::None) {
        return next.run(request).await;
    }

    // Extract session cookie
    let session_id = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';')
                .filter_map(|c| {
                    let mut parts = c.trim().splitn(2, '=');
                    let name = parts.next()?;
                    let value = parts.next()?;
                    if name == "aleph_session" { Some(value.to_string()) } else { None }
                })
                .next()
        });

    match session_id {
        Some(id) if state.session_mgr.validate_session(&id).unwrap_or(false) => {
            next.run(request).await
        }
        _ => {
            // Redirect to login
            Redirect::to("/login").into_response()
        }
    }
}

/// Bearer token middleware for API routes (/v1/*, /a2a/*)
pub async fn bearer_auth_middleware(
    State(state): State<Arc<AuthState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if matches!(state.auth_mode, AuthMode::None) {
        return next.run(request).await;
    }

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header_val) => {
            if let Some(token) = crate::gateway::openai_api::auth::extract_bearer_token(header_val) {
                // Try shared token first
                if state.shared_token_mgr.validate(token).unwrap_or(false) {
                    return next.run(request).await;
                }
                // Try device token (format: token:signature)
                if let Some((tok, sig)) = token.split_once(':') {
                    // Device token validation would go through TokenManager
                    // For now, shared token is the primary API auth method
                    let _ = (tok, sig); // placeholder for future device token API auth
                }
            }
            (StatusCode::UNAUTHORIZED, "Invalid token").into_response()
        }
        None => {
            (StatusCode::UNAUTHORIZED, "Authorization header required").into_response()
        }
    }
}

/// Generate the login page HTML (pure HTML, no WASM dependency)
fn login_page_html(error: &str) -> String {
    let error_block = if error.is_empty() {
        String::new()
    } else {
        format!(r#"<div class="error">{}</div>"#, error)
    };

    format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Aleph — Login</title>
<style>
  * {{ margin: 0; padding: 0; box-sizing: border-box; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
         background: #0a0a0f; color: #e0e0e0; display: flex; justify-content: center;
         align-items: center; min-height: 100vh; }}
  .card {{ background: #14141f; border: 1px solid #2a2a3a; border-radius: 16px;
           padding: 40px; max-width: 400px; width: 100%; }}
  h1 {{ font-size: 24px; margin-bottom: 8px; }}
  p {{ color: #888; font-size: 14px; margin-bottom: 24px; }}
  input {{ width: 100%; padding: 12px 16px; background: #0a0a0f; border: 1px solid #2a2a3a;
          border-radius: 8px; color: #e0e0e0; font-size: 16px; margin-bottom: 16px; }}
  input:focus {{ outline: none; border-color: #6366f1; }}
  button {{ width: 100%; padding: 12px; background: #6366f1; color: white; border: none;
           border-radius: 8px; font-size: 16px; cursor: pointer; }}
  button:hover {{ background: #5558e6; }}
  .error {{ background: #3b1419; border: 1px solid #7f1d1d; color: #fca5a5;
            padding: 12px; border-radius: 8px; margin-bottom: 16px; font-size: 14px; }}
</style>
</head>
<body>
<div class="card">
  <h1>Aleph</h1>
  <p>Enter your access token to continue</p>
  {}
  <form method="POST" action="/auth/login">
    <input type="password" name="token" placeholder="Access token" autofocus required>
    <button type="submit">Sign in</button>
  </form>
</div>
</body>
</html>"#, error_block)
}
```

**Step 3: Add `secret()` method to SharedTokenManager and `expiry_hours()` to HttpSessionManager**

In `shared_token.rs`, add:
```rust
pub fn secret(&self) -> &[u8; 32] {
    &self.secret
}
```

In `session.rs`, add:
```rust
pub fn expiry_hours(&self) -> u64 {
    self.expiry_hours
}
```

**Step 4: Register module in `gateway/mod.rs`**

Add `pub mod auth_middleware;`

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib gateway::auth_middleware`
Expected: All PASS

**Step 6: Commit**

```
gateway: add HTTP auth middleware with login page and session cookies
```

---

### Task 5: Wire Auth Into Server Router

**Files:**
- Modify: `src/gateway/server.rs:85-115` (GatewaySharedState, GatewayConfig)
- Modify: `src/gateway/server.rs:272-310` (build_router)
- Modify: `src/gateway/server.rs:400-415` (ConnectionContext)
- Modify: `src/gateway/server.rs:494` (auth gating: `require_auth` → `auth_mode`)

**Step 1: Replace `require_auth: bool` with `auth_mode: AuthMode` in GatewaySharedState**

In `server.rs`:

```rust
// GatewaySharedState: change require_auth to auth_mode
pub auth_mode: AuthMode,   // was: pub require_auth: bool,

// GatewayConfig (the server::GatewayConfig, not config::GatewayConfig):
pub auth_mode: AuthMode,   // was: pub require_auth: bool,

// ConnectionContext:
auth_mode: AuthMode,       // was: require_auth: bool,
```

**Step 2: Update `build_router()` to add middleware**

```rust
pub fn build_router(&self) -> Router {
    let shared = Arc::new(GatewaySharedState {
        // ... existing fields ...
        auth_mode: self.config.auth_mode.clone(),  // was: require_auth
    });

    // Auth state for middleware
    let auth_state = Arc::new(crate::gateway::auth_middleware::AuthState {
        shared_token_mgr: self.shared_token_mgr.clone(),
        session_mgr: self.session_mgr.clone(),
        auth_mode: self.config.auth_mode.clone(),
    });

    // Login routes (no auth needed)
    let login_routes = crate::gateway::auth_middleware::auth_routes(auth_state.clone());

    // Control plane with session middleware
    let control_plane = create_control_plane_router()
        .route_layer(middleware::from_fn_with_state(
            auth_state.clone(),
            crate::gateway::auth_middleware::session_auth_middleware,
        ));

    // OpenAI routes with bearer middleware
    let openai = openai_routes(openai_state)
        .route_layer(middleware::from_fn_with_state(
            auth_state.clone(),
            crate::gateway::auth_middleware::bearer_auth_middleware,
        ));

    Router::new()
        .route("/ws", get(ws_upgrade_handler))
        .merge(login_routes)      // /login, /auth/login, /auth/logout
        .fallback_service(control_plane)  // Panel UI (session-protected)
        .with_state(shared)
        .merge(openai)            // /v1/* (bearer-protected)
}
```

**Step 3: Update auth gating in `handle_connection`**

Replace all `ctx.require_auth` with `ctx.auth_mode.is_auth_required()` (or `matches!(ctx.auth_mode, AuthMode::Token)`).

Specifically at line 494:
```rust
// was: if ctx.require_auth && !is_authenticated {
if ctx.auth_mode.is_auth_required() && !is_authenticated {
```

**Step 4: Update `AuthContext.require_auth` → `auth_mode`**

In `handlers/auth.rs`:
```rust
pub struct AuthContext {
    // ... existing fields ...
    pub auth_mode: AuthMode,  // was: pub require_auth: bool,
}
```

And in `handle_connect()`:
```rust
// was: if !ctx.require_auth {
if !ctx.auth_mode.is_auth_required() {
```

**Step 5: Update all files referencing `require_auth`**

Update the 8 files found by grep to use `auth_mode`:
- `src/bin/aleph/commands/start/builder/subsystems.rs`
- `src/bin/aleph/commands/start/mod.rs`
- `src/gateway/config.rs` (keep the field for compat but don't use it)
- `src/gateway/handlers/config.rs`
- `src/gateway/server.rs`
- `src/config/ui_hints/definitions.rs`
- `src/gateway/handlers/auth.rs`
- `src/gateway/handlers/security_config.rs`

**Step 6: Run tests**

Run: `cargo test -p alephcore --lib`
Expected: All PASS (existing tests may need `require_auth` → `auth_mode` updates)

**Step 7: Commit**

```
gateway: wire auth middleware into server router, replace require_auth with auth_mode
```

---

### Task 6: Shared Token Auto-Generation on Startup

**Files:**
- Modify: `src/bin/aleph/commands/start/mod.rs` (startup sequence)

**Step 1: Add shared token initialization to startup**

In the server startup sequence, after SecurityStore is created:

```rust
// Initialize shared token
let shared_token_mgr = Arc::new(SharedTokenManager::new(security_store.clone()));

// Generate token if none exists (first run)
match shared_token_mgr.get_current_token() {
    Some(_) => {
        info!("Shared token loaded from previous session");
    }
    None => {
        let token = shared_token_mgr.generate_token()
            .expect("Failed to generate shared token");
        info!("========================================");
        info!("  Access token: {}", token);
        info!("========================================");

        // Write to file for reference
        let token_file = data_dir.join(".shared_token");
        if let Err(e) = std::fs::write(&token_file, &token) {
            warn!("Failed to write token file: {}", e);
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600));
            }
            info!("  Token saved to: {}", token_file.display());
        }
    }
}
```

**Step 2: Pass shared_token_mgr and session_mgr to GatewayServer**

The GatewayServer needs these new fields. Add them to whatever builder pattern is used in `start/mod.rs`.

**Step 3: Run full startup test**

Run: `cargo check -p alephcore`
Expected: Compiles successfully

**Step 4: Commit**

```
gateway: auto-generate shared token on first startup
```

---

### Task 7: WebSocket Connect — Shared Token Support

**Files:**
- Modify: `src/gateway/handlers/auth.rs:29-41` (ConnectParams)
- Modify: `src/gateway/handlers/auth.rs:85-240` (handle_connect)

**Step 1: Add `shared_token` to ConnectParams**

```rust
pub struct ConnectParams {
    pub token: Option<String>,
    pub shared_token: Option<String>,  // NEW: shared token for first-time auth
    pub invitation_token: Option<String>,
    pub device_name: Option<String>,
    pub device_type: Option<String>,
    pub device_id: Option<String>,
}
```

**Step 2: Add shared token validation in `handle_connect`**

After the guest invitation check and before the `!ctx.require_auth` block, add shared token validation:

```rust
// Case 0: Shared token authentication (before device token check)
if let Some(shared_token) = &params.shared_token {
    if ctx.shared_token_mgr.validate(shared_token).unwrap_or(false) {
        // Shared token valid — auto-register device and issue device token
        let device_id = params.device_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let device_name = params.device_name.as_deref().unwrap_or("Web Panel");
        // ... register device and issue token (same as current no-auth flow) ...
        info!(device_id = %device_id, "Connection authenticated via shared token");
        return JsonRpcResponse::success(request.id, json!(ConnectResult { ... }));
    } else {
        return JsonRpcResponse::error(request.id, AUTH_FAILED, "Invalid shared token");
    }
}
```

**Step 3: Add `shared_token_mgr` to AuthContext**

```rust
pub struct AuthContext {
    // ... existing fields ...
    pub shared_token_mgr: Arc<SharedTokenManager>,
}
```

**Step 4: Write test**

```rust
#[tokio::test]
async fn test_connect_with_shared_token() {
    let ctx = create_test_context(); // needs to include shared_token_mgr
    // Generate a shared token
    let token = ctx.shared_token_mgr.generate_token().unwrap();

    let request = JsonRpcRequest::new(
        "connect",
        Some(json!({"shared_token": token, "device_name": "Test"})),
        Some(json!(1)),
    );

    let response = handle_connect(request, ctx).await;
    assert!(response.is_success());
}
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib handlers::auth`
Expected: All PASS

**Step 6: Commit**

```
gateway: add shared token authentication to WebSocket connect
```

---

### Task 8: Panel UI — Login Flow & Device Token Storage

**Files:**
- Modify: `apps/panel/src/context.rs` (DashboardState connect flow)
- Modify: `apps/panel/src/lib.rs` (if needed for login redirect)

**Step 1: Update DashboardState::connect to handle auth**

In `apps/panel/src/context.rs`, the connect flow needs to:

1. Read device token from `localStorage`
2. If available, send `connect { token: "stored_token" }`
3. If not, read shared token from `localStorage` (cached from login)
4. Send `connect { shared_token: "xxx" }`
5. On success, store returned device token to `localStorage`

```rust
pub async fn connect(&self) -> Result<(), String> {
    let url = self.gateway_url.get();
    let mut connector = WasmConnector::new();

    match connector.connect(&url).await {
        Ok(()) => {
            // ... existing stream/channel setup ...

            // After connection established, authenticate
            let auth_result = self.authenticate().await;
            if let Err(e) = auth_result {
                // If auth fails, redirect to login
                #[cfg(target_arch = "wasm32")]
                {
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_href("/login");
                    }
                }
                return Err(e);
            }

            Ok(())
        }
        Err(e) => { /* ... existing error handling ... */ }
    }
}

async fn authenticate(&self) -> Result<(), String> {
    // Try stored device token first
    let device_token = get_local_storage("aleph_device_token");
    if let Some(token) = device_token {
        let result = self.rpc_call("connect", serde_json::json!({
            "token": token,
            "device_name": "Web Panel"
        })).await;

        if result.is_ok() {
            return Ok(());
        }
        // Token invalid, clear it
        remove_local_storage("aleph_device_token");
    }

    // Try shared token from localStorage (set during login)
    let shared_token = get_local_storage("aleph_shared_token");
    if let Some(token) = shared_token {
        let result = self.rpc_call("connect", serde_json::json!({
            "shared_token": token,
            "device_name": "Web Panel"
        })).await?;

        // Store device token for future use
        if let Some(device_token) = result.get("token").and_then(|t| t.as_str()) {
            set_local_storage("aleph_device_token", device_token);
        }
        return Ok(());
    }

    Err("No authentication token available".to_string())
}
```

**Step 2: Add localStorage helpers**

```rust
#[cfg(target_arch = "wasm32")]
fn get_local_storage(key: &str) -> Option<String> {
    web_sys::window()?
        .local_storage().ok()??
        .get_item(key).ok()?
}

#[cfg(target_arch = "wasm32")]
fn set_local_storage(key: &str, value: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok()).flatten() {
        let _ = storage.set_item(key, value);
    }
}

#[cfg(target_arch = "wasm32")]
fn remove_local_storage(key: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok()).flatten() {
        let _ = storage.remove_item(key);
    }
}
```

**Step 3: Update login page to cache shared token**

In the login page HTML (`auth_middleware.rs`), add JavaScript to cache the token before form submit:

```javascript
document.querySelector('form').addEventListener('submit', function() {
    var token = document.querySelector('input[name="token"]').value;
    localStorage.setItem('aleph_shared_token', token);
});
```

**Step 4: Commit**

```
panel: add device token storage and shared token auth flow
```

---

### Task 9: LLM Auth Tools

**Files:**
- Create: `src/gateway/handlers/auth_tools.rs`
- Modify: `src/gateway/handlers/mod.rs` (register handlers)

**Step 1: Implement auth management RPC handlers**

```rust
//! Auth management tools exposed as RPC handlers.
//! Follows R9 (Everything is a Tool) — auth config via natural language.

pub async fn handle_auth_show_token(request: JsonRpcRequest, ctx: Arc<AuthContext>) -> JsonRpcResponse {
    match ctx.shared_token_mgr.get_current_token() {
        Some(token) => JsonRpcResponse::success(request.id, json!({"token": token})),
        None => JsonRpcResponse::success(request.id, json!({"token": null, "message": "No token in memory. Check ~/.aleph/data/.shared_token"})),
    }
}

pub async fn handle_auth_reset_token(request: JsonRpcRequest, ctx: Arc<AuthContext>) -> JsonRpcResponse {
    match ctx.shared_token_mgr.generate_token() {
        Ok(token) => {
            // Also update file
            if let Some(home) = dirs::home_dir() {
                let path = home.join(".aleph/data/.shared_token");
                let _ = std::fs::write(&path, &token);
            }
            JsonRpcResponse::success(request.id, json!({"token": token, "message": "Token regenerated. All existing sessions invalidated."}))
        }
        Err(e) => JsonRpcResponse::error(request.id, -32603, format!("Failed: {}", e)),
    }
}

pub async fn handle_auth_list_sessions(request: JsonRpcRequest, ctx: Arc<AuthContext>) -> JsonRpcResponse {
    match ctx.session_mgr.list_sessions() {
        Ok(sessions) => {
            let items: Vec<_> = sessions.iter().map(|s| json!({
                "session_id": s.session_id,
                "created_at": s.created_at,
                "expires_at": s.expires_at,
                "last_used_at": s.last_used_at,
            })).collect();
            JsonRpcResponse::success(request.id, json!({"sessions": items}))
        }
        Err(e) => JsonRpcResponse::error(request.id, -32603, format!("Failed: {}", e)),
    }
}

pub async fn handle_auth_revoke_session(request: JsonRpcRequest, ctx: Arc<AuthContext>) -> JsonRpcResponse {
    let params: serde_json::Value = request.params.unwrap_or(json!({}));
    let session_id = params.get("session_id").and_then(|v| v.as_str());
    match session_id {
        Some(id) => {
            match ctx.session_mgr.revoke_session(id) {
                Ok(()) => JsonRpcResponse::success(request.id, json!({"revoked": true})),
                Err(e) => JsonRpcResponse::error(request.id, -32603, format!("Failed: {}", e)),
            }
        }
        None => JsonRpcResponse::error(request.id, -32602, "Missing session_id parameter"),
    }
}
```

**Step 2: Register handlers in HandlerRegistry**

Register methods: `auth.show_token`, `auth.reset_token`, `auth.list_sessions`, `auth.revoke_session`

**Step 3: Commit**

```
gateway: add auth management LLM tools (R9 principle)
```

---

### Task 10: Cleanup & Final Integration

**Files:**
- All modified files from previous tasks

**Step 1: Remove dead code**

- Remove any remaining standalone `require_auth` usage that isn't the serde compat field
- Remove `TODO: populate from GatewayConfig when token auth is configured` comment in `build_router()`
- Clean up any unused imports

**Step 2: Update `create_hello_notification`**

In `handlers/auth.rs`:
```rust
pub fn create_hello_notification(auth_mode: &AuthMode) -> JsonRpcRequest {
    JsonRpcRequest::notification(
        "hello",
        Some(json!(HelloParams {
            version: "1".to_string(),
            server: format!("aleph-gateway/{}", env!("CARGO_PKG_VERSION")),
            auth_required: auth_mode.is_auth_required(),
        })),
    )
}
```

**Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All PASS

**Step 4: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

**Step 5: Commit**

```
gateway: cleanup old require_auth code, finalize auth integration
```

---

### Task Summary

| Task | Component | Est. Lines |
|------|-----------|-----------|
| 1 | AuthConfig & AuthMode | ~60 |
| 2 | SharedTokenManager | ~120 |
| 3 | HttpSessionManager | ~100 |
| 4 | Login Page & Auth Routes | ~200 |
| 5 | Wire Into Server Router | ~80 (modifications) |
| 6 | Startup Auto-Generation | ~30 |
| 7 | WS Connect Shared Token | ~50 (modifications) |
| 8 | Panel UI Auth Flow | ~80 |
| 9 | LLM Auth Tools | ~80 |
| 10 | Cleanup & Integration | ~30 (deletions) |

**Total:** ~830 lines of new code, ~100 lines of modifications, significant dead code removal
