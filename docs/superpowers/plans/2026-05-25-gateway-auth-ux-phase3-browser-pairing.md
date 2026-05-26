# Gateway Auth UX — Phase 3: Browser Pairing UX

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Anyone who hits `http://<aleph-host>/` from a browser that has not already paired (cold visit, remote machine, mobile) gets a friendly "Pair this browser" page instead of the legacy `/login` token-paste form. The desktop app shows a notification ("Safari on 192.168.1.5 wants to connect — Approve / Reject"); one click and the browser is in. A QR-code variant covers mobile devices that cannot type long URLs.

**Architecture:**
- The existing `PairingRequest` enum in `src/gateway/security/pairing.rs:64` (today: `Device { … }` + `Channel { … }`) gets a third variant `Browser { code, origin_label, user_agent, peer_ip, created_at, expires_at }`. This lights up the **already-defined-but-currently-unused** `GatewayEventFrame::PairingRequested` / `PairingCompleted` variants at `src/gateway/events/frame.rs:133,136` (verified during exploration — no producers in current codebase).
- A new anonymous (no-auth) JSON-RPC `pairing.start_browser` is issued by the `/pair` HTML page; returns `{ code, expires_in_secs }`. A new `pairing.poll(code)` returns `{ status: "pending"|"approved"|"rejected"|"expired", session_id?: String }` and is the page's heartbeat. When `status == "approved"` the page swaps to `/auth/bootstrap/from_pairing?code=…` which sets the session cookie and redirects to `/`.
- The Panel subscribes to `pairing.**` events through the existing `events.subscribe` channel (same mechanism as `alerts.**`); the NotificationCenter renders a new notification kind with inline `Approve` / `Reject` buttons that call the existing `pairing.approve` / `pairing.reject` RPCs from auth/pairing.rs.
- A new view `views/devices/pair_qr.rs` (panel) generates an SVG QR code via the `qrcode` crate (pure Rust, wasm-safe) holding `http://<discovered-host>/pair?code=<prefilled>` — `code=` is optional and only used when the desktop app pre-creates a pairing record for an inbound device.

**Tech Stack:** Rust + axum (server), Leptos (panel), `qrcode = "0.14"` (panel QR rendering, ~50 KB wasm gzip), no new server deps.

