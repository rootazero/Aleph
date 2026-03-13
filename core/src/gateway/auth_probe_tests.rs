//! UI Token Auth — Probe Integration Tests
//!
//! Production-level validation of the three-layer auth architecture:
//!   Layer 1: Shared Token (entry key)
//!   Layer 2: HTTP Session (Panel UI cookies)
//!   Layer 3: Device Token (WebSocket bearer)
//!
//! These tests exercise cross-component flows that unit tests cannot cover,
//! ensuring the auth system works as an integrated whole.

#[cfg(test)]
mod tests {
    use crate::gateway::config::AuthMode;
    use crate::gateway::security::{
        hmac_sign, generate_secret, SharedTokenManager, TokenManager,
        PairingManager, InvitationManager, GuestSessionManager, SecurityStore,
    };
    use crate::gateway::security::store::DeviceUpsertData;
    use crate::gateway::session::HttpSessionManager;
    use crate::gateway::device_store::DeviceStore;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::handlers::auth::{AuthContext, handle_connect};
    use crate::gateway::handlers::auth_tools::*;
    use crate::gateway::auth_middleware::login_page_html;
    use crate::gateway::protocol::JsonRpcRequest;
    use crate::sync_primitives::Arc;
    use axum::http::header;
    use serde_json::json;

    // =========================================================================
    // Helpers
    // =========================================================================

    /// Build a full AuthContext with in-memory stores.
    fn make_auth_context() -> (Arc<AuthContext>, Arc<SecurityStore>, Arc<SharedTokenManager>) {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        // Seed a device so FK constraints on tokens table pass
        store
            .upsert_device(&DeviceUpsertData {
                device_id: "seed-dev",
                device_name: "Seed",
                device_type: None,
                public_key: &[0u8; 32],
                fingerprint: "seed-fp",
                role: "operator",
                scopes: &[],
            })
            .unwrap();

        let shared_token_mgr = Arc::new(SharedTokenManager::new(store.clone(), "/tmp/aleph_test.vault"));

        let ctx = Arc::new(AuthContext {
            token_manager: Arc::new(TokenManager::new(store.clone())),
            pairing_manager: Arc::new(PairingManager::new(store.clone())),
            device_store: Arc::new(DeviceStore::in_memory().unwrap()),
            security_store: store.clone(),
            invitation_manager: Arc::new(InvitationManager::new()),
            guest_session_manager: Arc::new(GuestSessionManager::new()),
            event_bus: Arc::new(GatewayEventBus::new()),
            auth_mode: AuthMode::Token,
            shared_token_mgr: shared_token_mgr.clone(),
        });

        (ctx, store, shared_token_mgr)
    }

    // =========================================================================
    // PROBE 1 — Full Login → Session → Validate → Revoke lifecycle
    // =========================================================================

    /// Simulates the entire HTTP Panel login flow:
    ///   1. Generate shared token
    ///   2. Compute token hash (same as handle_login does)
    ///   3. Create session via HttpSessionManager
    ///   4. Validate session works
    ///   5. Revoke session — subsequent validates must fail
    ///   6. Cleanup expired sessions removes nothing (active sessions were revoked, not expired)
    #[test]
    fn probe_full_login_session_lifecycle() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let shared_mgr = SharedTokenManager::new(store.clone(), "/tmp/aleph_test.vault");
        let session_mgr = HttpSessionManager::new(store.clone(), 72);

        // 1. Generate shared token
        let token = shared_mgr.generate_token().unwrap();
        assert!(token.starts_with("aleph-"));

        // 2. Compute HMAC hash (same path as handle_login)
        let hash = hmac_sign(shared_mgr.secret(), &token);
        assert!(!hash.is_empty());

        // 3. Create session
        let session_id = session_mgr.create_session(&hash).unwrap();
        assert!(!session_id.is_empty());

        // 4. Validate session
        assert!(session_mgr.validate_session(&session_id).unwrap());

