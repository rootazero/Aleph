# UI Token Authentication Design

Date: 2026-03-12
Status: Approved
Reference: OpenClaw token auth system (~/Workspace/openclaw)

## Problem

Aleph's Leptos Panel UI and HTTP API endpoints are completely unprotected:
- `GET /` serves Panel UI without any authentication
- `POST /v1/*` OpenAI-compatible API has no auth
- `require_auth` defaults to `false`
- Only WebSocket `connect` RPC has authentication

## Design

### Part 1: Configuration Refactoring

Replace `require_auth: bool` with `auth_mode` enum:

```toml
# ~/.aleph/config.toml
[gateway.auth]
mode = "token"              # "token" | "none"
session_expiry_hours = 72   # HTTP session cookie lifetime
token_expiry_hours = 24     # Device token lifetime
# allowed_origins = ["https://my-domain.com"]  # Optional
```

Rust types:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub mode: AuthMode,
    pub session_expiry_hours: u64,
    pub token_expiry_hours: u64,
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    #[default]
    Token,
    None,
}
```

Migration: old `require_auth: bool` deserialized via serde compat (`true` -> Token, `false` -> None).

### Part 2: HTTP Layer Auth (Login Page + Session Cookie)

Flow:
```
Browser -> GET / -> middleware checks "aleph_session" cookie
  -> no cookie -> redirect to /login
  -> /login page -> user enters shared token
  -> POST /auth/login { token } -> validate -> Set-Cookie: aleph_session=<session_id>; HttpOnly; SameSite=Strict
  -> redirect to / -> Panel UI loads
```

Key decisions:
1. Login page: pure HTML via `include_str!()`, not Leptos WASM
2. Session storage: SecurityStore SQLite, new `sessions` table
3. Whitelist routes: `/login`, `/auth/login`, `/auth/logout`
4. `/v1/*` API: `Authorization: Bearer <token:signature>` via TokenManager
5. `/ws` WebSocket: keeps existing `connect` RPC auth
6. `auth_mode: none`: middleware passes all requests through

New routes:
- `GET /login` - login page (no auth needed)
- `POST /auth/login` - validate token, set session cookie
- `POST /auth/logout` - clear session cookie
- `GET /`, `GET /{*path}` - protected by session cookie
- `POST /v1/*`, `POST /a2a/*` - protected by Bearer token

Shared Token (auto-generated on first start):
- Random token generated, hash stored in security.db
- Plaintext written to `~/.aleph/data/.shared_token` (mode 0600)
- Printed to terminal on startup

### Part 3: Three-Layer Auth Model

```
Layer 1: Shared Token (entry key)
  - User enters on first access
  - HTTP login -> session cookie
  - WS connect -> device registration -> device token
  - /v1/* API -> Bearer header validation

Layer 2: Session (HTTP session)
  - Server generates session_id after login
  - HttpOnly cookie, auto-attached
  - 72h expiry, sliding renewal (refresh last_used_at)
  - Only protects Panel UI static resources

Layer 3: Device Token (device identity)
  - Existing HMAC-SHA256 signed token, unchanged
  - Issued on WebSocket connect
  - Device-level permission control and revocation
  - 24h expiry, rotation supported
```

Auth paths per entry point:

| Entry | Auth Method | Validation |
|-------|------------|------------|
| `GET /` (Panel) | Session cookie | middleware -> sessions table |
| `GET /login` | None | whitelist |
| `POST /auth/login` | Shared token (body) | compare shared_token hash -> create session |
| `WS /ws` | connect RPC | shared token -> device register -> device token |
| `POST /v1/*` | Bearer token | `Authorization: Bearer <token:sig>` -> TokenManager |
| `POST /a2a/*` | Bearer token | same as /v1/* |

WebSocket connect changes:
- `auth_mode: token` -> must provide shared token or existing device token
- `auth_mode: none` -> auto-pass (current behavior)
- `ConnectParams` gains `shared_token: Option<String>` field

Panel UI changes:
- On WS connect: read device token from localStorage
- Has device token -> `connect { token: "xxx:sig" }`
- No device token -> `connect { shared_token: "xxx" }` (cached from login)
- Store returned device token to localStorage

### Part 4: Security Hardening & Cleanup

Security additions:
1. Origin validation on WebSocket upgrade (allowlist + same-origin auto)
2. Session cookie attributes: HttpOnly, SameSite=Strict, Secure (auto on HTTPS), Path=/
3. Rate limiting for `/auth/login` (5 failures/minute/IP), reuse existing RateLimiter
4. Shared token file permissions 0600

Code changes:

| Action | File | Details |
|--------|------|---------|
| Modify | `config.rs` | `require_auth: bool` -> `auth: AuthConfig` |
| Modify | `server.rs` | Auth checks use `auth_mode` |
| Modify | `handlers/auth.rs` | `AuthContext.require_auth` -> `auth_mode`, add shared_token path |
| Modify | `control_plane/server.rs` | Add session middleware |
| Add | `gateway/auth_middleware.rs` | Axum session middleware + login routes |
| Add | `gateway/session.rs` | Session CRUD on sessions table |
| Add | `security/shared_token.rs` | Shared token generate/validate/store |
| Modify | `security/store.rs` | Add sessions table DDL |
| Modify | `apps/panel/src/context.rs` | Pass shared_token on connect, store device token in localStorage |
| Add | Panel login page | `include_str!` embedded HTML |

Unchanged (stable):
- TokenManager, PairingManager, DeviceStore, GuestSessionManager
- RateLimiter core logic (only add scope)
- EventBus

New LLM tools (R9 principle):
- `auth.show_token` - display current shared token
- `auth.reset_token` - regenerate shared token
- `auth.list_sessions` - list active HTTP sessions
- `auth.revoke_session` - revoke specific session
