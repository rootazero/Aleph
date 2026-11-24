//! Real production router/Core/auth acceptance without the monolithic lib harness.
#[path = "panel_http/common.rs"]
mod common;
#[path = "panel_http/live.rs"]
mod live;

use std::net::SocketAddr;
use std::sync::Arc;

use alephcore::gateway::security::store::{UserRole, UserStatus};
use alephcore::gateway::security::{DeviceTokenManager, SecurityStore, SharedTokenManager};
use alephcore::gateway::server::GatewayConfig;
use alephcore::gateway::GatewayServer;
use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

const FIRST: &[u8] = b"export const revision = 1;";
const SECOND: &[u8] = b"export const revision = 2;";
const ENTRY: &str = "/tabs/community-live/1.0.0/ui/panel.mjs";

fn router(tokens: Arc<DeviceTokenManager>, config: GatewayConfig) -> Router {
    let mut gateway = GatewayServer::with_config("127.0.0.1:0".parse().unwrap(), config);
    gateway.set_security_store(tokens.store().clone());
    gateway.set_device_token_manager(tokens);
    gateway.build_router()
}

fn request(path: &str, token: Option<&str>, peer: &str) -> Request<Body> {
    let mut request = Request::builder()
        .uri(path)
        .header(header::HOST, "127.0.0.1");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let mut request = request.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    request
}

async fn body(response: axum::response::Response) -> Vec<u8> {
    to_bytes(response.into_body(), 5 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn public_builtin_assets_keep_the_existing_http_owner() {
    // A temporary build artifact exercises the real public asset owner, never a
    // plugin root or a tracked source file. RustEmbed debug serving reads it.
    let parent = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("interfaces/webchat/dist/tabs/terminal");
    std::fs::create_dir_all(&parent).unwrap();
    let public = tempfile::Builder::new()
        .prefix("http-compat-")
        .tempdir_in(parent)
        .unwrap();
    std::fs::write(
        public.path().join("terminal.js"),
        b"export const builtin = true;",
    )
    .unwrap();
    let path = format!(
        "/tabs/terminal/{}/terminal.js",
        public.path().file_name().unwrap().to_str().unwrap()
    );
    let original = alephcore::gateway::control_plane::server::serve_static_asset(
        axum::http::HeaderMap::new(),
        axum::extract::Path(path.trim_start_matches('/').to_owned()),
    )
    .await;
    assert_eq!(original.status(), StatusCode::OK);
    assert_eq!(body(original).await, b"export const builtin = true;");
    let gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap()).build_router();
    let response = gateway
        .oneshot(request(&path, None, "127.0.0.1:4000"))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "community route must preserve public built-in delivery"
    );
    assert_eq!(body(response).await, b"export const builtin = true;");
}