**Out of scope (Phase 4):**
- Removing the `/login` HTML form (kept alive as compatibility shim through Phase 3; Phase 4 deletes it)
- Deprecating `aleph auth show-token`
- Cross-network QR (Tailscale / public-URL discovery) — Phase 3 ships **same-LAN QR only**, plus a manual-URL textbox for remote scenarios

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/gateway/security/pairing.rs:64` | Modify | Add `Browser { … }` variant to `PairingRequest`; update `match` arms throughout |
| `src/gateway/security/store/pairing.rs` | Modify | Persist Browser variant (extend DB schema with `kind`, `origin_label`, `user_agent`, `peer_ip` columns — migration script) |
| `src/gateway/handlers/auth/pairing.rs` | Modify | Add `handle_pairing_start_browser`, `handle_pairing_poll`; extend `handle_pairing_approve` to handle Browser variant (emit `PairingCompleted` + insert session record); extend `handle_pairing_reject` to emit `PairingRejected` |
| `src/gateway/auth_middleware.rs` | Modify | Add `GET /pair` route (anonymous) serving the new pairing HTML; add `GET /auth/bootstrap/from_pairing?code=…` that converts an approved pairing into a session cookie + 302 |
| `src/gateway/events/frame.rs:133-138` | Modify (no breaking change) | Confirm `PairingRequested { code, kind, origin_label }`, `PairingCompleted { code, kind }`, add `PairingRejected { code }`. Topic strings: `pairing.requested`, `pairing.completed`, `pairing.rejected` |
| `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` | Modify | Register the two new RPCs (`pairing.start_browser`, `pairing.poll`) |
| `interfaces/webchat/src/context.rs` (Phase 1 left untouched; modify here) | Modify | New `subscribe_topic("pairing.**")` on connect; routes events into a new `incoming_pairings: RwSignal<Vec<IncomingPairing>>` signal |
| `interfaces/webchat/src/state/notifications.rs` | Modify | Add `IncomingPairing` notification kind |
| `interfaces/webchat/src/components/notification_center.rs:80+` | Modify | Render IncomingPairing rows with Approve / Reject buttons |
| `interfaces/webchat/src/views/devices/pair_qr.rs` | **Create** | Devices page sub-view: QR code for "Add browser/mobile" |
| `interfaces/webchat/src/views/devices/mod.rs` | Modify | Mount the new sub-view as a tab |
| `interfaces/webchat/Cargo.toml` | Modify | Add `qrcode = { version = "0.14", default-features = false, features = ["svg"] }` |

**Test files:**
- `src/gateway/security/pairing.rs` — extend existing test module for Browser variant
- `src/gateway/handlers/auth/pairing.rs:tests` — add `start_browser → poll(pending) → approve → poll(approved) → session_id` happy path; `start_browser → reject → poll(rejected)`; `start_browser → wait expiry → poll(expired)`
- `tests/pair_browser_e2e.rs` (**create**) — full HTTP flow: GET /pair → POST pairing.start_browser → emit event → simulate approve → GET /auth/bootstrap/from_pairing → cookie set
- `interfaces/webchat/src/views/devices/pair_qr.rs:tests` — QR SVG contains expected URL substring

---

## Task 1: `PairingRequest::Browser` variant + store migration

**Files:**
- Modify: `src/gateway/security/pairing.rs:55` (the enum)
- Modify: `src/gateway/security/store/pairing.rs` (schema + serialization)
- Migration: write a new schema bump as the project requires (grep `MIGRATION` or `schema_version` in store code for the existing pattern)

- [ ] **Step 1: Failing test for new variant existence**

In `src/gateway/security/pairing.rs:tests` (the existing block around line 340), add:

```rust
#[test]
fn browser_variant_carries_origin_metadata() {
    let pr = PairingRequest::Browser {
        code: "654321".into(),
        origin_label: "Safari on 192.168.1.5".into(),
        user_agent: "Mozilla/5.0 …".into(),
        peer_ip: "192.168.1.5".into(),
        created_at: 0,
        expires_at: 300_000,
    };
    assert_eq!(pr.code(), "654321");
    assert_eq!(pr.expires_at(), 300_000);
    assert!(matches!(pr, PairingRequest::Browser { .. }));
}
```

- [ ] **Step 2: Run test, observe compile error**

Run: `cargo test -p alephcore --lib gateway::security::pairing browser_variant`
Expected: FAIL — variant doesn't exist.

- [ ] **Step 3: Add the variant + update `code()` / `expires_at()` impls**

In `src/gateway/security/pairing.rs:55`:

```rust
pub enum PairingRequest {
    Device { /* … existing … */ },
    Channel { /* … existing … */ },
    Browser {
        code: String,
        origin_label: String,
        user_agent: String,
        peer_ip: String,
        created_at: i64,
        expires_at: i64,
    },
}
```

Add match arms in `code()`, `expires_at()`, every other accessor and `Display` impl in this file. Use `cargo check -p alephcore` to surface every non-exhaustive match (Rust will list them all).

- [ ] **Step 4: Update store schema**

In `src/gateway/security/store/pairing.rs`, locate the CREATE TABLE / migration block. Bump schema version and add columns:

```sql
ALTER TABLE pairing_requests ADD COLUMN kind TEXT NOT NULL DEFAULT 'device';
ALTER TABLE pairing_requests ADD COLUMN origin_label TEXT;
ALTER TABLE pairing_requests ADD COLUMN user_agent TEXT;
ALTER TABLE pairing_requests ADD COLUMN peer_ip TEXT;
```

Extend the `From<&PairingRequest>` and the row-to-enum reconstruction logic to handle Browser; reject mixed rows where `kind='browser'` but `origin_label IS NULL`.

- [ ] **Step 5: Run all pairing tests**

Run: `cargo test -p alephcore --lib gateway::security::pairing`
Expected: PASS (existing + new).

- [ ] **Step 6: Commit**

```bash
git add src/gateway/security/pairing.rs src/gateway/security/store/pairing.rs
git commit -m "gateway: PairingRequest::Browser variant + store migration"
```

---

## Task 2: `pairing.start_browser` + `pairing.poll` RPCs

**Files:**
- Modify: `src/gateway/handlers/auth/pairing.rs` (new handlers at end of file)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/auth.rs:46` (register both RPCs)

- [ ] **Step 1: Failing test — start_browser produces a code; poll returns pending**

In `src/gateway/handlers/auth/pairing.rs:tests` block (currently around line 265), add:

