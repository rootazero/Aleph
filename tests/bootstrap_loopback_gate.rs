//! Loopback hard-gate for `/auth/bootstrap?nonce=…`.
//!
//! `axum::extract::ConnectInfo<SocketAddr>` reports the real peer address;
//! the handler must refuse anything that is not 127.0.0.0/8 or ::1, even
//! if the nonce is otherwise valid.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

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
        IpAddr::V6(Ipv6Addr::LOCALHOST),
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

/// Full-flow: issue a nonce via [`BootstrapNonceManager`], drive the
/// real axum router via a loopback `TcpListener`, and verify the 303
/// redirect + `Set-Cookie` header.
///
/// Uses a real loopback socket rather than `oneshot` so axum's
/// `ConnectInfo<SocketAddr>` extractor sees a real peer — exactly what
/// the loopback hard-gate inspects.
#[tokio::test]
async fn consume_sets_cookie_on_loopback() {
    use alephcore::gateway::auth_middleware::{auth_routes, AuthState};
    use alephcore::gateway::bootstrap::BootstrapNonceManager;
    use alephcore::gateway::config::AuthMode;
    use alephcore::gateway::security::{SecurityStore, SharedTokenManager};
    use alephcore::gateway::session::HttpSessionManager;
    use std::sync::Arc;
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let store = Arc::new(SecurityStore::open(&dir.path().join("sec.db")).expect("open store"));
    let shared = Arc::new(SharedTokenManager::new(
        store.clone(),
        dir.path().join("vault"),
    ));
    let _ = shared.generate_token().expect("generate token");
    let session_mgr = Arc::new(HttpSessionManager::new(store.clone(), 24));
    let bootstrap = Arc::new(BootstrapNonceManager::default());
    let pairing_mgr = Arc::new(alephcore::gateway::security::PairingManager::new(
        store.clone(),
    ));
    let state = Arc::new(AuthState {
        shared_token_mgr: shared,
        session_mgr,
        auth_mode: AuthMode::Token,
        bootstrap_mgr: bootstrap.clone(),
        pairing_mgr,
        auth_ctx: None,
    });

    let (nonce, _) = bootstrap.issue();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("local_addr");
    let app = auth_routes(state).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");
    let resp = client
        .get(format!("http://{addr}/auth/bootstrap?nonce={nonce}"))
        .send()
        .await
        .expect("send request");

    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/")
    );
    let cookie = resp
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("cookie header present");
    assert!(
        cookie.starts_with("aleph_session="),
        "expected aleph_session cookie, got {cookie}"
    );
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
}

/// Replay protection: a nonce can be consumed once. The second request
/// receives 401 even from loopback.
#[tokio::test]
async fn replay_is_refused() {
    use alephcore::gateway::auth_middleware::{auth_routes, AuthState};
    use alephcore::gateway::bootstrap::BootstrapNonceManager;
    use alephcore::gateway::config::AuthMode;
    use alephcore::gateway::security::{SecurityStore, SharedTokenManager};
    use alephcore::gateway::session::HttpSessionManager;
    use std::sync::Arc;
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let store = Arc::new(SecurityStore::open(&dir.path().join("sec.db")).expect("open store"));
    let shared = Arc::new(SharedTokenManager::new(
        store.clone(),
        dir.path().join("vault"),
    ));
    let _ = shared.generate_token().expect("generate token");
    let session_mgr = Arc::new(HttpSessionManager::new(store.clone(), 24));
    let bootstrap = Arc::new(BootstrapNonceManager::default());
    let pairing_mgr = Arc::new(alephcore::gateway::security::PairingManager::new(
        store.clone(),
    ));
    let state = Arc::new(AuthState {
        shared_token_mgr: shared,
        session_mgr,
        auth_mode: AuthMode::Token,
        bootstrap_mgr: bootstrap.clone(),
        pairing_mgr,
        auth_ctx: None,
    });
    let (nonce, _) = bootstrap.issue();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("local_addr");
    let app = auth_routes(state).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");
    let url = format!("http://{addr}/auth/bootstrap?nonce={nonce}");
    let first = client.get(&url).send().await.expect("first request");
    assert_eq!(first.status(), reqwest::StatusCode::SEE_OTHER);

    let second = client.get(&url).send().await.expect("second request");
    assert_eq!(second.status(), reqwest::StatusCode::UNAUTHORIZED);
}