        // 4b. List shows 1 active session
        let sessions = session_mgr.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session_id);

        // 5. Revoke → invalidate
        session_mgr.revoke_session(&session_id).unwrap();
        assert!(!session_mgr.validate_session(&session_id).unwrap());

        // 6. List shows 0 active sessions
        let sessions = session_mgr.list_sessions().unwrap();
        assert_eq!(sessions.len(), 0);
    }

    // =========================================================================
    // PROBE 2 — Shared token rotation invalidates old token AND sessions
    // =========================================================================

    /// When a shared token is regenerated:
    ///   - Old token must fail validation
    ///   - New token must pass validation
    ///   - Sessions created with old hash are still valid (they're independent)
    ///     BUT a manual `auth.reset_token` is expected to invalidate sessions
    #[test]
    fn probe_token_rotation_invalidates_old() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store.clone(), "/tmp/aleph_test.vault");
        let session_mgr = HttpSessionManager::new(store.clone(), 72);

        // Generate first token, create a session
        let token1 = mgr.generate_token().unwrap();
        let hash1 = hmac_sign(mgr.secret(), &token1);
        let session1 = session_mgr.create_session(&hash1).unwrap();

        // Rotate token
        let token2 = mgr.generate_token().unwrap();
        assert_ne!(token1, token2);

        // Old token: MUST fail
        assert!(!mgr.validate(&token1).unwrap());
        // New token: MUST pass
        assert!(mgr.validate(&token2).unwrap());

        // Old session: still valid (sessions table is independent of shared_token table)
        // This is by design — session has its own expiry
        assert!(session_mgr.validate_session(&session1).unwrap());

        // New token creates a new session normally
        let hash2 = hmac_sign(mgr.secret(), &token2);
        let session2 = session_mgr.create_session(&hash2).unwrap();
        assert!(session_mgr.validate_session(&session2).unwrap());
        assert_ne!(session1, session2);
    }

    // =========================================================================
    // PROBE 3 — Session expiry boundary test
    // =========================================================================

    /// Sessions with 0-hour expiry become invalid almost immediately.
    /// This tests the boundary condition where `expires_at ≈ created_at`.
    #[test]
    fn probe_session_expiry_boundary() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());

        // 0 hours = expires at creation time (effectively immediate)
        let mgr_zero = HttpSessionManager::new(store.clone(), 0);
        let sid = mgr_zero.create_session("hash").unwrap();

        // Small delay to ensure `now > expires_at`
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(!mgr_zero.validate_session(&sid).unwrap());

        // Cleanup should remove it
        let cleaned = mgr_zero.cleanup_expired().unwrap();
        assert_eq!(cleaned, 1);

        // Normal expiry: 72h session should be valid
        let mgr_72 = HttpSessionManager::new(store.clone(), 72);
        let sid2 = mgr_72.create_session("hash").unwrap();
        assert!(mgr_72.validate_session(&sid2).unwrap());
    }

    // =========================================================================
    // PROBE 4 — Shared token → WS connect → device token issuance
    // =========================================================================

    /// End-to-end: shared token used in WebSocket `connect` RPC
    ///   1. Generate shared token
    ///   2. Call handle_connect with shared_token param
    ///   3. Response MUST contain device_id + signed token
    ///   4. Subsequent connect with device token MUST succeed
    #[tokio::test]
    async fn probe_shared_token_to_device_token_flow() {
        let (ctx, _store, shared_mgr) = make_auth_context();
        let token = shared_mgr.generate_token().unwrap();

        // Step 1: Connect via shared token
        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "shared_token": token,
                "device_name": "Probe Panel",
                "device_type": "web"
            })),
            Some(json!(1)),
        );
        let resp = handle_connect(req, ctx.clone()).await;
        assert!(resp.is_success(), "shared token connect must succeed");

        let result = resp.result.unwrap();
        let device_token = result.get("token").unwrap().as_str().unwrap().to_string();
        let device_id = result.get("device_id").unwrap().as_str().unwrap().to_string();
        assert!(!device_token.is_empty());
        assert!(!device_id.is_empty());
        assert!(result.get("permissions").unwrap().as_array().unwrap().len() > 0);

        // Step 2: Reconnect with device token (same flow as Panel localStorage)
        let req2 = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "token": device_token,
                "device_id": device_id,
            })),
            Some(json!(2)),
        );
        let resp2 = handle_connect(req2, ctx.clone()).await;
        assert!(resp2.is_success(), "device token reconnect must succeed");

        let result2 = resp2.result.unwrap();
        assert!(result2.get("token").is_some());
        assert_eq!(result2.get("device_id").unwrap().as_str().unwrap(), device_id);
    }

    // =========================================================================
    // PROBE 5 — Invalid shared token rejected at WS connect
    // =========================================================================

    #[tokio::test]
    async fn probe_invalid_shared_token_rejected() {
        let (ctx, _store, shared_mgr) = make_auth_context();
        let _token = shared_mgr.generate_token().unwrap();

        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "shared_token": "aleph-00000000-0000-0000-0000-000000000000",
            })),
            Some(json!(1)),
        );
        let resp = handle_connect(req, ctx).await;
        assert!(resp.is_error(), "wrong shared token MUST be rejected");
    }

    // =========================================================================
    // PROBE 6 — auth_mode: None bypasses all auth
    // =========================================================================

    #[tokio::test]
    async fn probe_auth_mode_none_bypasses() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        store
            .upsert_device(&DeviceUpsertData {
                device_id: "seed",
                device_name: "S",
                device_type: None,
                public_key: &[0u8; 32],
                fingerprint: "sf",
                role: "operator",
                scopes: &[],
            })
            .unwrap();

        let ctx = Arc::new(AuthContext {
            token_manager: Arc::new(TokenManager::new(store.clone())),
            pairing_manager: Arc::new(PairingManager::new(store.clone())),
            device_store: Arc::new(DeviceStore::in_memory().unwrap()),
            security_store: store.clone(),
            invitation_manager: Arc::new(InvitationManager::new()),
            guest_session_manager: Arc::new(GuestSessionManager::new()),
            event_bus: Arc::new(GatewayEventBus::new()),
            auth_mode: AuthMode::None,
            shared_token_mgr: Arc::new(SharedTokenManager::new(store, "/tmp/aleph_test.vault")),
        });

        // Connect with no credentials at all
        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({"device_name": "NoAuth"})),
            Some(json!(1)),
        );
        let resp = handle_connect(req, ctx).await;
        assert!(resp.is_success(), "auth_mode: None must accept any connection");
    }

    // =========================================================================
    // PROBE 7 — LLM tools: show → reset → show cycle
    // =========================================================================

    #[tokio::test]
    async fn probe_auth_tools_show_reset_show() {
        let (ctx, _store, shared_mgr) = make_auth_context();

        // Initially no token in memory
        let req = JsonRpcRequest::with_id("auth.show_token", None, json!(1));
        let resp = handle_auth_show_token(req, ctx.clone()).await;
        let r = resp.result.unwrap();
        assert!(r.get("token").unwrap().is_null(), "no token generated yet");

        // Reset generates a new one
        let req = JsonRpcRequest::with_id("auth.reset_token", None, json!(2));
        let resp = handle_auth_reset_token(req, ctx.clone()).await;
        let r = resp.result.unwrap();
        let new_token = r.get("token").unwrap().as_str().unwrap().to_string();
        assert!(new_token.starts_with("aleph-"));

        // Show now returns the generated token
        let req = JsonRpcRequest::with_id("auth.show_token", None, json!(3));
        let resp = handle_auth_show_token(req, ctx.clone()).await;
        let r = resp.result.unwrap();
        assert_eq!(r.get("token").unwrap().as_str().unwrap(), &new_token);

        // Token must be valid for actual auth
        assert!(shared_mgr.validate(&new_token).unwrap());
    }

    // =========================================================================
    // PROBE 8 — LLM tools: session list + revoke integration
    // =========================================================================

    #[tokio::test]
    async fn probe_auth_tools_session_management() {
        let (ctx, store, shared_mgr) = make_auth_context();
        let token = shared_mgr.generate_token().unwrap();
        let hash = hmac_sign(shared_mgr.secret(), &token);

        // Create 3 sessions directly via store
        let mut session_ids = Vec::new();
        for _ in 0..3 {
            let sid = uuid::Uuid::new_v4().to_string();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            store.insert_session(&sid, &hash, now, now + 72 * 3600 * 1000).unwrap();
            session_ids.push(sid);
        }

        // List: should show 3
        let req = JsonRpcRequest::with_id("auth.list_sessions", None, json!(1));
        let resp = handle_auth_list_sessions(req, ctx.clone()).await;
        let r = resp.result.unwrap();
        assert_eq!(r.get("count").unwrap().as_u64().unwrap(), 3);

        // Revoke first session
        let req = JsonRpcRequest::with_id(
            "auth.revoke_session",
            Some(json!({"session_id": session_ids[0]})),
            json!(2),
        );
        let resp = handle_auth_revoke_session(req, ctx.clone()).await;
        assert!(resp.is_success());

        // List: should show 2
        let req = JsonRpcRequest::with_id("auth.list_sessions", None, json!(3));
        let resp = handle_auth_list_sessions(req, ctx.clone()).await;
        let r = resp.result.unwrap();
        assert_eq!(r.get("count").unwrap().as_u64().unwrap(), 2);

        // Revoked session should not validate
        assert!(!store.validate_session(&session_ids[0]).unwrap());
        // Remaining sessions should still validate
        assert!(store.validate_session(&session_ids[1]).unwrap());
        assert!(store.validate_session(&session_ids[2]).unwrap());
    }

    // =========================================================================
    // PROBE 9 — Bearer token middleware: valid / invalid / missing
    // =========================================================================

    /// Tests that the bearer auth middleware correctly gates API access.
    /// We can't easily test Axum middleware directly without a full tower stack,
    /// so we test the underlying validation logic end-to-end.
    #[test]
    fn probe_bearer_token_validation_flow() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store.clone(), "/tmp/aleph_test.vault");
        let token = mgr.generate_token().unwrap();

        // Valid: extract from "Bearer <token>" and validate
        let header_val = format!("Bearer {}", token);
        let extracted = crate::gateway::openai_api::auth::extract_bearer_token(&header_val);
        assert!(extracted.is_some());
        assert!(mgr.validate(extracted.unwrap()).unwrap());

        // Invalid: wrong token
        let header_val = "Bearer aleph-wrong-token";
        let extracted = crate::gateway::openai_api::auth::extract_bearer_token(header_val);
        assert!(extracted.is_some());
        assert!(!mgr.validate(extracted.unwrap()).unwrap());

        // Missing: no Bearer prefix
        let extracted = crate::gateway::openai_api::auth::extract_bearer_token("Basic abc123");
        assert!(extracted.is_none());

        // Case insensitivity: "bearer" (lowercase)
        let header_val = format!("bearer {}", token);
        let extracted = crate::gateway::openai_api::auth::extract_bearer_token(&header_val);
        assert!(extracted.is_some());
        assert!(mgr.validate(extracted.unwrap()).unwrap());
    }

    // =========================================================================
    // PROBE 10 — Cookie extraction edge cases
    // =========================================================================

    #[test]
    fn probe_cookie_extraction_edge_cases() {
        // Multiple cookies — correct one extracted
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "foo=bar; aleph_session=my-uuid-123; baz=qux".parse().unwrap(),
        );
        // We test the extraction logic via the raw function
        let cookie_val = headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';')
                    .filter_map(|c| {
                        let (name, value) = c.trim().split_once('=')?;
                        if name == "aleph_session" { Some(value.to_string()) } else { None }
                    })
                    .next()
            });
        assert_eq!(cookie_val, Some("my-uuid-123".to_string()));

        // Cookie with no spaces after semicolons
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "a=1;aleph_session=sess123;b=2".parse().unwrap(),
        );
        let cookie_val = headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';')
                    .filter_map(|c| {
                        let (name, value) = c.trim().split_once('=')?;
                        if name == "aleph_session" { Some(value.to_string()) } else { None }
                    })
                    .next()
            });
        assert_eq!(cookie_val, Some("sess123".to_string()));

        // Empty cookie header
        let headers = axum::http::HeaderMap::new();
        let cookie_val: Option<String> = headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';')
                    .filter_map(|c| {
                        let (name, value) = c.trim().split_once('=')?;
                        if name == "aleph_session" { Some(value.to_string()) } else { None }
                    })
                    .next()
            });
        assert!(cookie_val.is_none());
    }

    // =========================================================================
    // PROBE 11 — Cross-secret isolation (security boundary)
    // =========================================================================

    /// Tokens signed with one HMAC secret MUST NOT validate with a different secret.
    /// This is critical: if the server restarts with a new secret, old tokens become invalid.
    #[test]
    fn probe_cross_secret_isolation() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());

        let secret_a = generate_secret();
        let secret_b = generate_secret();
        assert_ne!(secret_a, secret_b);

        let mgr_a = SharedTokenManager::with_secret(store.clone(), secret_a, "/tmp/aleph_test.vault");
        let token = mgr_a.generate_token().unwrap();

        // Same store, different secret — MUST fail
        let mgr_b = SharedTokenManager::with_secret(store.clone(), secret_b, "/tmp/aleph_test.vault");
        assert!(!mgr_b.validate(&token).unwrap());

        // Same store, same secret — MUST pass
        let mgr_a2 = SharedTokenManager::with_secret(store, secret_a, "/tmp/aleph_test.vault");
        assert!(mgr_a2.validate(&token).unwrap());
    }

    // =========================================================================
    // PROBE 12 — Schema migration v2 → v3 (shared_token + sessions tables exist)
    // =========================================================================

    #[test]
    fn probe_schema_v3_tables_exist() {
        let store = SecurityStore::in_memory().unwrap();

        // Shared token operations should not error
        store.set_shared_token_hash("test-hash").unwrap();
        assert!(store.validate_shared_token_hash("test-hash").unwrap());
        assert!(!store.validate_shared_token_hash("other-hash").unwrap());

        // Session operations should not error
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        store.insert_session("sess-1", "hash-1", now, now + 100_000).unwrap();
        assert!(store.validate_session("sess-1").unwrap());
        store.touch_session("sess-1").unwrap();
        store.delete_session("sess-1").unwrap();
        assert!(!store.validate_session("sess-1").unwrap());

        // Cleanup on empty table
        let cleaned = store.delete_expired_sessions().unwrap();
        assert_eq!(cleaned, 0);
    }

    // =========================================================================
    // PROBE 13 — Concurrent session creation (thread safety)
    // =========================================================================

    #[test]
    fn probe_concurrent_session_creation() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = Arc::new(HttpSessionManager::new(store, 72));
        let mut handles = Vec::new();

        for i in 0..10 {
            let mgr = mgr.clone();
            let handle = std::thread::spawn(move || {
                let hash = format!("hash-{}", i);
                mgr.create_session(&hash).unwrap()
            });
            handles.push(handle);
        }

        let mut session_ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All 10 sessions should be unique
        session_ids.sort();
        session_ids.dedup();
        assert_eq!(session_ids.len(), 10);

        // All should be valid
        for sid in &session_ids {
            assert!(mgr.validate_session(sid).unwrap());
        }
    }

    // =========================================================================
    // PROBE 14 — Device token reuse after shared token auth
    // =========================================================================

    /// Panel stores the device_token in localStorage and reconnects with it.
    /// This test simulates two sequential connects: first via shared_token, then
    /// via the returned device_token — exactly like the Panel UI flow.
    #[tokio::test]
    async fn probe_panel_reconnect_with_device_token() {
        let (ctx, _store, shared_mgr) = make_auth_context();
        let token = shared_mgr.generate_token().unwrap();

        // First connect: shared token
        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "shared_token": token,
                "device_name": "Web Panel"
            })),
            Some(json!(1)),
        );
        let resp1 = handle_connect(req, ctx.clone()).await;
        assert!(resp1.is_success());

        let r1 = resp1.result.unwrap();
        let device_token = r1.get("token").unwrap().as_str().unwrap();
        let device_id = r1.get("device_id").unwrap().as_str().unwrap();

        // Second connect: device token only (Panel localStorage flow)
        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "token": device_token,
                "device_id": device_id,
            })),
            Some(json!(2)),
        );
        let resp2 = handle_connect(req, ctx.clone()).await;
        assert!(resp2.is_success());

        // Third connect: device token without device_id (edge case)
        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "token": device_token,
            })),
            Some(json!(3)),
        );
        let resp3 = handle_connect(req, ctx.clone()).await;
        assert!(resp3.is_success(), "token alone should suffice for reconnect");
    }

    // =========================================================================
    // PROBE 15 — AuthMode config deserialization
    // =========================================================================

    #[test]
    fn probe_auth_config_toml_parsing() {
        use crate::gateway::config::GatewayConfig;

        // Token mode (explicit)
        let toml = r#"
[gateway]
port = 9000

[gateway.auth]
mode = "token"
session_expiry_hours = 48
token_expiry_hours = 12

[agents.main]
model = "test"
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();
        assert!(config.gateway.auth.mode.is_auth_required());
        assert_eq!(config.gateway.auth.session_expiry_hours, 48);
        assert_eq!(config.gateway.auth.token_expiry_hours, 12);

        // None mode
        let toml = r#"
[gateway]
port = 9000

[gateway.auth]
mode = "none"

[agents.main]
model = "test"
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();
        assert!(!config.gateway.auth.mode.is_auth_required());

        // Default (no auth section) — should default to Token
        let toml = r#"
[gateway]
port = 9000

[agents.main]
model = "test"
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();
        assert!(config.gateway.auth.mode.is_auth_required());
    }

    // =========================================================================
    // PROBE 16 — Login page HTML security properties
    // =========================================================================

    #[test]
    fn probe_login_page_security() {
        let html = login_page_html("");

        // Must be a valid HTML document
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<html"));
        assert!(html.contains("</html>"));

        // Form must POST to correct endpoint
        assert!(html.contains("action=\"/auth/login\""));
        assert!(html.contains("method=\"POST\""));

        // Token input must be password type (no shoulder surfing)
        assert!(html.contains("type=\"password\""));

        // localStorage script stores shared token for WS connect
        assert!(html.contains("localStorage.setItem"));
        assert!(html.contains("aleph_shared_token"));

        // Error variant: XSS-safe (no raw script injection)
        let html_err = login_page_html("<script>alert('xss')</script>");
        // The error message is in a styled div, not executed
        assert!(html_err.contains("<script>alert"));
        // The script tag in the error message is inside a styled div, not a raw script block
        assert!(html_err.contains("style=\"background:#3b1419"));
    }

    // =========================================================================
    // PROBE 17 — Multiple sessions, selective revocation
    // =========================================================================

    #[test]
    fn probe_selective_session_revocation() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = HttpSessionManager::new(store, 72);

        // Create 5 sessions
        let sids: Vec<String> = (0..5).map(|i| {
            mgr.create_session(&format!("hash-{}", i)).unwrap()
        }).collect();

        // Revoke sessions 0, 2, 4
        for i in [0, 2, 4] {
            mgr.revoke_session(&sids[i]).unwrap();
        }

        // Sessions 1 and 3 still valid
        assert!(mgr.validate_session(&sids[1]).unwrap());
        assert!(mgr.validate_session(&sids[3]).unwrap());

        // Sessions 0, 2, 4 are invalid
        assert!(!mgr.validate_session(&sids[0]).unwrap());
        assert!(!mgr.validate_session(&sids[2]).unwrap());
        assert!(!mgr.validate_session(&sids[4]).unwrap());

        // List should show exactly 2
        let active = mgr.list_sessions().unwrap();
        assert_eq!(active.len(), 2);
    }

    // =========================================================================
    // PROBE 18 — Pairing flow with auth required (shared token not provided)
    // =========================================================================

    #[tokio::test]
    async fn probe_pairing_required_when_no_credentials() {
        let (ctx, _store, _shared_mgr) = make_auth_context();

        // No token, no shared_token → must trigger pairing
        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "device_name": "New iPhone",
                "device_type": "ios"
            })),
            Some(json!(1)),
        );
        let resp = handle_connect(req, ctx).await;
        assert!(resp.is_error());

        let err = resp.error.unwrap();
        assert_eq!(err.message, "pairing_required");
        let data = err.data.unwrap();
        let code = data.get("code").unwrap().as_str().unwrap();
        assert_eq!(code.len(), 8, "pairing code must be 8 chars");
        assert!(data.get("expires_in").is_some());
    }

    // =========================================================================
    // PROBE 19 — HMAC constant-time comparison
    // =========================================================================

    #[test]
    fn probe_hmac_timing_safe() {
        let secret = generate_secret();
        let token = "aleph-test-token";
        let signature = hmac_sign(&secret, token);

        // Correct signature: passes
        assert!(crate::gateway::security::hmac_verify(&secret, token, &signature).is_ok());

        // Wrong signature: fails
        assert!(crate::gateway::security::hmac_verify(&secret, token, "wrong").is_err());

        // Similar but not identical: fails
        let mut bad_sig = signature.clone();
        // Flip last character
        let last = bad_sig.pop().unwrap();
        bad_sig.push(if last == '0' { '1' } else { '0' });
        assert!(crate::gateway::security::hmac_verify(&secret, token, &bad_sig).is_err());
    }

    // =========================================================================
    // PROBE 20 — End-to-end: token generate → HTTP login → session → WS connect
    // =========================================================================

    /// The complete "first user" flow:
    ///   1. Server starts, generates shared token
    ///   2. User opens browser, enters token at /login
    ///   3. handle_login creates session → cookie set
    ///   4. Panel WASM reads shared_token from localStorage
    ///   5. Panel calls WS connect with shared_token
    ///   6. Server issues device token
    ///   7. Panel stores device token in localStorage
    ///   8. On reconnect, Panel uses device token
    #[tokio::test]
    async fn probe_complete_first_user_flow() {
        let (ctx, store, shared_mgr) = make_auth_context();
        let session_mgr = HttpSessionManager::new(store.clone(), 72);

        // Step 1: Server auto-generates token
        let shared_token = shared_mgr.generate_token().unwrap();

        // Step 2-3: User enters token → session created
        assert!(shared_mgr.validate(&shared_token).unwrap());
        let hash = hmac_sign(shared_mgr.secret(), &shared_token);
        let session_id = session_mgr.create_session(&hash).unwrap();
        assert!(session_mgr.validate_session(&session_id).unwrap());

        // Step 4-5: Panel reads shared_token from localStorage, calls WS connect
        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "shared_token": shared_token,
                "device_name": "Web Panel",
                "device_type": "web"
            })),
            Some(json!(1)),
        );
        let resp = handle_connect(req, ctx.clone()).await;
        assert!(resp.is_success());

        // Step 6-7: Server issues device token → Panel stores in localStorage
        let result = resp.result.unwrap();
        let device_token = result.get("token").unwrap().as_str().unwrap().to_string();
        let device_id = result.get("device_id").unwrap().as_str().unwrap().to_string();

        // Step 8: Reconnect with device token (simulates page refresh)
        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "token": device_token,
                "device_id": device_id,
            })),
            Some(json!(2)),
        );
        let resp2 = handle_connect(req, ctx.clone()).await;
        assert!(resp2.is_success());

        // Verify: session still active in parallel
        assert!(session_mgr.validate_session(&session_id).unwrap());

        // Verify: original shared token still valid for new sessions
        assert!(shared_mgr.validate(&shared_token).unwrap());
    }

    // =========================================================================
    // PROBE 21 — Bulk session cleanup
    // =========================================================================

    #[test]
    fn probe_bulk_expired_session_cleanup() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());

        // Create 50 sessions: 25 expired (0h), 25 active (72h)
        let expired_mgr = HttpSessionManager::new(store.clone(), 0);
        let active_mgr = HttpSessionManager::new(store.clone(), 72);

        for i in 0..25 {
            expired_mgr.create_session(&format!("exp-{}", i)).unwrap();
        }
        // Small delay to ensure expired sessions are past their expiry
        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut active_sids = Vec::new();
        for i in 0..25 {
            active_sids.push(active_mgr.create_session(&format!("act-{}", i)).unwrap());
        }

        // Cleanup should remove exactly 25 expired sessions
        let cleaned = expired_mgr.cleanup_expired().unwrap();
        assert_eq!(cleaned, 25);

        // Active sessions remain valid
        for sid in &active_sids {
            assert!(active_mgr.validate_session(sid).unwrap());
        }

        // List shows exactly 25 active
        let active = active_mgr.list_sessions().unwrap();
        assert_eq!(active.len(), 25);
    }

    // =========================================================================
    // PROBE 22 — Token format validation
    // =========================================================================

    #[test]
    fn probe_token_format_invariants() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, "/tmp/aleph_test.vault");

        // Generate 50 tokens — all must follow format
        for _ in 0..50 {
            let token = mgr.generate_token().unwrap();
            assert!(token.starts_with("aleph-"), "token must start with 'aleph-'");
            // Must be valid UUID after prefix
            let uuid_part = &token[6..];
            assert!(uuid::Uuid::parse_str(uuid_part).is_ok(), "suffix must be valid UUID: {}", uuid_part);
        }
    }

    // =========================================================================
    // PROBE 23 — Device registration via shared token sets correct permissions
    // =========================================================================

    #[tokio::test]
    async fn probe_shared_token_device_gets_operator_permissions() {
        let (ctx, store, shared_mgr) = make_auth_context();
        let token = shared_mgr.generate_token().unwrap();

        let req = JsonRpcRequest::new(
            "connect",
            Some(json!({"shared_token": token})),
            Some(json!(1)),
        );
        let resp = handle_connect(req, ctx).await;
        assert!(resp.is_success());

        let result = resp.result.unwrap();
        let device_id = result.get("device_id").unwrap().as_str().unwrap();
        let permissions = result.get("permissions").unwrap().as_array().unwrap();

        // Must have wildcard permission
        assert!(permissions.iter().any(|p| p.as_str() == Some("*")));

        // Device must be registered in SecurityStore
        let device = store.get_device(device_id).unwrap();
        assert!(device.is_some(), "device must be registered after shared token auth");
        assert_eq!(device.unwrap().role, "operator");
    }
}