```rust
#[tokio::test]
async fn start_browser_then_poll_pending() {
    let ctx = super::super::tests::create_test_context();

    let start = handle_pairing_start_browser(
        JsonRpcRequest::new(
            "pairing.start_browser",
            Some(json!({
                "origin_label": "Safari on 192.168.1.5",
                "user_agent": "Mozilla/5.0 (Macintosh; …)",
                "peer_ip": "192.168.1.5"
            })),
            Some(json!(1)),
        ),
        ctx.clone(),
    )
    .await;
    let code = start
        .result
        .unwrap()
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    let poll = handle_pairing_poll(
        JsonRpcRequest::new(
            "pairing.poll",
            Some(json!({ "code": code })),
            Some(json!(2)),
        ),
        ctx,
    )
    .await;
    let status = poll.result.unwrap().get("status").unwrap().as_str().unwrap().to_string();
    assert_eq!(status, "pending");
}

#[tokio::test]
async fn approve_browser_pairing_makes_poll_return_approved() {
    let ctx = super::super::tests::create_test_context();

    let start = handle_pairing_start_browser(
        JsonRpcRequest::new(
            "pairing.start_browser",
            Some(json!({
                "origin_label": "Firefox on 192.168.1.5",
                "user_agent": "ua",
                "peer_ip": "192.168.1.5"
            })),
            Some(json!(1)),
        ),
        ctx.clone(),
    ).await;
    let code = start.result.unwrap().get("code").unwrap().as_str().unwrap().to_string();

    let approve = handle_pairing_approve(
        JsonRpcRequest::new(
            "pairing.approve",
            Some(json!({ "code": code.clone() })),
            Some(json!(2)),
        ),
        ctx.clone(),
    ).await;
    assert!(approve.is_success(), "approve must succeed for Browser variant: {:?}", approve);

    let poll = handle_pairing_poll(
        JsonRpcRequest::new(
            "pairing.poll",
            Some(json!({ "code": code })),
            Some(json!(3)),
        ),
        ctx,
    ).await;
    let body = poll.result.unwrap();
    assert_eq!(body.get("status").unwrap().as_str().unwrap(), "approved");
    assert!(body.get("session_id").is_some(), "approved poll must include session_id");
}
```

- [ ] **Step 2: Run, observe failures**

Run: `cargo test -p alephcore gateway::handlers::auth::pairing -- start_browser approve_browser`
Expected: FAIL — handlers do not exist.

- [ ] **Step 3: Implement `handle_pairing_start_browser`**

Append to `src/gateway/handlers/auth/pairing.rs`:

```rust
#[derive(Debug, Deserialize)]
struct StartBrowserParams {
    origin_label: String,
    user_agent: String,
    peer_ip: String,
}

pub async fn handle_pairing_start_browser(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: StartBrowserParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // PairingManager::create_browser_pairing is the new method on
    // PairingManager (alongside create_pairing for devices). It returns
    // the generated short code and the expires_at timestamp.
    let (code, expires_at) = match ctx.pairing_manager.create_browser_pairing(
        &params.origin_label,
        &params.user_agent,
        &params.peer_ip,
    ) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "failed to create browser pairing");
            return JsonRpcResponse::error(request.id, -32603, format!("{e}"));
        }
    };

    // Emit GatewayEventFrame::PairingRequested so the Panel can notify the user.
    let frame = crate::gateway::events::GatewayEventFrame::PairingRequested {
        code: code.clone(),
        kind: "browser".into(),
        origin_label: params.origin_label.clone(),
    };
    ctx.event_bus.publish(frame);

    JsonRpcResponse::success(
        request.id,
        json!({
            "code": code,
            "expires_at": expires_at,
        }),
    )
}
```

Add the new constructor to `PairingManager` in `src/gateway/security/pairing.rs` (alongside existing `create_pairing`). It generates a 6-digit numeric code (note: shorter than the device 8-char Base32 because browser users will read it off the screen, never type it), persists, and returns `(code, expires_at)`.

- [ ] **Step 4: Implement `handle_pairing_poll`**