#[tokio::test]
async fn authenticated_canonical_publication_through_the_production_router() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = common::Home::set(&home, root.path());
    common::write_panel(&home, "1.0.0", FIRST);
    let manager = common::manager(root.path()).await;
    assert_eq!(manager.panel_ui_capability_snapshot().declarations.len(), 1);
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    store
        .create_user("panel-member", "Panel member", UserRole::Member)
        .unwrap();
    let tokens = Arc::new(DeviceTokenManager::new(store.clone()));
    let ticket = tokens
        .create_bootstrap_ticket(None, Some("panel-member"))
        .unwrap();
    let issued = tokens
        .exchange_bootstrap_ticket(
            &ticket,
            Some("panel-device".into()),
            Some("Panel".into()),
            None,
        )
        .unwrap();
    let gateway = router(tokens.clone(), GatewayConfig::default());
    let missing = gateway
        .clone()
        .oneshot(request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000"))
        .await
        .unwrap();
    assert_eq!(
        missing.status(),
        StatusCode::NOT_FOUND,
        "missing Core must never fall through to the SPA"
    );
    assert!(alephcore::extension::init_extension_manager(manager.clone()).is_ok());
    let ok = gateway
        .clone()
        .oneshot(request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000"))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.headers()[header::CACHE_CONTROL], "private, no-store");
    assert_eq!(ok.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(
        ok.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(body(ok).await, FIRST);
    let denied = gateway
        .clone()
        .oneshot(request(ENTRY, None, "127.0.0.1:4000"))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let refusal = body(denied).await;
    for path in [
        format!("{ENTRY}?revision=1"),
        format!("{ENTRY}?url=https://attacker.invalid"),
        format!("{ENTRY}?root=/private/plugin"),
        format!("{ENTRY}?manifest=generic.mjs"),
        "/tabs/community-live/1.0.0/generic.mjs".into(),
        "/tabs/community-live/1.0.0/%75i/panel.mjs".into(),
        "/tabs/community-live/1.0.0/../generic.mjs".into(),
        "/tabs/missing/1.0.0/ui/panel.mjs".into(),
    ] {
        let response = gateway
            .clone()
            .oneshot(request(&path, Some(&issued.device_token), "127.0.0.1:4000"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(body(response).await, refusal);
    }
    let mut origin = request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000");
    origin
        .headers_mut()
        .insert(header::ORIGIN, "https://attacker.invalid".parse().unwrap());
    assert_eq!(
        gateway.clone().oneshot(origin).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let mut duplicate = request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000");
    duplicate
        .headers_mut()
        .append(header::AUTHORIZATION, "Bearer invalid".parse().unwrap());
    assert_eq!(
        gateway.clone().oneshot(duplicate).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    for value in ["Basic invalid", "Bearer ", "Bearer invalid"] {
        let mut invalid = request(ENTRY, None, "127.0.0.1:4000");
        invalid
            .headers_mut()
            .insert(header::AUTHORIZATION, value.parse().unwrap());
        assert_eq!(
            gateway.clone().oneshot(invalid).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
    for claim in ["x-aleph-project", "x-project-id"] {
        let mut claimed = request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000");
        claimed
            .headers_mut()
            .insert(claim, "untrusted".parse().unwrap());
        assert_eq!(
            gateway.clone().oneshot(claimed).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
    let remote = request(ENTRY, Some(&issued.device_token), "203.0.113.7:4000");
    assert_eq!(
        gateway.clone().oneshot(remote).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let mut spoof = request(ENTRY, Some(&issued.device_token), "203.0.113.7:4000");
    spoof
        .headers_mut()
        .insert("x-forwarded-proto", "https".parse().unwrap());
    assert_eq!(
        gateway.clone().oneshot(spoof).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let secure = router(
        tokens.clone(),
        GatewayConfig {
            tls_enabled: true,
            ..GatewayConfig::default()
        },
    );
    assert_eq!(
        secure
            .oneshot(request(
                ENTRY,
                Some(&issued.device_token),
                "203.0.113.7:4000"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let proxy = router(
        tokens.clone(),
        GatewayConfig {
            trusted_proxy_enabled: true,
            ..GatewayConfig::default()
        },
    );
    let mut forwarded = request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000");
    forwarded
        .headers_mut()
        .insert("x-forwarded-for", "203.0.113.7".parse().unwrap());
    assert_eq!(
        proxy.clone().oneshot(forwarded).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let mut forwarded = request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000");
    forwarded
        .headers_mut()
        .insert("x-forwarded-for", "203.0.113.7".parse().unwrap());
    forwarded
        .headers_mut()
        .insert("x-forwarded-proto", "https".parse().unwrap());
    assert_eq!(
        proxy.oneshot(forwarded).await.unwrap().status(),
        StatusCode::OK
    );
    let limited = router(tokens.clone(), GatewayConfig::default());
    for _ in 0..120 {
        assert_eq!(
            limited
                .clone()
                .oneshot(request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        limited
            .oneshot(request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    store
        .update_user("panel-member", None, None, Some(UserStatus::Deactivated))
        .unwrap();
    assert_eq!(
        gateway
            .clone()
            .oneshot(request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    store
        .update_user("panel-member", None, None, Some(UserStatus::Active))
        .unwrap();
    tokens.revoke_device(&issued.device_id).unwrap();
    assert_eq!(
        gateway
            .clone()
            .oneshot(request(ENTRY, Some(&issued.device_token), "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let shared = Arc::new(SharedTokenManager::with_secret(
        store,
        [7; 32],
        root.path().join("vault"),
    ));
    SharedTokenManager::set_global(shared.clone());
    let first = shared.generate_token().unwrap();
    assert_eq!(
        gateway
            .clone()
            .oneshot(request(ENTRY, Some(&first), "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let second = shared.generate_token().unwrap();
    assert_eq!(
        gateway
            .clone()
            .oneshot(request(ENTRY, Some(&first), "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        gateway
            .clone()
            .oneshot(request(ENTRY, Some(&second), "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    std::fs::write(home.join("plugins/community-live/ui/panel.mjs"), SECOND).unwrap();
    assert_eq!(
        body(
            gateway
                .clone()
                .oneshot(request(ENTRY, Some(&second), "127.0.0.1:4000"))
                .await
                .unwrap()
        )
        .await,
        FIRST
    );
    manager.reload_plugin("community-live").await.unwrap();
    assert_eq!(
        gateway
            .clone()
            .oneshot(request(ENTRY, Some(&second), "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    common::write_panel(&home, "2.0.0", SECOND);
    manager.reload_plugin("community-live").await.unwrap();
    assert_eq!(
        gateway
            .clone()
            .oneshot(request(ENTRY, Some(&second), "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        body(
            gateway
                .clone()
                .oneshot(request(
                    "/tabs/community-live/2.0.0/ui/panel.mjs",
                    Some(&second),
                    "127.0.0.1:4000"
                ))
                .await
                .unwrap()
        )
        .await,
        SECOND
    );
    manager.unmount("community-live").await.unwrap();
    assert_eq!(
        gateway
            .clone()
            .oneshot(request(
                "/tabs/community-live/2.0.0/ui/panel.mjs",
                Some(&second),
                "127.0.0.1:4000"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    // Load the same valid Panel only under a project-scoped discovery parent.
    common::write_panel(&root.path().join("project/.aleph"), "1.0.0", FIRST);
    std::fs::remove_dir_all(home.join("plugins/community-live")).unwrap();
    manager.reload().await.unwrap();
    assert!(manager.get_plugin_record("community-live").await.is_some());
    assert!(manager
        .panel_ui_capability_snapshot()
        .declarations
        .is_empty());
    assert_eq!(
        gateway
            .oneshot(request(ENTRY, Some(&second), "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let unavailable = GatewayServer::new("127.0.0.1:0".parse().unwrap()).build_router();
    assert_eq!(
        unavailable
            .oneshot(request(ENTRY, None, "127.0.0.1:4000"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}