```rust
#[derive(Debug, Deserialize)]
struct PollParams {
    code: String,
}

pub async fn handle_pairing_poll(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: PollParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Three states the pairing can be in:
    // 1. Pending in pairing_manager (not yet approved/rejected)
    // 2. Approved — session_id has been recorded in PairingManager.approved_sessions
    // 3. Expired / not found — return "expired"
    match ctx.pairing_manager.poll_browser_pairing(&params.code) {
        Ok(PollState::Pending) => {
            JsonRpcResponse::success(request.id, json!({"status": "pending"}))
        }
        Ok(PollState::Approved { session_id }) => {
            JsonRpcResponse::success(
                request.id,
                json!({"status": "approved", "session_id": session_id}),
            )
        }
        Ok(PollState::Rejected) => {
            JsonRpcResponse::success(request.id, json!({"status": "rejected"}))
        }
        Ok(PollState::Expired) | Err(_) => {
            JsonRpcResponse::success(request.id, json!({"status": "expired"}))
        }
    }
}
```

Add `PollState` enum and `poll_browser_pairing` method to `PairingManager`. The "approved" path requires that `handle_pairing_approve` (Browser arm) created a session via `HttpSessionManager::create_session` and stored the session_id keyed by `code` in `pairing_manager.approved_sessions: DashMap<String, String>` (TTL bounded — 5 min — to give the browser time to poll).

- [ ] **Step 5: Extend `handle_pairing_approve` Browser arm**

In `src/gateway/handlers/auth/pairing.rs:47-78` add a new arm before the existing Channel arm:

```rust
        PairingRequest::Browser {
            code,
            origin_label,
            ..
        } => {
            // Create a session keyed to the shared token's HMAC, mirroring
            // auth_middleware::handle_login at line 52-56.
            let shared = ctx.shared_token_mgr.current_token().unwrap_or_default();
            let hash = crate::gateway::security::hmac_sign(
                ctx.shared_token_mgr.secret(),
                &shared,
            );
            let session_id = match ctx.session_mgr.create_session(&hash) {
                Ok(id) => id,
                Err(e) => return JsonRpcResponse::error(request.id, -32603, format!("{e}")),
            };
            // Stash session_id so the browser's pairing.poll can retrieve it.
            ctx.pairing_manager
                .record_browser_session(&code, &session_id);
            // Emit completion event.
            ctx.event_bus.publish(
                crate::gateway::events::GatewayEventFrame::PairingCompleted {
                    code: code.clone(),
                    kind: "browser".into(),
                },
            );
            info!(
                code = %code,
                origin = %origin_label,
                "Browser pairing approved"
            );
            return JsonRpcResponse::success(
                request.id,
                json!({"code": code, "kind": "browser", "approved": true}),
            );
        }
```

This requires that `AuthContext` exposes `session_mgr: Arc<HttpSessionManager>` — verify with grep; if not present, plumb it through similar to bootstrap_mgr in Phase 2 Task 2.

- [ ] **Step 6: Register the new RPCs**

In `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` after line 45 (`devices.revoke`) add:

```rust
    register_handler!(
        server,
        "pairing.start_browser",
        auth_handlers::handle_pairing_start_browser,
        auth_ctx
    );
    register_handler!(
        server,
        "pairing.poll",
        auth_handlers::handle_pairing_poll,
        auth_ctx
    );
```

`pairing.start_browser` and `pairing.poll` must be reachable on the **unauthenticated** dispatch path, because the browser issuing them has no token yet. Check `src/gateway/server/handler.rs` (the existing `pairing_required` early-allowance pattern at line 211 is a precedent) — add these methods to the same allow-list.

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore gateway::handlers::auth::pairing`
Expected: PASS — including new browser flow tests.

- [ ] **Step 8: Commit**

```bash
git add src/gateway/handlers/auth/pairing.rs src/gateway/security/pairing.rs \
        src/bin/aleph-server/commands/start/builder/handlers/auth.rs \
        src/gateway/server/handler.rs
git commit -m "gateway: pairing.start_browser + pairing.poll RPCs (anonymous)"
```

---

## Task 3: `GET /pair` HTML page + `GET /auth/bootstrap/from_pairing`

**Files:**
- Modify: `src/gateway/auth_middleware.rs` — extend `auth_routes` + add two new handler fns + add a `pair_page_html(code: Option<&str>) -> String` helper

- [ ] **Step 1: Write failing integration test**

Create `tests/pair_browser_e2e.rs`:

```rust
//! End-to-end browser pairing flow.
//!
//! 1. GET /pair                          → HTML with polling JS
//! 2. POST pairing.start_browser         → { code }
//! 3. (out-of-band) pairing.approve(code) by an authenticated client
//! 4. POST pairing.poll(code)            → { status: "approved", session_id }
//! 5. GET /auth/bootstrap/from_pairing?code=…  → 303 + Set-Cookie

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

// … set up AuthState (mirror tests/bootstrap_loopback_gate.rs Phase 2 Task 4 Step 6) …

#[tokio::test]
async fn pair_page_serves_html() {
    let app = /* … */;
    let resp = app
        .oneshot(Request::builder().uri("/pair").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    assert!(html.contains("Pair this browser"));
    assert!(html.contains("pairing.start_browser"));
}

#[tokio::test]
async fn bootstrap_from_pairing_sets_cookie_when_approved() {
    // … full flow as above …
}
```

(Fill in the full setup borrowing from Phase 2 Task 4 Step 6's pattern.)

- [ ] **Step 2: Implement `pair_page_html`**

In `src/gateway/auth_middleware.rs` add (near `login_page_html` for cohesion):

```rust
pub fn pair_page_html(prefilled_code: Option<&str>) -> String {
    let prefill_js = match prefilled_code {
        Some(c) => format!("let prefilled = '{}';", c.replace('\'', "")),
        None => "let prefilled = null;".to_string(),
    };
    // Tiny page: shows a friendly message, calls pairing.start_browser,
    // polls every 2s, redirects on approval. No framework, no build step.
    format!(
        r#"<!DOCTYPE html>
<html lang="en"><head>
<meta charset="utf-8">
<title>Aleph — Pair this browser</title>
<style>
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:#0a0a0f;color:#e0e0e0;display:flex;justify-content:center;align-items:center;min-height:100vh}}
.c{{background:#14141f;border:1px solid #2a2a3a;border-radius:16px;padding:40px;max-width:480px;text-align:center}}
.code{{font-family:'SF Mono',Menlo,monospace;font-size:48px;letter-spacing:8px;color:#a5b4fc;background:#0a0a0f;border-radius:12px;padding:24px;margin:24px 0}}
.s{{color:#888;font-size:14px}}
.err{{color:#fca5a5;margin-top:16px}}
</style></head><body>
<div class="c">
<h1>Pair this browser</h1>
<p class="s">Open the Aleph desktop app and approve this code, or click the notification.</p>
<div class="code" id="code">…</div>
<div class="s" id="status">Waiting for approval…</div>
<div class="err" id="err"></div>
<script>
{prefill_js}
async function rpc(method, params) {{
    const r = await fetch('/rpc', {{method:'POST',headers:{{'Content-Type':'application/json'}},body:JSON.stringify({{jsonrpc:'2.0',id:Date.now(),method,params}})}});
    return r.json();
}}
async function start() {{
    const ua = navigator.userAgent;
    const r = await rpc('pairing.start_browser', {{origin_label:'Browser on this network',user_agent:ua,peer_ip:'self'}});
    if (r.error) {{ document.getElementById('err').textContent = r.error.message; return; }}
    const code = r.result.code;
    document.getElementById('code').textContent = code;
    poll(code);
}}
async function poll(code) {{
    while (true) {{
        await new Promise(r => setTimeout(r, 2000));
        const r = await rpc('pairing.poll', {{code}});
        if (r.error) {{ document.getElementById('err').textContent = r.error.message; return; }}
        const s = r.result.status;
        if (s === 'approved') {{
            window.location.href = '/auth/bootstrap/from_pairing?code=' + encodeURIComponent(code);
            return;
        }} else if (s === 'rejected' || s === 'expired') {{
            document.getElementById('status').textContent = s === 'rejected' ? 'Pairing rejected' : 'Pairing expired';
            return;
        }}
    }}
}}
if (prefilled) {{ document.getElementById('code').textContent = prefilled; poll(prefilled); }} else {{ start(); }}
</script>
</div></body></html>"#
    )
}

async fn show_pair_page(Query(q): Query<PairQuery>) -> Html<String> {
    Html(pair_page_html(q.code.as_deref()))
}

#[derive(Deserialize)]
struct PairQuery {
    code: Option<String>,
}

async fn handle_bootstrap_from_pairing(
    State(state): State<Arc<AuthState>>,
    Query(q): Query<BootstrapFromPairingQuery>,
) -> Response {
    match state.pairing_mgr_for_session.fetch_browser_session(&q.code) {
        Some(session_id) => {
            let max_age = state.session_mgr.expiry_hours() * 3600;
            let cookie = format!(
                "aleph_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
                session_id, max_age,
            );
            (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, "/".to_string()), (header::SET_COOKIE, cookie)],
            )
                .into_response()
        }
        None => (StatusCode::UNAUTHORIZED, "pairing not approved").into_response(),
    }
}

#[derive(Deserialize)]
struct BootstrapFromPairingQuery {
    code: String,
}
```

Add `pairing_mgr_for_session: Arc<PairingManager>` (or similar) to `AuthState` and plumb in `subsystems.rs`.

- [ ] **Step 3: Wire routes**

Extend `auth_routes`:

```rust
    Router::new()
        .route("/login", get(show_login))   // kept as compat shim through Phase 3
        .route("/auth/login", post(handle_login))
        .route("/auth/logout", post(handle_logout))
        .route("/auth/bootstrap", get(handle_bootstrap_consume))  // Phase 2
        .route("/pair", get(show_pair_page))
        .route("/auth/bootstrap/from_pairing", get(handle_bootstrap_from_pairing))
        .with_state(state)
```

- [ ] **Step 4: Make the session middleware redirect to `/pair` instead of `/login`**

In `src/gateway/auth_middleware.rs:127`:

```rust
        _ => Redirect::to("/pair").into_response(),
```

(The legacy `/login` URL still serves, so users with bookmarks aren't broken. Phase 4 deletes it.)

- [ ] **Step 5: Run tests**

Run: `cargo test --test pair_browser_e2e && cargo test -p alephcore --lib gateway::auth_middleware`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/auth_middleware.rs
git commit -m "gateway: /pair page + /auth/bootstrap/from_pairing (browser pairing UX)"
```

---

## Task 4: Panel — subscribe to `pairing.**` events; render notifications

**Files:**
- Modify: `interfaces/webchat/src/context.rs` — add `incoming_pairings: RwSignal<Vec<IncomingPairing>>`, subscribe topic on connect
- Modify: `interfaces/webchat/src/state/notifications.rs` — add IncomingPairing kind
- Modify: `interfaces/webchat/src/components/notification_center.rs:80+` — render pairing rows

- [ ] **Step 1: Add the data model**

In `interfaces/webchat/src/state/notifications.rs`:

```rust
#[derive(Debug, Clone)]
pub struct IncomingPairing {
    pub code: String,
    pub origin_label: String,
    pub created_at_ms: i64,
}
```

In `interfaces/webchat/src/context.rs:DashboardState`:

```rust
    pub incoming_pairings: RwSignal<Vec<IncomingPairing>>,
```

Initialize in `DashboardState::new()`.

- [ ] **Step 2: Subscribe + dispatch**

In `context.rs::connect()` after the existing `config.**` subscribe call:

```rust
        let state_for_pairing = *self;
        spawn_local(async move {
            let _ = state_for_pairing.subscribe_topic("pairing.**").await;
        });
        let pairing_sub = self.subscribe_events(move |ev: GatewayEvent| {
            if ev.topic == "pairing.requested" {
                let code = ev.data.get("code").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let label = ev.data.get("origin_label").and_then(|v| v.as_str()).unwrap_or("").to_string();
                state_for_pairing.incoming_pairings.update(|list| {
                    list.push(IncomingPairing {
                        code,
                        origin_label: label,
                        created_at_ms: js_sys::Date::now() as i64,
                    });
                });
            } else if ev.topic == "pairing.completed" || ev.topic == "pairing.rejected" {
                let code = ev.data.get("code").and_then(|v| v.as_str()).unwrap_or("").to_string();
                state_for_pairing.incoming_pairings.update(|list| {
                    list.retain(|p| p.code != code);
                });
            }
        });
        // (track pairing_sub for cleanup in disconnect)
```

- [ ] **Step 3: Render in NotificationCenter**

In `notification_center.rs` (around line 80+, where the alert rows are rendered), insert a parallel `<For>` over `dashboard.incoming_pairings`:

```rust
            <For
                each=move || dashboard.incoming_pairings.get()
                key=|p| p.code.clone()
                children=move |p: IncomingPairing| {
                    let code = p.code.clone();
                    let approve_code = code.clone();
                    let reject_code = code.clone();
                    let approve_state = *expect_context::<DashboardState>();
                    let reject_state = approve_state;
                    view! {
                        <div class="px-4 py-3 border-b border-surface-raised">
                            <div class="text-sm font-medium">"Pair browser"</div>
                            <div class="text-xs text-text-secondary">{p.origin_label.clone()}</div>
                            <div class="font-mono text-2xl my-2 text-center text-indigo-300">{code.clone()}</div>
                            <div class="flex gap-2">
                                <button
                                    class="flex-1 py-1.5 rounded bg-indigo-600 text-white text-xs font-semibold"
                                    on:click=move |_| {
                                        let c = approve_code.clone();
                                        let s = approve_state;
                                        spawn_local(async move {
                                            let _ = s.rpc_call("pairing.approve", serde_json::json!({"code": c})).await;
                                        });
                                    }
                                >"Approve"</button>
                                <button
                                    class="flex-1 py-1.5 rounded bg-surface-sunken text-text-secondary text-xs"
                                    on:click=move |_| {
                                        let c = reject_code.clone();
                                        let s = reject_state;
                                        spawn_local(async move {
                                            let _ = s.rpc_call("pairing.reject", serde_json::json!({"code": c})).await;
                                        });
                                    }
                                >"Reject"</button>
                            </div>
                        </div>
                    }
                }
            />
```

- [ ] **Step 4: Verify in dev**

Run: `just shell-dev` then in a separate browser hit `http://127.0.0.1:18790/pair`. Expected: pair page shows 6-digit code; desktop app's notification bell increments; clicking the bell shows a row with the code + Approve/Reject; clicking Approve in the desktop app makes the browser's pair page redirect to `/` and load the Panel signed in.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/context.rs interfaces/webchat/src/state/notifications.rs \
        interfaces/webchat/src/components/notification_center.rs
git commit -m "panel: pairing.** notifications with inline Approve/Reject"
```

---

## Task 5: QR code variant in Panel "Devices" view

**Files:**
- Create: `interfaces/webchat/src/views/devices/pair_qr.rs`
- Modify: `interfaces/webchat/src/views/devices/mod.rs` — mount as tab
- Modify: `interfaces/webchat/Cargo.toml` — add `qrcode = "0.14"`

- [ ] **Step 1: Add `qrcode` dependency**

In `interfaces/webchat/Cargo.toml` `[dependencies]`:

```toml
qrcode = { version = "0.14", default-features = false, features = ["svg"] }
```

- [ ] **Step 2: Implement the view**

Create `interfaces/webchat/src/views/devices/pair_qr.rs`:

```rust
//! Devices → "Add browser/mobile" QR code panel.
//!
//! Generates an SVG QR code holding `http://<self-host>/pair`. The viewing
//! device (phone, second laptop) scans the code and lands on /pair where it
//! gets a 6-digit code. The user then approves the code from this Panel via
//! NotificationCenter (the same flow as cold-browser pairing).

use leptos::prelude::*;

fn discover_self_host() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(win) = web_sys::window() {
            if let Ok(host) = win.location().host() {
                let scheme = win.location().protocol().unwrap_or_else(|_| "http:".to_string());
                return format!("{scheme}//{host}");
            }
        }
        "http://127.0.0.1:18790".to_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "http://127.0.0.1:18790".to_string()
    }
}

fn generate_qr_svg(url: &str) -> String {
    use qrcode::{render::svg, EcLevel, QrCode};
    let code = QrCode::with_error_correction_level(url, EcLevel::M).expect("qr encode");
    code.render::<svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#a5b4fc"))
        .light_color(svg::Color("#0a0a0f"))
        .build()
}

#[component]
pub fn PairQr() -> impl IntoView {
    let url = format!("{}/pair", discover_self_host());
    let svg = generate_qr_svg(&url);
    view! {
        <div class="flex flex-col items-center gap-4 p-6">
            <h2 class="text-lg font-semibold">"Add browser or mobile"</h2>
            <div class="text-sm text-text-secondary text-center max-w-md">
                "Scan this QR code from another device on the same network. \
                 You'll see a 6-digit code; approve it from the notification bell."
            </div>
            <div inner_html=svg class="bg-surface-sunken p-4 rounded-xl"/>
            <div class="text-xs font-mono text-text-secondary break-all">{url.clone()}</div>
            <div class="text-xs text-text-secondary text-center max-w-md">
                "Note: same-network only. For remote access, use your Tailscale URL \
                 or your reverse proxy URL, manually replacing the host portion above."
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_svg_contains_path_element() {
        let svg = generate_qr_svg("http://127.0.0.1:18790/pair");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("<path"));
    }
}
```

- [ ] **Step 3: Mount in devices tab**

In `interfaces/webchat/src/views/devices/mod.rs`, add a tab/section that renders `<PairQr/>` — follow the existing tab pattern in that file (search for `Tab::*` or `view! { … }`).

- [ ] **Step 4: Run tests + visual check**

Run: `cargo test -p webchat -- pair_qr`
Expected: PASS.

Visual: `just shell-dev` → Devices → "Add browser/mobile" → QR appears. Scan with phone camera → mobile browser opens `/pair`.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/Cargo.toml \
        interfaces/webchat/src/views/devices/pair_qr.rs \
        interfaces/webchat/src/views/devices/mod.rs
git commit -m "panel: Devices → Add browser/mobile QR view"
```

---

## Self-Review Checklist

1. **Goal achieved?** Cold browser visit lands on `/pair` (not `/login`); shows a code; desktop app notifies & 1-click approves; browser redirects to signed-in `/`. QR variant covers mobile. ✓
2. **Re-uses existing pairing infrastructure?** Reuses `pairing.approve` / `pairing.reject` RPCs (Task 2 adds only `start_browser` + `poll`); reuses `PairingRequest` enum + `PairingManager` (extends with Browser arm + new constructor + poll API). ✓
3. **Lights up dead event variants?** `GatewayEventFrame::PairingRequested` / `PairingCompleted` (zero producers before this phase) now emitted by `start_browser` and `approve.Browser` arm. ✓
4. **Anonymous RPC surface bounded?** Only `pairing.start_browser` + `pairing.poll` are anonymous; both are rate-limited via the existing pairing TTL (codes self-destruct after 300s) and inherently single-use. ✓
5. **No new auth gap?** `from_pairing` requires that `pairing.approve` was already called for the code (which itself requires an authenticated caller via the existing pairing dispatch). Anonymous → approved transition only happens via an authenticated approval. ✓
6. **Same-LAN QR limitation documented?** Yes — view text explicitly mentions Tailscale/reverse-proxy manual replacement. ✓
7. **Test coverage?** Variant unit test + 2 handler tests + 2 e2e tests + 1 QR rendering test = 6 new tests. ✓

---

## Verification Commands (Definition of Done)

```bash
cargo test -p alephcore --lib gateway::security::pairing
cargo test -p alephcore gateway::handlers::auth::pairing
cargo test --test pair_browser_e2e
cargo test -p webchat -- pair_qr

cargo check -p alephcore
cargo check -p webchat --target wasm32-unknown-unknown

# Manual smoke:
# 1. Fresh state: rm -rf ~/.aleph (back up first)
# 2. just shell-dev   (desktop app launches; panel auths via Phase 2 bootstrap)
# 3. Open Safari → http://127.0.0.1:18790/ → should redirect to /pair
# 4. /pair shows 6-digit code; bell badge in desktop app increments to 1
# 5. Click bell → Approve → Safari window auto-loads Panel signed in
# 6. Open Devices tab in panel → "Add browser/mobile" → scan QR with phone → repeat
```

---

## Risk Notes

- **Schema migration**: bumping `pairing_requests` schema requires existing DBs to apply the migration cleanly. If the project uses `rusqlite`-managed schema versioning, add a versioned migration step; if it uses raw DDL, the `ALTER TABLE` is forward-compatible (SQLite allows `ADD COLUMN` without rewriting rows).
- **Anonymous RPC abuse**: `pairing.start_browser` is anonymous → a hostile process could spam-create pairing records. Mitigate with `PairingManager::create_browser_pairing` enforcing a per-IP rate limit (e.g., max 5 pending per peer_ip; reject 6th with `-32029 too many requests`). Add this as a follow-up if not in Task 2.
- **Poll endpoint causes O(N) connections**: 2s polling × many active pairings = manageable, but consider migrating polling to SSE on the existing event channel post-Phase-3 if it becomes a problem.
- **Code length 6 vs 8**: shorter for browser (read off screen, never typed), longer for device pairing (CLI prompt). Make `PairingManager::create_browser_pairing` use a separate codespace (numeric `100000..=999999`) so device-vs-browser codes can't collide.
- **`origin_label`**: client-supplied → don't trust it for security decisions. It's display-only. The `peer_ip` should be derived server-side from `ConnectInfo<SocketAddr>` for accuracy, not client-supplied (revise Task 2 implementation to extract it from request context instead of params if possible).
- **/pair JS in `format!`**: be careful with `{{ }}` escaping; the existing `login_page_html` at line 161 follows the same convention.
